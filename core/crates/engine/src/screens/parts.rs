//! Progressive loading for screens with slow sections (TODAY, CALENDAR).
//!
//! Each slow section is a [`Part`]: a background fetch on the engine runtime
//! that the screen waits on for at most [`FIRST_PAINT`]. Sections still
//! missing show a "…: loading…" note and the screen asks to be refreshed
//! after [`FILL_IN_MS`], when they fill in. With a fixed clock (tests and
//! snapshots) screens wait for every part, so output is deterministic.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

/// How long a screen waits for its slower sections before showing what it
/// has.
pub(crate) const FIRST_PAINT: Duration = Duration::from_millis(250);
/// Refresh interval while any section is still loading.
pub(crate) const FILL_IN_MS: u32 = 1_000;

/// One slower screen section, loading in the background. Panes showing the
/// screen share one fetch per set of inputs, and while a refresh fetches
/// again the last result keeps showing, so sections don't blink out when a
/// cache expires.
pub(crate) struct Part<T> {
    inner: Mutex<Option<PartState<T>>>,
}

struct PartState<T> {
    sig: String,
    rx: watch::Receiver<Option<Arc<T>>>,
    /// The fetch's result has been returned to a caller; the next call
    /// fetches again (cheaply, when the cache is still fresh).
    delivered: bool,
    last: Option<Arc<T>>,
}

impl<T> Default for Part<T> {
    fn default() -> Self {
        Self { inner: Mutex::new(None) }
    }
}

impl<T: Send + Sync + 'static> Part<T> {
    /// The section for inputs `sig`: waits up to `FIRST_PAINT` (until it's
    /// done with `wait_all`, i.e. a fixed clock), else returns the last result
    /// for the same inputs, or `None` while it has never loaded.
    pub(crate) async fn get<F>(&self, rt: &tokio::runtime::Handle, wait_all: bool, sig: &str, fetch: impl FnOnce() -> F) -> Option<Arc<T>>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let mut rx = {
            let mut g = self.inner.lock();
            // Still running, or finished with a result nobody has seen yet.
            let reuse = g.as_ref().filter(|p| {
                let done = p.rx.borrow().is_some();
                p.sig == sig && ((!done && p.rx.has_changed().is_ok()) || (done && !p.delivered))
            });
            if let Some(p) = reuse {
                p.rx.clone()
            } else {
                let last = g.as_ref().filter(|p| p.sig == sig).and_then(|p| p.rx.borrow().clone().or_else(|| p.last.clone()));
                let (tx, rx) = watch::channel(None);
                let fut = fetch();
                rt.spawn(async move {
                    let _ = tx.send(Some(Arc::new(fut.await)));
                });
                *g = Some(PartState { sig: sig.to_owned(), rx: rx.clone(), delivered: false, last });
                rx
            }
        };
        let got: Option<Arc<T>> = if wait_all {
            rx.wait_for(Option::is_some).await.ok().and_then(|v| v.clone())
        } else {
            match tokio::time::timeout(FIRST_PAINT, rx.wait_for(Option::is_some)).await {
                Ok(Ok(v)) => v.clone(),
                _ => None,
            }
        };
        let mut g = self.inner.lock();
        let state = g.as_mut().filter(|p| p.sig == sig);
        match got {
            Some(v) => {
                if let Some(p) = state {
                    p.last = Some(v.clone());
                    p.delivered |= p.rx.same_channel(&rx);
                }
                Some(v)
            }
            None => state.and_then(|p| p.last.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fetch that takes `ms` and counts how often it was started.
    fn slow(n: &Arc<AtomicUsize>, ms: u64, v: u32) -> impl Future<Output = u32> + Send + 'static {
        n.fetch_add(1, Ordering::SeqCst);
        async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            v
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_part_is_fetched_once_and_fills_in_later() {
        let (part, n, rt) = (Part::<u32>::default(), Arc::new(AtomicUsize::new(0)), tokio::runtime::Handle::current());
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 600, 1)).await, None, "still loading after the first paint");
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 600, 1)).await, None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 600, 1)).await.as_deref(), Some(&1));
        assert_eq!(n.load(Ordering::SeqCst), 1, "callers share one fetch");
    }

    #[tokio::test(start_paused = true)]
    async fn a_refresh_shows_the_last_result_until_the_new_one_arrives() {
        let (part, n, rt) = (Part::<u32>::default(), Arc::new(AtomicUsize::new(0)), tokio::runtime::Handle::current());
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 10, 1)).await.as_deref(), Some(&1));
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 900, 2)).await.as_deref(), Some(&1), "stale while refreshing");
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_eq!(part.get(&rt, false, "a", || slow(&n, 900, 3)).await.as_deref(), Some(&2));
        assert_eq!(part.get(&rt, false, "b", || slow(&n, 900, 4)).await, None, "never another input's data");
        assert_eq!(part.get(&rt, true, "c", || slow(&n, 900, 5)).await.as_deref(), Some(&5), "a fixed clock waits");
    }
}
