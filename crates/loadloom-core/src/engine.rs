//! 打流核心引擎：基于 Tokio 的异步高并发流量生成器。
//!
//! ## 无头设计约束
//! 本模块**不含任何 UI 代码**：没有窗口、没有渲染回调、没有对话框、没有事件循环、
//! 没有阻塞主线程的胶水逻辑。它负责编排，不负责底层实现：
//! 1. 在后台 tokio 任务中产生流量（限速委托给 `rate::RateLimiter`）；
//! 2. 维护运行状态与环形采样历史（指标聚合委托给 `metrics::Metrics`）；
//! 3. 通过 `broadcast` 通道**主动推流**（metrics / log / run_event），调用方永不轮询。
//!
//! 限速算法在 `rate` 模块，指标与抖动统计在 `metrics` 模块；两个子模块都不对 crate 外暴露。
//!
//! ## 并发模型
//! * 一次性拉起 `MAX_WORKERS` 个异步 worker，靠 `AtomicU32` 实时增减「在岗」并发；
//! * 全局令牌桶限速，速率可运行中动态修改；
//! * 指标采集固定 250ms 一 tick，与 UI 帧率完全解耦。
//!
//! ## 信任边界
//!
//! 本模块的每个数字都可能来自 IPC 载荷（`start_run` / `set_live_config` 是
//! Command，调用方可以是任意前端代码或调试工具），因此一律按**不可信输入**处理：
//!
//! * 明确的非法值（`NaN`、非有限数）→ 拒绝，并通过日志与 `run_event` 广播原因；
//! * 超出安全范围但语义合法的值 → 收敛到范围内，且**如实告知**被收敛过
//!   （绝不静默修改用户设定的限速与安全停止边界；并发数按 `1..=MAX_WORKERS` 收敛）；
//! * 任何情况下都不 panic：畸形载荷最多让本次请求失败，不允许掀翻进程。
//!
//! 运行期另有两条硬约束：worker 只认同一个**运行代次**（见 [`Control`]），
//! 退避等待有界且可被 `stop()` 打断。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::broadcast;

use crate::codes;
use crate::contract::{
    CoreError, EngineLimits, HistoryPoint, LiveConfigPatch, LogEntry, LogLevel, MetricsSnapshot,
    RunEvent, RunEventKind, RunPhase, StartRunRequest,
};
use crate::metrics::Metrics;
use crate::rate::RateLimiter;

/// 允许的最大并发连接数（同时创建的 worker 任务数）。
pub const MAX_WORKERS: u32 = 32;
/// 允许的最大限速值（MiB/s），0 表示不限速。
pub const MAX_RATE_MIB: f64 = 4096.0;
/// 历史采样点数量（约 30 秒，250ms 一点）。
pub const HISTORY_LEN: usize = 120;
/// 指标采集/推送周期。
pub const TICK: Duration = Duration::from_millis(250);

const GIB: f64 = 1_073_741_824.0;
const MIB: f64 = 1_048_576.0;
const DEFAULT_THREADS: u32 = 4;
/// 目标地址长度上限（字节）：URL 进入每一帧快照并随日志广播，必须在边界截断。
const MAX_URL_LEN: usize = 2048;

/// 自动停止「流量上限」的硬上限（GB，1 PiB）。
///
/// 必须保证 `MAX_LIMIT_GB * GIB` 仍在 `u64` 字节计数可表达的范围内：否则
/// `total_bytes as f64 >= limit_gb * GIB` 的右侧会溢出成 `inf`，比较永远为假 ——
/// 用户以为设了安全停止，实际是把守卫静默关掉，流量会一直打到手动叫停。
const MAX_LIMIT_GB: f64 = 1024.0 * 1024.0;
/// 自动停止「时长上限」的硬上限（分钟，约 1 年）。
const MAX_LIMIT_MINUTES: f64 = 365.0 * 24.0 * 60.0;
/// 连接超时。
///
/// 与 [`READ_TIMEOUT`] 一起构成「请求不会无限挂起」的硬保证，任何降级路径
/// 都必须保留它们（见 [`build_client`]）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 读空闲超时：两个数据块之间允许的最长间隔。
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 失败退避基数与上限。
const BACKOFF_BASE_MS: u64 = 400;
const BACKOFF_MAX_MS: u64 = 3_200;
/// 退避的分片粒度：每片结束后重新校验运行代次，让 `stop()` 不必等满整个退避。
const BACKOFF_SLICE: Duration = Duration::from_millis(100);
/// 停止判定的轮询间隔。在途请求靠它和「停止」赛跑：用户按下停止后流量最迟
/// 在 25ms 内停下，而不是把连接 / 读取超时（10s / 30s）跑完 —— 只做到
/// 「不再发起新请求」等于让流量在用户以为已经停止之后继续打出去。
const CANCEL_POLL: Duration = Duration::from_millis(25);
/// `User-Agent`。版本号取自包元数据，避免手写字面量随版本漂移（旧值恒为 `0.4`）。
const USER_AGENT: &str = concat!(
    "loadloom/",
    env!("CARGO_PKG_VERSION"),
    " (+authorized-load-test)"
);

/// MiB/s -> byte/s
#[inline]
pub fn mib_to_bps(mib: f64) -> f64 {
    mib * MIB
}

// ---------------------------------------------------------------------------
// 纯函数：可独立单测的业务判定（不依赖任何 IO / 状态）
// ---------------------------------------------------------------------------

/// 追加缓存破坏参数，确保每个请求都真实穿透到源站。
///
/// 分隔符处理三种收尾：`?` / `&` 结尾直接接着写（否则会出现 `?&_ll=` 这种
/// 空参数），已有查询串用 `&`，其余用 `?`。参数名固定为 `_ll`，同时充当
/// 「本次流量来自 LoadLoom」的溯源标记。
pub fn cache_busted_url(url: &str, worker_id: u32, request_id: u64) -> String {
    let (base, fragment) = url.split_once('#').unwrap_or((url, ""));
    let suffix = if fragment.is_empty() {
        String::new()
    } else {
        format!("#{fragment}")
    };
    let separator = if base.ends_with('?') || base.ends_with('&') {
        ""
    } else if base.contains('?') {
        "&"
    } else {
        "?"
    };
    format!("{base}{separator}_ll={worker_id}-{request_id}{suffix}")
}

/// 校验并收敛限速值（MiB/s）。
///
/// **必须显式拒绝 `NaN`**：`f64::clamp` 会原样返回 `NaN`，而 `NaN > 0.0` 为假，
/// 令牌桶于是把它当作「不限速」—— 用户设了限速、实际全速放行，这是最危险的
/// 失效方向（对第三方产生远超授权的流量）。`1e-300` 这类极小正值语义合法，
/// 交给限速器按有界等待处理（见 `rate` 模块的 `MAX_WAIT`）。
fn sanitize_rate_mib(rate_mib: f64) -> Result<f64, CoreError> {
    if !rate_mib.is_finite() {
        return Err(CoreError::InvalidInput("限速值必须是有限数字".to_owned()));
    }
    Ok(rate_mib.clamp(0.0, MAX_RATE_MIB))
}

/// 校验并收敛自动停止阈值（`<= 0` 表示关闭）。
///
/// 不能只做 `max(0.0)`：`NaN.max(0.0)` 返回 `0.0`，而 `0` 在契约里的含义是
/// 「关闭自动停止」—— 一个畸形载荷就这样把用户的安全守卫静默关掉了。
/// 大到溢出的阈值同样致命（见 [`MAX_LIMIT_GB`]），因此按硬上限收敛。
fn sanitize_limit(value: f64, ceiling: f64, label: &str) -> Result<f64, CoreError> {
    if !value.is_finite() {
        return Err(CoreError::InvalidInput(format!("{label}必须是有限数字")));
    }
    if value <= 0.0 {
        return Ok(0.0);
    }
    Ok(value.min(ceiling))
}

/// 自动停止判定：返回 `Some(原因)` 表示应当停止。
///
/// 非有限阈值一律视为「未设置」：`NaN` 参与任何比较都为假，若不加判断，
/// 一个 `NaN` 上限会让条件永远为假 —— 守卫看起来在、实际永远不触发。
/// 阈值本身的可达性由 [`sanitize_limit`] 在信任边界上保证。
pub fn decide_auto_stop(
    limit_gb: f64,
    limit_minutes: f64,
    total_bytes: u64,
    elapsed_secs: f64,
) -> Option<String> {
    if limit_gb.is_finite() && limit_gb > 0.0 && total_bytes as f64 >= limit_gb * GIB {
        return Some(format!("已达流量目标 {limit_gb:.2} GB，自动停止"));
    }
    if limit_minutes.is_finite() && limit_minutes > 0.0 && elapsed_secs >= limit_minutes * 60.0 {
        return Some(format!("已达时长目标 {limit_minutes:.1} 分钟，自动停止"));
    }
    None
}

/// 环形历史：超长后丢弃最旧点。
fn push_history(history: &mut VecDeque<HistoryPoint>, point: HistoryPoint) {
    history.push_back(point);
    while history.len() > HISTORY_LEN {
        history.pop_front();
    }
}

/// 成功率百分比（0..=100）。
fn success_rate(completed: u64, failures: u64) -> f64 {
    let attempts = completed + failures;
    if attempts == 0 {
        0.0
    } else {
        completed as f64 / attempts as f64 * 100.0
    }
}

/// 把 reqwest 错误归类为稳定的错误码，便于前端聚合展示。
fn classify(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "TIMEOUT".to_owned()
    } else if error.is_connect() {
        "CONNECT_FAILED".to_owned()
    } else if error.is_body() {
        "BODY_STREAM".to_owned()
    } else if error.is_decode() {
        "DECODE_ERROR".to_owned()
    } else if error.is_redirect() {
        "REDIRECT_ERROR".to_owned()
    } else if error.is_request() {
        "REQUEST_ERROR".to_owned()
    } else {
        "UNKNOWN".to_owned()
    }
}

// ---------------------------------------------------------------------------
// 运行时控制
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Control {
    /// 停止标志：worker 在热路径上高频读取，因此单独一个原子量。
    stop: AtomicBool,
    /// 目标并发数（在岗 worker 数）。
    threads: AtomicU32,
    /// 运行代次（run identity）：`start()` 与「真正停止一次运行」各递增一次。
    ///
    /// **为什么必须有它（真实竞态）**：worker 的循环条件原本只看 `stop`。
    /// 「stop → 立刻 start」时，一个正在 400ms 退避里休眠的旧 worker 会错过
    /// 那次 stop（醒来时 `stop` 已被 start 清成 false），于是作为**第 33 个**
    /// worker 继续打流：既突破了用户授权的并发上限，也会在 `metrics.reset()`
    /// 之后用一个迟到的 `in_flight` 收尾把在途计数下溢成 `u32::MAX`。
    /// 有了代次，旧 worker 在下一次判定时就发现自己已经过期。
    run: AtomicU64,
}

impl Control {
    /// 开始新一轮运行：递增代次并返回本轮标识。
    fn begin_run(&self) -> u64 {
        self.run.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 让当前轮次作废（`stop()` 使用）。
    fn invalidate_run(&self) {
        self.run.fetch_add(1, Ordering::AcqRel);
    }

    /// worker 是否仍属于当前轮次、且未被要求停止。
    fn is_active(&self, run: u64) -> bool {
        !self.stop.load(Ordering::Acquire) && self.run.load(Ordering::Acquire) == run
    }
}

struct Inner {
    running: bool,
    url: String,
    started: Option<Instant>,
    elapsed_frozen: f64,
    threads: u32,
    rate_mib: f64,
    limit_gb: f64,
    limit_minutes: f64,
    status: String,
    history: VecDeque<HistoryPoint>,
    speed_bps: f64,
    peak_bps: f64,
    sum_bps: f64,
    samples: u64,
    last_bytes: u64,
    last_tick: Instant,
}

impl Inner {
    fn idle() -> Self {
        Self {
            running: false,
            url: String::new(),
            started: None,
            elapsed_frozen: 0.0,
            threads: DEFAULT_THREADS,
            rate_mib: 0.0,
            limit_gb: 0.0,
            limit_minutes: 0.0,
            status: "准备就绪 · 等待开始".to_owned(),
            history: VecDeque::new(),
            speed_bps: 0.0,
            peak_bps: 0.0,
            sum_bps: 0.0,
            samples: 0,
            last_bytes: 0,
            last_tick: Instant::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------

/// 引擎后台任务的执行器。
///
/// **为什么需要它（真实事故复盘）**：早期实现在 `start()` 里直接调用 `tokio::spawn`，
/// 这隐含要求「调用方所在线程必须处于 Tokio 运行时上下文」。Tauri v2 的**同步**
/// command 运行在事件循环主线程上，没有运行时上下文，于是点击「开始」时：
///
/// ```text
/// thread 'main' panicked at 'there is no reactor running,
/// must be called from the context of a Tokio 1.x runtime'
/// ```
///
/// 又因为 release profile 配了 `panic = "abort"`，整个进程当场 abort —— 表现为闪退。
///
/// 修复思路：把执行器在**构造时固化**下来，此后从**任意线程**派发任务都安全。
/// * 构造时已在运行时内（`tauri::async_runtime`、`#[tokio::test]`）→ 复用该运行时；
/// * 构造时无运行时（裸线程）→ 自建专属多线程运行时。
///
/// 两者都经 `Handle::spawn` 派发，而 `Handle::spawn` 明确允许跨线程调用。
enum Executor {
    Ambient(tokio::runtime::Handle),
    Owned(OwnedRuntime),
}

/// 自建运行时的包装，唯一职责是保证 `Runtime` **不会在运行时上下文里被 drop**。
///
/// tokio 的 `Runtime::drop` 要阻塞等待自己的工作线程退出；若此刻正处于某个运行时
/// 上下文里（典型情形：引擎最后一个 `Arc<Engine>` 恰好被它自己的 worker 任务释放），
/// tokio 会直接 panic：
///
/// ```text
/// Cannot drop a runtime in a context where blocking is not allowed.
/// ```
///
/// 这个坑在吞吐标定台上被真实踩到过（停止打流后每个并发档位都炸一次），所以把真正
/// 的释放动作挪到一条裸线程上做。
struct OwnedRuntime(Option<tokio::runtime::Runtime>);

impl OwnedRuntime {
    fn get(&self) -> &tokio::runtime::Runtime {
        self.0.as_ref().expect("运行时只在析构时被取走")
    }
}

impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self.0.take() else { return };
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || drop(runtime));
        } else {
            drop(runtime);
        }
    }
}

impl Executor {
    fn acquire() -> Self {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => Executor::Ambient(handle),
            Err(_) => Executor::Owned(OwnedRuntime(Some(
                tokio::runtime::Builder::new_multi_thread()
                    .thread_name("loadloom-core")
                    .enable_all()
                    .build()
                    .expect("创建打流引擎运行时失败"),
            ))),
        }
    }

    /// 派发一个后台任务。**可在任意线程调用**（这正是修复闪退的关键）。
    ///
    /// 只接受无返回值的 future：引擎里的后台任务都是常驻循环或 fire-and-forget，
    /// 不允许调用方 `.await` 任务结果，避免把后台任务重新耦合回调用栈。
    fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        match self {
            Executor::Ambient(handle) => {
                handle.spawn(future);
            }
            Executor::Owned(runtime) => {
                runtime.get().spawn(future);
            }
        }
    }
}

/// 无头业务计算引擎：唯一的对外门面。
pub struct Engine {
    ex: Executor,
    control: Arc<Control>,
    metrics: Arc<Metrics>,
    limiter: Arc<RateLimiter>,
    /// `None` = 连「只保留超时约束」的最小客户端都构造不出来。此时引擎仍然
    /// 可以构造与订阅，但 `start()` 会明确拒绝，而不是拿一个没有超时的客户端
    /// 去裸奔（见 `build_client`）。
    client: Option<reqwest::Client>,
    /// 客户端降级原因，留到第一次 `start()` 再播报。
    ///
    /// 不能在 `Engine::spawn()` 里播：广播通道此时还没有订阅者（桌面壳是在
    /// spawn 之后才订阅日志流的），`broadcast::Sender::send` 会直接丢弃它 ——
    /// 一条"已降级"的告警就此消失，正是本模块最反对的静默。
    client_note: Mutex<Option<String>>,
    /// 失败日志节流器：`"{代次}:{事件码}" -> 已出现次数`（见 [`FAILURE_LOG_BURST`]）。
    ///
    /// 键里带代次，新一代运行天然是一份新配额，无需在 `start()` 里清理；
    /// 条目数只与「代次 × 错误码」成正比，异常增长时整体清空兜底。
    failure_log_seen: Mutex<HashMap<String, u64>>,
    inner: Mutex<Inner>,
    epoch: Instant,
    seq: AtomicU64,
    metrics_tx: broadcast::Sender<MetricsSnapshot>,
    log_tx: broadcast::Sender<LogEntry>,
    event_tx: broadcast::Sender<RunEvent>,
}

impl Engine {
    /// 上限常量，供前端渲染范围控件。
    pub fn limits() -> EngineLimits {
        EngineLimits {
            max_workers: MAX_WORKERS,
            max_rate_mib: MAX_RATE_MIB,
            history_len: HISTORY_LEN as u32,
            tick_ms: TICK.as_millis() as u32,
        }
    }

    /// 创建引擎并启动后台指标采集任务。
    ///
    /// 可在**任意线程**调用：引擎会自动接管或自建 Tokio 运行时，
    /// 不要求调用方处于运行时上下文中（见 [`Executor`] 的事故复盘）。
    pub fn spawn() -> Arc<Engine> {
        let (metrics_tx, _) = broadcast::channel(64);
        let (log_tx, _) = broadcast::channel(256);
        let (event_tx, _) = broadcast::channel(64);
        let (client, client_note) = build_client();
        let engine = Arc::new(Engine {
            ex: Executor::acquire(),
            control: Arc::new(Control::default()),
            metrics: Arc::new(Metrics::default()),
            limiter: Arc::new(RateLimiter::new()),
            client,
            client_note: Mutex::new(client_note),
            failure_log_seen: Mutex::new(HashMap::new()),
            inner: Mutex::new(Inner::idle()),
            epoch: Instant::now(),
            seq: AtomicU64::new(0),
            metrics_tx,
            log_tx,
            event_tx,
        });
        engine.log_full(
            LogLevel::Info,
            codes::sys::ENGINE_READY,
            format!(
                "打流引擎已就绪（无头模式）· 并发上限 {MAX_WORKERS} · 限速上限 {MAX_RATE_MIB:.0} MiB/s · 采样周期 {} ms",
                TICK.as_millis()
            ),
        );
        let weak = Arc::downgrade(&engine);
        engine.ex.spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match weak.upgrade() {
                    Some(engine) => engine.tick(),
                    None => break,
                }
            }
        });
        engine
    }

    /// 订阅指标推流（每 TICK 一帧，前端由 Channel 转发，不做轮询）。
    pub fn subscribe_metrics(&self) -> broadcast::Receiver<MetricsSnapshot> {
        self.metrics_tx.subscribe()
    }

    /// 订阅日志推流。
    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    /// 订阅生命周期事件推流。
    pub fn subscribe_events(&self) -> broadcast::Receiver<RunEvent> {
        self.event_tx.subscribe()
    }

    fn at_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// 从外部写入一条日志（例如桌面壳捕获到的 panic、前端 JS 异常）。
    ///
    /// 复用同一条日志流，使前端「运行日志」视图与落盘日志都能看到崩溃原因，
    /// 而不是让 panic 信息只存在于没人会去看的 stderr 里。`source` 同样由
    /// `#[track_caller]` 记成调用点，壳层不必自己拼位置。
    #[track_caller]
    pub fn log_external(&self, level: LogLevel, message: impl Into<String>) {
        self.log_full(level, "", message);
    }

    /// 带稳定事件码的日志（自有代码一律走这里）。
    ///
    /// `source` 取**调用点**的 `文件:行`（`#[track_caller]`），而不是让每个调用方
    /// 手写常量：手写的位置会随代码移动而失效，日志反而变成误导排障的假线索。
    #[track_caller]
    fn log_full(&self, level: LogLevel, code: &str, message: impl Into<String>) {
        let location = std::panic::Location::caller();
        let entry = LogEntry {
            level,
            code: code.to_owned(),
            source: format!("{}:{}", location.file(), location.line()),
            message: message.into(),
            at_ms: self.at_ms(),
        };
        let _ = self.log_tx.send(entry);
    }

    /// 广播生命周期事件（带事件码与调用点，口径与日志完全一致）。
    #[track_caller]
    fn emit_event(&self, kind: RunEventKind, code: &str, message: impl Into<String>) {
        let location = std::panic::Location::caller();
        let event = RunEvent {
            kind,
            code: code.to_owned(),
            source: format!("{}:{}", location.file(), location.line()),
            message: message.into(),
            at_ms: self.at_ms(),
        };
        let _ = self.event_tx.send(event);
    }

    /// 失败日志的节流阀：返回 `Some(第几次)` 表示本次值得记录。
    ///
    /// 规则：同一 `key` 的前 [`codes::LOG_BURST`] 次完整记录，之后只记 10 的幂
    /// （第 10、100、1000… 次）。既抓住**首个故障现场**，又留下一条「故障在持续、
    /// 量级在涨」的轨迹，而不会让同一句话在几秒内把主日志刷爆、把故障前的上下文
    /// 挤出轮转窗口。
    fn failure_log_slot(&self, key: &str) -> Option<u64> {
        let mut seen = self
            .failure_log_seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if seen.len() > 1024 {
            seen.clear();
        }
        let count = seen.entry(key.to_owned()).or_insert(0);
        *count += 1;
        let count = *count;
        if codes::is_log_milestone(count) {
            Some(count)
        } else {
            None
        }
    }

    /// 记录一次网络失败（按代次 + 事件码节流）。
    ///
    /// 消息里显式带 `run=` 与 `worker#`：「哪一代、哪个 worker 出的问题」是
    /// 复现并发类缺陷的必要上下文，缺了它就只能靠猜。
    #[track_caller]
    fn note_failure(&self, level: LogLevel, run: u64, worker: u32, code: &str, detail: &str) {
        let key = format!("{run}:{code}");
        let Some(count) = self.failure_log_slot(&key) else {
            return;
        };
        let suffix = if count > codes::LOG_BURST {
            format!("（同类失败第 {count} 次，日志按里程碑节流）")
        } else {
            String::new()
        };
        self.log_full(
            level,
            code,
            format!("run={run} worker#{worker} {detail}{suffix}"),
        );
    }

    fn inner_lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 启动打流。所有入参校验都在此完成 —— 这里是**信任边界**。
    ///
    /// `start_run` 是 IPC Command，载荷可以来自任意前端代码或调试工具，
    /// 因此每个数字都按不可信输入处理：非法值明确拒绝，超范围值收敛后再如实
    /// 告知（见模块头的「信任边界」小节）。
    pub fn start(self: &Arc<Self>, request: StartRunRequest) -> Result<(), CoreError> {
        let url = request.url.trim().to_owned();
        if url.is_empty() {
            return Err(self.reject(
                codes::cfg::REJECTED,
                CoreError::InvalidInput("请填写目标地址".to_owned()),
            ));
        }
        if url.len() > MAX_URL_LEN {
            return Err(self.reject(
                codes::cfg::URL_TOO_LONG,
                CoreError::InvalidInput(format!("目标地址过长：上限 {MAX_URL_LEN} 字节")),
            ));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(self.reject(
                codes::cfg::URL_SCHEME,
                CoreError::InvalidInput("目标地址需以 http:// 或 https:// 开头".to_owned()),
            ));
        }
        if !request.authorized {
            return Err(self.reject(
                codes::cfg::NOT_AUTHORIZED,
                CoreError::NotAuthorized(
                    "请先确认：你拥有目标地址及其网络路径的明确测试授权".to_owned(),
                ),
            ));
        }
        if self.client.is_none() {
            return Err(self.reject(
                codes::sys::CLIENT_UNAVAILABLE,
                CoreError::Internal("HTTP 客户端不可用，无法发起请求".to_owned()),
            ));
        }
        // 降级原因留到这里播报：此时前端一定在监听日志流（见字段注释）。
        let degraded = self.announce_client_degradation();

        let threads = request.threads.clamp(1, MAX_WORKERS);
        let rate_mib = self.checked(codes::cfg::NON_FINITE, sanitize_rate_mib(request.rate_mib))?;
        let limit_gb = self.checked(
            codes::cfg::NON_FINITE,
            sanitize_limit(request.limit_gb, MAX_LIMIT_GB, "流量上限"),
        )?;
        let limit_minutes = self.checked(
            codes::cfg::NON_FINITE,
            sanitize_limit(request.limit_minutes, MAX_LIMIT_MINUTES, "时长上限"),
        )?;

        // 将“是否已运行”的检查和状态切换放在同一把锁内。Tauri command 可以并发
        // 调用，原先在这里分两次取锁会让两个 start 都观察到 Idle，继而各自拉起一组
        // worker。先标记 Running 后再释放锁，保证同一时刻只有一个调用能赢得启动权。
        //
        // 块的值是本轮运行代次，供下面的 worker 派发使用。
        let run = {
            let mut inner = self.inner_lock();
            if inner.running {
                drop(inner);
                return Err(self.reject(
                    codes::run::ALREADY_RUNNING,
                    CoreError::AlreadyRunning("测试已在运行中，请先停止".to_owned()),
                ));
            }

            self.metrics.reset();
            self.control.stop.store(false, Ordering::Release);
            // 代次在清掉 stop 之后递增：旧代次的 worker 即使醒来看到
            // `stop == false`，也会因为代次不匹配而退出，不会复活成多余并发。
            let run = self.control.begin_run();
            self.control.threads.store(threads, Ordering::Release);
            self.limiter.set_rate(mib_to_bps(rate_mib));

            inner.running = true;
            inner.url = url.clone();
            inner.started = Some(Instant::now());
            inner.elapsed_frozen = 0.0;
            inner.threads = threads;
            inner.rate_mib = rate_mib;
            inner.limit_gb = limit_gb;
            inner.limit_minutes = limit_minutes;
            inner.status = "测试运行中 · 并发与限速可实时调整".to_owned();
            inner.history.clear();
            inner.speed_bps = 0.0;
            inner.peak_bps = 0.0;
            inner.sum_bps = 0.0;
            inner.samples = 0;
            inner.last_bytes = 0;
            inner.last_tick = Instant::now();
            run
        };

        // 被收敛过就必须说清楚：限速值与安全停止边界是用户对第三方的承诺，
        // 静默修改等于让用户以为约束生效了。
        //
        // 播报刻意放在「已在运行」判定之后：一次注定被拒绝的启动不该在日志里
        // 留下「已收敛」的记录，那会把真正生效的那一次掩盖掉。
        for (requested, applied, label) in [
            (request.limit_gb, limit_gb, "流量上限"),
            (request.limit_minutes, limit_minutes, "时长上限"),
            (request.rate_mib, rate_mib, "限速值"),
        ] {
            if applied != requested {
                self.log_full(
                    LogLevel::Warn,
                    codes::cfg::CLAMPED,
                    format!(
                        "run={run} {label}已按引擎允许范围收敛：请求 {requested:e} → 实际 {applied:.3}"
                    ),
                );
            }
        }

        for index in 0..MAX_WORKERS {
            let engine = Arc::clone(self);
            let target = url.clone();
            self.ex
                .spawn(async move { worker(index, engine, target, run).await });
        }

        // 生效参数快照：事后复现一次运行需要的第一手材料。用户报「打流有问题」时，
        // 这一行就回答了「当时到底是几个并发、什么限速、有没有设上限」。
        self.log_full(
            LogLevel::Info,
            codes::run::CONFIG,
            format!(
                "run={run} 生效参数 · 目标 {url} · 并发 {threads}/{MAX_WORKERS} · 限速 {} · 流量上限 {} · 时长上限 {} · HTTP 客户端 {}",
                rate_summary(rate_mib),
                limit_summary(limit_gb, "GB"),
                limit_summary(limit_minutes, "分钟"),
                if degraded { "已降级" } else { "正常" },
            ),
        );
        self.emit_event(
            RunEventKind::Started,
            codes::run::STARTED,
            format!("已开始打流：{url}（run={run}）"),
        );
        Ok(())
    }

    /// 记录拒绝原因并同步广播，保证前端一定能看到失败原因。
    ///
    /// `code` 由调用点给出（见 [`codes`]），并随事件一起广播：前端与落盘日志
    /// 拿到的是同一个码，报障时只需报出它。
    #[track_caller]
    fn reject(&self, code: &str, error: CoreError) -> CoreError {
        let message = error.to_string();
        self.log_full(LogLevel::Error, code, message.clone());
        self.emit_event(RunEventKind::Rejected, code, message);
        error
    }

    /// 校验失败的统一出口：先广播原因，再原样返回错误。
    #[track_caller]
    fn checked<T>(&self, code: &str, result: Result<T, CoreError>) -> Result<T, CoreError> {
        result.map_err(|error| self.reject(code, error))
    }

    /// 播报一次「HTTP 客户端已降级」，播过即清空。
    ///
    /// 幂等：第一次 `start()` 时前端必定已在监听，这条告警只该出现一次。
    /// 返回本次会话的客户端是否处于降级配置（供生效参数快照引用）。
    fn announce_client_degradation(&self) -> bool {
        let note = self
            .client_note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(note) = note else { return false };
        self.log_full(LogLevel::Warn, codes::sys::CLIENT_DEGRADED, note);
        true
    }

    /// 停止打流（幂等）。
    ///
    /// 手动停止只作用于「当前正在运行的那一代」。
    pub fn stop(&self, reason: &str) {
        self.stop_run(None, reason);
    }

    /// 停止某一代运行：状态切换与 `control` 的两个原子量在**同一次取锁**内完成。
    ///
    /// **为什么必须同锁（真实竞态）**：旧实现先在锁外写 `stop` 与代次，再取
    /// `inner_lock()` 改 `inner.running`。并发的 `start()` 若在两步之间完成整个
    /// 启动临界区（清 `stop`、开新代次、置 `running = true`），随后 `stop()` 拿到
    /// 锁就会把**新的一代**标成已停止：快照显示待机，新一代 worker 却仍在打流，
    /// `tick()` 的流量/时长自动停止守卫随之全部失效。原子量与 `running` 同锁更新
    /// 后，`start()` 与停止互相串行，快照与 worker 看到的是同一个状态。
    ///
    /// `target` 是代次围栏：`tick()` 在锁内算出自动停止原因，但要到锁外才执行
    /// （不持锁广播）；若这期间用户已停止并重新启动，带过期代次的停止直接放弃，
    /// 不会误杀新任务。返回是否真的停止了这一次运行。
    fn stop_run(&self, target: Option<u64>, reason: &str) -> bool {
        let mut inner = self.inner_lock();
        if !inner.running {
            return false;
        }
        if let Some(run) = target {
            if self.control.run.load(Ordering::Acquire) != run {
                return false;
            }
        }

        // 自动停止（带代次围栏）与手动停止的事件类型/事件码不同，在此一次性判定，
        // 避免调用方再补一次广播、产生两条语义重复的记录。
        let (kind, code) = if target.is_some() {
            (RunEventKind::AutoStopped, codes::run::AUTO_STOPPED)
        } else {
            (RunEventKind::Stopped, codes::run::STOPPED)
        };
        let run = self.control.run.load(Ordering::Acquire);

        // 同时作废当前代次：worker 不必依赖 `stop` 标志本身 —— 该标志会被
        // 紧随其后的 `start()` 清掉，只看它会让旧 worker 有机会「复活」。
        self.control.stop.store(true, Ordering::Release);
        self.control.invalidate_run();
        inner.elapsed_frozen = inner
            .started
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(inner.elapsed_frozen);
        inner.running = false;
        inner.started = None;
        inner.speed_bps = 0.0;
        inner.status = reason.to_owned();
        let elapsed = inner.elapsed_frozen;
        drop(inner);

        // 停止时打一份「复现包」：时长、累计流量、成功/失败计数、错误分布。
        // 只留一句「已停止」无法回答「是正常收工，还是守卫/并发出了问题」，
        // 而这四个数字可以 —— 它们正是复现一次运行所需的最小上下文。
        let total_bytes = self.metrics.bytes.load(Ordering::Relaxed);
        let completed = self.metrics.completed.load(Ordering::Relaxed);
        let failures = self.metrics.failures.load(Ordering::Relaxed);
        let breakdown = self.metrics.error_snapshot();
        let errors = if breakdown.is_empty() {
            "无".to_owned()
        } else {
            breakdown
                .iter()
                .take(3)
                .map(|item| format!("{}×{}", item.code, item.count))
                .collect::<Vec<_>>()
                .join("、")
        };
        self.log_full(
            LogLevel::Warn,
            code,
            format!(
                "run={run} 已停止 · 原因「{reason}」 · 时长 {elapsed:.1}s · 累计 {:.2} MiB · 成功 {completed} · 失败 {failures} · 错误分布 {errors}",
                total_bytes as f64 / MIB
            ),
        );
        self.emit_event(kind, code, format!("{reason}（run={run}）"));
        true
    }

    /// 运行中动态调整并发与限速。
    ///
    /// 返回 `Result` 而不是静默忽略：`set_live_config` 同样是 IPC Command，
    /// 畸形载荷（`NaN`、超出范围的速率）必须让用户看见，而不是变成一次
    /// 悄无声息的「限速被关掉」。校验在取锁之前完成，避免持锁做广播。
    pub fn set_live(&self, patch: LiveConfigPatch) -> Result<(), CoreError> {
        let requested_rate = patch.rate_mib;
        let rate_mib = match requested_rate {
            Some(value) => Some(self.checked(codes::cfg::NON_FINITE, sanitize_rate_mib(value))?),
            None => None,
        };

        // 与 start() 同一口径：被收敛过就必须说清楚。运行中悄悄把限速换成别的
        // 数值，用户会以为新的限速已经生效 —— 而它正是用户对第三方做出的承诺。
        // 在取锁之前播报，避免持锁做广播。
        if let (Some(requested), Some(applied)) = (requested_rate, rate_mib) {
            if requested != applied {
                self.log_full(
                    LogLevel::Warn,
                    codes::cfg::CLAMPED,
                    format!("限速值已按引擎允许范围收敛：请求 {requested:e} → 实际 {applied:.3}"),
                );
            }
        }

        let has_patch = patch.threads.is_some() || rate_mib.is_some();
        if !has_patch {
            return Ok(());
        }

        let mut inner = self.inner_lock();
        if let Some(value) = patch.threads {
            let value = value.clamp(1, MAX_WORKERS);
            inner.threads = value;
            self.control.threads.store(value, Ordering::Release);
        }
        if let Some(value) = rate_mib {
            inner.rate_mib = value;
            self.limiter.set_rate(mib_to_bps(value));
        }
        if inner.running {
            inner.status = "测试运行中 · 并发与限速可实时调整".to_owned();
        }
        let applied = (inner.threads, inner.rate_mib, inner.running);
        drop(inner);

        // 生效值同样要留痕：运行中改过一次并发/限速，事后复盘的第一步就是
        // 确认「那一刻引擎实际用的是什么」，而不是用户以为自己填了什么。
        self.log_full(
            LogLevel::Info,
            codes::cfg::LIVE_APPLIED,
            format!(
                "实时调整已生效 · 并发 {} · 限速 {} · 运行中 {}",
                applied.0,
                rate_summary(applied.1),
                if applied.2 { "是" } else { "否" }
            ),
        );
        Ok(())
    }

    /// 当前是否正在打流。
    pub fn is_running(&self) -> bool {
        self.inner_lock().running
    }

    /// 生成快照。`with_history = true` 用于握手帧，`false` 用于流帧。
    pub fn snapshot(&self, with_history: bool) -> MetricsSnapshot {
        let inner = self.inner_lock();
        let total_bytes = self.metrics.bytes.load(Ordering::Relaxed);
        let completed = self.metrics.completed.load(Ordering::Relaxed);
        let failures = self.metrics.failures.load(Ordering::Relaxed);
        let elapsed_secs = if inner.running {
            inner
                .started
                .map(|started| started.elapsed().as_secs_f64())
                .unwrap_or(inner.elapsed_frozen)
        } else {
            inner.elapsed_frozen
        };
        MetricsSnapshot {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            phase: if inner.running {
                RunPhase::Running
            } else {
                RunPhase::Idle
            },
            url: inner.url.clone(),
            threads: inner.threads,
            rate_mib: inner.rate_mib,
            rate_limited: inner.rate_mib > 0.0,
            elapsed_secs,
            total_bytes,
            speed_bps: inner.speed_bps,
            peak_bps: inner.peak_bps,
            avg_bps: if inner.samples == 0 {
                0.0
            } else {
                inner.sum_bps / inner.samples as f64
            },
            latency_ms: self.metrics.latency_ms(),
            jitter_ms: self.metrics.jitter_ms(),
            completed,
            failures,
            success_rate: success_rate(completed, failures),
            in_flight: self.metrics.in_flight.load(Ordering::Relaxed),
            errors: self.metrics.error_snapshot(),
            last_error: self.metrics.last_error(),
            status: inner.status.clone(),
            history: if with_history {
                inner.history.iter().copied().collect()
            } else {
                Vec::new()
            },
            latest: inner.history.back().copied(),
        }
    }

    /// 周期性采集：计算速率、维护历史、判定自动停止条件、主动广播快照。
    fn tick(self: &Arc<Self>) {
        let auto_stop = {
            let mut inner = self.inner_lock();
            let now = Instant::now();
            let total_bytes = self.metrics.bytes.load(Ordering::Relaxed);
            let delta_time = now.duration_since(inner.last_tick).as_secs_f64().max(1e-6);
            let delta_bytes = total_bytes.saturating_sub(inner.last_bytes);
            inner.last_bytes = total_bytes;
            inner.last_tick = now;

            let mut reason = None;
            if inner.running {
                let speed = delta_bytes as f64 / delta_time;
                inner.speed_bps = speed;
                inner.peak_bps = inner.peak_bps.max(speed);
                inner.sum_bps += speed;
                inner.samples += 1;
                push_history(
                    &mut inner.history,
                    HistoryPoint {
                        speed_bps: speed,
                        latency_ms: self.metrics.latency_ms(),
                        jitter_ms: self.metrics.jitter_ms(),
                    },
                );
                let elapsed = inner
                    .started
                    .map(|started| started.elapsed().as_secs_f64())
                    .unwrap_or(inner.elapsed_frozen);
                reason =
                    decide_auto_stop(inner.limit_gb, inner.limit_minutes, total_bytes, elapsed);
            } else {
                inner.speed_bps = 0.0;
            }
            reason.map(|reason| (self.control.run.load(Ordering::Acquire), reason))
        };

        if let Some((run, reason)) = auto_stop {
            // 代次围栏：算原因与执行停止之间用户可能已经停止并重新启动，
            // 过期代次的自动停止必须放弃，否则会误杀刚启动的新任务。
            // 停止日志与事件的播报在 stop_run 内部完成（含 run 号与复现汇总）。
            self.stop_run(Some(run), &reason);
        }
        let _ = self.metrics_tx.send(self.snapshot(false));
    }
}

fn rate_summary(rate_mib: f64) -> String {
    if rate_mib > 0.0 {
        format!("{rate_mib:.1} MiB/s")
    } else {
        "不限速".to_owned()
    }
}

/// 自动停止上限摘要：`0` 在契约里表示「未设置」，日志里不应显示成 `0 GB`。
fn limit_summary(value: f64, unit: &str) -> String {
    if value > 0.0 {
        format!("{value:.3} {unit}")
    } else {
        "未设置".to_owned()
    }
}

// ---------------------------------------------------------------------------
// worker
// ---------------------------------------------------------------------------

/// 构造 HTTP 客户端，返回客户端与「降级原因」。
///
/// 旧实现结尾是 `Err(_) => reqwest::Client::new()`，有两个问题：
///
/// 1. **静默 fail-open**：兜底客户端会把连接/读空闲超时一起丢掉，于是任何一个
///    不回包的源站都能把 worker 永久挂死，用户只看到「卡住、字节不涨」；
/// 2. `Client::new()` 自身在构建失败时会 **panic**，而它是在 `Engine::spawn()`
///    里被调用的 —— 一个网络层问题可以变成启动即崩溃。
///
/// 现在先退到「只保留超时约束」的最小构建，并把降级原因交给调用方如实上报；
/// 连最小构建都失败时返回 `None`，由 `start()` 明确拒绝，而不是拿一个没有超时
/// 约束的客户端去裸奔。
fn build_client() -> (Option<reqwest::Client>, Option<String>) {
    let tuned = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .pool_max_idle_per_host(MAX_WORKERS as usize)
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .user_agent(USER_AGENT)
        .build();

    match tuned {
        Ok(client) => (Some(client), None),
        Err(error) => {
            let minimal = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .user_agent(USER_AGENT)
                .build();
            let reason = format!("HTTP 客户端未能按预期构建（{error}）");
            match minimal {
                Ok(client) => (
                    Some(client),
                    Some(format!("{reason}，已退到保留超时约束的最小配置")),
                ),
                Err(fallback) => (
                    None,
                    Some(format!(
                        "{reason}；最小配置同样失败（{fallback}），本次会话无法发起请求"
                    )),
                ),
            }
        }
    }
}

/// 由种子派生的确定性伪随机数（splitmix64 的收尾混合）。
///
/// 刻意不引入 `rand`：退避抖动只需要「不同 worker 取到不同值」，
/// 不需要密码学质量，也不值得为此扩大依赖树。
fn jitter_ms(seed: u64, span: u64) -> u64 {
    if span == 0 {
        return 0;
    }
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % span
}

/// 失败退避：指数增长 + 按 worker 抖动，并把等待切成小片。
///
/// 固定 `sleep(400ms)` 有两个问题：32 个 worker 会踩着同一节拍一起重试
/// （对已经不健康的源站形成惊群），以及 `stop()` 最坏要等满整个退避才生效。
/// 等待本轮运行被停止（来自 stop()，或新一轮 start() 让本代次作废）。
///
/// 为什么需要它：worker 循环顶部的 is_active 检查只拦得住**下一次**请求，
/// 拦不住已经发出去的那一次 —— 在途请求最长会拖到连接超时（10s）或读取超时
/// （30s），用户按下停止之后流量仍在继续，这与「安全停止」的承诺相悖。
/// 把请求整个包进 select! 里与它赛跑，停止就从「不再发新的」变成
/// 「已经发出去的也立刻停」。
async fn wait_for_stop(engine: &Engine, run: u64) {
    while engine.control.is_active(run) {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

async fn backoff(engine: &Engine, run: u64, index: u32, failures: u32) {
    let capped = BACKOFF_BASE_MS
        .saturating_mul(1_u64 << failures.min(4))
        .min(BACKOFF_MAX_MS);
    let seed = u64::from(index)
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(u64::from(failures));
    // 抖动落在 [capped/2, capped]，既错开重试时刻，又不让退避趋近于零。
    let total = Duration::from_millis(capped - jitter_ms(seed, capped / 2 + 1));

    // 退避同样留痕（按里程碑节流）：它回答的是「失败之后引擎在做什么」。
    // 缺了这段，日志就只剩一串「失败」，无法区分「快速重试」与「指数退避中」。
    engine.note_failure(
        LogLevel::Warn,
        run,
        index,
        codes::net::BACKOFF,
        &format!(
            "连续失败 {failures} 次，退避 {}ms 后重试",
            total.as_millis()
        ),
    );

    let mut slept = Duration::ZERO;
    while slept < total {
        // 每片都重新校验代次：旧代次的 worker 在下一片就能退出，
        // 不必睡满整个退避才发现自己已经过期。
        if !engine.control.is_active(run) {
            return;
        }
        let step = BACKOFF_SLICE.min(total - slept);
        tokio::time::sleep(step).await;
        slept += step;
    }
}

/// 单个 worker 的请求循环。
///
/// `run` 是它所属的运行代次：任何一次 `start()` / `stop()` 都会让它作废，
/// worker 随即退出。这是「实际并发不超过用户授权值」的硬保证。
async fn worker(index: u32, engine: Arc<Engine>, url: String, run: u64) {
    let Some(client) = engine.client.as_ref() else {
        return;
    };
    let mut request_id = 0_u64;
    let mut consecutive_failures = 0_u32;

    while engine.control.is_active(run) {
        let active = engine
            .control
            .threads
            .load(Ordering::Acquire)
            .clamp(1, MAX_WORKERS);
        if index >= active {
            tokio::time::sleep(Duration::from_millis(60)).await;
            continue;
        }

        request_id = request_id.wrapping_add(1);
        let target = cache_busted_url(&url, index, request_id);
        let started = Instant::now();
        engine.metrics.request_started();

        // 在途请求与「停止」赛跑：停止生效后立刻丢下这次请求（并如实收尾在途
        // 计数），而不是让超时替用户决定流量什么时候停。
        let result = tokio::select! {
            result = client
                .get(&target)
                .header("Cache-Control", "no-cache, no-store, max-age=0")
                .header("Pragma", "no-cache")
                .header("Accept-Encoding", "identity")
                .send() => result,
            _ = wait_for_stop(&engine, run) => {
                engine.metrics.request_finished();
                return;
            }
        };

        match result {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                    engine.metrics.add_error(
                        &format!("HTTP {}", status.as_u16()),
                        format!("服务器返回 HTTP {}", status.as_u16()),
                    );
                    engine.note_failure(
                        LogLevel::Error,
                        run,
                        index,
                        codes::net::HTTP_STATUS,
                        &format!(
                            "服务器返回 HTTP {}（连续失败 {consecutive_failures} 次）",
                            status.as_u16()
                        ),
                    );
                    engine.metrics.request_finished();
                    backoff(&engine, run, index, consecutive_failures).await;
                    continue;
                }
                consecutive_failures = 0;

                let mut stream = response.bytes_stream();
                let mut waiting_first_chunk = true;
                let mut healthy = true;
                let mut scaled_down = false;

                loop {
                    let chunk = tokio::select! {
                        chunk = stream.next() => chunk,
                        // 响应体读到一半被停止：同样立刻放弃，不再把一个可能
                        // 长达 READ_TIMEOUT 的读取跑完。
                        _ = wait_for_stop(&engine, run) => {
                            healthy = false;
                            break;
                        }
                    };
                    let Some(chunk) = chunk else { break };

                    if !engine.control.is_active(run) {
                        healthy = false;
                        break;
                    }
                    let active = engine
                        .control
                        .threads
                        .load(Ordering::Acquire)
                        .clamp(1, MAX_WORKERS);
                    if index >= active {
                        scaled_down = true;
                        break;
                    }
                    match chunk {
                        Ok(data) => {
                            let length = data.len();
                            if waiting_first_chunk {
                                engine
                                    .metrics
                                    .record_latency(started.elapsed().as_secs_f64() * 1000.0);
                                waiting_first_chunk = false;
                            }
                            if length > 0 {
                                engine
                                    .metrics
                                    .bytes
                                    .fetch_add(length as u64, Ordering::Relaxed);
                                engine.limiter.acquire(length).await;
                            }
                        }
                        Err(error) => {
                            healthy = false;
                            let code = classify(&error);
                            engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                            engine
                                .metrics
                                .add_error(&code, format!("响应流中断：{error}"));
                            engine.note_failure(
                                LogLevel::Error,
                                run,
                                index,
                                codes::net::STREAM_BROKEN,
                                &format!("响应流中断（{code}）：{error}"),
                            );
                            break;
                        }
                    }
                }

                if waiting_first_chunk && healthy {
                    engine
                        .metrics
                        .record_latency(started.elapsed().as_secs_f64() * 1000.0);
                }
                if healthy && !scaled_down {
                    engine.metrics.completed.fetch_add(1, Ordering::Relaxed);
                }
                engine.metrics.request_finished();
            }
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let code = classify(&error);
                engine.metrics.request_finished();
                engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                engine
                    .metrics
                    .add_error(&code, format!("请求失败：{error}"));
                engine.note_failure(
                    LogLevel::Error,
                    run,
                    index,
                    codes::net::REQUEST_FAILED,
                    &format!("请求失败（{code}）：{error}"),
                );
                backoff(&engine, run, index, consecutive_failures).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 无头单元测试：不启动任何窗口 / 浏览器即可验证核心逻辑
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_buster_preserves_query_and_fragment() {
        assert_eq!(
            cache_busted_url("https://example.test/file?a=1#part", 2, 3),
            "https://example.test/file?a=1&_ll=2-3#part"
        );
        assert_eq!(
            cache_busted_url("https://example.test/file", 0, 1),
            "https://example.test/file?_ll=0-1"
        );
    }

    #[test]
    fn auto_stop_respects_byte_limit() {
        let gib = 1_073_741_824_u64;
        assert!(decide_auto_stop(1.0, 0.0, gib - 1, 0.0).is_none());
        let reason = decide_auto_stop(1.0, 0.0, gib, 0.0).expect("应当触发流量阈值");
        assert!(reason.contains("流量目标"));
    }

    #[test]
    fn auto_stop_respects_time_limit_and_can_be_disabled() {
        assert!(decide_auto_stop(0.0, 10.0, 0, 599.9).is_none());
        assert!(decide_auto_stop(0.0, 10.0, 0, 600.0).is_some());
        // 两个阈值都为 0 表示关闭自动停止。
        assert!(decide_auto_stop(0.0, 0.0, u64::MAX, f64::MAX).is_none());
    }

    #[test]
    fn history_ring_is_bounded() {
        let mut history = VecDeque::new();
        for index in 0..(HISTORY_LEN + 50) {
            push_history(
                &mut history,
                HistoryPoint {
                    speed_bps: index as f64,
                    latency_ms: 0.0,
                    jitter_ms: 0.0,
                },
            );
        }
        assert_eq!(history.len(), HISTORY_LEN);
        assert_eq!(history.front().unwrap().speed_bps, 50.0);
    }

    #[test]
    fn success_rate_math() {
        assert_eq!(success_rate(0, 0), 0.0);
        assert_eq!(success_rate(3, 1), 75.0);
    }

    #[test]
    fn limits_are_published_to_frontend() {
        let limits = Engine::limits();
        assert_eq!(limits.max_workers, MAX_WORKERS);
        assert_eq!(limits.history_len, HISTORY_LEN as u32);
        assert_eq!(limits.tick_ms, 250);
    }

    #[test]
    fn cache_buster_handles_an_empty_query_string() {
        assert_eq!(
            cache_busted_url("https://example.test/file?", 0, 1),
            "https://example.test/file?_ll=0-1"
        );
        assert_eq!(
            cache_busted_url("https://example.test/file?a=1&", 0, 1),
            "https://example.test/file?a=1&_ll=0-1"
        );
    }

    #[test]
    fn a_new_run_invalidates_the_previous_generation() {
        let control = Control::default();
        let first = control.begin_run();
        assert!(control.is_active(first));

        // stop：代次作废。
        control.stop.store(true, Ordering::Release);
        control.invalidate_run();
        assert!(!control.is_active(first));

        // 关键在于 stop 标志随后被下一轮 start 清掉 —— 只看标志的实现会让旧
        // worker 在这里「复活」，变成第 33 个并发。
        control.stop.store(false, Ordering::Release);
        assert!(
            !control.is_active(first),
            "旧代次的 worker 不得因为 stop 被清掉而复活"
        );

        let second = control.begin_run();
        assert_ne!(first, second);
        assert!(
            control.is_active(second),
            "新代次的 worker 必须处于在岗状态"
        );
    }

    #[test]
    fn non_finite_rate_is_rejected_not_silently_unlimited() {
        assert!(sanitize_rate_mib(f64::NAN).is_err());
        assert!(sanitize_rate_mib(f64::INFINITY).is_err());
        assert!(sanitize_rate_mib(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn rate_is_clamped_but_tiny_positive_rates_survive() {
        assert_eq!(sanitize_rate_mib(-1.0).unwrap(), 0.0);
        assert_eq!(sanitize_rate_mib(0.0).unwrap(), 0.0);
        // 1e-300 语义合法：限速器以有界等待处理，不得改写成「不限速」。
        assert_eq!(sanitize_rate_mib(1e-300).unwrap(), 1e-300);
        assert_eq!(
            sanitize_rate_mib(MAX_RATE_MIB * 10.0).unwrap(),
            MAX_RATE_MIB
        );
    }

    #[test]
    fn non_finite_stop_limits_are_rejected() {
        assert!(sanitize_limit(f64::NAN, MAX_LIMIT_GB, "流量上限").is_err());
        assert!(sanitize_limit(f64::INFINITY, MAX_LIMIT_GB, "流量上限").is_err());
        assert!(sanitize_limit(f64::NAN, MAX_LIMIT_MINUTES, "时长上限").is_err());
    }

    #[test]
    fn the_hard_stop_ceiling_is_always_reachable() {
        // 关键不变量：上限乘上单位换算后仍落在 u64 字节计数可表达的范围内。
        // 否则 `limit_gb * GIB` 溢出成 `inf`，比较永远为假 —— 用户以为设了
        // 安全停止，实际是把守卫静默关掉。
        assert!(
            MAX_LIMIT_GB * GIB < u64::MAX as f64,
            "流量上限换算后会溢出，自动停止将永远不触发"
        );
        // 时长上限换算成秒之后同样不得溢出成 inf，否则 `elapsed >= limit`
        // 会永远为假。用函数返回值做断言，避免被常量折叠掉。
        let ceiling_seconds = sanitize_limit(MAX_LIMIT_MINUTES, MAX_LIMIT_MINUTES, "时长上限")
            .expect("上限本身必须合法")
            * 60.0;
        assert!(ceiling_seconds.is_finite());

        assert_eq!(
            sanitize_limit(f64::MAX, MAX_LIMIT_GB, "流量上限").unwrap(),
            MAX_LIMIT_GB
        );
        assert_eq!(sanitize_limit(-5.0, MAX_LIMIT_GB, "流量上限").unwrap(), 0.0);
        assert_eq!(sanitize_limit(2.0, MAX_LIMIT_GB, "流量上限").unwrap(), 2.0);
    }

    #[test]
    fn auto_stop_ignores_non_finite_thresholds() {
        assert!(decide_auto_stop(f64::NAN, f64::NAN, u64::MAX, f64::MAX).is_none());
        assert!(decide_auto_stop(f64::INFINITY, 0.0, u64::MAX, 0.0).is_none());
        // 正常阈值不受影响。
        assert!(decide_auto_stop(1.0, 0.0, GIB as u64, 0.0).is_some());
        assert!(decide_auto_stop(0.0, 1.0, 0, 60.0).is_some());
    }

    #[test]
    fn jitter_is_bounded_and_worker_specific() {
        assert_eq!(jitter_ms(7, 0), 0);
        for seed in 0..8u64 {
            assert!(jitter_ms(seed, 101) < 101);
        }
        let spread: std::collections::BTreeSet<u64> =
            (0..32u64).map(|id| jitter_ms(id, 1000)).collect();
        assert!(
            spread.len() > 1,
            "所有 worker 取到同一个抖动值 = 退避没有去同步"
        );
    }

    #[test]
    fn start_rejects_degenerate_payloads_before_spawning_anything() {
        // 校验发生在 worker 派发之前，因此本测试不需要任何真实 IO。
        let engine = Engine::spawn();
        let base = StartRunRequest {
            url: "https://example.test/file".to_owned(),
            threads: 4,
            rate_mib: 0.0,
            limit_gb: 0.0,
            limit_minutes: 0.0,
            authorized: true,
        };

        let nan_rate = engine.start(StartRunRequest {
            rate_mib: f64::NAN,
            ..base.clone()
        });
        assert!(matches!(nan_rate, Err(CoreError::InvalidInput(_))));
        assert!(!engine.is_running(), "被拒绝的请求不得进入运行态");

        let nan_limit = engine.start(StartRunRequest {
            limit_gb: f64::NAN,
            ..base.clone()
        });
        assert!(matches!(nan_limit, Err(CoreError::InvalidInput(_))));
        assert!(!engine.is_running());

        // 回归点：极小速率是合法 JSON，旧实现会让 `Duration::from_secs_f64`
        // 在 worker 里 panic（任务静默消失），这里必须能正常启动。
        engine
            .start(StartRunRequest {
                rate_mib: 1e-300,
                ..base
            })
            .expect("1e-300 MiB/s 是合法载荷");
        assert!(engine.is_running());
        assert_eq!(engine.snapshot(false).rate_mib, 1e-300);
        engine.stop("单测收尾");
    }

    /// 空转的 `stop()` 必须什么都不做。旧实现会在锁外写 `stop` 标志并作废代次，
    /// 一旦它与 `start()` 的启动临界区交错，就会把刚启动的一代标成已停止：
    /// 快照显示待机、worker 仍在打流。
    #[test]
    fn stop_while_idle_does_not_touch_control_state() {
        let engine = Engine::spawn();
        assert!(!engine.control.stop.load(Ordering::Acquire));
        let run_before = engine.control.run.load(Ordering::Acquire);

        engine.stop("空转停止");

        assert!(
            !engine.control.stop.load(Ordering::Acquire),
            "空转的 stop() 不得留下停止标志：并发的 start() 会被它回写"
        );
        assert_eq!(
            engine.control.run.load(Ordering::Acquire),
            run_before,
            "空转的 stop() 不得作废代次"
        );
        assert_eq!(engine.snapshot(false).phase, RunPhase::Idle);
    }

    /// 自动停止带代次围栏：`tick()` 算原因与执行停止之间用户若已停止并重新启动，
    /// 过期代次的自动停止必须放弃，否则会误杀新任务。
    #[test]
    fn stale_auto_stop_cannot_kill_a_newer_run() {
        let engine = Engine::spawn();
        let request = StartRunRequest {
            url: "https://example.test/file".to_owned(),
            threads: 2,
            rate_mib: 0.0,
            limit_gb: 0.0,
            limit_minutes: 0.0,
            authorized: true,
        };

        engine.start(request.clone()).expect("首轮启动必须成功");
        let stale_run = engine.control.run.load(Ordering::Acquire);
        engine.stop("用户先停止");
        engine.start(request).expect("用户重新启动必须成功");

        assert!(
            !engine.stop_run(Some(stale_run), "过期代次的自动停止"),
            "过期代次不得停止当前运行"
        );
        assert!(engine.is_running(), "新任务不得被过期的自动停止误杀");

        let current = engine.control.run.load(Ordering::Acquire);
        assert!(engine.stop_run(Some(current), "当前代次的自动停止"));
        assert!(!engine.is_running());
    }

    #[test]
    fn repeated_failures_are_throttled_but_keep_the_first_site() {
        let engine = Engine::spawn();
        let mut logs = engine.subscribe_logs();

        for _ in 0..105 {
            engine.note_failure(
                LogLevel::Error,
                7,
                3,
                codes::net::REQUEST_FAILED,
                "连接超时",
            );
        }

        let mut kept = Vec::new();
        while let Ok(entry) = logs.try_recv() {
            kept.push(entry);
        }
        assert_eq!(kept.len(), 5, "105 次失败只该留下 1/2/3/10/100 五条");
        assert!(kept
            .iter()
            .all(|entry| entry.code == codes::net::REQUEST_FAILED));
        assert!(
            kept[0].message.contains("run=7 worker#3"),
            "首条必须带代次与 worker 号：{}",
            kept[0].message
        );
        assert!(
            kept.last().expect("至少一条").message.contains("第 100 次"),
            "里程碑条目必须标明累计次数"
        );
    }

    /// 一轮运行必须留下「生效参数快照」与「停止汇总」：前者回答「当时配置是什么」，
    /// 后者回答「跑成了什么样」。二者缺一，事后就无法复现用户看到的现象。
    #[test]
    fn a_run_records_its_effective_config_and_a_stop_summary() {
        let engine = Engine::spawn();
        let mut logs = engine.subscribe_logs();

        engine
            .start(StartRunRequest {
                url: "https://example.test/file".to_owned(),
                threads: 8,
                rate_mib: 10.0,
                limit_gb: 1.0,
                limit_minutes: 0.0,
                authorized: true,
            })
            .expect("启动必须成功");
        engine.stop("单测收尾");

        let mut entries = Vec::new();
        while let Ok(entry) = logs.try_recv() {
            entries.push(entry);
        }

        let config = entries
            .iter()
            .find(|entry| entry.code == codes::run::CONFIG)
            .expect("必须留下生效参数快照");
        assert!(config.message.contains("并发 8/32"), "{}", config.message);
        assert!(
            config.message.contains("流量上限 1.000 GB"),
            "{}",
            config.message
        );
        assert!(
            config.message.contains("时长上限 未设置"),
            "{}",
            config.message
        );
        assert!(
            config.source.contains("engine.rs"),
            "来源必须指向产生它的代码：{}",
            config.source
        );

        let stopped = entries
            .iter()
            .find(|entry| entry.code == codes::run::STOPPED)
            .expect("停止必须留下复现汇总");
        assert!(
            stopped.message.contains("原因「单测收尾」"),
            "{}",
            stopped.message
        );
        assert!(stopped.message.contains("· 失败 "), "{}", stopped.message);
        assert!(
            stopped.message.contains("· 错误分布 "),
            "{}",
            stopped.message
        );
    }

    /// 被收敛过的安全边界必须在日志里留下「请求值 → 实际值」，并带上 run 号。
    #[test]
    fn clamped_limits_are_reported_with_the_run_id() {
        let engine = Engine::spawn();
        let mut logs = engine.subscribe_logs();

        engine
            .start(StartRunRequest {
                url: "https://example.test/file".to_owned(),
                threads: 4,
                rate_mib: MAX_RATE_MIB * 1000.0,
                limit_gb: 0.0,
                limit_minutes: 0.0,
                authorized: true,
            })
            .expect("超范围但语义合法的限速应当被收敛后启动");
        engine.stop("单测收尾");

        let mut clamps = Vec::new();
        while let Ok(entry) = logs.try_recv() {
            if entry.code == codes::cfg::CLAMPED {
                clamps.push(entry);
            }
        }
        assert_eq!(clamps.len(), 1, "只该为限速值产生一条收敛告警");
        assert!(
            clamps[0].message.contains("限速值"),
            "{}",
            clamps[0].message
        );
        assert!(clamps[0].message.contains("run="), "{}", clamps[0].message);
        assert!(
            clamps[0].message.contains("→ 实际"),
            "{}",
            clamps[0].message
        );
    }
}
