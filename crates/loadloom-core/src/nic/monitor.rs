//! 监测器本体：后台采样、状态聚合、事件与日志播报、报告渲染。
//!
//! 所有判定都委托给 [`super::analysis`] 的纯函数，本模块只负责「什么时候采、
//! 采到之后怎么存、往哪里播报」这类编排工作 —— 于是编排与判定可以各自被单独
//! 测试，互不牵连。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tokio::sync::broadcast;

use super::analysis::{
    admin_enabled, advance, analyze, code_of, describe, fmt_link_speed, is_resolved, kind_of,
    level_of, link_state_of, monitored_by_default, worth_logging, Memory, NicDelta,
};
use super::{
    CAPABILITY_NOTE, MAX_ID_LEN, MAX_SELECTION, NIC_EVENT_LIMIT, NIC_HISTORY_LEN, NIC_SAMPLE,
};
use crate::codes;
use crate::contract::{
    CoreError, LogEntry, LogLevel, NicAdapterClass, NicAdapterDto, NicAdapterHistory, NicEvent,
    NicSeriesPoint, NicSnapshot,
};
use crate::engine::Executor;
use nicmon::RawAdapter;

/// 发送队列积压阈值（与判定层共用同一个数字，避免两处各写一份）。
use super::QUEUE_BACKLOG_PACKETS;

/// 一行待播报的日志（级别 + 稳定事件码 + 正文）。
struct LogLine {
    level: LogLevel,
    code: &'static str,
    message: String,
}

/// 待发日志的缓冲上限。
///
/// 桌面壳是在监测器构造**之后**才订阅日志流的，构造时那一帧（启动横幅、
/// 首次采样清单）必然没有订阅者。`broadcast::send` 无人接收时会**静默丢弃**消息
/// —— 一条「首次采样发现了哪些网卡」就这样消失，而它恰恰是排查「认不到我的网卡」
/// 时唯一的线索。这里把没人收的日志攒起来，等第一个订阅者出现再补发。
///
/// 上限存在的意义：如果整个进程生命周期内都没有订阅者（例如无头测试），
/// 这个缓冲必须是有界的，不能随采样无限增长。
const PENDING_LOG_LIMIT: usize = 64;

/// 单块网卡的运行时视图。
struct AdapterView {
    raw: RawAdapter,
    memory: Option<Memory>,
    delta: NicDelta,
    series: VecDeque<NicSeriesPoint>,
    monitored: bool,
}

struct NicInner {
    views: Vec<AdapterView>,
    /// 用户显式勾选的适配器 id（空 = 默认只监测物理网卡）。
    selection: Vec<String>,
    /// 最近事件（新的在前）。
    events: VecDeque<NicEvent>,
    last_error: String,
    samples: u64,
    last_tick: Instant,
    /// 还没有订阅者时攒下的日志（见 [`PENDING_LOG_LIMIT`]）。
    pending: VecDeque<LogLine>,
}

/// 网卡监测器：后台每 [`NIC_SAMPLE`] 采一次样，主动推流快照，并把异常写进日志流。
///
/// 与 [`crate::Engine`] 一样自带执行器（[`Executor`]），因此可以在任意线程构造；
/// 日志走**独立的广播通道**，由桌面壳转成与打流日志同一条 Event 流 —— 于是网卡
/// 事件与打流事件落在同一个时间轴、同一份磁盘日志里。
pub struct NicMonitor {
    ex: Executor,
    sampler: super::NicSampler,
    inner: Mutex<NicInner>,
    seq: AtomicU64,
    frame_tx: broadcast::Sender<NicSnapshot>,
    log_tx: broadcast::Sender<LogEntry>,
    epoch: Instant,
}

impl NicMonitor {
    /// 用真实网卡采集器创建并启动后台采样。
    pub fn spawn() -> Arc<Self> {
        let monitor = Self::new_with(Arc::new(nicmon::list_adapters));
        monitor.start_ticker();
        monitor
    }

    /// 用注入的采集器创建并启动后台采样（自定义数据源 / 端到端测试）。
    pub fn spawn_with(sampler: super::NicSampler) -> Arc<Self> {
        let monitor = Self::new_with(sampler);
        monitor.start_ticker();
        monitor
    }

    /// 只创建、不启动后台任务（单元测试用 [`Self::pump`] 逐帧驱动，结果完全确定）。
    pub(crate) fn new_with(sampler: super::NicSampler) -> Arc<Self> {
        let (frame_tx, _) = broadcast::channel(32);
        let (log_tx, _) = broadcast::channel(256);
        let supported = nicmon::is_supported();
        let banner = LogLine {
            level: if supported {
                LogLevel::Info
            } else {
                LogLevel::Warn
            },
            code: if supported {
                codes::nic::MONITOR_READY
            } else {
                codes::nic::UNSUPPORTED
            },
            message: if supported {
                format!(
                    "网卡监测已启动 · 平台 {} · 采样 {} ms · {}",
                    nicmon::platform_label(),
                    NIC_SAMPLE.as_millis(),
                    CAPABILITY_NOTE
                )
            } else {
                format!(
                    "网卡监测在当前平台不可用（{}）：{}",
                    nicmon::platform_label(),
                    CAPABILITY_NOTE
                )
            },
        };
        Arc::new(NicMonitor {
            ex: Executor::acquire(),
            sampler,
            inner: Mutex::new(NicInner {
                views: Vec::new(),
                selection: Vec::new(),
                events: VecDeque::new(),
                last_error: if supported {
                    String::new()
                } else {
                    format!(
                        "当前平台（{}）不支持读取网卡计数器",
                        nicmon::platform_label()
                    )
                },
                samples: 0,
                last_tick: Instant::now(),
                pending: VecDeque::from([banner]),
            }),
            seq: AtomicU64::new(0),
            frame_tx,
            log_tx,
            epoch: Instant::now(),
        })
    }

    /// 订阅快照推流（每 [`NIC_SAMPLE`] 一帧）。
    pub fn subscribe(&self) -> broadcast::Receiver<NicSnapshot> {
        self.frame_tx.subscribe()
    }

    /// 订阅监测日志（链路通断、速率变化、丢弃 / 错误 / 队列事件）。
    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    /// 拉取一次快照。`with_history = true` 时附带每块网卡的历史曲线。
    pub fn snapshot(&self, with_history: bool) -> NicSnapshot {
        let inner = self.lock();
        self.build_snapshot(&inner, with_history)
    }

    /// 全部接口（含虚拟 / 隧道），供界面勾选 —— 静态信息 + 最近一次的实时值。
    pub fn adapters(&self) -> Vec<NicAdapterDto> {
        let inner = self.lock();
        inner
            .views
            .iter()
            .map(|view| to_dto(view, inner.selection.contains(&view.raw.id)))
            .collect()
    }

    /// 设置监测范围。
    ///
    /// 不在当前列表里的 id **保留**在勾选里：网卡被临时拔出、休眠后未恢复时，
    /// 直接把它从勾选里删掉会让用户「明明勾了却不再监测」，插回后又要重新勾一遍。
    /// 这里只如实记录一条「当前不在列表里」的说明。
    pub fn select(&self, ids: Vec<String>) -> Result<(), CoreError> {
        let mut cleaned: Vec<String> = Vec::new();
        for id in ids {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            if id.len() > MAX_ID_LEN {
                return Err(CoreError::InvalidInput(format!(
                    "网卡标识过长：上限 {MAX_ID_LEN} 字节"
                )));
            }
            if !cleaned.iter().any(|kept| kept == id) {
                cleaned.push(id.to_owned());
            }
        }
        if cleaned.len() > MAX_SELECTION {
            return Err(CoreError::InvalidInput(format!(
                "一次最多监测 {MAX_SELECTION} 块网卡（收到 {}）",
                cleaned.len()
            )));
        }

        let mut lines: Vec<LogLine> = Vec::new();
        {
            let mut inner = self.lock();
            inner.selection = cleaned.clone();
            let known: HashSet<String> =
                inner.views.iter().map(|view| view.raw.id.clone()).collect();
            let missing: Vec<&str> = cleaned
                .iter()
                .map(String::as_str)
                .filter(|id| !known.contains(*id))
                .collect();
            if !missing.is_empty() {
                lines.push(LogLine {
                    level: LogLevel::Warn,
                    code: codes::nic::SELECTION_UNKNOWN,
                    message: format!(
                        "以下网卡当前不在接口列表里（可能已拔出 / 已禁用），插回后会自动恢复监测：{}",
                        missing.join("、")
                    ),
                });
            }
            apply_selection(&mut inner);
            lines.push(LogLine {
                level: LogLevel::Info,
                code: codes::nic::SELECTION_APPLIED,
                message: format!(
                    "监测范围已更新：{}",
                    if cleaned.is_empty() {
                        "默认（全部物理网卡）".to_owned()
                    } else {
                        format!("{} 块网卡", cleaned.len())
                    }
                ),
            });
        }
        self.emit(lines);
        self.broadcast();
        Ok(())
    }

    /// 生成可直接粘贴给维护者的网卡报告，并记录一条 `NIC-007`。
    pub fn report(&self) -> String {
        let text = self.report_text();
        self.emit(vec![LogLine {
            level: LogLevel::Info,
            code: codes::nic::REPORT,
            message: "已生成网卡报告（含当前读数、累计计数器与最近事件）".to_owned(),
        }]);
        text
    }

    /// 只渲染报告、不写日志：诊断导出会把报告拼在日志尾部，导出动作本身不该
    /// 再往日志里插一行。
    pub fn report_text(&self) -> String {
        let inner = self.lock();
        render_report(&inner)
    }

    /// 启动后台采样任务。
    fn start_ticker(self: &Arc<Self>) {
        // 先手工采一帧：界面刚打开就能看到读数，而不是先空半秒。
        self.pump();
        let weak = Arc::downgrade(self);
        self.ex.spawn(async move {
            let mut ticker = tokio::time::interval(NIC_SAMPLE);
            // 被系统挂起（休眠唤醒、CPU 满载）时不补跑：补跑会一次性产出间隔极短
            // 的一串帧，把速率算成根本不存在的尖峰。
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Some(monitor) = weak.upgrade() else { break };
                monitor.pump();
            }
        });
    }

    /// 采一帧、分析、推流。测试通过它逐帧驱动，因此判定结果完全确定。
    pub(crate) fn pump(&self) {
        self.pump_with_interval(None);
    }

    /// 与 [`Self::pump`] 相同，但允许调用方指定采样间隔。
    ///
    /// 测试必须走这条路：真实耗时是微秒级，速率断言会随之抖动到毫无意义
    /// （1.25 MB / 0.0002 秒 = 50 Gbit/s）。固定间隔让「速率换算」这件事
    /// 变成可断言的确定行为。
    pub(crate) fn pump_with_interval(&self, fixed_seconds: Option<f64>) {
        let sampled = (self.sampler)();
        let now_ms = self.at_ms();
        let mut lines: Vec<LogLine> = Vec::new();
        {
            let mut inner = self.lock();
            let now = Instant::now();
            let seconds =
                fixed_seconds.unwrap_or_else(|| now.duration_since(inner.last_tick).as_secs_f64());
            inner.last_tick = now;
            inner.samples += 1;
            match sampled {
                Ok(adapters) => {
                    inner.last_error.clear();
                    apply_sample(&mut inner, adapters, seconds, now_ms, &mut lines);
                }
                Err(error) => {
                    inner.last_error = error.detail.clone();
                    // 采样失败按里程碑节流：驱动故障时每 500ms 一行会把日志的轮转
                    // 窗口挤满，反而看不到故障前发生了什么。
                    if codes::is_log_milestone(inner.samples) {
                        lines.push(LogLine {
                            level: LogLevel::Warn,
                            code: codes::nic::SAMPLE_FAILED,
                            message: format!(
                                "网卡采样失败（第 {} 次，按里程碑节流）：{}",
                                inner.samples, error.detail
                            ),
                        });
                    }
                }
            }
        }
        self.emit(lines);
        self.broadcast();
    }

    fn lock(&self) -> MutexGuard<'_, NicInner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn at_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn emit(&self, lines: Vec<LogLine>) {
        // 没有订阅者时先攒着：这些日志迟早要写进磁盘，丢了就再也补不回来。
        if self.log_tx.receiver_count() == 0 {
            if lines.is_empty() {
                return;
            }
            let mut inner = self.lock();
            inner.pending.extend(lines);
            while inner.pending.len() > PENDING_LOG_LIMIT {
                inner.pending.pop_front();
            }
            return;
        }
        let staged: Vec<LogLine> = {
            let mut inner = self.lock();
            inner.pending.drain(..).collect()
        };
        if staged.is_empty() && lines.is_empty() {
            return;
        }
        for line in staged.into_iter().chain(lines) {
            let _ = self.log_tx.send(LogEntry {
                level: line.level,
                code: line.code.to_owned(),
                // 代码位置由桌面壳在落盘时补齐（编排层不该假装知道文件布局之外的
                // 东西），这里给出稳定的模块标识。
                source: "crates/loadloom-core/src/nic/monitor.rs".to_owned(),
                message: line.message,
                at_ms: self.at_ms(),
            });
        }
    }

    fn broadcast(&self) {
        let snapshot = {
            let inner = self.lock();
            self.build_snapshot(&inner, false)
        };
        let _ = self.frame_tx.send(snapshot);
    }

    fn build_snapshot(&self, inner: &NicInner, with_history: bool) -> NicSnapshot {
        let adapters: Vec<NicAdapterDto> = inner
            .views
            .iter()
            .filter(|view| view.monitored)
            .map(|view| to_dto(view, inner.selection.contains(&view.raw.id)))
            .collect();
        let history = if with_history {
            inner
                .views
                .iter()
                .filter(|view| view.monitored)
                .map(|view| NicAdapterHistory {
                    id: view.raw.id.clone(),
                    points: view.series.iter().copied().collect(),
                })
                .collect()
        } else {
            Vec::new()
        };
        NicSnapshot {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            supported: nicmon::is_supported(),
            note: CAPABILITY_NOTE.to_owned(),
            sampling: true,
            sample_ms: NIC_SAMPLE.as_millis() as u32,
            selection: inner.selection.clone(),
            adapters,
            events: inner.events.iter().cloned().collect(),
            last_error: inner.last_error.clone(),
            history,
        }
    }
}

/// 按当前勾选刷新每块网卡的「是否监测」。
fn apply_selection(inner: &mut NicInner) {
    let selection: HashSet<&str> = inner.selection.iter().map(String::as_str).collect();
    for view in &mut inner.views {
        let monitored = if selection.is_empty() {
            monitored_by_default(&view.raw)
        } else {
            selection.contains(view.raw.id.as_str())
        };
        if view.monitored && !monitored {
            // 停止监测即丢弃基线：重新勾选后若沿用旧基线，中间那段时间的计数器
            // 增量会被算成「刚刚一秒钟的流量」。
            view.memory = None;
            view.delta = NicDelta::default();
        }
        view.monitored = monitored;
    }
}

/// 应用一帧新采样：更新视图、产出事件与日志。
fn apply_sample(
    inner: &mut NicInner,
    adapters: Vec<RawAdapter>,
    seconds: f64,
    now_ms: u64,
    lines: &mut Vec<LogLine>,
) {
    let selection: HashSet<String> = inner.selection.iter().cloned().collect();
    let first_sample = inner.views.is_empty();
    let mut previous: HashMap<String, AdapterView> = inner
        .views
        .drain(..)
        .map(|view| (view.raw.id.clone(), view))
        .collect();

    let mut views: Vec<AdapterView> = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        let monitored = if selection.is_empty() {
            monitored_by_default(&adapter)
        } else {
            selection.contains(&adapter.id)
        };
        let old = previous.remove(&adapter.id);
        let mut series = old
            .as_ref()
            .map(|view| view.series.clone())
            .unwrap_or_default();
        let mut memory = old.as_ref().and_then(|view| view.memory);
        let mut delta = NicDelta::default();

        if monitored {
            // 第一帧没有上一帧可比，速率只能是 0；这一帧只用来建立基线。
            if let Some(old_view) = old.as_ref() {
                delta = analyze(&old_view.raw, &adapter, seconds);
            }
            let (next, findings) = advance(memory, &adapter, &delta, now_ms);
            memory = Some(next);
            for finding in findings {
                let code = code_of(&finding);
                let message = describe(&adapter, &finding, &delta);
                push_event(
                    inner,
                    NicEvent {
                        at_ms: now_ms,
                        adapter_id: adapter.id.clone(),
                        adapter: adapter.name.clone(),
                        kind: kind_of(&finding),
                        code: code.to_owned(),
                        resolved: is_resolved(&finding),
                        message: message.clone(),
                    },
                );
                if worth_logging(&finding) {
                    lines.push(LogLine {
                        level: level_of(&finding),
                        code,
                        message,
                    });
                }
            }
            series.push_back(NicSeriesPoint {
                rx_bps: delta.rx_bps,
                tx_bps: delta.tx_bps,
                rx_utilization: delta.rx_utilization,
                tx_utilization: delta.tx_utilization,
            });
            while series.len() > NIC_HISTORY_LEN {
                series.pop_front();
            }
        } else {
            // 未监测的网卡不保留基线：勾选回来的第一帧必须重新建立基准。
            memory = None;
        }

        views.push(AdapterView {
            raw: adapter,
            memory,
            delta,
            series,
            monitored,
        });
    }

    // 接口列表变化：只对「上次见过、这次没了」记一条；顺序变化不产生噪声。
    // （新出现的接口不需要单独播报：它已经出现在下一次快照里，而日志里多一行
    //  「发现了一块新网卡」对排障没有增量价值。）
    for (_, view) in previous {
        lines.push(LogLine {
            level: LogLevel::Info,
            code: codes::nic::ADAPTERS_CHANGED,
            message: format!(
                "网卡「{}」已从接口列表消失（拔出 / 禁用 / 驱动卸载）；勾选会保留，插回后自动恢复监测",
                view.raw.name
            ),
        });
    }
    // 首次采样：把「看见了什么」写下来。用户报「监测不到我的网卡」时，这一行
    // 直接说明 LoadLoom 究竟看到了哪些接口、默认纳入了哪几块 —— 没有它，
    // 就只能靠用户口述「设备管理器里那块网卡」来猜。
    if first_sample {
        let mut parts: Vec<String> = Vec::new();
        for class in [
            nicmon::AdapterClass::Physical,
            nicmon::AdapterClass::Virtual,
            nicmon::AdapterClass::Tunnel,
            nicmon::AdapterClass::Loopback,
            nicmon::AdapterClass::Other,
        ] {
            let count = views.iter().filter(|view| view.raw.class == class).count();
            if count > 0 {
                parts.push(format!("{} {}", class.label(), count));
            }
        }
        let monitored: Vec<&str> = views
            .iter()
            .filter(|view| view.monitored)
            .map(|view| view.raw.name.as_str())
            .collect();
        let shown = monitored
            .iter()
            .take(8)
            .copied()
            .collect::<Vec<_>>()
            .join("、");
        lines.push(LogLine {
            level: LogLevel::Info,
            code: codes::nic::DISCOVERED,
            message: format!(
                "首次采样：发现 {} 个接口（{}）· 默认监测 {}",
                views.len(),
                parts.join(" / "),
                if shown.is_empty() {
                    "（无）".to_owned()
                } else if monitored.len() > 8 {
                    format!("{shown} 等 {} 块", monitored.len())
                } else {
                    shown
                }
            ),
        });
    }
    inner.views = views;
    apply_selection(inner);
}

/// 事件入环形缓冲（新的在前）。
fn push_event(inner: &mut NicInner, event: NicEvent) {
    inner.events.push_front(event);
    while inner.events.len() > NIC_EVENT_LIMIT {
        inner.events.pop_back();
    }
}

/// 视图 -> 对外 DTO。
fn to_dto(view: &AdapterView, selected: bool) -> NicAdapterDto {
    let raw = &view.raw;
    let delta = &view.delta;
    NicAdapterDto {
        id: raw.id.clone(),
        name: raw.name.clone(),
        description: raw.description.clone(),
        class: NicAdapterClass::from(raw.class),
        media: raw.media.clone(),
        mac: raw.mac.clone(),
        mtu: raw.mtu,
        link_state: link_state_of(raw),
        oper_status: nicmon::oper_status_label(raw.oper_status).to_owned(),
        admin_enabled: admin_enabled(raw),
        transmit_speed_bps: raw.transmit_speed_bps as f64,
        receive_speed_bps: raw.receive_speed_bps as f64,
        monitored: view.monitored,
        selected,
        rx_bps: delta.rx_bps,
        tx_bps: delta.tx_bps,
        rx_utilization: delta.rx_utilization,
        tx_utilization: delta.tx_utilization,
        out_queue_len: raw.counters.out_queue_len,
        queue_backlog: raw.counters.out_queue_len >= QUEUE_BACKLOG_PACKETS,
        rx_discards_per_sec: delta.rx_discards_per_sec,
        tx_discards_per_sec: delta.tx_discards_per_sec,
        rx_errors_per_sec: delta.rx_errors_per_sec,
        tx_errors_per_sec: delta.tx_errors_per_sec,
        rx_bytes_total: raw.counters.in_octets,
        tx_bytes_total: raw.counters.out_octets,
        rx_discards_total: raw.counters.in_discards,
        tx_discards_total: raw.counters.out_discards,
        rx_errors_total: raw.counters.in_errors,
        tx_errors_total: raw.counters.out_errors,
        rx_unknown_protos_total: raw.counters.in_unknown_protos,
    }
}

/// 渲染可直接粘贴的报告。**必须把能力边界写进去**：用户拿到的报告里要能看出
/// 「温度与缓冲区占用率不在其中，且不是漏采，而是消费级设备读不到」。
fn render_report(inner: &NicInner) -> String {
    let mut text = String::new();
    text.push_str("LoadLoom 网卡报告\n");
    text.push_str(&format!(
        "平台：{}（支持读取：{}）\n采样周期：{} ms · 已采样 {} 次 · 监测范围：{}\n",
        nicmon::platform_label(),
        if nicmon::is_supported() { "是" } else { "否" },
        NIC_SAMPLE.as_millis(),
        inner.samples,
        if inner.selection.is_empty() {
            "默认（全部物理网卡）".to_owned()
        } else {
            format!("{} 块网卡（显式勾选）", inner.selection.len())
        }
    ));
    if !inner.last_error.is_empty() {
        text.push_str(&format!("最近一次采样错误：{}\n", inner.last_error));
    }
    text.push_str(&format!("能力边界：{CAPABILITY_NOTE}\n"));

    text.push_str("\n--- 正在监测的网卡 ---\n");
    let monitored: Vec<&AdapterView> = inner.views.iter().filter(|view| view.monitored).collect();
    if monitored.is_empty() {
        text.push_str("（无）\n");
    }
    for view in monitored {
        let raw = &view.raw;
        let delta = &view.delta;
        text.push_str(&format!(
            "\n● {} [{}] {}\n  驱动：{}\n  链路：{} · 协商速率 发 {} / 收 {} · MTU {} · MAC {}\n  实时：收 {:.2} MB/s（{:.1}%）· 发 {:.2} MB/s（{:.1}%）· 发送队列 {} 包\n  累计：收 {} / 发 {} 字节 · 丢弃 收 {} / 发 {} · 错误 收 {} / 发 {} · 未知协议 {}\n",
            raw.name,
            raw.class.label(),
            raw.media,
            if raw.description.is_empty() {
                "（驱动未提供）"
            } else {
                raw.description.as_str()
            },
            nicmon::oper_status_label(raw.oper_status),
            fmt_link_speed(raw.transmit_speed_bps),
            fmt_link_speed(raw.receive_speed_bps),
            raw.mtu,
            if raw.mac.is_empty() { "（无）" } else { raw.mac.as_str() },
            delta.rx_bps / 1_048_576.0,
            delta.rx_utilization,
            delta.tx_bps / 1_048_576.0,
            delta.tx_utilization,
            raw.counters.out_queue_len,
            raw.counters.in_octets,
            raw.counters.out_octets,
            raw.counters.in_discards,
            raw.counters.out_discards,
            raw.counters.in_errors,
            raw.counters.out_errors,
            raw.counters.in_unknown_protos,
        ));
    }

    text.push_str("\n--- 最近事件（新的在前）---\n");
    if inner.events.is_empty() {
        text.push_str("（暂无）\n");
    }
    for event in inner.events.iter().take(20) {
        text.push_str(&format!(
            "[{} ms] ({}) {} · {}\n",
            event.at_ms, event.code, event.adapter, event.message
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::NicEventKind;
    use nicmon::Counters;
    use std::sync::Mutex as StdMutex;

    fn adapter(id: &str, name: &str, class: nicmon::AdapterClass) -> RawAdapter {
        RawAdapter {
            id: id.to_owned(),
            index: 1,
            luid: 1,
            name: name.to_owned(),
            description: "Test NIC".to_owned(),
            class,
            media: "以太网".to_owned(),
            mac: "AA-BB-CC-DD-EE-FF".to_owned(),
            mtu: 1500,
            oper_status: nicmon::IF_OPER_UP,
            admin_status: 1,
            connect_state: nicmon::MEDIA_CONNECT_CONNECTED,
            hardware: true,
            connector_present: true,
            not_media_connected: false,
            transmit_speed_bps: 1_000_000_000,
            receive_speed_bps: 1_000_000_000,
            counters: Counters::default(),
        }
    }

    fn with_bytes(mut adapter: RawAdapter, in_octets: u64, out_octets: u64) -> RawAdapter {
        adapter.counters.in_octets = in_octets;
        adapter.counters.out_octets = out_octets;
        adapter
    }

    /// 按顺序回放采样帧，用完之后重复最后一帧（模拟「状态不再变化」）。
    fn sampler(frames: Vec<Vec<RawAdapter>>) -> super::super::NicSampler {
        let queue = StdMutex::new(frames);
        Arc::new(move || {
            let mut queue = queue.lock().expect("测试队列");
            if queue.len() > 1 {
                Ok(queue.remove(0))
            } else {
                Ok(queue.first().cloned().unwrap_or_default())
            }
        })
    }

    /// 取走当前排队的全部日志。
    ///
    /// 一帧可能同时产出多条（启动横幅 + 首次采样清单），逐条 `try_recv` 会让断言
    /// 悄悄读到另一条记录 —— 那种失败看起来像功能坏了，其实只是测试读错了行。
    fn pending(logs: &mut broadcast::Receiver<LogEntry>) -> Vec<LogEntry> {
        let mut entries = Vec::new();
        while let Ok(entry) = logs.try_recv() {
            entries.push(entry);
        }
        entries
    }

    #[test]
    fn frames_carry_only_monitored_adapters_and_real_rates() {
        let physical = adapter("luid-p", "以太网", nicmon::AdapterClass::Physical);
        let virtual_nic = adapter("luid-v", "vEthernet", nicmon::AdapterClass::Virtual);
        let monitor = NicMonitor::new_with(sampler(vec![
            vec![physical.clone(), virtual_nic.clone()],
            vec![
                with_bytes(physical.clone(), 1_250_000, 2_500_000),
                with_bytes(virtual_nic, 9_000_000, 0),
            ],
        ]));
        let mut frames = monitor.subscribe();
        monitor.pump_with_interval(Some(1.0));
        monitor.pump_with_interval(Some(1.0));

        // 第一帧只建立基线（速率必然为 0），要看的是第二帧。
        let baseline = frames.try_recv().expect("基线帧");
        assert_eq!(baseline.adapters[0].rx_bps, 0.0);
        let frame = frames.try_recv().expect("差值帧");
        assert_eq!(frame.selection, Vec::<String>::new());
        assert_eq!(frame.sample_ms, NIC_SAMPLE.as_millis() as u32);
        assert_eq!(frame.adapters.len(), 1, "默认只监测物理网卡");
        let dto = &frame.adapters[0];
        assert_eq!(dto.id, "luid-p");
        assert!(dto.monitored);
        assert!(!dto.selected, "来自默认范围而非显式勾选");
        assert!((dto.rx_bps - 10_000_000.0).abs() < 1.0, "{}", dto.rx_bps);
        assert!((dto.tx_bps - 20_000_000.0).abs() < 1.0);
        assert!((dto.rx_utilization - 1.0).abs() < 1e-6);
        assert!((dto.tx_utilization - 2.0).abs() < 1e-6);
        assert_eq!(dto.link_state, crate::contract::NicLinkState::Connected);
        assert!(dto.admin_enabled);

        let all = monitor.adapters();
        assert_eq!(all.len(), 2, "完整清单里虚拟网卡也要在（供勾选）");
        assert_eq!(all.iter().filter(|item| item.monitored).count(), 1);
        assert!(monitor.snapshot(false).history.is_empty());
    }

    #[test]
    fn an_explicit_selection_overrides_the_default_scope() {
        let physical = adapter("luid-p", "以太网", nicmon::AdapterClass::Physical);
        let virtual_nic = adapter("luid-v", "vEthernet", nicmon::AdapterClass::Virtual);
        let monitor = NicMonitor::new_with(sampler(vec![vec![physical, virtual_nic]]));
        let mut frames = monitor.subscribe();
        monitor.pump_with_interval(Some(1.0));
        let _ = frames.try_recv();

        monitor.select(vec!["luid-v".to_owned()]).expect("合法选择");
        let frame = frames.try_recv().expect("选择后立即推一帧");
        assert_eq!(frame.selection, vec!["luid-v".to_owned()]);
        assert_eq!(frame.adapters.len(), 1);
        assert_eq!(frame.adapters[0].id, "luid-v");
        assert!(frame.adapters[0].selected);

        monitor.select(Vec::new()).expect("清空选择");
        let frame = frames.try_recv().expect("清空后立即推一帧");
        assert!(frame.selection.is_empty());
        assert_eq!(frame.adapters.len(), 1);
        assert_eq!(frame.adapters[0].id, "luid-p");
    }

    #[test]
    fn selection_rejects_absurd_payloads_instead_of_truncating_silently() {
        let monitor = NicMonitor::new_with(sampler(vec![Vec::new()]));
        assert!(monitor.select(vec!["x".repeat(MAX_ID_LEN + 1)]).is_err());
        let too_many: Vec<String> = (0..=MAX_SELECTION)
            .map(|index| format!("id-{index}"))
            .collect();
        assert!(monitor.select(too_many).is_err());
        // 重复项与空白项被规整掉，而不是被当成两块网卡。
        monitor
            .select(vec![" a ".to_owned(), "a".to_owned(), "   ".to_owned()])
            .expect("规整后合法");
        assert_eq!(monitor.snapshot(false).selection, vec!["a".to_owned()]);
    }

    #[test]
    fn a_link_flap_lands_in_the_log_stream_and_the_event_list() {
        let live = adapter("luid-p", "以太网", nicmon::AdapterClass::Physical);
        let mut down = live.clone();
        down.oper_status = nicmon::IF_OPER_DOWN;
        down.connect_state = nicmon::MEDIA_CONNECT_DISCONNECTED;
        down.not_media_connected = true;
        let monitor =
            NicMonitor::new_with(sampler(vec![vec![live.clone()], vec![down], vec![live]]));
        let mut logs = monitor.subscribe_logs();

        monitor.pump_with_interval(Some(1.0));
        let first = pending(&mut logs);
        let banner = first
            .iter()
            .find(|entry| entry.code == codes::nic::MONITOR_READY)
            .expect("启动横幅");
        assert_eq!(banner.level, LogLevel::Info);
        let discovered = first
            .iter()
            .find(|entry| entry.code == codes::nic::DISCOVERED)
            .expect("首次采样清单");
        assert!(
            discovered.message.contains("以太网"),
            "{}",
            discovered.message
        );
        assert!(
            discovered.message.contains("物理网卡 1"),
            "{}",
            discovered.message
        );

        monitor.pump_with_interval(Some(1.0));
        let second = pending(&mut logs);
        assert_eq!(second.len(), 1, "一帧只该留下一条日志：{second:?}");
        let flap = &second[0];
        assert_eq!(flap.code, codes::nic::LINK_DOWN);
        assert_eq!(flap.level, LogLevel::Warn);
        assert!(flap.message.contains("以太网"), "{}", flap.message);

        monitor.pump_with_interval(Some(1.0));
        let third = pending(&mut logs);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].code, codes::nic::LINK_UP);

        let snapshot = monitor.snapshot(false);
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].code, codes::nic::LINK_UP);
        assert_eq!(snapshot.events[1].kind, NicEventKind::LinkDown);
        assert_eq!(snapshot.events[1].adapter_id, "luid-p");
        assert!(!snapshot.events[0].resolved);
        assert!(snapshot.last_error.is_empty());
    }

    #[test]
    fn a_failing_sampler_is_reported_without_killing_the_monitor() {
        let monitor = NicMonitor::new_with(Arc::new(|| Err(nicmon::NicError::call_failed(5))));
        let mut logs = monitor.subscribe_logs();
        let mut frames = monitor.subscribe();
        monitor.pump_with_interval(Some(1.0));
        let entries = pending(&mut logs);
        let failure = entries
            .iter()
            .find(|entry| entry.code == codes::nic::SAMPLE_FAILED)
            .expect("采样失败日志");
        assert!(
            failure.message.contains("Win32 状态 5"),
            "{}",
            failure.message
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry.code == codes::nic::DISCOVERED),
            "没读到数据就不该报「发现了哪些网卡」"
        );

        let frame = frames.try_recv().expect("失败也要出帧（界面才能显示原因）");
        assert!(frame.last_error.contains("Win32 状态 5"));
        assert!(frame.adapters.is_empty());
    }

    #[test]
    fn a_removed_adapter_is_reported_and_keeps_its_selection() {
        let first = adapter("luid-a", "以太网", nicmon::AdapterClass::Physical);
        let second = adapter("luid-b", "WLAN", nicmon::AdapterClass::Physical);
        let monitor = NicMonitor::new_with(sampler(vec![
            vec![first, second],
            vec![adapter("luid-a", "以太网", nicmon::AdapterClass::Physical)],
        ]));
        let mut logs = monitor.subscribe_logs();
        monitor.pump_with_interval(Some(1.0));
        let _ = pending(&mut logs);
        monitor.pump_with_interval(Some(1.0));
        let entries = pending(&mut logs);
        let removed = entries
            .iter()
            .find(|entry| entry.code == codes::nic::ADAPTERS_CHANGED)
            .expect("接口消失日志");
        assert!(removed.message.contains("WLAN"), "{}", removed.message);

        // 勾选里保留已拔出的网卡：插回后自动恢复监测，不必重新勾一遍。
        monitor.select(vec!["luid-b".to_owned()]).expect("合法选择");
        let snapshot = monitor.snapshot(false);
        assert_eq!(snapshot.selection, vec!["luid-b".to_owned()]);
        assert!(snapshot.adapters.is_empty(), "此刻它确实不在列表里");
    }

    #[test]
    fn history_is_only_filled_on_the_handshake_snapshot() {
        let monitor = NicMonitor::new_with(sampler(vec![vec![adapter(
            "luid-p",
            "以太网",
            nicmon::AdapterClass::Physical,
        )]]));
        let mut frames = monitor.subscribe();
        monitor.pump_with_interval(Some(1.0));
        monitor.pump_with_interval(Some(1.0));
        let frame = frames.try_recv().expect("帧");
        assert!(frame.history.is_empty(), "流帧不重传历史");

        let handshake = monitor.snapshot(true);
        assert_eq!(handshake.history.len(), 1);
        assert_eq!(handshake.history[0].id, "luid-p");
        assert_eq!(handshake.history[0].points.len(), 2);
        assert!(handshake.seq > frame.seq);
    }

    #[test]
    fn the_report_states_the_capability_boundary_and_the_numbers() {
        let monitor = NicMonitor::new_with(sampler(vec![vec![adapter(
            "luid-p",
            "以太网",
            nicmon::AdapterClass::Physical,
        )]]));
        let mut logs = monitor.subscribe_logs();
        monitor.pump_with_interval(Some(1.0));
        let _ = pending(&mut logs);
        let text = monitor.report();
        assert!(text.contains("以太网"), "{text}");
        assert!(text.contains("能力边界"), "{text}");
        assert!(text.contains("不可获得"), "{text}");
        assert!(text.contains("温度"), "{text}");
        assert!(text.contains("1.00 Gbps"), "{text}");
        let entries = pending(&mut logs);
        assert!(
            entries.iter().any(|entry| entry.code == codes::nic::REPORT),
            "生成报告也要留痕：{entries:?}"
        );
    }

    /// 回归点：桌面壳在监测器构造**之后**才订阅日志流，早期那几帧日志没有订阅者，
    /// 若直接 `broadcast::send` 就会被静默丢弃 —— 用户看到的日志里会缺掉
    /// 「首次采样发现了哪些网卡」，而那是排查「认不到我的网卡」的唯一线索。
    #[test]
    fn logs_emitted_before_anyone_subscribed_are_delivered_later() {
        let monitor = NicMonitor::new_with(sampler(vec![vec![adapter(
            "luid-p",
            "以太网",
            nicmon::AdapterClass::Physical,
        )]]));
        // 还没有订阅者就先跑两帧（真实场景里这就是窗口刚起来的那半秒）。
        monitor.pump_with_interval(Some(1.0));
        monitor.pump_with_interval(Some(1.0));

        let mut logs = monitor.subscribe_logs();
        monitor.pump_with_interval(Some(1.0));
        let entries = pending(&mut logs);
        let codes: Vec<&str> = entries.iter().map(|entry| entry.code.as_str()).collect();
        assert!(
            codes.contains(&codes::nic::MONITOR_READY),
            "补发的日志里必须有启动横幅：{codes:?}"
        );
        assert!(
            codes.contains(&codes::nic::DISCOVERED),
            "补发的日志里必须有首次采样清单：{codes:?}"
        );
        assert!(
            codes.len() <= PENDING_LOG_LIMIT + 1,
            "补发不能无上限：{codes:?}"
        );
    }

    /// 真机自检：走一遍真实采集器（Windows 上是 `GetIfTable2`），确认「系统调用 ->
    /// 结构解析 -> 分析」这条链在本机真的通。
    ///
    /// 不支持的平台上直接返回：CI 的 Linux 跑腿不需要假装自己能读网卡，
    /// 但 Windows 上如果读不到（接口表为空、字段解析崩了），这里必须失败。
    #[test]
    fn the_real_sampler_reads_this_machine_without_panicking() {
        if !nicmon::is_supported() {
            return;
        }
        let monitor = NicMonitor::new_with(Arc::new(nicmon::list_adapters));
        monitor.pump_with_interval(Some(1.0));
        let snapshot = monitor.snapshot(true);
        assert!(snapshot.supported);
        assert!(
            snapshot.last_error.is_empty(),
            "真机采集失败：{}",
            snapshot.last_error
        );
        let all = monitor.adapters();
        assert!(!all.is_empty(), "任何一台联网机器都至少有回环接口");
        let mut ids: Vec<&str> = all.iter().map(|item| item.id.as_str()).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "接口标识必须唯一（LUID 冲突）");
        for adapter in &all {
            assert!(
                adapter.class == crate::contract::NicAdapterClass::Loopback
                    || !adapter.media.is_empty(),
                "介质类型必须能翻译成人类可读的标签"
            );
        }
    }
}
