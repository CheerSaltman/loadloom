//! 稳定事件码 —— 日志与生命周期事件的「错误码」体系。
//!
//! **为什么要有它（可定位、可复现）**：只有自由文本的日志在排障时有一个致命缺口 ——
//! 用户复述的是「打了一半就停了」这类描述，维护者无法指认究竟哪一条分支失效，
//! 只能靠猜；同一条分支的文字还会随措辞调整而漂移，旧报告里的描述再也搜不到。
//! 事件码形如 `域-三位序号`（`DOMAIN-NNN`），与代码分支一一对应、**永不复用**：
//! 用户只要报出 `NET-002`，就能直接跳到唯一一处产生它的代码分支。
//!
//! 规则：
//! * 新增分支 → 取本域下一个未用序号，不回收已删除的号；
//! * 序号一旦发布就不再改变含义（`is_log_milestone` 的单元测试与契约测试共同钉住格式）；
//! * 空字符串表示「无码」：只适用于第三方直通文本，自有代码一律带码。
//!
//! 域划分：
//! * [`sys`] —— 进程与基础设施（会话、panic、前端异常、HTTP 客户端降级）；
//! * [`cfg`] —— 入参校验与收敛（信任边界上的每一次拒绝 / 改写）；
//! * [`run`] —— 运行生命周期（开始 / 停止 / 自动停止 / 重复启动 / 参数快照）；
//! * [`net`] —— 网络请求、响应流与重试；
//! * [`nic`] —— 网卡链路监测（链路通断、协商速率、丢弃 / 错误 / 队列积压）。

/// 进程与基础设施。
pub mod sys {
    /// 会话开始（桌面壳写入的环境头）。
    pub const SESSION_START: &str = "SYS-000";
    /// 进程 panic（崩溃报告同时落盘为 `crash-*.md`）。
    pub const PANIC: &str = "SYS-001";
    /// 前端（WebView）未捕获异常 / 未处理的 Promise 拒绝。
    pub const FRONTEND_ERROR: &str = "SYS-002";
    /// 打流引擎构造完成。
    pub const ENGINE_READY: &str = "SYS-003";
    /// HTTP 客户端完全不可用（启动会被明确拒绝）。
    pub const CLIENT_UNAVAILABLE: &str = "SYS-004";
    /// HTTP 客户端按降级配置构建（可用，但丢掉了部分约束）。
    pub const CLIENT_DEGRADED: &str = "SYS-005";
    /// 崩溃报告已写入文件（消息里带路径）。
    pub const CRASH_REPORT: &str = "SYS-006";
}

/// 入参校验与收敛。
pub mod cfg {
    /// 参数缺失或形态非法（空地址等）。
    pub const REJECTED: &str = "CFG-001";
    /// 目标地址超过长度上限。
    pub const URL_TOO_LONG: &str = "CFG-002";
    /// 目标地址协议不符（非 http/https）。
    pub const URL_SCHEME: &str = "CFG-003";
    /// 未确认测试授权。
    pub const NOT_AUTHORIZED: &str = "CFG-004";
    /// 非有限数字（`NaN` / `±Infinity`）被拒绝。
    pub const NON_FINITE: &str = "CFG-005";
    /// 数值超出允许范围，被收敛后如实告知请求值与实际值。
    pub const CLAMPED: &str = "CFG-010";
    /// 运行中的实时调整已生效。
    pub const LIVE_APPLIED: &str = "CFG-011";
    /// 渐进升压计划已开始。
    pub const RAMP_STARTED: &str = "CFG-012";
    /// 渐进升压计划已完成。
    pub const RAMP_COMPLETED: &str = "CFG-013";
    /// 本地 PT 模式包含公网或非字面量局域网地址，被拒绝。
    pub const PT_NOT_LOCAL: &str = "CFG-014";
}

/// 运行生命周期。
pub mod run {
    /// 一轮打流开始。
    pub const STARTED: &str = "RUN-001";
    /// 一轮打流被停止（手动或程序化）。
    pub const STOPPED: &str = "RUN-002";
    /// 命中流量 / 时长上限，自动停止。
    pub const AUTO_STOPPED: &str = "RUN-003";
    /// 已有任务在运行，重复启动被拒绝。
    pub const ALREADY_RUNNING: &str = "RUN-004";
    /// 生效参数快照（用于事后复现当次运行的配置）。
    pub const CONFIG: &str = "RUN-005";
    /// 压力监测进入预警状态。
    pub const PRESSURE_WARNING: &str = "RUN-006";
    /// 压力保护器达到熔断阈值并停止。
    pub const PRESSURE_STOPPED: &str = "RUN-007";
}

/// 网络请求与重试。
pub mod net {
    /// 服务器返回非 2xx 状态码。
    pub const HTTP_STATUS: &str = "NET-001";
    /// 请求整体失败（连接、超时等）。
    pub const REQUEST_FAILED: &str = "NET-002";
    /// 响应体读到一半中断。
    pub const STREAM_BROKEN: &str = "NET-003";
    /// 失败退避重试（高频事件，按里程碑节流）。
    pub const BACKOFF: &str = "NET-004";
}

/// 网卡链路监测。
///
/// 这一域的日志有**双重用途**：既给用户看（「是不是网线松了」），也是打流结果的
/// 证据链 —— 吞吐在某秒断崖式下跌时，同一条时间轴上必须有网卡侧的记录，否则
/// 「是源站限速还是本地链路抖动」永远只能靠猜。因此链路与计数器的异常一律带码。
pub mod nic {
    /// 监测已启动（平台、采样周期、默认范围）。
    pub const MONITOR_READY: &str = "NIC-001";
    /// 采样失败（系统调用出错，按里程碑节流）。
    pub const SAMPLE_FAILED: &str = "NIC-002";
    /// 本平台不支持网卡计数器读取（明确降级，不假装没有网卡）。
    pub const UNSUPPORTED: &str = "NIC-003";
    /// 接口列表变化（新增 / 移除 / 拔出）。
    pub const ADAPTERS_CHANGED: &str = "NIC-004";
    /// 被勾选的适配器当前不在列表里（可能已拔出，插回后自动恢复监测）。
    pub const SELECTION_UNKNOWN: &str = "NIC-005";
    /// 监测范围已更新（如实记录这一刻选了哪些网卡）。
    pub const SELECTION_APPLIED: &str = "NIC-006";
    /// 网卡报告已生成（报告内容随日志一起可导出）。
    pub const REPORT: &str = "NIC-007";
    /// 首次采样发现清单（哪些接口被看见、默认监测了哪几块）。
    pub const DISCOVERED: &str = "NIC-008";
    /// 链路断开。
    pub const LINK_DOWN: &str = "NIC-010";
    /// 链路恢复（附中断时长）。
    pub const LINK_UP: &str = "NIC-011";
    /// 协商速率变化（换网线 / 换端口 / 无线降速 / 省电降频）。
    pub const SPEED_CHANGED: &str = "NIC-012";
    /// 累计计数器被清零（网卡重插 / 驱动重载）：速率换算必须先归零再统计。
    pub const COUNTER_RESET: &str = "NIC-013";
    /// 丢弃尖峰开始（收 / 发丢弃持续高于阈值）。
    pub const DISCARD_SPIKE: &str = "NIC-020";
    /// 丢弃尖峰结束（附峰值与持续时长）。
    pub const DISCARD_SPIKE_END: &str = "NIC-021";
    /// 错误尖峰开始。
    pub const ERROR_SPIKE: &str = "NIC-022";
    /// 错误尖峰结束（附峰值与持续时长）。
    pub const ERROR_SPIKE_END: &str = "NIC-023";
    /// 发送队列开始积压（驱动来不及把包发出去）。
    pub const QUEUE_BACKLOG: &str = "NIC-024";
    /// 发送队列积压结束（附峰值与持续时长）。
    pub const QUEUE_BACKLOG_END: &str = "NIC-025";
}

/// 全部事件码，供契约测试与诊断导出使用。
pub const ALL: &[&str] = &[
    sys::SESSION_START,
    sys::PANIC,
    sys::FRONTEND_ERROR,
    sys::ENGINE_READY,
    sys::CLIENT_UNAVAILABLE,
    sys::CLIENT_DEGRADED,
    sys::CRASH_REPORT,
    cfg::REJECTED,
    cfg::URL_TOO_LONG,
    cfg::URL_SCHEME,
    cfg::NOT_AUTHORIZED,
    cfg::NON_FINITE,
    cfg::CLAMPED,
    cfg::LIVE_APPLIED,
    cfg::RAMP_STARTED,
    cfg::RAMP_COMPLETED,
    cfg::PT_NOT_LOCAL,
    run::STARTED,
    run::STOPPED,
    run::AUTO_STOPPED,
    run::ALREADY_RUNNING,
    run::CONFIG,
    run::PRESSURE_WARNING,
    run::PRESSURE_STOPPED,
    net::HTTP_STATUS,
    net::REQUEST_FAILED,
    net::STREAM_BROKEN,
    net::BACKOFF,
    nic::MONITOR_READY,
    nic::SAMPLE_FAILED,
    nic::UNSUPPORTED,
    nic::ADAPTERS_CHANGED,
    nic::SELECTION_UNKNOWN,
    nic::SELECTION_APPLIED,
    nic::REPORT,
    nic::DISCOVERED,
    nic::LINK_DOWN,
    nic::LINK_UP,
    nic::SPEED_CHANGED,
    nic::COUNTER_RESET,
    nic::DISCARD_SPIKE,
    nic::DISCARD_SPIKE_END,
    nic::ERROR_SPIKE,
    nic::ERROR_SPIKE_END,
    nic::QUEUE_BACKLOG,
    nic::QUEUE_BACKLOG_END,
];

/// 高频重复日志的「突发配额」：同一类别的前 N 条完整保留。
pub const LOG_BURST: u64 = 3;

/// 重复日志的里程碑判定：前 [`LOG_BURST`] 条，以及之后 10 的整数次幂。
///
/// 纯函数，供引擎（网络失败）与桌面壳（前端异常）共用 —— 两边各写一份迟早会漂移，
/// 而这条规则一旦写错，要么让 4 MiB 的日志在几秒内被同一句话刷爆、把故障前的
/// 上下文挤进轮转窗口，要么把真正的首个故障现场吞掉，两种失效都极难察觉。
pub fn is_log_milestone(count: u64) -> bool {
    if count <= LOG_BURST {
        return true;
    }
    let mut step = 10_u64;
    while step < count {
        step = step.saturating_mul(10);
    }
    step == count
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 事件码是对外承诺的稳定接口：格式必须是 `域-三位数字`，不得重复。
    #[test]
    fn codes_are_unique_and_follow_the_domain_number_scheme() {
        let mut seen = std::collections::BTreeSet::new();
        for code in ALL {
            let (domain, number) = code
                .split_once('-')
                .unwrap_or_else(|| panic!("事件码缺少域分隔符：{code}"));
            assert_eq!(domain.len(), 3, "域名必须是 3 个大写字母：{code}");
            assert!(
                domain.chars().all(|c| c.is_ascii_uppercase()),
                "域名必须是 3 个大写字母：{code}"
            );
            assert_eq!(number.len(), 3, "序号必须是 3 位数字：{code}");
            assert!(
                number.chars().all(|c| c.is_ascii_digit()),
                "序号必须是 3 位数字：{code}"
            );
            assert!(seen.insert(*code), "事件码重复：{code}");
        }
    }

    #[test]
    fn milestones_cover_the_burst_then_powers_of_ten() {
        for count in [1_u64, 2, 3] {
            assert!(is_log_milestone(count), "{count} 属于突发配额");
        }
        for count in [4_u64, 5, 9, 11, 99, 101] {
            assert!(!is_log_milestone(count), "{count} 应当被节流");
        }
        for count in [10_u64, 100, 1000, 10_000] {
            assert!(is_log_milestone(count), "{count} 是数量级里程碑");
        }
    }
}
