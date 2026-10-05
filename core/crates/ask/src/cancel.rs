use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// Cooperative cancellation for one ASK question. Cloning shares the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelFlag {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`cancel`](Self::cancel) has been called.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            // Register before checking so a concurrent cancel can't slip
            // between the check and the await.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn cancelled_resolves_after_cancel() {
        let c = CancelFlag::new();
        let c2 = c.clone();
        let h = tokio::spawn(async move { c2.cancelled().await });
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!h.is_finished());
        c.cancel();
        tokio::time::timeout(Duration::from_secs(1), h).await.unwrap().unwrap();
        assert!(c.is_cancelled());
        // Already-cancelled resolves immediately.
        c.cancelled().await;
    }
}
