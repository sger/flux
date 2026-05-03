//! Task scheduler — Rust-side `Task<a>` infrastructure (proposal 0174
//! Phase 1a-vi).
//!
//! Wraps the [`TaskManager`](super::task_manager::TaskManager) worker pool
//! with per-task bookkeeping so callers get the `spawn` / `blocking_join` /
//! `cancel` shape the Flux source surface (`Flow.Task`) needs. The Flux-
//! source layer + LLVM C-shim wiring lands in a follow-up slice; this slice
//! pins the scheduler API and proves the substrate is sound under the
//! concurrent / panicking / cancelled cases.
//!
//! Cancellation semantics — Phase 1a only:
//!   - Cancel **before** the worker picks up the task → the task body is
//!     skipped, the join surfaces `TaskJoinError::Cancelled`.
//!   - Cancel **after** the worker started running → the flag is set, but
//!     Phase 1a tasks run to completion (no yield points yet). Cancellation
//!     becomes observable to running fibers in Phase 1b.
//!   - Cancel **after** the task already finished → no-op; the join still
//!     returns the completed value.
//!
//! Panic semantics: a task body that panics is caught by the worker so it
//! does not poison the pool. The join surfaces `TaskJoinError::Panicked`
//! with the panic payload as a string.

use super::runtime_target::TaskId;
use super::task_manager::{MAX_PRIO, TaskManager};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Outcome of a finished task. Stored on the per-task state so the joiner
/// can pick it up. Cancellation and panic are first-class so callers don't
/// have to bake their own error model into the value type.
enum TaskOutcome<T> {
    Completed(T),
    Cancelled,
    Panicked(String),
}

/// Error returned from [`TaskScheduler::blocking_join`] when the task did
/// not produce a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskJoinError {
    /// The task was cancelled before it produced a value.
    Cancelled,
    /// The task body panicked. The string is best-effort; Rust panics that
    /// don't carry a `String`/`&str` payload are reported as
    /// `"<unknown panic payload>"`.
    Panicked(String),
}

/// Per-task shared state — read by the worker (which writes the outcome)
/// and by the joiner (which waits and consumes it).
struct TaskState<T> {
    outcome: Mutex<Option<TaskOutcome<T>>>,
    finished: Condvar,
    cancelled: AtomicBool,
}

impl<T> TaskState<T> {
    fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
            finished: Condvar::new(),
            cancelled: AtomicBool::new(false),
        }
    }

    fn set_outcome(&self, outcome: TaskOutcome<T>) {
        let mut slot = self.outcome.lock().expect("task outcome poisoned");
        *slot = Some(outcome);
        self.finished.notify_all();
    }
}

/// Handle to a running task. Owns the per-task state via `Arc`; cloning it
/// is cheap and lets multiple consumers cancel concurrently. Joining
/// consumes the handle.
pub struct TaskHandle<T> {
    id: TaskId,
    state: Arc<TaskState<T>>,
}

impl<T> TaskHandle<T> {
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Mark the task cancelled. See module docs for the per-phase
    /// cancellation contract. Idempotent.
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Scheduler — owns the worker pool and the task-id allocator.
#[derive(Debug)]
pub struct TaskScheduler {
    pool: TaskManager,
    next_id: AtomicU64,
}

impl TaskScheduler {
    /// Spin up a scheduler with `n_workers` worker threads.
    pub fn new(n_workers: usize) -> io::Result<Self> {
        let pool = TaskManager::new();
        pool.start(n_workers)?;
        Ok(Self {
            pool,
            next_id: AtomicU64::new(1),
        })
    }

    /// Schedule `action` to run on a worker thread and return a handle.
    ///
    /// `action` must be `Send + 'static` because the worker may execute it
    /// on any thread and after the caller has returned. The Flux-source
    /// surface gates this with the `Sendable<a>` type-class constraint
    /// (see proposal 0174 Phase 1a-v); the Rust analogue is `Send`.
    pub fn spawn<T, F>(&self, action: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let state = Arc::new(TaskState::<T>::new());
        let state_for_job = Arc::clone(&state);
        self.pool.submit(
            // Default priority. A `spawn_with_priority` overload can
            // come in later if user-facing tuning shows up as a need.
            MAX_PRIO / 2,
            Box::new(move || {
                if state_for_job.cancelled.load(Ordering::SeqCst) {
                    state_for_job.set_outcome(TaskOutcome::Cancelled);
                    return;
                }
                let result = catch_unwind(AssertUnwindSafe(action));
                let outcome = match result {
                    Ok(v) => TaskOutcome::Completed(v),
                    Err(payload) => TaskOutcome::Panicked(panic_payload(&payload)),
                };
                state_for_job.set_outcome(outcome);
            }),
        );
        TaskHandle { id, state }
    }

    /// Block the calling thread until the task finishes, then return its
    /// outcome. Consumes the handle.
    pub fn blocking_join<T>(&self, handle: TaskHandle<T>) -> Result<T, TaskJoinError>
    where
        T: Send + 'static,
    {
        let mut guard = handle.state.outcome.lock().expect("task outcome poisoned");
        while guard.is_none() {
            guard = handle
                .state
                .finished
                .wait(guard)
                .expect("task condvar poisoned");
        }
        match guard.take().expect("loop exits only on Some") {
            TaskOutcome::Completed(v) => Ok(v),
            TaskOutcome::Cancelled => Err(TaskJoinError::Cancelled),
            TaskOutcome::Panicked(s) => Err(TaskJoinError::Panicked(s)),
        }
    }

    /// Drain the worker pool and join every worker. Called from `Drop`.
    pub fn shutdown(&self) {
        self.pool.shutdown();
    }
}

impl Drop for TaskScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn panic_payload(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn spawn_returns_completed_value() {
        let sched = TaskScheduler::new(2).unwrap();
        let h = sched.spawn(|| 7 * 6);
        assert_eq!(sched.blocking_join(h), Ok(42));
    }

    #[test]
    fn spawn_many_concurrent_tasks_all_complete() {
        // 200 tasks, 4 workers, each task summing 0..n. Catches deadlocks
        // and lost-wakeup bugs when N tasks > N workers.
        let sched = TaskScheduler::new(4).unwrap();
        let mut handles = Vec::new();
        for i in 0..200u64 {
            handles.push(sched.spawn(move || (0..i).sum::<u64>()));
        }
        for (i, h) in handles.into_iter().enumerate() {
            let expected = (0..i as u64).sum::<u64>();
            assert_eq!(sched.blocking_join(h), Ok(expected));
        }
    }

    #[test]
    fn tasks_run_on_multiple_threads() {
        // Use a barrier wider than 1 worker; the only way it unblocks is if
        // tasks really run on different threads in parallel.
        let n = 4;
        let sched = TaskScheduler::new(n).unwrap();
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::new();
        for i in 0..n {
            let b = Arc::clone(&barrier);
            handles.push(sched.spawn(move || {
                b.wait();
                i
            }));
        }
        let mut results: Vec<usize> = handles
            .into_iter()
            .map(|h| sched.blocking_join(h).unwrap())
            .collect();
        results.sort();
        assert_eq!(results, (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn cancel_before_pickup_short_circuits() {
        // Single worker held busy by a blocking job; the second-submitted
        // task is cancelled while still queued. Its body must NOT run.
        let sched = TaskScheduler::new(1).unwrap();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = Arc::new(Mutex::new(Some(gate_rx)));
        let gate_rx_for_job = Arc::clone(&gate_rx);

        let blocker = sched.spawn(move || {
            let rx = gate_rx_for_job.lock().unwrap().take().unwrap();
            rx.recv().unwrap();
            "blocker-done".to_string()
        });

        let ran = Arc::new(AtomicBool::new(false));
        let ran_for_job = Arc::clone(&ran);
        let cancelled_task = sched.spawn(move || {
            ran_for_job.store(true, Ordering::SeqCst);
            "should-not-run".to_string()
        });
        cancelled_task.cancel();

        gate_tx.send(()).unwrap();
        assert_eq!(sched.blocking_join(blocker), Ok("blocker-done".to_string()));
        assert_eq!(
            sched.blocking_join(cancelled_task),
            Err(TaskJoinError::Cancelled)
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "cancelled-before-pickup task body must not execute"
        );
    }

    #[test]
    fn cancel_after_completion_is_a_noop() {
        let sched = TaskScheduler::new(1).unwrap();
        let h = sched.spawn(|| 99);
        // Wait until the task has obviously completed.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if h.state.outcome.lock().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "task never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
        h.cancel();
        assert_eq!(sched.blocking_join(h), Ok(99));
    }

    #[test]
    fn panicking_task_is_reported_and_does_not_poison_pool() {
        let sched = TaskScheduler::new(2).unwrap();
        let h_panic = sched.spawn(|| -> i32 { panic!("kaboom") });
        let err = sched.blocking_join(h_panic).unwrap_err();
        match err {
            TaskJoinError::Panicked(s) => assert!(s.contains("kaboom"), "got: {s}"),
            other => panic!("expected Panicked, got {other:?}"),
        }
        // Pool is not poisoned — subsequent tasks still run.
        let h_ok = sched.spawn(|| 5);
        assert_eq!(sched.blocking_join(h_ok), Ok(5));
    }

    #[test]
    fn shutdown_joins_running_tasks_promptly() {
        // Forgotten shutdown must not leak workers — past async branches
        // wedged libtest on Windows. The Drop impl on TaskScheduler joins.
        let counter = Arc::new(AtomicUsize::new(0));
        let start = Instant::now();
        {
            let sched = TaskScheduler::new(4).unwrap();
            for _ in 0..32 {
                let c = Arc::clone(&counter);
                let _h = sched.spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                });
            }
            // Drop sched here.
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Drop must not block the test indefinitely"
        );
    }
}
