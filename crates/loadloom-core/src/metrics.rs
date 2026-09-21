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
