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
    pub history: Vec<HistoryPoint>,
    pub latest: Option<HistoryPoint>,
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
