//! # loadloom-core
//!
//! 高并发打流**无头计算核心**。
//!
//! 本 crate 的硬性约束：
//! * **零 UI 依赖**——不引入任何窗口库、渲染器、浏览器、事件循环或对话框；
//! * **零壳耦合**——不知道 Tauri / web / CLI 的存在，只暴露纯函数与 `Engine` 门面；
//! * **可无界面测试**——`cargo test -p loadloom-core` 在无桌面环境下即可完整验证业务逻辑。
//!
//! 上层（桌面壳）只负责三件事：调用 [`Engine`] 的方法、订阅广播通道、把结果推给前端。

pub mod codes;
pub mod contract;
pub mod engine;
mod metrics;
mod rate;

pub use contract::{
    CoreError, EngineLimits, ErrorCount, HistoryPoint, LiveConfigPatch, LogEntry, LogLevel,
    MetricsSnapshot, RunEvent, RunEventKind, RunPhase, StartRunRequest,
};
pub use engine::Engine;
