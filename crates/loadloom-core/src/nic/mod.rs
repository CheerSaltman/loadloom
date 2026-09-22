//! 网卡链路监测 —— 把「消费级能拿到的网卡数据」变成可定位、可复现的证据。
//!
//! ## 为什么它必须和打流日志共用一条时间轴
//!
//! 打流时吞吐断崖式下跌，用户能提供的信息通常只有「忽然就慢了」。要区分
//! 「源站限速」「本机链路抖动」「网卡开始丢包」这三种完全不同的原因，必须有一条
//! 与吞吐同源、同一时钟的网卡侧记录：链路什么时候断的、协商速率什么时候从
//! 1 Gbps 掉到 100 Mbps、哪个瞬间开始出现丢弃。本模块就是这条记录。
//!
//! ## 能力边界（只做拿得到的，拿不到的一律不编）
//!
//! 采集层 [`nicmon`] 只读 Windows 公开的 `GetIfTable2` 计数器，因此这里能给出的
//! 是：链路状态与通断时间线、协商速率、收发速率与**利用率**、收发丢弃 / 错误、
//! **发送队列长度**（`OutQLen`）。**不提供**网卡温度、收发缓冲区占用率、光功率
//! —— 那些属于驱动私有数据，消费级设备读不到；界面与报告里会明确写出来，
//! 而不是留一个看起来像数据的 0。
//!
//! ## 设计要点
//!
//! * **采样可注入**：采集闭包由调用方提供，于是「Windows 真机取数」与「测试里的
//!   构造序列」走的是同一条分析代码路径，判定逻辑可以逐帧重放；
//! * **纯函数判定**：差值、链路跳变、尖峰判定都不读时钟、不碰 IO，时间与阈值
//!   全部由调用方传入（见 [`analysis`]）；
//! * **事件有始有终**：丢弃 / 错误 / 队列积压按「一次事件」记录开始与结束，
//!   结束时附上**峰值与持续时长**。逐帧刷日志会把真正持续的异常淹掉，
//!   只报开始又会让人无法判断它到底影响了多久。

mod analysis;
mod monitor;

use std::sync::Arc;

pub use monitor::NicMonitor;

use nicmon::RawAdapter;

/// 采样周期。500ms 足够抓住「网线被碰了一下」这类瞬断，又不会让 `GetIfTable2`
/// 的调用开销进入可观测范围。
pub const NIC_SAMPLE: std::time::Duration = std::time::Duration::from_millis(500);
/// 每块网卡保留的历史点数（500ms × 240 ≈ 2 分钟）。
pub const NIC_HISTORY_LEN: usize = 240;
/// 事件环形缓冲上限（界面只展示最近若干条，磁盘日志里则是全量）。
pub const NIC_EVENT_LIMIT: usize = 200;
/// 一次最多监测多少块网卡（IPC 载荷是信任边界，必须有硬上限）。
pub(super) const MAX_SELECTION: usize = 64;
/// 适配器标识长度上限。
pub(super) const MAX_ID_LEN: usize = 128;
/// 丢弃速率尖峰阈值（个/秒）。
pub(super) const DISCARD_SPIKE_PER_SEC: f64 = 1.0;
/// 错误速率尖峰阈值（个/秒）。
pub(super) const ERROR_SPIKE_PER_SEC: f64 = 1.0;
/// 发送队列积压阈值（包）。
pub(super) const QUEUE_BACKLOG_PACKETS: u64 = 16;
/// `NET_IF_ADMIN_STATUS_UP`。
pub(super) const ADMIN_STATUS_UP: u32 = 1;

/// 采样闭包：返回本机全部接口的当前状态。
///
/// 之所以是「全部接口」而不是「已选接口」：过滤与聚合属于业务决策，采集层不该
/// 替用户决定哪块网卡值得看 —— 否则「我的网卡怎么不在列表里」就成了无法解释的
/// 黑箱。
pub type NicSampler = Arc<dyn Fn() -> Result<Vec<RawAdapter>, nicmon::NicError> + Send + Sync>;

/// 平台能力说明（如实交代能拿到什么、拿不到什么）。
pub const CAPABILITY_NOTE: &str = "可获得：链路通断与协商速率、收发速率与利用率、收发丢弃 / 错误、发送队列长度（OutQLen）。不可获得：网卡温度、收发缓冲区占用率、光模块功率 —— 这些属于驱动私有数据，消费级设备读不到，本页不猜也不编。";
