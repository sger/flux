//! Worker thread pool with shared priority queue (proposal 0174 Phase 1a-iii).
//!
//! Lean 4's `task_manager` (`src/runtime/object.cpp:706-916`) is the shape
//! reference: N OS threads, a shared per-priority FIFO of submittable jobs,
//! a parking condvar, and a single shutdown flag. Workers loop pulling the
//! highest-priority queued job; an empty queue parks them on the condvar
//! until either a new submission or shutdown wakes them.
//!
//! What's deliberately out of scope for this slice:
//!   - The user-facing `Task<a>` primitive (`spawn`/`blocking_join`/`cancel`).
//!     That's slice 1a-vi, once `Sendable<T>` (1a-v) and atomic-on-share RC
//!     (1a-iv) are in place to support cross-worker value transfer.
//!   - Completion routing from the `mio` reactor. Pinned by 1a-iii as the
//!     `Job` shape but not wired end-to-end until the registry is shared
//!     across workers (incremental on this skeleton).
//!   - Per-worker affinity. Phase 1a runs every job on whichever worker
//!     picks it up; Phase 1b's fiber layer is what enforces home-worker
//!     no-migration (Eio's model).
//!
//! Shutdown contract — load-bearing on Windows, where leaked worker threads
//! prevented libtest from exiting on the previous async branch:
//!   - `shutdown` flips the flag, broadcasts the condvar, and joins every
//!     worker.
//!   - Submissions after shutdown are dropped silently. Workers re-check
//!     the flag each time they wake.
//!   - `Drop` calls `shutdown` so a forgotten teardown can't leak threads.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// Number of priority levels. `0` = high (runs first), `MAX_PRIO` = low.
pub const MAX_PRIO: usize = 2;
const NUM_QUEUES: usize = MAX_PRIO + 1;

/// Unit of work the pool runs. Boxed `FnOnce` so each submission is one-shot.
/// `Send + 'static` because workers may execute the job on any thread and
/// after the submitter has returned.
pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// Shared state read/written by every worker and by submitters.
struct Shared {
    shutdown: AtomicBool,
    queues: Mutex<[VecDeque<Job>; NUM_QUEUES]>,
    not_empty: Condvar,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("shutdown", &self.shutdown.load(Ordering::Relaxed))
            .field(
                "queue_lens",
                &self
                    .queues
                    .lock()
                    .map(|q| q.iter().map(|qq| qq.len()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

#[derive(Debug)]
struct Inner {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

/// Worker pool managing a shared priority queue.
#[derive(Debug, Default)]
pub struct TaskManager {
    inner: Mutex<Option<Inner>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn `n_workers` worker threads. Returns an error if any spawn fails;
    /// already-spawned workers are joined by the subsequent `shutdown`.
    /// Calling `start` while running is a no-op.
    pub fn start(&self, n_workers: usize) -> io::Result<()> {
        assert!(
            n_workers > 0,
            "TaskManager::start needs at least one worker"
        );
        let mut slot = self.inner.lock().expect("TaskManager mutex poisoned");
        if slot.is_some() {
            return Ok(());
        }

        let shared = Arc::new(Shared {
            shutdown: AtomicBool::new(false),
            queues: Mutex::new(std::array::from_fn(|_| VecDeque::new())),
            not_empty: Condvar::new(),
        });

        let mut workers = Vec::with_capacity(n_workers);
        for i in 0..n_workers {
            let s = Arc::clone(&shared);
            let handle = thread::Builder::new()
                .name(format!("flux-worker-{i}"))
                .spawn(move || run_worker(s))
                .map_err(io::Error::other)?;
            workers.push(handle);
        }

        *slot = Some(Inner { shared, workers });
        Ok(())
    }

    /// Submit a job at the given priority. Lower numeric values run first
    /// (`0` is highest). Submissions after `shutdown` are dropped silently —
    /// the manager is a fire-and-forget surface, not a result channel.
    pub fn submit(&self, prio: usize, job: Job) {
        assert!(
            prio <= MAX_PRIO,
            "priority {prio} exceeds MAX_PRIO={MAX_PRIO}"
        );
        let slot = self.inner.lock().expect("TaskManager mutex poisoned");
        let Some(inner) = slot.as_ref() else {
            return;
        };
        if inner.shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        inner.shared.queues.lock().expect("queue mutex poisoned")[prio].push_back(job);
        inner.shared.not_empty.notify_one();
    }

    /// Flip the shutdown flag, wake every worker, and join them. Idempotent.
    pub fn shutdown(&self) {
        let mut inner = match self
            .inner
            .lock()
            .expect("TaskManager mutex poisoned")
            .take()
        {
            Some(inner) => inner,
            None => return,
        };
        inner.shared.shutdown.store(true, Ordering::SeqCst);
        inner.shared.not_empty.notify_all();

        for w in inner.workers.drain(..) {
            // Workers should never panic; if one does, surface via stderr —
            // the pool stays best-effort on shutdown so test teardown is
            // never blocked by a poisoned worker.
            if let Err(e) = w.join() {
                eprintln!("flux task manager: worker panicked during shutdown: {e:?}");
            }
        }
    }

    /// Number of running workers, or 0 if not started.
    pub fn worker_count(&self) -> usize {
        self.inner
            .lock()
            .expect("TaskManager mutex poisoned")
            .as_ref()
            .map(|i| i.workers.len())
            .unwrap_or(0)
    }

    /// Snapshot of queued jobs across all priorities. Test-only.
    #[cfg(test)]
    pub(crate) fn queued_len(&self) -> usize {
        self.inner
            .lock()
            .expect("TaskManager mutex poisoned")
            .as_ref()
            .and_then(|i| i.shared.queues.lock().ok())
            .map(|q| q.iter().map(|qq| qq.len()).sum())
            .unwrap_or(0)
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_worker(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut queues = shared.queues.lock().expect("queue mutex poisoned");
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(j) = pop_highest_priority(&mut queues) {
                    break j;
                }
                queues = shared.not_empty.wait(queues).expect("condvar poisoned");
            }
        };
        // Run the job outside the queue lock so other workers can steal in
        // parallel.
        job();
    }
}

fn pop_highest_priority(queues: &mut [VecDeque<Job>; NUM_QUEUES]) -> Option<Job> {
    queues.iter_mut().find_map(|q| q.pop_front())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn start_then_shutdown_joins_cleanly() {
        let pool = TaskManager::new();
        pool.start(2).unwrap();
        assert_eq!(pool.worker_count(), 2);
        pool.shutdown();
        assert_eq!(pool.worker_count(), 0);
    }

    #[test]
    fn start_is_idempotent() {
        let pool = TaskManager::new();
        pool.start(2).unwrap();
        pool.start(2).unwrap();
        assert_eq!(pool.worker_count(), 2);
        pool.shutdown();
    }

    #[test]
    fn shutdown_is_idempotent() {
        let pool = TaskManager::new();
        pool.shutdown();
        pool.start(1).unwrap();
        pool.shutdown();
        pool.shutdown();
        assert_eq!(pool.worker_count(), 0);
    }

    #[test]
    fn drop_shuts_down_pool_promptly() {
        // Forgotten shutdown must not leak workers — the previous async
        // branch wedged libtest on Windows when threads outlived their
        // owner. Verify Drop joins synchronously.
        let start = Instant::now();
        {
            let pool = TaskManager::new();
            pool.start(4).unwrap();
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Drop must join workers without blocking the test"
        );
    }

    #[test]
    fn submitted_jobs_all_run() {
        let pool = TaskManager::new();
        pool.start(4).unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let n = 64;

        for _ in 0..n {
            let c = Arc::clone(&counter);
            pool.submit(
                1,
                Box::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }

        // Drain via shutdown: workers run remaining jobs in their loop until
        // the queue is empty AND the shutdown flag is observed. To avoid
        // racing against shutdown dropping pending jobs, we busy-wait for
        // the counter to reach n first.
        let deadline = Instant::now() + Duration::from_secs(5);
        while counter.load(Ordering::SeqCst) < n && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(counter.load(Ordering::SeqCst), n);
        pool.shutdown();
    }

    #[test]
    fn higher_priority_jobs_run_first() {
        // Single worker so ordering is deterministic. Submit a low-priority
        // job that blocks on a channel until we say go — this guarantees the
        // queue has both priorities present when the worker next looks. Then
        // submit a high-priority job, release the gate, and verify the high-
        // priority one ran before the second low-priority one.
        let pool = TaskManager::new();
        pool.start(1).unwrap();

        let (gate_tx, gate_rx) = mpsc::sync_channel::<()>(0);
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        // Block worker on a channel.
        let o = Arc::clone(&order);
        pool.submit(
            MAX_PRIO,
            Box::new(move || {
                gate_rx.recv().unwrap();
                o.lock().unwrap().push("blocker");
            }),
        );

        // Wait until the worker has actually picked up the blocker (queue
        // empty + worker now parked on the channel). Without this, the worker
        // could grab all three jobs at once and `pop_highest_priority` would
        // (correctly) return the high-prio one before the blocker — defeating
        // the test's setup.
        let deadline = Instant::now() + Duration::from_secs(2);
        while pool.queued_len() != 0 {
            assert!(Instant::now() < deadline, "worker never picked up blocker");
            thread::sleep(Duration::from_millis(2));
        }

        // Now stack the queue: high-prio AFTER another low-prio. The high
        // one must still be picked first when the blocker releases.
        let o1 = Arc::clone(&order);
        pool.submit(
            MAX_PRIO,
            Box::new(move || {
                o1.lock().unwrap().push("low");
            }),
        );
        let o2 = Arc::clone(&order);
        pool.submit(
            0,
            Box::new(move || {
                o2.lock().unwrap().push("high");
            }),
        );

        // Release the blocker.
        gate_tx.send(()).unwrap();
        // Wait for all three to run.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if order.lock().unwrap().len() == 3 {
                break;
            }
            assert!(Instant::now() < deadline, "jobs did not all run");
            thread::sleep(Duration::from_millis(2));
        }

        let final_order = order.lock().unwrap().clone();
        assert_eq!(
            final_order,
            vec!["blocker", "high", "low"],
            "high-priority job must run before queued low-priority job"
        );
        pool.shutdown();
    }

    #[test]
    fn submission_after_shutdown_is_dropped_silently() {
        let pool = TaskManager::new();
        pool.start(1).unwrap();
        pool.shutdown();

        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        pool.submit(
            0,
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );

        // No worker to run it; counter stays zero.
        thread::sleep(Duration::from_millis(20));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(pool.queued_len(), 0, "manager not running, queue absent");
    }

    #[test]
    fn many_workers_concurrent_jobs_no_deadlock() {
        // Catches condvar / queue-lock ordering bugs where a wake gets lost
        // and a worker stays parked while others have jobs to feed it.
        let pool = TaskManager::new();
        pool.start(8).unwrap();
        let counter = Arc::new(AtomicUsize::new(0));

        for i in 0..1000 {
            let c = Arc::clone(&counter);
            let prio = i % NUM_QUEUES;
            pool.submit(
                prio,
                Box::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while counter.load(Ordering::SeqCst) < 1000 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1000);
        pool.shutdown();
    }
}
