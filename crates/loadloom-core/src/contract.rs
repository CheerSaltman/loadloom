//! 前后端通信契约 —— **单一事实来源 (Single Source of Truth)**。
//!
//! 每个结构体同时派生：
//! * `serde`   —— 定义 JSON 线格式；
//! * `specta::Type` —— 供 `tauri-specta` 自动导出 TypeScript 类型。
//!
//! 前端 `src/bindings.ts` 是与之逐字段对齐的**镜像契约**。
//!
//! > 之所以是「镜像」而非「生成物」：`tauri-specta` 目前只发布到 `2.0.0-rc.25`
//! > （非稳定版），因此当前走**降级路径** —— 手写镜像 + 机械护栏。
//! > 护栏由 `tests/contract_wire.rs` 承担：它直接读取并解析 `bindings.ts`，
//! > 逐字段比对 serde 的真实输出，任何一侧漂移都会导致测试失败。
//! > 待 `tauri-specta` 稳定后，仅需用自动生成结果覆盖 `bindings.ts`，调用侧零改动。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 引擎硬性上限。前端据此渲染滑块范围，避免硬编码数字与后端不一致。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EngineLimits {
    pub max_workers: u32,
    pub max_rate_mib: f64,
    pub history_len: u32,
    pub tick_ms: u32,
}

/// 流量模型。`LocalPt` 只接受回环或私有地址，避免误把仿真流量发往公网。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TrafficProfile {
    #[default]
    HttpDownload,
    LocalPt,
}

/// 启动打流请求 —— Command `start_run` 的入参。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartRunRequest {
    /// 目标地址，必须以 `http://` 或 `https://` 开头。
    pub url: String,
    /// 并发 worker 数（1..=MAX_WORKERS）。
    #[serde(default)]
    pub threads: u32,
    /// 全局限速（MiB/s），`0` 表示不限速。
    #[serde(default)]
    pub rate_mib: f64,
    /// 累计流量达到该值（GB）自动停止，`0` 表示关闭。
    #[serde(default)]
    pub limit_gb: f64,
    /// 运行时长达到该值（分钟）自动停止，`0` 表示关闭。
    #[serde(default)]
    pub limit_minutes: f64,
    /// 渐进升压时长（秒）。`0` 表示启动时立即达到目标并发。
    #[serde(default)]
    pub ramp_up_secs: f64,
    /// 流量模型；PT 仿真会轮转多个局域网 peer，并请求不同字节区间。
    #[serde(default)]
    pub profile: TrafficProfile,
    /// PT 仿真的额外 peer 地址，主 `url` 始终作为第一个 peer。
    #[serde(default)]
    pub peer_urls: Vec<String>,
    /// 尝试数达到最小样本后，失败率达到该百分比自动熔断；`0` 表示关闭。
    #[serde(default)]
    pub failure_stop_percent: f64,
    /// 首包时延连续超标约 1 秒后自动熔断；`0` 表示关闭。
    #[serde(default)]
    pub latency_stop_ms: f64,
    /// 授权确认门禁：未确认则拒绝启动。
    #[serde(default)]
    pub authorized: bool,
}

/// 运行中动态调整 —— Command `set_live_config` 的入参。
/// 两个字段均为 `None` 时表示「保持现状」。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LiveConfigPatch {
    #[serde(default)]
    pub threads: Option<u32>,
    #[serde(default)]
    pub rate_mib: Option<f64>,
}

/// 单个采样点（每 TICK 产生一个）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub speed_bps: f64,
    pub latency_ms: f64,
    pub jitter_ms: f64,
}

/// 按错误码聚合的计数项。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCount {
    pub code: String,
    pub count: u64,
}

/// 运行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    Idle,
    Running,
}

/// 运行态快照 —— Command `get_snapshot` 的返回值，同时是 Event `metrics` 流的帧载荷。
///
/// * `history` **仅在握手帧**（`get_snapshot`）填充，流帧恒为空数组；
/// * `latest`  **仅在流帧**填充，供前端增量追加，从而避免每 250ms 重传 120 点历史。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    /// 单调递增序号，用于前端丢弃乱序/重复帧。
    pub seq: u64,
    pub phase: RunPhase,
    pub url: String,
    pub threads: u32,
    pub rate_mib: f64,
    pub rate_limited: bool,
    pub elapsed_secs: f64,
    pub total_bytes: u64,
    pub speed_bps: f64,
    pub peak_bps: f64,
    pub avg_bps: f64,
    pub latency_ms: f64,
    pub jitter_ms: f64,
    pub completed: u64,
    pub failures: u64,
    /// 成功率百分比（0..=100）。
    pub success_rate: f64,
    pub in_flight: u32,
    pub errors: Vec<ErrorCount>,
    pub last_error: String,
    pub status: String,
    pub pressure_level: PressureLevel,
    pub pressure_reason: String,
    pub history: Vec<HistoryPoint>,
    pub latest: Option<HistoryPoint>,
}

/// 压力监测等级。Critical 会触发自动停止，Warning 只告警。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PressureLevel {
    #[default]
    Normal,
    Warning,
    Critical,
}

/// 真实 BitTorrent swarm 压测请求。负载数据只允许进入 RAM piece 缓冲，完成
/// 哈希校验后立即丢弃；Go sidecar 不创建下载文件。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PtStartRequest {
    pub source: String,
    #[serde(default)]
    pub max_connections: u32,
    #[serde(default)]
    pub ram_mib: u32,
    #[serde(default)]
    pub duration_secs: u32,
    #[serde(default)]
    pub max_download_gib: f64,
    #[serde(default)]
    pub rate_mib: f64,
    #[serde(default)]
    pub stalled_peer_secs: u32,
    #[serde(default)]
    pub authorized: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PtPhase {
    #[default]
    Idle,
    Starting,
    Metadata,
    Downloading,
    Completed,
    Failed,
}

/// Go PT sidecar 每 500 ms 推送的只读快照。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PtSnapshot {
    pub seq: u64,
    pub phase: PtPhase,
    pub name: String,
    pub info_hash: String,
    pub elapsed_secs: f64,
    pub progress_percent: f64,
    pub total_bytes: u64,
    pub wire_bytes: u64,
    pub verified_bytes: u64,
    pub wasted_bytes: u64,
    pub speed_bps: f64,
    pub average_bps: f64,
    pub active_peers: u32,
    pub pending_peers: u32,
    pub half_open_peers: u32,
    pub connected_seeders: u32,
    pub useful_peers: u32,
    pub stalled_peers: u32,
    pub peer_handshakes: u64,
    pub closed_peers: u64,
    pub dead_peers: u64,
    pub dead_peer_percent: f64,
    pub tracker_errors: u64,
    pub tracker_successes: u64,
    pub good_pieces: u64,
    pub bad_pieces: u64,
    pub ram_used_bytes: u64,
    pub ram_peak_bytes: u64,
    pub ram_limit_bytes: u64,
    pub storage_errors: u64,
    pub pressure_level: PressureLevel,
    pub pressure_reason: String,
    pub status: String,
    pub last_error: String,
    pub payload_persistence: String,
}

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 日志条目 —— Event `log` 流的载荷（长耗时任务的进度/告警广播，前端零轮询）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub level: LogLevel,
    /// 稳定事件码（见 [`crate::codes`]），形如 `NET-002`；空串表示无码。
    ///
    /// 有了它，用户报障时不必复述整段中文文案：报出事件码即可精确定位到
    /// 产生它的那一条代码分支（这是「可定位」的硬前提）。
    pub code: String,
    /// 产生这条日志的代码位置（`文件:行`），panic 由 shell 填充 `文件:行:列`。
    pub source: String,
    pub message: String,
    /// 相对进程启动的毫秒时间戳。
    pub at_ms: u64,
}

/// 生命周期事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RunEventKind {
    Started,
    Stopped,
    AutoStopped,
    Rejected,
}

/// 生命周期事件 —— Event `run_event` 流的载荷。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub kind: RunEventKind,
    /// 稳定事件码（见 [`crate::codes`]）；拒绝事件沿用被拒原因对应的码。
    pub code: String,
    /// 产生这个事件的代码位置（`文件:行`）。
    pub source: String,
    pub message: String,
    pub at_ms: u64,
}

/// 核心错误 —— Command 失败时返回给前端的强类型错误。
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CoreError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotAuthorized(String),
    #[error("{0}")]
    AlreadyRunning(String),
    #[error("{0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// 网卡链路监测
// ---------------------------------------------------------------------------

/// 适配器类别 —— 与 `nicmon::AdapterClass` 的线格式一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NicAdapterClass {
    /// 物理网卡（默认纳入监测）。
    Physical,
    /// 虚拟网卡（Hyper-V / VMware / VPN 客户端……）。
    Virtual,
    /// 软件回环。
    Loopback,
    /// 隧道（Teredo / ISATAP / 6to4……）。
    Tunnel,
    /// 其它接口（点对点端点等）。
    Other,
}

impl From<nicmon::AdapterClass> for NicAdapterClass {
    fn from(class: nicmon::AdapterClass) -> Self {
        match class {
            nicmon::AdapterClass::Physical => NicAdapterClass::Physical,
            nicmon::AdapterClass::Virtual => NicAdapterClass::Virtual,
            nicmon::AdapterClass::Loopback => NicAdapterClass::Loopback,
            nicmon::AdapterClass::Tunnel => NicAdapterClass::Tunnel,
            nicmon::AdapterClass::Other => NicAdapterClass::Other,
        }
    }
}

/// 链路状态（把操作状态、连接状态与驱动标志位归成一个用户能懂的判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NicLinkState {
    /// 已连接且可用。
    Connected,
    /// 已断开（网线拔出 / 无线未关联 / 驱动标志位说未连接）。
    Disconnected,
    /// 休眠等待（例如等待 Wi-Fi 关联或拨号）。
    Dormant,
    /// 设备不存在（已拔出 / 已禁用；历史残留条目常见）。
    NotPresent,
    /// 无法判定。
    Unknown,
}

/// 监测事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NicEventKind {
    LinkUp,
    LinkDown,
    SpeedChange,
    CounterReset,
    DiscardSpike,
    ErrorSpike,
    QueueBacklog,
    AdapterAdded,
    AdapterRemoved,
    Selection,
}

/// 实时曲线点（收 / 发速率与利用率）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NicSeriesPoint {
    pub rx_bps: f64,
    pub tx_bps: f64,
    /// 收方向利用率百分比（`0..=100`）。
    pub rx_utilization: f64,
    /// 发方向利用率百分比（`0..=100`）。
    pub tx_utilization: f64,
}

/// 单块网卡的历史序列（**仅握手帧**填充，流帧恒为空数组）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NicAdapterHistory {
    pub id: String,
    pub points: Vec<NicSeriesPoint>,
}

/// 一块网卡的实时状态与累计读数。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NicAdapterDto {
    /// 稳定标识（LUID）：重插网卡、接口索引变化后依然是同一块网卡。
    pub id: String,
    /// 系统名称（「以太网 2」「WLAN」）。
    pub name: String,
    /// 驱动描述（芯片型号 / 厂商）。
    pub description: String,
    pub class: NicAdapterClass,
    /// 介质标签（以太网 / Wi-Fi / 移动宽带……）。
    pub media: String,
    /// MAC 地址；无物理地址时为空串。
    pub mac: String,
    pub mtu: u32,
    pub link_state: NicLinkState,
    /// 运行状态的中文标签（日志与报告引用同一份翻译）。
    pub oper_status: String,
    pub admin_enabled: bool,
    /// 协商发送速率（bit/s，0 = 未知）。
    pub transmit_speed_bps: f64,
    /// 协商接收速率（bit/s，0 = 未知）。
    pub receive_speed_bps: f64,
    /// 是否正在被采样（未采样时下面所有实时字段恒为 0，界面据此显示「未监测」）。
    pub monitored: bool,
    /// 是否被用户显式勾选（`false` 表示来自「默认纳入物理网卡」）。
    pub selected: bool,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_utilization: f64,
    pub tx_utilization: f64,
    /// 发送队列当前积压的包数（`OutQLen`）。
    pub out_queue_len: u64,
    /// 是否处于队列积压状态（阈值判定结果，界面用它染红）。
    pub queue_backlog: bool,
    pub rx_discards_per_sec: f64,
    pub tx_discards_per_sec: f64,
    pub rx_errors_per_sec: f64,
    pub tx_errors_per_sec: f64,
    pub rx_bytes_total: u64,
    pub tx_bytes_total: u64,
    pub rx_discards_total: u64,
    pub tx_discards_total: u64,
    pub rx_errors_total: u64,
    pub tx_errors_total: u64,
    pub rx_unknown_protos_total: u64,
}

/// 监测事件 —— 事件流的「发生了什么」，与日志同源同码。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NicEvent {
    /// 相对进程启动的毫秒时间戳（与日志时间轴一致，便于对齐）。
    pub at_ms: u64,
    pub adapter_id: String,
    pub adapter: String,
    pub kind: NicEventKind,
    /// 稳定事件码（见 [`crate::codes::nic`]）。
    pub code: String,
    /// 是否已结束（尖峰类事件的收尾记录为 `true`，界面用普通样式显示）。
    pub resolved: bool,
    pub message: String,
}

/// 网卡监测快照 —— Command `get_nic_snapshot` 的返回值，也是 Event 推流的帧。
///
/// * `history` **仅在握手帧**（`get_nic_snapshot(true)`）填充，流帧恒为空数组；
/// * `adapters` 只包含**正在监测**的网卡；完整清单（含虚拟 / 隧道）走
///   Command `list_nic_adapters` —— 44 个接口的长字符串没必要每 500ms 重传一遍。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NicSnapshot {
    /// 单调递增序号，用于前端丢弃乱序帧。
    pub seq: u64,
    /// 当前平台是否支持读取网卡计数器。
    pub supported: bool,
    /// 平台说明与**不可获取项**的如实交代（温度、缓冲区占用率等）。
    pub note: String,
    /// 采样任务是否在运行。
    pub sampling: bool,
    pub sample_ms: u32,
    /// 用户显式勾选的适配器 id（空 = 默认只监测物理网卡）。
    pub selection: Vec<String>,
    pub adapters: Vec<NicAdapterDto>,
    /// 最近事件（新的在前，最多保留 [`crate::nic::NIC_EVENT_LIMIT`] 条）。
    pub events: Vec<NicEvent>,
    /// 最近一次采样失败的原因（成功后清空）。
    pub last_error: String,
    pub history: Vec<NicAdapterHistory>,
}
