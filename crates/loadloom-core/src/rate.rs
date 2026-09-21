//! 令牌桶限速器：所有 worker 共享一个桶，速率可在运行中动态调整。
//!
//! 关键设计：`rate == 0`（不限速）是压测下的绝对热路径，此时通过 `rate_bits`
//! 原子镜像做前置判断，完全免锁、免等待；只有真正限速时才进入互斥区。
//! 桶容量随速率缩放（`rate * 0.05`）并设 128 KiB 下限，避免小速率下过度平滑。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 限速器
// ---------------------------------------------------------------------------

struct Bucket {
    rate: f64,
    tokens: f64,
    capacity: f64,
    last: Instant,
}

impl Bucket {
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        if self.rate > 0.0 {
            self.tokens = (self.tokens + self.rate * elapsed).min(self.capacity);
        }
    }
}

/// 全局令牌桶：速率在所有 worker 之间共享。
pub(crate) struct RateLimiter {
    inner: Mutex<Bucket>,
    /// `rate` 的原子镜像（f64 位模式），让**不限速**路径可以完全免锁。
    rate_bits: AtomicU64,
}

impl RateLimiter {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Bucket {
                rate: 0.0,
                tokens: 0.0,
                capacity: 256.0 * 1024.0,
                last: Instant::now(),
            }),
            rate_bits: AtomicU64::new(0),
        }
    }

    /// 动态设置速率（byte/s），`<= 0` 表示不限速。
    pub(crate) fn set_rate(&self, rate: f64) {
        let mut bucket = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        bucket.refill();
        bucket.rate = if rate > 0.0 { rate } else { 0.0 };
        bucket.capacity = (bucket.rate * 0.05).max(128.0 * 1024.0);
        bucket.tokens = bucket.tokens.min(bucket.capacity);
        self.rate_bits
            .store(bucket.rate.to_bits(), Ordering::Release);
    }

    /// 消费指定字节数的令牌，必要时异步等待（不阻塞线程）。
    pub(crate) async fn acquire(&self, bytes: usize) {
        // ---- 快路径：不限速时直接返回，完全不加锁 ----
        //
        // 这是压测下的绝对热路径：`worker` 对**每一个**数据块都要调用一次
        // （典型 8~64KB 一块），100 MB/s 就是每秒数千次调用。原实现即使
        // `rate == 0` 也要先抢一次全局 `Mutex`，于是 32 个 worker 被一把锁
        // 人为串行化 —— 这正是「要开 8 线程才顶得上原来 4 线程」的元凶之一。
        //
        // 现在用原子镜像做前置判断：不限速时零锁、零等待、零唤醒。
        if f64::from_bits(self.rate_bits.load(Ordering::Acquire)) <= 0.0 {
            return;
        }
        loop {
            let wait = {
                let mut bucket = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                bucket.refill();
                if bucket.rate <= 0.0 {
                    return;
                }
                let need = bytes as f64;
                if bucket.tokens >= need {
                    bucket.tokens -= need;
                    return;
                }
                let deficit = need - bucket.tokens;
                bucket.tokens = 0.0;
                Duration::from_secs_f64(deficit / bucket.rate)
            };
            tokio::time::sleep(wait.max(Duration::from_micros(200))).await;
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror(limiter: &RateLimiter) -> f64 {
        f64::from_bits(limiter.rate_bits.load(Ordering::Relaxed))
    }

    fn capacity(limiter: &RateLimiter) -> f64 {
        limiter
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .capacity
    }

    #[test]
    fn set_rate_is_mirrored_for_the_lock_free_fast_path() {
        let limiter = RateLimiter::new();
        limiter.set_rate(1_000_000.0);
        assert!((mirror(&limiter) - 1_000_000.0).abs() < 1.0);
    }

    #[test]
    fn non_positive_rate_means_unlimited_and_clears_the_mirror() {
        let limiter = RateLimiter::new();
        limiter.set_rate(1_000_000.0);
        limiter.set_rate(0.0);
        assert!(mirror(&limiter).abs() < 1e-9);
        limiter.set_rate(1_000_000.0);
        limiter.set_rate(-42.0);
        assert!(mirror(&limiter).abs() < 1e-9);
    }

    #[test]
    fn capacity_tracks_rate_with_a_128kib_floor() {
        let limiter = RateLimiter::new();
        limiter.set_rate(10_000_000.0);
        assert!((capacity(&limiter) - 500_000.0).abs() < 1.0);
        limiter.set_rate(1.0);
        assert!((capacity(&limiter) - 128.0 * 1024.0).abs() < 1.0);
    }
}
