//! 令牌桶限速器。可以在下载过程中随时调整速度（界面上拖动限速滑块）。
//! 多个任务共用同一个限速器就是"全局限速"。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    /// 每秒字节数，0 表示不限速。
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// 等到可以发送 `n` 字节为止。
    pub async fn acquire(&self, n: usize) {
        loop {
            let rate = self.rate();
            if rate == 0 {
                return;
            }
            let wait = {
                let mut b = self.bucket.lock().unwrap();
                let now = Instant::now();
                let elapsed = now.duration_since(b.last).as_secs_f64();
                b.last = now;
                // 最多攒 0.5 秒的额度，避免突发。
                let cap = (rate as f64 * 0.5).max(n as f64);
                b.tokens = (b.tokens + elapsed * rate as f64).min(cap);
                if b.tokens >= n as f64 {
                    b.tokens -= n as f64;
                    return;
                }
                let deficit = n as f64 - b.tokens;
                Duration::from_secs_f64((deficit / rate as f64).min(0.25))
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limits_throughput() {
        let l = RateLimiter::new(1_000_000);
        let start = Instant::now();
        for _ in 0..20 {
            l.acquire(100_000).await; // 共 2MB
        }
        let secs = start.elapsed().as_secs_f64();
        assert!(secs > 1.6 && secs < 2.8, "took {secs}s");
    }

    #[tokio::test]
    async fn unlimited_is_instant() {
        let l = RateLimiter::unlimited();
        let start = Instant::now();
        for _ in 0..1000 {
            l.acquire(1 << 20).await;
        }
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
