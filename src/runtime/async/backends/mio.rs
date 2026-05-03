//! `mio`-backed async reactor (proposal 0174 Phase 1a).
//!
//! Reactor lifecycle (1a-i):
//!   - `start()` spawns a dedicated reactor thread that owns a `mio::Poll`.
//!   - `shutdown()` flips a flag, wakes the reactor, and joins.
//!   - `Drop` calls `shutdown` so a forgotten teardown can't leak the thread.
//!
//! Timer service (1a-ii):
//!   - `timer_start(req, ms)` pushes a deadline into a shared min-heap and
//!     wakes the reactor so it can recompute its next `poll` timeout.
//!   - The reactor passes the next deadline as `Poll::poll`'s timeout. When
//!     `poll` returns it pops every expired entry, drops cancelled ones,
//!     and pushes a `CompletionPayload::Unit` for the rest into a shared
//!     completions queue. Owners pull via `next_completion()`.
//!   - `cancel(req)` records the request as cancelled; the reactor skips
//!     producing a completion for it on fire and drops any already-queued
//!     completion for that request.
//!
//! Threading model:
//!   - One reactor thread per [`MioBackend`]; it is the only thread that
//!     touches `Poll` or pops from the timer heap.
//!   - The owning thread shares the heap, completions queue, cancelled set,
//!     a `mio::Waker`, and an `AtomicBool` shutdown flag with the reactor.
//!   - `Waker::wake` is the one and only mechanism used to nudge the reactor
//!     out of a blocking `poll` — used both for shutdown and for "a new timer
//!     was scheduled, recompute your timeout".

use super::super::backend::{AsyncBackend, Completion, CompletionPayload, RequestId};
use mio::{Events, Poll, Token, Waker};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Token reserved for the cross-thread `Waker`. Phase 1a-vii allocates per-
/// TCP-source tokens above this; the registry will own that allocation.
const WAKER_TOKEN: Token = Token(0);

/// One scheduled timer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TimerEntry {
    deadline: Instant,
    request_id: RequestId,
}

// `BinaryHeap` is a max-heap; reverse the comparison so the soonest deadline
// is at the top. Ties broken by request_id for stable ordering.
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.request_id.cmp(&self.request_id))
    }
}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

/// Shared state between the owning thread and the reactor thread.
#[derive(Debug)]
struct ReactorShared {
    waker: Waker,
    shutdown: AtomicBool,
    timers: Mutex<BinaryHeap<TimerEntry>>,
    cancelled: Mutex<HashSet<RequestId>>,
    completions: Mutex<VecDeque<Completion>>,
}

/// `mio`-backed async backend.
#[derive(Debug, Default)]
pub struct MioBackend {
    inner: Mutex<Option<Inner>>,
}

#[derive(Debug)]
struct Inner {
    shared: Arc<ReactorShared>,
    handle: Option<JoinHandle<()>>,
}

impl MioBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn start_inner(&self) -> io::Result<()> {
        let mut slot = self.inner.lock().expect("MioBackend mutex poisoned");
        if slot.is_some() {
            return Ok(());
        }

        let poll = Poll::new()?;
        let waker = Waker::new(poll.registry(), WAKER_TOKEN)?;
        let shared = Arc::new(ReactorShared {
            waker,
            shutdown: AtomicBool::new(false),
            timers: Mutex::new(BinaryHeap::new()),
            cancelled: Mutex::new(HashSet::new()),
            completions: Mutex::new(VecDeque::new()),
        });

        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("flux-mio-reactor".into())
            .spawn(move || run_reactor(poll, thread_shared))
            .map_err(io::Error::other)?;

        *slot = Some(Inner {
            shared,
            handle: Some(handle),
        });
        Ok(())
    }

    fn shutdown_inner(&self) -> io::Result<()> {
        let mut inner = match self.inner.lock().expect("MioBackend mutex poisoned").take() {
            Some(inner) => inner,
            None => return Ok(()),
        };

        inner.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = inner.shared.waker.wake();

        if let Some(handle) = inner.handle.take()
            && let Err(e) = handle.join()
        {
            return Err(io::Error::other(format!(
                "mio reactor thread panicked: {e:?}"
            )));
        }
        Ok(())
    }

    /// Run `f` against the live shared state, or no-op if the backend is
    /// stopped. Avoids exposing `Inner`/`Arc` to the trait impls.
    fn with_shared<R>(&self, f: impl FnOnce(&Arc<ReactorShared>) -> R) -> Option<R> {
        self.inner
            .lock()
            .expect("MioBackend mutex poisoned")
            .as_ref()
            .map(|inner| f(&inner.shared))
    }

    /// True when the reactor thread is currently running. Test-only helper.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.inner
            .lock()
            .expect("MioBackend mutex poisoned")
            .is_some()
    }
}

impl AsyncBackend for MioBackend {
    fn start(&self) -> Result<(), String> {
        self.start_inner().map_err(|e| e.to_string())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.shutdown_inner().map_err(|e| e.to_string())
    }

    fn cancel(&self, req: RequestId) {
        // Record the cancellation so the reactor skips firing it. Also drop
        // any already-queued completion for the request — once the registry
        // has marked the entry cancelled, the synthetic-cancelled-error path
        // owns delivery, not the original timer fire.
        if let Some(()) = self.with_shared(|shared| {
            shared
                .cancelled
                .lock()
                .expect("cancelled-set poisoned")
                .insert(req);
            shared
                .completions
                .lock()
                .expect("completions poisoned")
                .retain(|c| c.request_id != req);
            // Wake the reactor so it can prune cancelled head-of-heap entries
            // promptly rather than waiting on its next deadline.
            let _ = shared.waker.wake();
        }) {
            // ran
        }
    }

    fn timer_start(&self, req: RequestId, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        if let Some(()) = self.with_shared(|shared| {
            shared
                .timers
                .lock()
                .expect("timer-heap poisoned")
                .push(TimerEntry {
                    deadline,
                    request_id: req,
                });
            // Wake the reactor so it recomputes its next poll timeout against
            // the new deadline.
            let _ = shared.waker.wake();
        }) {
            // ran
        }
    }

    fn next_completion(&self) -> Option<Completion> {
        self.with_shared(|shared| {
            shared
                .completions
                .lock()
                .expect("completions poisoned")
                .pop_front()
        })
        .flatten()
    }
}

impl Drop for MioBackend {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

/// Reactor thread entry point.
fn run_reactor(mut poll: Poll, shared: Arc<ReactorShared>) {
    let mut events = Events::with_capacity(64);
    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }

        // Compute the timeout for `poll`: the time until the soonest live
        // (non-cancelled) deadline, or `None` to block forever if no timers
        // are scheduled. Cancelled head entries are pruned here so the
        // timeout is accurate.
        let timeout = next_live_deadline(&shared).map(|d| {
            let now = Instant::now();
            d.checked_duration_since(now).unwrap_or(Duration::ZERO)
        });

        match poll.poll(&mut events, timeout) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("flux mio reactor: poll failed: {e}");
                return;
            }
        }

        // The Waker is the only registered source today, so this loop is
        // short. Future slices will dispatch I/O events here.
        for _event in events.iter() {
            // Token-routing comes in 1a-vii.
        }

        // Independently of why `poll` returned (timeout, waker, spurious),
        // fire any timers whose deadline has passed.
        fire_due_timers(&shared);
    }
}

/// Pop expired and cancelled entries off the front of the heap, returning
/// the soonest live deadline (or `None` if the heap is empty).
fn next_live_deadline(shared: &ReactorShared) -> Option<Instant> {
    let mut timers = shared.timers.lock().expect("timer-heap poisoned");
    let cancelled = shared.cancelled.lock().expect("cancelled-set poisoned");
    while let Some(top) = timers.peek() {
        if cancelled.contains(&top.request_id) {
            timers.pop();
            continue;
        }
        return Some(top.deadline);
    }
    None
}

/// Pop all entries with `deadline <= now` and produce completions for the
/// ones whose request was not cancelled.
fn fire_due_timers(shared: &ReactorShared) {
    let now = Instant::now();
    let mut timers = shared.timers.lock().expect("timer-heap poisoned");
    let mut cancelled = shared.cancelled.lock().expect("cancelled-set poisoned");
    let mut completions = shared.completions.lock().expect("completions poisoned");

    while let Some(top) = timers.peek() {
        if top.deadline > now {
            break;
        }
        let entry = timers.pop().expect("peek matched");
        if cancelled.remove(&entry.request_id) {
            // Cancelled before fire — drop without producing a completion.
            continue;
        }
        completions.push_back(Completion {
            request_id: entry.request_id,
            payload: CompletionPayload::Unit,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Spin-poll `next_completion` for up to `timeout`. Returns the
    /// completion or `None` if the timeout elapses. Used because the
    /// scheduler/worker pool does not exist yet — owners poll today.
    fn wait_for(backend: &MioBackend, timeout: Duration) -> Option<Completion> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(c) = backend.next_completion() {
                return Some(c);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn start_then_shutdown_joins_cleanly() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        assert!(backend.is_running());
        backend.shutdown().unwrap();
        assert!(!backend.is_running());
    }

    #[test]
    fn start_is_idempotent() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        backend.start().unwrap();
        backend.start().unwrap();
        assert!(backend.is_running());
        backend.shutdown().unwrap();
    }

    #[test]
    fn shutdown_is_idempotent() {
        let backend = MioBackend::new();
        backend.shutdown().unwrap(); // before any start
        backend.start().unwrap();
        backend.shutdown().unwrap();
        backend.shutdown().unwrap(); // after shutdown
        assert!(!backend.is_running());
    }

    #[test]
    fn drop_shuts_down_reactor_thread() {
        let start = Instant::now();
        {
            let backend = MioBackend::new();
            backend.start().unwrap();
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Drop must join the reactor thread without blocking the test"
        );
    }

    #[test]
    fn many_start_shutdown_cycles_do_not_leak() {
        let backend = MioBackend::new();
        for _ in 0..8 {
            backend.start().unwrap();
            backend.shutdown().unwrap();
        }
        assert!(!backend.is_running());
    }

    #[test]
    fn timer_fires_after_delay() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        let scheduled = Instant::now();
        backend.timer_start(RequestId(1), 30);

        let completion =
            wait_for(&backend, Duration::from_secs(2)).expect("timer must fire within 2s");
        assert_eq!(completion.request_id, RequestId(1));
        assert_eq!(completion.payload, CompletionPayload::Unit);
        // Sanity: the timer didn't fire instantly. Allow generous slack for
        // CI scheduling jitter — we mostly want to catch a "fire immediately"
        // bug, not enforce tight timing.
        assert!(
            scheduled.elapsed() >= Duration::from_millis(20),
            "timer fired suspiciously early: {:?}",
            scheduled.elapsed()
        );
        backend.shutdown().unwrap();
    }

    #[test]
    fn two_timers_fire_in_deadline_order() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        // Schedule the longer timer first, the shorter one second. The short
        // one must fire first regardless of registration order.
        backend.timer_start(RequestId(2), 120);
        backend.timer_start(RequestId(1), 30);

        let first =
            wait_for(&backend, Duration::from_secs(2)).expect("first timer fires within 2s");
        let second =
            wait_for(&backend, Duration::from_secs(2)).expect("second timer fires within 2s");

        assert_eq!(first.request_id, RequestId(1));
        assert_eq!(second.request_id, RequestId(2));
        backend.shutdown().unwrap();
    }

    #[test]
    fn cancel_before_fire_suppresses_completion() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        backend.timer_start(RequestId(1), 200);
        backend.timer_start(RequestId(2), 30);

        // Cancel the long timer before it could fire.
        backend.cancel(RequestId(1));

        // The short timer fires.
        let c = wait_for(&backend, Duration::from_secs(2)).expect("short timer fires within 2s");
        assert_eq!(c.request_id, RequestId(2));

        // No further completion should arrive within a reasonable grace period.
        let extra = wait_for(&backend, Duration::from_millis(400));
        assert!(
            extra.is_none(),
            "cancelled timer must not produce a completion: got {extra:?}"
        );
        backend.shutdown().unwrap();
    }

    #[test]
    fn shutdown_with_pending_timers_does_not_hang() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        // Schedule a timer well beyond the test budget.
        backend.timer_start(RequestId(1), 60_000);

        let start = Instant::now();
        backend.shutdown().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "shutdown must wake the reactor out of its deadline-sized poll"
        );
        assert!(!backend.is_running());
    }
}
