//! 判定逻辑：差值、链路跳变、尖峰事件 —— **全部是纯函数**。
//!
//! 这一层的每个函数都不读时钟、不碰 IO、不持有锁：时间与阈值由调用方传入，
//! 于是测试可以用构造出来的采样序列逐帧重放「网线拔了 3 秒又插回」「协商速率
//! 掉档」「开始丢包」这些场景，而不需要真的去拔网线、换交换机。
//!
//! 判定结果 [`Finding`] 只描述**发生了什么事实**；把它翻译成事件码、日志级别与
//! 中文文案的映射也放在这里，保证「同一个事实」在事件流、日志、报告里三处
//! 完全一致。

use super::{ADMIN_STATUS_UP, DISCARD_SPIKE_PER_SEC, ERROR_SPIKE_PER_SEC, QUEUE_BACKLOG_PACKETS};
use crate::codes;
use crate::contract::{LogLevel, NicEventKind, NicLinkState};
use nicmon::RawAdapter;

/// 两次采样之间的差值分析结果（全部为瞬时速率，不含累计值）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct NicDelta {
    pub rx_bps: f64,
    pub tx_bps: f64,
    /// 收方向利用率百分比（`0..=100`；链路速率未知时为 0）。
    pub rx_utilization: f64,
    pub tx_utilization: f64,
    pub rx_discards_per_sec: f64,
    pub tx_discards_per_sec: f64,
    pub rx_errors_per_sec: f64,
    pub tx_errors_per_sec: f64,
    pub unknown_protos_per_sec: f64,
    /// 累计计数器被清零（网卡重插 / 驱动重载）。
    pub counter_reset: bool,
}

/// 计算两帧之间的速率。
///
/// **计数器倒退必须先归零**：`u64` 累计值一旦变小（拔插网卡、驱动重载、休眠
/// 唤醒），`saturating_sub` 会给出一个巨大的差值，界面上的读数就是「瞬间
/// 18 EB/s」这种荒唐数字 —— 而它看起来完全像是真的。
pub(super) fn analyze(previous: &RawAdapter, current: &RawAdapter, seconds: f64) -> NicDelta {
    let seconds = seconds.max(1e-3);
    let before = previous.counters;
    let now = current.counters;
    if now.regressed_from(&before) {
        return NicDelta {
            counter_reset: true,
            ..NicDelta::default()
        };
    }

    let in_bytes = now.in_octets.saturating_sub(before.in_octets) as f64;
    let out_bytes = now.out_octets.saturating_sub(before.out_octets) as f64;
    let in_discards = now.in_discards.saturating_sub(before.in_discards) as f64;
    let out_discards = now.out_discards.saturating_sub(before.out_discards) as f64;
    let in_errors = now.in_errors.saturating_sub(before.in_errors) as f64;
    let out_errors = now.out_errors.saturating_sub(before.out_errors) as f64;
    let unknown = now
        .in_unknown_protos
        .saturating_sub(before.in_unknown_protos) as f64;

    let rx_bps = in_bytes * 8.0 / seconds;
    let tx_bps = out_bytes * 8.0 / seconds;
    NicDelta {
        rx_bps,
        tx_bps,
        // 利用率按各自方向的协商速率算：链路上报的是物理层速率（含帧头与帧间隙），
        // 所以这里得到的是「占用率」而不是「有效载荷占比」，不会算出 >100%。
        rx_utilization: utilization(rx_bps, current.receive_speed_bps),
        tx_utilization: utilization(tx_bps, current.transmit_speed_bps),
        rx_discards_per_sec: in_discards / seconds,
        tx_discards_per_sec: out_discards / seconds,
        rx_errors_per_sec: in_errors / seconds,
        tx_errors_per_sec: out_errors / seconds,
        unknown_protos_per_sec: unknown / seconds,
        counter_reset: false,
    }
}

/// 利用率百分比：链路速率未知（0）时返回 0，绝不返回 `inf` / `NaN`。
fn utilization(bps: f64, link_speed_bps: u64) -> f64 {
    if link_speed_bps == 0 {
        return 0.0;
    }
    (bps / link_speed_bps as f64 * 100.0).clamp(0.0, 100.0)
}

/// 一次「持续异常」的会话记录。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Episode {
    active: bool,
    started_ms: u64,
    peak: f64,
    samples: u64,
}

/// 单块网卡上一次采样的记忆（差值、链路跳变与尖峰判定的唯一依据）。
#[derive(Debug, Clone, Copy)]
pub(super) struct Memory {
    link_up: bool,
    link_speed_bps: u64,
    down_since_ms: Option<u64>,
    discards: Episode,
    errors: Episode,
    queue: Episode,
}

/// 本帧判定出的事实（尚未翻译成文案与事件码）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Finding {
    LinkDown,
    LinkUp {
        down_ms: u64,
    },
    SpeedChange {
        from_bps: u64,
        to_bps: u64,
    },
    CounterReset,
    SpikeStarted {
        spike: Spike,
    },
    SpikeEnded {
        spike: Spike,
        peak: f64,
        duration_ms: u64,
        samples: u64,
    },
}

/// 尖峰类别（丢弃 / 错误 / 队列积压）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Spike {
    Discards,
    Errors,
    Queue,
}

/// 用一帧新采样推进记忆，并产出本帧的判定结果。
///
/// 纯函数：不读时钟、不碰 IO，`now_ms` 与阈值全部由调用方传入。
pub(super) fn advance(
    memory: Option<Memory>,
    current: &RawAdapter,
    delta: &NicDelta,
    now_ms: u64,
) -> (Memory, Vec<Finding>) {
    let mut findings = Vec::new();
    let link_up = current.link_up();
    let speed_bps = current.link_speed_bps();
    let fresh = memory.is_none();
    let mut state = memory.unwrap_or(Memory {
        link_up,
        link_speed_bps: speed_bps,
        down_since_ms: if link_up { None } else { Some(now_ms) },
        discards: Episode::default(),
        errors: Episode::default(),
        queue: Episode::default(),
    });

    // 第一次见到这块网卡时只建立基线：否则每一块刚被勾选（或刚插入）的网卡都会
    // 立刻产出「链路断开」「速率变化」这类假事件。
    if !fresh {
        if state.link_up && !link_up {
            findings.push(Finding::LinkDown);
        } else if !state.link_up && link_up {
            findings.push(Finding::LinkUp {
                down_ms: now_ms.saturating_sub(state.down_since_ms.unwrap_or(now_ms)),
            });
        }
        // 速率变化只在「两个方向都报出非零速率」时判定：网卡刚连接、驱动还没上报
        // 速率的那一帧会给出 0，把它当成一次降速是纯噪声。
        if state.link_up
            && link_up
            && state.link_speed_bps > 0
            && speed_bps > 0
            && state.link_speed_bps != speed_bps
        {
            findings.push(Finding::SpeedChange {
                from_bps: state.link_speed_bps,
                to_bps: speed_bps,
            });
        }
        if delta.counter_reset {
            findings.push(Finding::CounterReset);
        }
    }

    let discard_rate = delta.rx_discards_per_sec.max(delta.tx_discards_per_sec);
    let error_rate = delta.rx_errors_per_sec.max(delta.tx_errors_per_sec);
    push_episode(
        &mut state.discards,
        Spike::Discards,
        discard_rate,
        DISCARD_SPIKE_PER_SEC,
        now_ms,
        &mut findings,
    );
    push_episode(
        &mut state.errors,
        Spike::Errors,
        error_rate,
        ERROR_SPIKE_PER_SEC,
        now_ms,
        &mut findings,
    );
    push_episode(
        &mut state.queue,
        Spike::Queue,
        current.counters.out_queue_len as f64,
        QUEUE_BACKLOG_PACKETS as f64,
        now_ms,
        &mut findings,
    );

    state.link_up = link_up;
    state.link_speed_bps = speed_bps;
    state.down_since_ms = if link_up {
        None
    } else {
        state.down_since_ms.or(Some(now_ms))
    };
    (state, findings)
}

/// 推进一「集」尖峰：连续两帧超过阈值才算开始，回落即结束（附峰值与时长）。
///
/// 为什么不逐帧报：无线网卡每秒都可能有一两个瞬时丢弃，把它们都写成日志，
/// 真正持续十秒的丢包异常会被淹没；而只报开始不报结束，用户又无法判断它到底
/// 影响了多久 —— 打流的性能曲线恰恰需要这个「持续了多久」。
fn push_episode(
    episode: &mut Episode,
    spike: Spike,
    level: f64,
    threshold: f64,
    now_ms: u64,
    findings: &mut Vec<Finding>,
) {
    if level >= threshold {
        if episode.active {
            episode.samples += 1;
            episode.peak = episode.peak.max(level);
            if episode.samples == 2 {
                findings.push(Finding::SpikeStarted { spike });
            }
        } else {
            *episode = Episode {
                active: true,
                started_ms: now_ms,
                peak: level,
                samples: 1,
            };
        }
        return;
    }
    if !episode.active {
        return;
    }
    findings.push(Finding::SpikeEnded {
        spike,
        peak: episode.peak,
        duration_ms: now_ms.saturating_sub(episode.started_ms),
        samples: episode.samples,
    });
    *episode = Episode::default();
}

/// 判定结果 -> 事件码。
pub(super) fn code_of(finding: &Finding) -> &'static str {
    match finding {
        Finding::LinkDown => codes::nic::LINK_DOWN,
        Finding::LinkUp { .. } => codes::nic::LINK_UP,
        Finding::SpeedChange { .. } => codes::nic::SPEED_CHANGED,
        Finding::CounterReset => codes::nic::COUNTER_RESET,
        Finding::SpikeStarted { spike } => match spike {
            Spike::Discards => codes::nic::DISCARD_SPIKE,
            Spike::Errors => codes::nic::ERROR_SPIKE,
            Spike::Queue => codes::nic::QUEUE_BACKLOG,
        },
        Finding::SpikeEnded { spike, .. } => match spike {
            Spike::Discards => codes::nic::DISCARD_SPIKE_END,
            Spike::Errors => codes::nic::ERROR_SPIKE_END,
            Spike::Queue => codes::nic::QUEUE_BACKLOG_END,
        },
    }
}

/// 判定结果 -> 日志级别（尖峰由 Warn 起、以 Info 收）。
pub(super) fn level_of(finding: &Finding) -> LogLevel {
    match finding {
        Finding::LinkDown | Finding::SpeedChange { .. } => LogLevel::Warn,
        Finding::SpikeStarted { spike } => match spike {
            Spike::Discards | Spike::Errors => LogLevel::Warn,
            // 队列积压不等于故障：打满链路时它必然出现，记录但不报警。
            Spike::Queue => LogLevel::Info,
        },
        _ => LogLevel::Info,
    }
}

/// 判定结果 -> 事件类别（界面图标与配色用）。
pub(super) fn kind_of(finding: &Finding) -> NicEventKind {
    match finding {
        Finding::LinkDown => NicEventKind::LinkDown,
        Finding::LinkUp { .. } => NicEventKind::LinkUp,
        Finding::SpeedChange { .. } => NicEventKind::SpeedChange,
        Finding::CounterReset => NicEventKind::CounterReset,
        Finding::SpikeStarted { spike } | Finding::SpikeEnded { spike, .. } => match spike {
            Spike::Discards => NicEventKind::DiscardSpike,
            Spike::Errors => NicEventKind::ErrorSpike,
            Spike::Queue => NicEventKind::QueueBacklog,
        },
    }
}

/// 该判定结果是否属于「收尾记录」（尖峰结束）。
pub(super) fn is_resolved(finding: &Finding) -> bool {
    matches!(finding, Finding::SpikeEnded { .. })
}

/// 是否值得写日志。
///
/// 单帧抖动（`samples < 2`）只进事件流、不进日志：它没有开始记录，也就不该有
/// 一条孤立的收尾记录。
pub(super) fn worth_logging(finding: &Finding) -> bool {
    match finding {
        Finding::SpikeEnded { samples, .. } => *samples >= 2,
        _ => true,
    }
}

/// 把判定结果翻译成给用户看的一句话。
pub(super) fn describe(adapter: &RawAdapter, finding: &Finding, delta: &NicDelta) -> String {
    let name = adapter.name.as_str();
    match finding {
        Finding::LinkDown => format!(
            "网卡「{name}」链路断开（此前协商速率 {}）：本机方向的吞吐会立刻归零",
            fmt_link_speed(adapter.link_speed_bps())
        ),
        Finding::LinkUp { down_ms } => format!(
            "网卡「{name}」链路恢复，中断 {}（当前协商速率 {}）",
            fmt_duration(*down_ms),
            fmt_link_speed(adapter.link_speed_bps())
        ),
        Finding::SpeedChange { from_bps, to_bps } => format!(
            "网卡「{name}」协商速率变化：{} → {}（网线质量、交换机端口、无线信道或省电策略都可能触发）",
            fmt_link_speed(*from_bps),
            fmt_link_speed(*to_bps)
        ),
        Finding::CounterReset => format!(
            "网卡「{name}」累计计数器被清零（网卡重插 / 驱动重载 / 休眠唤醒），速率已按新基线重算"
        ),
        Finding::SpikeStarted { spike } => match spike {
            Spike::Discards => format!(
                "网卡「{name}」开始出现丢弃：{:.1} 个/秒（收 {:.1} / 发 {:.1}）· 累计丢弃 收 {} / 发 {}",
                delta.rx_discards_per_sec.max(delta.tx_discards_per_sec),
                delta.rx_discards_per_sec,
                delta.tx_discards_per_sec,
                adapter.counters.in_discards,
                adapter.counters.out_discards,
            ),
            Spike::Errors => format!(
                "网卡「{name}」开始出现收发错误：{:.1} 个/秒（收 {:.1} / 发 {:.1}）· 累计错误 收 {} / 发 {}",
                delta.rx_errors_per_sec.max(delta.tx_errors_per_sec),
                delta.rx_errors_per_sec,
                delta.tx_errors_per_sec,
                adapter.counters.in_errors,
                adapter.counters.out_errors,
            ),
            Spike::Queue => format!(
                "网卡「{name}」发送队列开始积压：当前 {} 包（驱动来不及把包交给网卡，通常意味着链路已被打满或对端不再接收）",
                adapter.counters.out_queue_len
            ),
        },
        Finding::SpikeEnded {
            spike,
            peak,
            duration_ms,
            samples,
        } => {
            let unit = match spike {
                Spike::Discards | Spike::Errors => "个/秒",
                Spike::Queue => "包",
            };
            let what = match spike {
                Spike::Discards => "丢弃",
                Spike::Errors => "收发错误",
                Spike::Queue => "发送队列积压",
            };
            format!(
                "网卡「{name}」{what}结束：持续 {}（{samples} 帧），峰值 {peak:.1} {unit}",
                fmt_duration(*duration_ms)
            )
        }
    }
}

/// 链路速率格式化（bit/s -> Gbps / Mbps / Kbps）。
pub(super) fn fmt_link_speed(bps: u64) -> String {
    let value = bps as f64;
    if bps >= 1_000_000_000 {
        format!("{:.2} Gbps", value / 1e9)
    } else if bps >= 1_000_000 {
        format!("{:.0} Mbps", value / 1e6)
    } else if bps >= 1_000 {
        format!("{:.0} Kbps", value / 1e3)
    } else {
        format!("{bps} bit/s")
    }
}

/// 时长格式化（毫秒 -> 「3.5 秒」/「120 毫秒」）。
fn fmt_duration(millis: u64) -> String {
    if millis >= 10_000 {
        format!("{:.0} 秒", millis as f64 / 1000.0)
    } else if millis >= 1000 {
        format!("{:.1} 秒", millis as f64 / 1000.0)
    } else {
        format!("{millis} 毫秒")
    }
}

/// 把原始采样翻译成对外的链路状态。
pub(super) fn link_state_of(adapter: &RawAdapter) -> NicLinkState {
    if adapter.oper_status == nicmon::IF_OPER_NOT_PRESENT {
        return NicLinkState::NotPresent;
    }
    if adapter.link_up() {
        return NicLinkState::Connected;
    }
    if adapter.oper_status == nicmon::IF_OPER_DORMANT {
        return NicLinkState::Dormant;
    }
    if adapter.oper_status == nicmon::IF_OPER_DOWN
        || adapter.connect_state == nicmon::MEDIA_CONNECT_DISCONNECTED
        || adapter.not_media_connected
    {
        return NicLinkState::Disconnected;
    }
    NicLinkState::Unknown
}

/// 默认纳入监测的网卡：物理网卡，且不是「设备不存在」的历史残留条目
/// （Windows 会把曾经插过的网卡留在接口表里，`OperStatus = NotPresent`）。
pub(super) fn monitored_by_default(adapter: &RawAdapter) -> bool {
    adapter.class == nicmon::AdapterClass::Physical
        && adapter.oper_status != nicmon::IF_OPER_NOT_PRESENT
}

/// 管理状态是否「已启用」。
pub(super) fn admin_enabled(adapter: &RawAdapter) -> bool {
    adapter.admin_status == ADMIN_STATUS_UP
}

#[cfg(test)]
mod tests {
    use super::*;
    use nicmon::Counters;

    /// 一块「在线、1 Gbps、计数器刚清零」的以太网卡，测试按需改写其中几项。
    fn base() -> RawAdapter {
        RawAdapter {
            id: "luid-test".to_owned(),
            index: 3,
            luid: 3,
            name: "以太网".to_owned(),
            description: "Test NIC".to_owned(),
            class: nicmon::AdapterClass::Physical,
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

    fn bytes(in_octets: u64, out_octets: u64) -> Counters {
        Counters {
            in_octets,
            out_octets,
            ..Counters::default()
        }
    }

    #[test]
    fn rates_and_utilization_follow_the_real_interval() {
        let before = base();
        let mut after = base();
        // 一秒里收了 1.25 MB、发了 2.5 MB -> 10 Mbit/s 与 20 Mbit/s。
        after.counters = bytes(1_250_000, 2_500_000);
        let delta = analyze(&before, &after, 1.0);
        assert!((delta.rx_bps - 10_000_000.0).abs() < 1.0);
        assert!((delta.tx_bps - 20_000_000.0).abs() < 1.0);
        assert!((delta.rx_utilization - 1.0).abs() < 1e-6);
        assert!((delta.tx_utilization - 2.0).abs() < 1e-6);

        // 同样的字节数在半个采样周期里产生 -> 速率翻倍（间隔必须真的用上）。
        let delta = analyze(&before, &after, 0.5);
        assert!((delta.rx_bps - 20_000_000.0).abs() < 1.0);

        // 链路速率未知时利用率是 0，不能是 inf / NaN。
        let mut unknown = after.clone();
        unknown.transmit_speed_bps = 0;
        unknown.receive_speed_bps = 0;
        let delta = analyze(&before, &unknown, 1.0);
        assert_eq!(delta.rx_utilization, 0.0);
        assert_eq!(delta.tx_utilization, 0.0);
        assert!(delta.rx_bps.is_finite());
    }

    #[test]
    fn counter_regression_is_never_reported_as_a_giant_rate() {
        let mut before = base();
        before.counters = bytes(9_000_000_000, 5_000_000_000);
        let mut after = base();
        after.counters = bytes(12, 8);
        let delta = analyze(&before, &after, 0.5);
        assert!(delta.counter_reset, "计数器倒退必须被识别出来");
        assert_eq!(delta.rx_bps, 0.0);
        assert_eq!(delta.tx_bps, 0.0);
    }

    #[test]
    fn link_down_then_up_reports_the_outage_duration() {
        let live = base();
        let idle = NicDelta::default();
        let (state, findings) = advance(None, &live, &idle, 1_000);
        assert!(findings.is_empty(), "第一帧只建立基线：{findings:?}");

        let mut down = base();
        down.oper_status = nicmon::IF_OPER_DOWN;
        down.connect_state = nicmon::MEDIA_CONNECT_DISCONNECTED;
        down.not_media_connected = true;
        let (state, findings) = advance(Some(state), &down, &idle, 1_500);
        assert_eq!(findings, vec![Finding::LinkDown]);
        assert_eq!(code_of(&findings[0]), codes::nic::LINK_DOWN);
        assert_eq!(level_of(&findings[0]), LogLevel::Warn);

        // 持续断开：同一次中断不能每帧重复播报。
        let (state, findings) = advance(Some(state), &down, &idle, 2_000);
        assert!(findings.is_empty());

        let (_, findings) = advance(Some(state), &live, &idle, 4_500);
        assert_eq!(findings, vec![Finding::LinkUp { down_ms: 3_000 }]);
        assert_eq!(code_of(&findings[0]), codes::nic::LINK_UP);
        assert_eq!(kind_of(&findings[0]), NicEventKind::LinkUp);
    }

    #[test]
    fn speed_change_needs_two_frames_that_both_report_a_speed() {
        let (state, _) = advance(None, &base(), &NicDelta::default(), 0);
        let mut slow = base();
        slow.transmit_speed_bps = 100_000_000;
        slow.receive_speed_bps = 100_000_000;
        let (state, findings) = advance(Some(state), &slow, &NicDelta::default(), 500);
        assert_eq!(
            findings,
            vec![Finding::SpeedChange {
                from_bps: 1_000_000_000,
                to_bps: 100_000_000,
            }]
        );

        // 驱动还没上报速率（0）时不误报降速。
        let mut unknown = base();
        unknown.transmit_speed_bps = 0;
        unknown.receive_speed_bps = 0;
        let (_, findings) = advance(Some(state), &unknown, &NicDelta::default(), 1_000);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_sustained_discard_spike_has_a_start_and_an_end_with_the_peak() {
        let live = base();
        let (state, _) = advance(None, &live, &NicDelta::default(), 0);
        let spike = NicDelta {
            rx_discards_per_sec: 40.0,
            ..NicDelta::default()
        };
        let (state, first) = advance(Some(state), &live, &spike, 500);
        assert!(first.is_empty(), "单帧抖动不算一次尖峰");

        let (state, second) = advance(Some(state), &live, &spike, 1_000);
        assert_eq!(
            second,
            vec![Finding::SpikeStarted {
                spike: Spike::Discards
            }]
        );
        assert_eq!(level_of(&second[0]), LogLevel::Warn);
        assert_eq!(code_of(&second[0]), codes::nic::DISCARD_SPIKE);

        // 持续期间不重复报开始，但峰值要一直累积。
        let higher = NicDelta {
            tx_discards_per_sec: 120.0,
            ..NicDelta::default()
        };
        let (state, ongoing) = advance(Some(state), &live, &higher, 1_500);
        assert!(ongoing.is_empty());

        let (_, ended) = advance(Some(state), &live, &NicDelta::default(), 2_000);
        match ended.as_slice() {
            [Finding::SpikeEnded {
                spike,
                peak,
                duration_ms,
                samples,
            }] => {
                assert_eq!(*spike, Spike::Discards);
                assert!((*peak - 120.0).abs() < 1e-9, "峰值必须取整段最大值");
                assert_eq!(*duration_ms, 1_500);
                assert_eq!(*samples, 3);
            }
            other => panic!("期望一条收尾记录，实际 {other:?}"),
        }
        assert!(is_resolved(&ended[0]));
        assert!(worth_logging(&ended[0]));
        assert_eq!(code_of(&ended[0]), codes::nic::DISCARD_SPIKE_END);
    }

    #[test]
    fn a_single_frame_blip_only_touches_the_event_stream() {
        let live = base();
        let (state, _) = advance(None, &live, &NicDelta::default(), 0);
        let spike = NicDelta {
            rx_errors_per_sec: 5.0,
            ..NicDelta::default()
        };
        let (state, findings) = advance(Some(state), &live, &spike, 500);
        assert!(findings.is_empty());
        let (_, findings) = advance(Some(state), &live, &NicDelta::default(), 1_000);
        assert_eq!(findings.len(), 1);
        assert!(!worth_logging(&findings[0]), "单帧抖动不该写进日志");
        assert!(!is_resolved(&Finding::LinkDown));
    }

    #[test]
    fn queue_backlog_episode_reports_the_peak_depth() {
        let mut loaded = base();
        loaded.counters.out_queue_len = 24;
        let (state, _) = advance(None, &loaded, &NicDelta::default(), 0);
        let (state, findings) = advance(Some(state), &loaded, &NicDelta::default(), 500);
        assert_eq!(
            findings,
            vec![Finding::SpikeStarted {
                spike: Spike::Queue
            }]
        );
        assert_eq!(level_of(&findings[0]), LogLevel::Info, "积压是记录不是报警");

        let mut deeper = loaded.clone();
        deeper.counters.out_queue_len = 90;
        let (state, _) = advance(Some(state), &deeper, &NicDelta::default(), 1_000);
        let (_, findings) = advance(Some(state), &base(), &NicDelta::default(), 1_500);
        match findings.as_slice() {
            [Finding::SpikeEnded {
                peak, duration_ms, ..
            }] => {
                assert!((*peak - 90.0).abs() < 1e-9);
                assert_eq!(*duration_ms, 1_500);
            }
            other => panic!("期望积压结束记录，实际 {other:?}"),
        }
    }

    #[test]
    fn messages_carry_the_numbers_needed_to_reproduce() {
        let adapter = base();
        let delta = NicDelta {
            rx_discards_per_sec: 12.5,
            ..NicDelta::default()
        };
        let text = describe(
            &adapter,
            &Finding::SpikeStarted {
                spike: Spike::Discards,
            },
            &delta,
        );
        assert!(text.contains("12.5"), "{text}");
        assert!(text.contains("以太网"), "{text}");

        let text = describe(&adapter, &Finding::LinkUp { down_ms: 3_500 }, &delta);
        assert!(text.contains("3.5 秒"), "{text}");
        assert!(text.contains("1.00 Gbps"), "{text}");

        let text = describe(
            &adapter,
            &Finding::SpeedChange {
                from_bps: 1_000_000_000,
                to_bps: 100_000_000,
            },
            &delta,
        );
        assert!(text.contains("1.00 Gbps → 100 Mbps"), "{text}");
    }

    #[test]
    fn link_state_mapping_distinguishes_absent_from_disconnected() {
        let mut adapter = base();
        assert_eq!(link_state_of(&adapter), NicLinkState::Connected);
        adapter.oper_status = nicmon::IF_OPER_DOWN;
        assert_eq!(link_state_of(&adapter), NicLinkState::Disconnected);
        adapter.oper_status = nicmon::IF_OPER_DORMANT;
        adapter.connect_state = nicmon::MEDIA_CONNECT_UNKNOWN;
        assert_eq!(link_state_of(&adapter), NicLinkState::Dormant);
        adapter.oper_status = nicmon::IF_OPER_NOT_PRESENT;
        assert_eq!(link_state_of(&adapter), NicLinkState::NotPresent);
        assert!(admin_enabled(&adapter));

        let mut virtual_nic = base();
        virtual_nic.class = nicmon::AdapterClass::Virtual;
        assert!(!monitored_by_default(&virtual_nic));
        assert!(monitored_by_default(&base()));
        let mut ghost = base();
        ghost.oper_status = nicmon::IF_OPER_NOT_PRESENT;
        assert!(!monitored_by_default(&ghost), "历史残留条目不默认监测");
    }
}
