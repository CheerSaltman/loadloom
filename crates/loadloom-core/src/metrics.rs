//! 运行期指标聚合：热路径用原子计数器，错误分布与抖动状态用互斥保护。
//!
//! 读侧（250ms 一 tick）基本无锁：延迟、抖动、错误计数都直接读原子值；
//! 只有写入错误分类和递推 RFC3550 抖动时才短暂持锁。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::contract::ErrorCount;

// ---------------------------------------------------------------------------
// 指标
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct JitterState {
    last: Option<f64>,
    jitter: f64,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) bytes: AtomicU64,
    pub(crate) completed: AtomicU64,
    pub(crate) failures: AtomicU64,
    pub(crate) in_flight: AtomicU32,
    pub(crate) latency_us: AtomicU64,
    pub(crate) jitter_us: AtomicU64,
    pub(crate) errors: Mutex<BTreeMap<String, u64>>,
    pub(crate) last_error: Mutex<String>,
    pub(crate) jitter: Mutex<JitterState>,
}

impl Metrics {
    pub(crate) fn reset(&self) {
        self.bytes.store(0, Ordering::Relaxed);
        self.completed.store(0, Ordering::Relaxed);
        self.failures.store(0, Ordering::Relaxed);
        self.in_flight.store(0, Ordering::Relaxed);
        self.latency_us.store(0, Ordering::Relaxed);
        self.jitter_us.store(0, Ordering::Relaxed);
        if let Ok(mut errors) = self.errors.lock() {
            errors.clear();
        }
        if let Ok(mut last) = self.last_error.lock() {
            last.clear();
        }
        if let Ok(mut jitter) = self.jitter.lock() {
            *jitter = JitterState::default();
        }
    }

    /// 在途请求 +1。
    pub(crate) fn request_started(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            });
    }

    /// 在途请求 -1，且**永不下溢**。
    ///
    /// 为什么不用裸 `fetch_sub`：`reset()` 与 worker 的收尾是并发的。
    /// 「stop 之后立刻 start」时，上一轮某个请求的回包可能落在 `reset()`
    /// 之后，于是这次 `fetch_sub` 会在已经归零的计数上再减一，把它变成
    /// `u32::MAX` —— 界面上就是「在途 42 亿」。这里改用饱和减法：已经是 0
    /// 就不再减，最坏情况只是少报一次，不会污染计数。
    pub(crate) fn request_finished(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_sub(1)
            });
    }

    /// 记录一次首包时延，并按 RFC3550 递推抖动。
    pub(crate) fn record_latency(&self, millis: f64) {
        self.latency_us
            .store((millis * 1000.0) as u64, Ordering::Relaxed);
        let mut state = self
            .jitter
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = state.last {
            let deviation = (millis - previous).abs();
            state.jitter += (deviation - state.jitter) / 16.0;
        }
        state.last = Some(millis);
        self.jitter_us
            .store((state.jitter * 1000.0) as u64, Ordering::Relaxed);
    }

    pub(crate) fn add_error(&self, code: &str, message: String) {
        let mut errors = self
            .errors
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *errors.entry(code.to_owned()).or_insert(0) += 1;
        drop(errors);
        let mut last = self
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *last = message;
    }

    pub(crate) fn latency_ms(&self) -> f64 {
        self.latency_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub(crate) fn jitter_ms(&self) -> f64 {
        self.jitter_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub(crate) fn error_snapshot(&self) -> Vec<ErrorCount> {
        let errors = self
            .errors
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut items: Vec<ErrorCount> = errors
            .iter()
            .map(|(code, count)| ErrorCount {
                code: code.clone(),
                count: *count,
            })
            .collect();
        items.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.code.cmp(&right.code))
        });
        items.truncate(12);
        items
    }

    pub(crate) fn last_error(&self) -> String {
        self.last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_snapshot_is_sorted_by_count_desc_and_capped_at_twelve() {
        let metrics = Metrics::default();
        for index in 0..15u64 {
            let code = format!("E{index:02}");
            for _ in 0..=index {
                metrics.add_error(&code, "boom".to_owned());
            }
        }
        let snapshot = metrics.error_snapshot();
        assert_eq!(snapshot.len(), 12);
        assert_eq!(snapshot[0].code, "E14");
        assert_eq!(snapshot[0].count, 15);
        assert!(snapshot
            .windows(2)
            .all(|pair| pair[0].count >= pair[1].count));
        assert_eq!(metrics.last_error(), "boom");
    }

    #[test]
    fn reset_clears_counters_and_error_state() {
        let metrics = Metrics::default();
        metrics.add_error("TIMEOUT", "timed out".to_owned());
        metrics.record_latency(12.0);
        metrics.reset();
        assert!(metrics.error_snapshot().is_empty());
        assert!(metrics.last_error().is_empty());
        assert!(metrics.latency_ms().abs() < 1e-9);
        assert!(metrics.jitter_ms().abs() < 1e-9);
    }

    #[test]
    fn in_flight_accounting_never_underflows() {
        let metrics = Metrics::default();
        // 归零状态下的迟到收尾：不得把计数减成 u32::MAX。
        metrics.request_finished();
        assert_eq!(metrics.in_flight.load(Ordering::Relaxed), 0);

        metrics.request_started();
        metrics.request_started();
        metrics.request_finished();
        assert_eq!(metrics.in_flight.load(Ordering::Relaxed), 1);
        metrics.request_finished();
        metrics.request_finished();
        assert_eq!(metrics.in_flight.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn jitter_follows_the_rfc3550_recursion() {
        let metrics = Metrics::default();
        metrics.record_latency(10.0);
        assert!(metrics.jitter_ms().abs() < 1e-9);
        metrics.record_latency(26.0);
        assert!((metrics.jitter_ms() - 1.0).abs() < 1e-9);
        assert!((metrics.latency_ms() - 26.0).abs() < 1e-9);
    }
}
