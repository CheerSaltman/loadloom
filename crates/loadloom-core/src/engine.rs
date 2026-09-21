//! 打流核心引擎：基于 Tokio 的异步高并发流量生成器。
//!
//! ## 无头设计约束
//! 本模块**不含任何 UI 代码**：没有窗口、没有渲染回调、没有对话框、没有事件循环、
//! 没有阻塞主线程的胶水逻辑。它只做三件事：
//! 1. 在后台 tokio 任务中产生流量并做令牌桶限速；
//! 2. 维护原子指标与环形采样历史；
//! 3. 通过 `broadcast` 通道**主动推流**（metrics / log / run_event），调用方永不轮询。
//!
//! ## 并发模型
//! * 一次性拉起 `MAX_WORKERS` 个异步 worker，靠 `AtomicU32` 实时增减「在岗」并发；
//! * 全局令牌桶限速，速率可运行中动态修改；
//! * 指标采集固定 250ms 一 tick，与 UI 帧率完全解耦。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::broadcast;

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

/// MiB/s -> byte/s
#[inline]
pub fn mib_to_bps(mib: f64) -> f64 {
    mib * MIB
}

// ---------------------------------------------------------------------------
// 纯函数：可独立单测的业务判定（不依赖任何 IO / 状态）
// ---------------------------------------------------------------------------

/// 追加缓存破坏参数，确保每个请求都真实穿透到源站。
pub fn cache_busted_url(url: &str, worker_id: u32, request_id: u64) -> String {
    let (base, fragment) = url.split_once('#').unwrap_or((url, ""));
    let suffix = if fragment.is_empty() {
        String::new()
    } else {
        format!("#{fragment}")
    };
    let separator = if base.contains('?') { "&" } else { "?" };
    format!("{base}{separator}_ll={worker_id}-{request_id}{suffix}")
}

/// 自动停止判定：返回 `Some(原因)` 表示应当停止。
pub fn decide_auto_stop(
    limit_gb: f64,
    limit_minutes: f64,
    total_bytes: u64,
    elapsed_secs: f64,
) -> Option<String> {
    if limit_gb > 0.0 && total_bytes as f64 >= limit_gb * GIB {
        return Some(format!("已达流量目标 {limit_gb:.2} GB，自动停止"));
    }
    if limit_minutes > 0.0 && elapsed_secs >= limit_minutes * 60.0 {
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
    stop: AtomicBool,
    threads: AtomicU32,
    rate_bps: AtomicU64,
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
    client: reqwest::Client,
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
        let engine = Arc::new(Engine {
            ex: Executor::acquire(),
            control: Arc::new(Control::default()),
            metrics: Arc::new(Metrics::default()),
            limiter: Arc::new(RateLimiter::new()),
            client: build_client(),
            inner: Mutex::new(Inner::idle()),
            epoch: Instant::now(),
            seq: AtomicU64::new(0),
            metrics_tx,
            log_tx,
            event_tx,
        });
        engine.log(LogLevel::Info, "打流引擎已就绪（无头模式）");
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

    /// 从外部写入一条日志（例如桌面壳捕获到的 panic）。
    ///
    /// 复用同一条日志流，使前端「运行日志」视图与落盘日志都能看到崩溃原因，
    /// 而不是让 panic 信息只存在于没人会去看的 stderr 里。
    pub fn log_external(&self, level: LogLevel, message: impl Into<String>) {
        self.log(level, message);
    }

    fn log(&self, level: LogLevel, message: impl Into<String>) {
        let entry = LogEntry {
            level,
            message: message.into(),
            at_ms: self.at_ms(),
        };
        let _ = self.log_tx.send(entry);
    }

    fn emit_event(&self, kind: RunEventKind, message: impl Into<String>) {
        let event = RunEvent {
            kind,
            message: message.into(),
            at_ms: self.at_ms(),
        };
        let _ = self.event_tx.send(event);
    }

    fn inner_lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 启动打流。所有入参校验都在此完成（对齐旧版业务规则，无行为变更）。
    pub fn start(self: &Arc<Self>, request: StartRunRequest) -> Result<(), CoreError> {
        let url = request.url.trim().to_owned();
        if url.is_empty() {
            return Err(self.reject(CoreError::InvalidInput("请填写目标地址".to_owned())));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(self.reject(CoreError::InvalidInput(
                "目标地址需以 http:// 或 https:// 开头".to_owned(),
            )));
        }
        if !request.authorized {
            return Err(self.reject(CoreError::NotAuthorized(
                "请先确认：你拥有目标地址及其网络路径的明确测试授权".to_owned(),
            )));
        }
        if self.inner_lock().running {
            return Err(self.reject(CoreError::AlreadyRunning(
                "测试已在运行中，请先停止".to_owned(),
            )));
        }

        let threads = request.threads.clamp(1, MAX_WORKERS);
        let rate_mib = request.rate_mib.clamp(0.0, MAX_RATE_MIB);

        self.metrics.reset();
        self.control.stop.store(false, Ordering::Release);
        self.control.threads.store(threads, Ordering::Release);
        self.control
            .rate_bps
            .store(mib_to_bps(rate_mib) as u64, Ordering::Release);
        self.limiter.set_rate(mib_to_bps(rate_mib));

        {
            let mut inner = self.inner_lock();
            inner.running = true;
            inner.url = url.clone();
            inner.started = Some(Instant::now());
            inner.elapsed_frozen = 0.0;
            inner.threads = threads;
            inner.rate_mib = rate_mib;
            inner.limit_gb = request.limit_gb.max(0.0);
            inner.limit_minutes = request.limit_minutes.max(0.0);
            inner.status = "测试运行中 · 并发与限速可实时调整".to_owned();
            inner.history.clear();
            inner.speed_bps = 0.0;
            inner.peak_bps = 0.0;
            inner.sum_bps = 0.0;
            inner.samples = 0;
            inner.last_bytes = 0;
            inner.last_tick = Instant::now();
        }

        for index in 0..MAX_WORKERS {
            let engine = Arc::clone(self);
            let target = url.clone();
            self.ex
                .spawn(async move { worker(index, engine, target).await });
        }

        self.log(
            LogLevel::Info,
            format!(
                "开始打流：{url} · 并发 {threads} · 限速 {}",
                rate_summary(rate_mib)
            ),
        );
        self.emit_event(RunEventKind::Started, format!("已开始打流：{url}"));
        Ok(())
    }

    /// 记录拒绝原因并同步广播，保证前端一定能看到失败原因。
    fn reject(&self, error: CoreError) -> CoreError {
        self.log(LogLevel::Error, error.to_string());
        self.emit_event(RunEventKind::Rejected, error.to_string());
        error
    }

    /// 停止打流（幂等）。
    pub fn stop(&self, reason: &str) {
        self.control.stop.store(true, Ordering::Release);
        let mut inner = self.inner_lock();
        if inner.running {
            inner.elapsed_frozen = inner
                .started
                .map(|started| started.elapsed().as_secs_f64())
                .unwrap_or(inner.elapsed_frozen);
            inner.running = false;
            inner.started = None;
            inner.speed_bps = 0.0;
            inner.status = reason.to_owned();
            drop(inner);
            self.log(LogLevel::Warn, reason.to_owned());
            self.emit_event(RunEventKind::Stopped, reason.to_owned());
        }
    }

    /// 运行中动态调整并发与限速。
    pub fn set_live(&self, patch: LiveConfigPatch) {
        let mut inner = self.inner_lock();
        if let Some(value) = patch.threads {
            let value = value.clamp(1, MAX_WORKERS);
            inner.threads = value;
            self.control.threads.store(value, Ordering::Release);
        }
        if let Some(value) = patch.rate_mib {
            let value = value.clamp(0.0, MAX_RATE_MIB);
            inner.rate_mib = value;
            let bps = mib_to_bps(value);
            self.control.rate_bps.store(bps as u64, Ordering::Release);
            self.limiter.set_rate(bps);
        }
        if inner.running {
            inner.status = "测试运行中 · 并发与限速可实时调整".to_owned();
        }
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
            reason
        };

        if let Some(reason) = auto_stop {
            self.stop(&reason);
            self.emit_event(RunEventKind::AutoStopped, reason);
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

// ---------------------------------------------------------------------------
// worker
// ---------------------------------------------------------------------------

fn build_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(MAX_WORKERS as usize)
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .user_agent("loadloom/0.4 (+authorized-load-test)");
    match builder.build() {
        Ok(client) => client,
        Err(_) => reqwest::Client::new(),
    }
}

async fn worker(index: u32, engine: Arc<Engine>, url: String) {
    let mut request_id = 0_u64;
    while !engine.control.stop.load(Ordering::Acquire) {
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
        engine.metrics.in_flight.fetch_add(1, Ordering::Relaxed);

        let result = engine
            .client
            .get(&target)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .header("Accept-Encoding", "identity")
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                    engine.metrics.add_error(
                        &format!("HTTP {}", status.as_u16()),
                        format!("服务器返回 HTTP {}", status.as_u16()),
                    );
                    engine.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    continue;
                }

                let mut stream = response.bytes_stream();
                let mut waiting_first_chunk = true;
                let mut healthy = true;
                let mut scaled_down = false;

                while let Some(chunk) = stream.next().await {
                    if engine.control.stop.load(Ordering::Acquire) {
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
                            engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                            engine
                                .metrics
                                .add_error(&classify(&error), format!("响应流中断：{error}"));
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
                engine.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
            }
            Err(error) => {
                engine.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
                engine.metrics.failures.fetch_add(1, Ordering::Relaxed);
                engine
                    .metrics
                    .add_error(&classify(&error), format!("请求失败：{error}"));
                tokio::time::sleep(Duration::from_millis(400)).await;
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
}
