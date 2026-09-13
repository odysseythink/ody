//! M4 concurrency alignment with Claude Code dynamic workflows
//! (`Behavior and limits` / `Prompt caching in a fan-out`, see the M4 plan):
//! a per-run concurrent-agent cap, a per-run total-spawn cap, and the
//! fan-out prefix stagger hold.
//!
//! All three script/declarative carriers land every spawn in
//! [`SessionFlowAgentHost`](super::host::SessionFlowAgentHost), so the gates
//! live here and carriers inherit them without driver changes. Checkpoint
//! replay hits never reach `RunConcurrency::acquire`, so cached agents
//! consume neither a concurrency slot nor a spawn count — matching the
//! Claude behavior where completed agents return their saved results.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::FlowHostError;

/// Claude Code `Behavior and limits`: up to 16 concurrent agents per run.
pub(crate) const FLOW_CONCURRENCY_LIMIT: usize = 16;

/// Claude Code `Behavior and limits`: at most 1,000 spawned agents per
/// run, bounding runaway loops.
pub(crate) const FLOW_MAX_AGENTS_PER_RUN: usize = 1000;

/// Default prefix-stagger window; overridable via
/// `ODY_FLOW_PREFIX_STAGGER_MS` (`0` disables the hold).
pub(crate) const DEFAULT_STAGGER_MS: u64 = 5000;

/// Per-run spawn gate. The semaphore bounds in-flight agents; the counter
/// bounds total spawns for the whole run.
pub(crate) struct RunConcurrency {
    permits: Arc<Semaphore>,
    spawned: Arc<AtomicUsize>,
    max_total: usize,
}

impl RunConcurrency {
    pub(crate) fn new() -> Self {
        Self::with_limits(FLOW_CONCURRENCY_LIMIT, FLOW_MAX_AGENTS_PER_RUN)
    }

    fn with_limits(limit: usize, max_total: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit)),
            spawned: Arc::new(AtomicUsize::new(0)),
            max_total,
        }
    }

    /// Gate one spawn: wait for a concurrency slot, then enforce the
    /// per-run total. The returned permit must be held until the agent
    /// reaches a final status; dropping it releases the concurrency slot.
    /// If the total cap rejects the spawn, the slot is released before
    /// returning the error.
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, FlowHostError> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| FlowHostError("flow run concurrency gate closed".to_string()))?;
        let seq = self.spawned.fetch_add(1, Ordering::SeqCst) + 1;
        if seq > self.max_total {
            drop(permit);
            return Err(FlowHostError(format!(
                "flow run exceeded the agent limit of {} total spawns",
                self.max_total
            )));
        }
        Ok(permit)
    }
}

/// Fan-out prefix stagger (Claude Code `Prompt caching in a fan-out`): the
/// first agent of a run starts immediately; agents that spawn within the
/// stagger window after it are held until the first agent's wait begins
/// (the observable proxy for "its response has begun") or the window
/// elapses, whichever comes first.
pub(crate) struct PrefixStagger {
    window: Duration,
    state: Mutex<StaggerState>,
    notify: Notify,
}

#[derive(Default)]
struct StaggerState {
    first_started: Option<Instant>,
    response_began: bool,
}

impl PrefixStagger {
    /// Window from `ODY_FLOW_PREFIX_STAGGER_MS` (default
    /// [`DEFAULT_STAGGER_MS`]; `0` disables the hold).
    pub(crate) fn from_env() -> Self {
        Self::with_window(stagger_window_from_env())
    }

    pub(crate) fn with_window(window: Duration) -> Self {
        Self {
            window,
            state: Mutex::new(StaggerState::default()),
            notify: Notify::new(),
        }
    }

    /// Gate one spawn. The first spawn through this gate records the
    /// window anchor and passes immediately. Spawns inside the window
    /// wait for [`signal_response_began`](Self::signal_response_began) or
    /// the remaining window; spawns at or past the window pass directly.
    pub(crate) async fn gate(&self) {
        loop {
            let remaining = {
                let mut state = self.state.lock().expect("stagger state poisoned");
                match state.first_started {
                    None => {
                        state.first_started = Some(Instant::now());
                        return;
                    }
                    Some(started) => {
                        if state.response_began {
                            return;
                        }
                        let elapsed = started.elapsed();
                        if elapsed >= self.window {
                            return;
                        }
                        Some(self.window - elapsed)
                    }
                }
            };
            let Some(remaining) = remaining else {
                return;
            };
            tokio::select! {
                _ = self.notify.notified() => continue,
                _ = tokio::time::sleep(remaining) => return,
            }
        }
    }

    /// Idempotent signal that the first agent's response has begun;
    /// releases every spawn currently held by [`gate`](Self::gate).
    pub(crate) async fn signal_response_began(&self) {
        {
            let mut state = self.state.lock().expect("stagger state poisoned");
            if state.response_began {
                return;
            }
            state.response_began = true;
        }
        self.notify.notify_waiters();
    }
}

pub(crate) fn stagger_window_from_env() -> Duration {
    Duration::from_millis(parse_stagger_ms(
        std::env::var("ODY_FLOW_PREFIX_STAGGER_MS").ok(),
    ))
}

/// Pure env-value parsing (`None`/unparseable → [`DEFAULT_STAGGER_MS`]).
fn parse_stagger_ms(raw: Option<String>) -> u64 {
    raw.and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STAGGER_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn assert_send_sync<T: Send + Sync>() {}
    #[test]
    fn gates_are_send_sync() {
        assert_send_sync::<RunConcurrency>();
        assert_send_sync::<PrefixStagger>();
    }

    #[tokio::test]
    async fn concurrency_allows_up_to_limit() {
        let gate = RunConcurrency::with_limits(2, 100);
        let p1 = gate.acquire().await.expect("first permit");
        let p2 = gate.acquire().await.expect("second permit");
        // Third acquire must block while both slots are held.
        let third = tokio::time::timeout(Duration::from_millis(100), gate.acquire());
        assert!(third.await.is_err(), "third acquire should wait for a free slot");
        drop(p1);
        let p3 = gate
            .acquire()
            .await
            .expect("third permit after a slot frees");
        drop((p2, p3));
    }

    #[tokio::test]
    async fn total_spawn_cap_rejects_and_releases_slot() {
        let gate = RunConcurrency::with_limits(1, 2);
        let p1 = gate.acquire().await.expect("first");
        drop(p1);
        let p2 = gate.acquire().await.expect("second");
        drop(p2);
        // Third spawn exceeds the total cap even though slots are free.
        let err = gate.acquire().await.expect_err("total cap must reject");
        assert!(err.0.contains("agent limit of 2"), "unexpected error: {err}");
        // The rejected acquire must have released its concurrency slot.
        assert_eq!(
            gate.permits.available_permits(),
            1,
            "rejected acquire must free its slot"
        );
    }

    #[tokio::test]
    async fn stagger_first_spawn_passes_immediately() {
        let stagger = PrefixStagger::with_window(Duration::from_secs(60));
        tokio::time::timeout(Duration::from_millis(50), stagger.gate())
            .await
            .expect("first gate must not block");
    }

    #[tokio::test(start_paused = true)]
    async fn stagger_signal_releases_waiter() {
        let stagger = std::sync::Arc::new(PrefixStagger::with_window(Duration::from_secs(60)));
        let first = stagger.clone();
        let anchor = tokio::spawn(async move {
            first.gate().await;
        });
        anchor.await.expect("first gate");
        let held = {
            let stagger = stagger.clone();
            tokio::spawn(async move {
                stagger.gate().await;
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!held.is_finished(), "second spawn should be held");
        stagger.signal_response_began().await;
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("signal must release the held spawn")
            .expect("held gate panics");
    }

    #[tokio::test(start_paused = true)]
    async fn stagger_window_elapses_without_signal() {
        let stagger = PrefixStagger::with_window(Duration::from_secs(1));
        stagger.gate().await;
        tokio::time::timeout(Duration::from_secs(30), stagger.gate())
            .await
            .expect("gate must return when the window elapses");
    }

    #[tokio::test(start_paused = true)]
    async fn stagger_disabled_window_passes_through() {
        let stagger = PrefixStagger::with_window(Duration::ZERO);
        stagger.gate().await;
        tokio::time::timeout(Duration::from_secs(1), stagger.gate())
            .await
            .expect("zero window must not hold");
    }

    #[test]
    fn stagger_window_parsing() {
        assert_eq!(parse_stagger_ms(None), 5000);
        assert_eq!(parse_stagger_ms(Some("250".to_string())), 250);
        assert_eq!(parse_stagger_ms(Some("0".to_string())), 0);
        assert_eq!(parse_stagger_ms(Some("not-a-number".to_string())), 5000);
    }
}
