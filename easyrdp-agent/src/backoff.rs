//! 指数退避重连（方案 §5：断线 1s→2s→…→60s 封顶）。

use std::time::Duration;

/// 指数退避序列：initial × factor^n，cap 封顶。
#[derive(Debug, Clone)]
pub struct Backoff {
    initial: Duration,
    cap: Duration,
    factor: u32,
    attempt: u32,
}

impl Backoff {
    pub fn new(initial: Duration, cap: Duration) -> Self {
        Self {
            initial: initial.max(Duration::from_millis(1)),
            cap: cap.max(Duration::from_millis(1)),
            factor: 2,
            attempt: 0,
        }
    }

    /// 自定义增长因子（备用场景；当前主循环固定 factor=2）。
    #[allow(dead_code)]
    pub fn with_factor(mut self, factor: u32) -> Self {
        self.factor = factor.max(1);
        self
    }

    /// 取下一个退避间隔并推进计数。
    pub fn next_delay(&mut self) -> Duration {
        let exp = self.initial.saturating_mul(self.factor.saturating_pow(self.attempt));
        self.attempt = self.attempt.saturating_add(1);
        exp.min(self.cap)
    }

    /// 连接成功后复位（下次断线从 initial 重新起步）。
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// 当前是第几次退避（测试/日志用）。
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_growth_then_cap() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
        assert_eq!(b.next_delay(), Duration::from_secs(4));
        assert_eq!(b.next_delay(), Duration::from_secs(8));
        assert_eq!(b.next_delay(), Duration::from_secs(16));
        assert_eq!(b.next_delay(), Duration::from_secs(32));
        assert_eq!(b.next_delay(), Duration::from_secs(60), "封顶 60s");
        assert_eq!(b.next_delay(), Duration::from_secs(60), "持续封顶");
    }

    #[test]
    fn reset_restarts_from_initial() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(10));
        let _ = b.next_delay();
        let _ = b.next_delay();
        let _ = b.next_delay();
        b.reset();
        assert_eq!(b.attempt(), 0);
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn custom_initial_and_factor() {
        let mut b = Backoff::new(Duration::from_millis(500), Duration::from_millis(1500)).with_factor(3);
        assert_eq!(b.next_delay(), Duration::from_millis(500));
        assert_eq!(b.next_delay(), Duration::from_millis(1500));
        assert_eq!(b.next_delay(), Duration::from_millis(1500), "封顶 1.5s");
    }

    #[test]
    fn tiny_initial_still_positive() {
        let mut b = Backoff::new(Duration::from_millis(0), Duration::from_secs(60));
        assert!(b.next_delay() >= Duration::from_millis(1));
    }
}
