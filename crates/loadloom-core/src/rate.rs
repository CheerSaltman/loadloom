//! 令牌桶限速器：所有 worker 共享一个桶，速率可在运行中动态调整。
//!
//! 关键设计：`rate == 0`（不限速）是压测下的绝对热路径，此时通过 `rate_bits`
//! 原子镜像做前置判断，完全免锁、免等待；只有真正限速时才进入互斥区。
//! 桶容量随速率缩放（`rate * 0.05`）并设 128 KiB 下限，避免小速率下过度平滑。
//!
//! ## 面对不可信输入的契约
//!
//! 速率最终来自 IPC 载荷（`rateMib` 是一个外部可控的 `f64`），因此本模块对
//! **任意** `f64` 都必须是全函数：不允许 panic，也不允许出现无界等待。
//!
//! * 非有限值（`NaN` / `±∞`）与非正数一律按「不限速」处理；
//! * 单次等待由 [`MAX_WAIT`] 封顶，且**只**通过 [`wait_from_seconds`] 构造
//!   `Duration` —— `Duration::from_secs_f64` 在超出可表示范围时直接 panic，
//!   而 `deficit / rate` 在极小速率下轻易越界（`rate = 1e-294` 时
//!   `65536 / rate ≈ 6e298`，远超 `Duration` 约 1.8e19 秒的上限）；
//! * 令牌债务（负余额）下探不超过一个桶容量，避免超长等待被逐次累积。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 单次 `acquire` 允许的最长等待。
///
/// 限速是软约束：等待到这个量级已经等价于停流，继续等下去只会让 worker
/// 无法及时响应 `stop()`。封顶后行为退化为「按上限放行」—— 宁可比标称速率
/// 快，也不制造一个不可中断的超长挂起。
const MAX_WAIT: Duration = Duration::from_secs(5);
/// 任何非零等待的下限，避免退化成忙等。
const MIN_WAIT: Duration = Duration::from_micros(200);
/// 桶容量下限（128 KiB）。
const MIN_CAPACITY: f64 = 128.0 * 1024.0;
/// 桶容量相对速率的比例，相当于 0.05 秒的突发额度。
const CAPACITY_RATIO: f64 = 0.05;

/// 把秒数换算成**有界**的 `Duration`。
///
/// 纯函数，可单测。`NaN` 与非正数输入返回 [`Duration::ZERO`]；
/// 超过 [`MAX_WAIT`] 的输入收敛到上限。这是「绝不用未校验的浮点构造
/// `Duration`」这条约束的唯一出口。
fn wait_from_seconds(seconds: f64) -> Duration {
    // `+inf` 必须封顶而不是归零：把溢出当成「无需等待」会让限速在这条路径上
    // 彻底失效（fail-open）。只有 `NaN` 与非正数才表示「不限速」。
    if seconds.is_nan() || seconds <= 0.0 {
        return Duration::ZERO;
    }
    if seconds == f64::INFINITY {
        return MAX_WAIT;
    }
    Duration::try_from_secs_f64(seconds)
        .unwrap_or(MAX_WAIT)
        .min(MAX_WAIT)
}

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

    /// 预订 `bytes` 字节，返回需要等待的时长（纯计算，不睡眠）。
    ///
    /// `tokens` 允许暂时为负数，代表已经为其它 worker 预订、但尚未到账的令牌：
    /// 这样单个响应块大于桶容量时仍能在一次等待后完成。如果每次等待都把余额
    /// 清零，超过容量的块将永远拿不到足够令牌而陷入循环。
    fn reserve(&mut self, bytes: f64) -> Duration {
        self.refill();
        if self.rate <= 0.0 {
            return Duration::ZERO;
        }
        let deficit = (bytes - self.tokens).max(0.0);
        // 债务封顶：余额最多透支一个桶容量。否则「响应块大于桶容量」的连续请求
        // 会把债务越滚越大，最终算出一个天文数字的等待 —— 那正是溢出 panic 的来源。
        self.tokens = (self.tokens - bytes).max(-self.capacity);
        wait_from_seconds(deficit / self.rate)
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

    /// 互斥锁中毒（上一次持锁时 panic）视同可用：一次 panic 就永久丢掉限速
    /// 能力，只会把问题放大成「限速静默消失」。
    fn lock(&self) -> MutexGuard<'_, Bucket> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 动态设置速率（byte/s）。非有限值与非正数表示**不限速**。
    pub(crate) fn set_rate(&self, rate: f64) {
        let mut bucket = self.lock();
        bucket.refill();
        bucket.rate = if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            0.0
        };
        bucket.capacity = (bucket.rate * CAPACITY_RATIO).max(MIN_CAPACITY);
        bucket.tokens = bucket.tokens.min(bucket.capacity);
        self.rate_bits
            .store(bucket.rate.to_bits(), Ordering::Release);
    }

    /// 预订指定字节数的令牌，必要时异步等待（不阻塞线程）。
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
        let wait = self.lock().reserve(bytes as f64);
        if !wait.is_zero() {
            tokio::time::sleep(wait.max(MIN_WAIT)).await;
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
        limiter.lock().capacity
    }

    /// 造一个「余额为零」的桶，用于直接驱动 `reserve`（不必真的睡）。
    fn empty_bucket(rate: f64) -> Bucket {
        Bucket {
            rate,
            tokens: 0.0,
            capacity: (rate * CAPACITY_RATIO).max(MIN_CAPACITY),
            last: Instant::now(),
        }
    }

    #[test]
    fn wait_is_bounded_for_any_seconds_value() {
        assert_eq!(wait_from_seconds(f64::NAN), Duration::ZERO);
        // 溢出（`+inf`）必须封顶，不能被当成「无需等待」——那会让限速静默失效。
        assert_eq!(wait_from_seconds(f64::INFINITY), MAX_WAIT);
        assert_eq!(wait_from_seconds(f64::NEG_INFINITY), Duration::ZERO);
        assert_eq!(wait_from_seconds(-1.0), Duration::ZERO);
        assert_eq!(wait_from_seconds(0.0), Duration::ZERO);
        assert_eq!(
            wait_from_seconds(Duration::from_millis(5).as_secs_f64()),
            Duration::from_millis(5)
        );
        // 表示范围之外与范围内的超大值都必须收敛到上限，而不是 panic。
        assert_eq!(wait_from_seconds(1e300), MAX_WAIT);
        assert_eq!(wait_from_seconds(f64::MAX), MAX_WAIT);
        assert_eq!(wait_from_seconds(MAX_WAIT.as_secs_f64() * 2.0), MAX_WAIT);
    }

    #[test]
    fn an_absurdly_small_rate_yields_a_bounded_wait() {
        // 回归点：`rateMib = 1e-300` 这类畸形载荷经 `mib_to_bps` 得到约
        // 1e-294 byte/s，`deficit / rate` 会算出约 6e298 秒 —— 旧实现直接把它
        // 交给 `Duration::from_secs_f64`，于是 panic，worker 静默消失。
        let mut bucket = empty_bucket(1e-294);
        assert_eq!(bucket.reserve(65_536.0), MAX_WAIT);
        assert_eq!(bucket.reserve(1024.0 * 1024.0), MAX_WAIT);
    }

    #[test]
    fn token_debt_is_capped_at_one_capacity() {
        let mut bucket = empty_bucket(10_000_000.0);
        let capacity = bucket.capacity;
        for _ in 0..8 {
            let _ = bucket.reserve(64.0 * 1024.0 * 1024.0);
        }
        assert!(
            bucket.tokens >= -capacity,
            "余额下探到 {}，超过了一个桶容量 {capacity} —— 债务会无限累积",
            bucket.tokens
        );
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
    fn non_finite_rate_is_treated_as_unlimited() {
        let limiter = RateLimiter::new();
        limiter.set_rate(1_000_000.0);
        limiter.set_rate(f64::NAN);
        assert!(mirror(&limiter).abs() < 1e-9);
        limiter.set_rate(1_000_000.0);
        limiter.set_rate(f64::INFINITY);
        assert!(mirror(&limiter).abs() < 1e-9);
        assert!(capacity(&limiter).is_finite());
    }

    #[test]
    fn capacity_tracks_rate_with_a_128kib_floor() {
        let limiter = RateLimiter::new();
        limiter.set_rate(10_000_000.0);
        assert!((capacity(&limiter) - 500_000.0).abs() < 1.0);
        limiter.set_rate(1.0);
        assert!((capacity(&limiter) - 128.0 * 1024.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn a_chunk_larger_than_capacity_is_scheduled_once() {
        let limiter = RateLimiter::new();
        limiter.set_rate(1_000_000.0);

        // 1 MB/s 时桶容量为 128 KiB。本请求的块更大，旧实现会在每轮等待
        // 后重新把余额置零，因而永远不能通过容量检查。
        let result =
            tokio::time::timeout(Duration::from_secs(1), limiter.acquire(256 * 1024)).await;
        assert!(result.is_ok(), "大于桶容量的响应块不应永久等待");
    }

    #[tokio::test]
    async fn acquire_returns_for_a_degenerate_rate() {
        let limiter = RateLimiter::new();
        limiter.set_rate(1e-294);
        // 判据是「有界返回」而不是「不等待」：等待被 MAX_WAIT 封顶。
        let result =
            tokio::time::timeout(MAX_WAIT + Duration::from_secs(2), limiter.acquire(4096)).await;
        assert!(result.is_ok(), "畸形速率不得让 worker 永久挂起");
    }
}
