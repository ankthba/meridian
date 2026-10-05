use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::capability::RateLimit;

/// Async token bucket. `acquire` waits until a token is available.
#[derive(Debug)]
pub struct TokenBucket {
    limit: RateLimit,
    state: Mutex<BucketState>,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    #[must_use]
    pub fn new(limit: RateLimit) -> Self {
        Self {
            limit,
            state: Mutex::new(BucketState { tokens: f64::from(limit.burst), last: Instant::now() }),
        }
    }

    /// Takes one token, or returns how long to wait for the next one.
    pub fn try_acquire(&self) -> Result<(), Duration> {
        let mut st = self.state.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(st.last).as_secs_f64();
        st.tokens = (st.tokens + elapsed * self.limit.per_second).min(f64::from(self.limit.burst));
        st.last = now;
        if st.tokens >= 1.0 {
            st.tokens -= 1.0;
            Ok(())
        } else {
            let missing = 1.0 - st.tokens;
            Err(Duration::from_secs_f64(missing / self.limit.per_second.max(1e-9)))
        }
    }

    pub async fn acquire(&self) {
        loop {
            match self.try_acquire() {
                Ok(()) => return,
                Err(wait) => tokio::time::sleep(wait).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn bucket_refills_over_time() {
        let b = TokenBucket::new(RateLimit { burst: 2, per_second: 1.0 });
        assert!(b.try_acquire().is_ok());
        assert!(b.try_acquire().is_ok());
        let wait = b.try_acquire().unwrap_err();
        assert!(wait <= Duration::from_secs(1));
        tokio::time::advance(Duration::from_millis(1001)).await;
        assert!(b.try_acquire().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_waits() {
        let b = TokenBucket::new(RateLimit { burst: 1, per_second: 10.0 });
        b.acquire().await;
        let start = Instant::now();
        b.acquire().await;
        assert!(start.elapsed() >= Duration::from_millis(99));
    }
}
