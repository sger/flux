//! `mio`-backed async reactor (proposal 0174 Phase 1a, slice 1a-i).
//!
//! Slice 1a-i ships the reactor lifecycle only — `start()` spawns a dedicated
//! reactor thread that owns a `mio::Poll`, and `shutdown()` wakes it and joins
//! cleanly. No timers, no TCP, no completions yet; those land in 1a-ii (timer
//! heap) and 1a-vii (TCP state machines). The point of this slice is to lock
//! down the cross-platform spin-up/tear-down story before anything real
//! depends on it — leaked reactor threads on Windows are how the previous
//! async branch wedged libtest.
//!
//! Threading model:
//!   - One reactor thread per [`MioBackend`]. It owns the `Poll` and the
//!     event loop.
//!   - The owning thread (whoever calls `start`/`shutdown`) shares a
//!     `mio::Waker` and an `AtomicBool` shutdown flag with the reactor.
//!   - `shutdown` flips the flag, wakes the reactor, and joins. The reactor
//!     wakes from `poll`, observes the flag, and returns.
//!   - `Drop` calls `shutdown` so a forgotten teardown can't leak the thread.

use super::super::backend::{AsyncBackend, RequestId};
use mio::{Events, Poll, Token, Waker};
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

/// Token reserved for the cross-thread `Waker`. Phase 1a-iii allocates per-
/// request tokens above this; the registry will own that allocation.
const WAKER_TOKEN: Token = Token(0);

/// Shared state between the owning thread and the reactor thread.
#[derive(Debug)]
struct ReactorShared {
    /// Used by the owning thread to nudge `poll` out of its blocking call.
    waker: Waker,
    /// Set by `shutdown` before waking; checked by the reactor each loop
    /// iteration.
    shutdown: AtomicBool,
}

/// `mio`-backed async backend.
///
/// `start` is idempotent (a second call after a successful start is a no-op);
/// `shutdown` is idempotent and called from `Drop`.
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

    /// Spin up the reactor thread. Returns an error only if creating the
    /// `Poll` or `Waker` fails (rare on supported platforms).
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

    /// Signal shutdown, wake the reactor, and join its thread. Idempotent.
    fn shutdown_inner(&self) -> io::Result<()> {
        let mut inner = match self.inner.lock().expect("MioBackend mutex poisoned").take() {
            Some(inner) => inner,
            None => return Ok(()),
        };

        inner.shared.shutdown.store(true, Ordering::SeqCst);
        // Wake the reactor so it observes the flag promptly. If the wake call
        // itself fails (poll already torn down by another path) we still try
        // to join — the thread should already be exiting.
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

    fn cancel(&self, _req: RequestId) {
        // 1a-iii wires this to the request registry; today there are no
        // requests to cancel.
    }
}

impl Drop for MioBackend {
    fn drop(&mut self) {
        // Best-effort: if shutdown fails (thread already gone, etc.) we have
        // nothing to do about it from a destructor.
        let _ = self.shutdown_inner();
    }
}

/// Reactor thread entry point.
///
/// Loops on `poll.poll(events, None)` (block until something happens) and
/// exits cleanly the first time the shutdown flag is observed. With no I/O
/// sources registered yet, the only event we ever see is the cross-thread
/// `Waker`. Future slices will register timers and TCP sources here.
fn run_reactor(mut poll: Poll, shared: Arc<ReactorShared>) {
    let mut events = Events::with_capacity(64);
    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        match poll.poll(&mut events, None) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // `poll` failing is unrecoverable here — log via stderr and exit
            // so `shutdown` can still join. Phase 1a-iii will wire this into
            // the scheduler's diagnostic channel.
            Err(e) => {
                eprintln!("flux mio reactor: poll failed: {e}");
                return;
            }
        }
        for event in events.iter() {
            if event.token() == WAKER_TOKEN {
                // The only purpose today is to break out of `poll` so we can
                // recheck the shutdown flag at the top of the loop.
            }
            // Other tokens land here in later slices.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
        // Without `Drop` calling shutdown, libtest worker threads on Windows
        // do not reliably run thread-locals before exit and the reactor leaks.
        // Verify the join happens promptly inside Drop.
        let start = Instant::now();
        {
            let backend = MioBackend::new();
            backend.start().unwrap();
            // backend dropped here; should join the reactor synchronously
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Drop must join the reactor thread without blocking the test"
        );
    }

    #[test]
    fn many_start_shutdown_cycles_do_not_leak() {
        // Cycle a single backend through start/shutdown several times. Past
        // bugs leaked a thread per start when shutdown was forgotten; this
        // catches the same shape if shutdown ever stops being load-bearing.
        let backend = MioBackend::new();
        for _ in 0..8 {
            backend.start().unwrap();
            backend.shutdown().unwrap();
        }
        assert!(!backend.is_running());
    }
}
