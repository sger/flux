//! Phase 1a task manager.
//!
//! This is the Rust runtime substrate for 0174's `Task<a>` primitive. Tasks
//! execute on OS worker threads and communicate results through typed Rust
//! handles. Flux heap `Value`s are deliberately not part of this API; VM/native
//! integration must cross this boundary only after `Sendable<T>` has approved a
//! copied or opaque representation.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

use super::context::TaskId;

const PRIORITY_LEVELS: usize = 3;

type TaskJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskPriority(usize);

impl TaskPriority {
    pub const HIGH: Self = Self(0);
    pub const NORMAL: Self = Self(1);
    pub const LOW: Self = Self(2);

    fn index(self) -> usize {
        self.0.min(PRIORITY_LEVELS - 1)
    }
}

impl Default for TaskPriority {
    fn default() -> Self {
        Self::NORMAL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskManagerConfig {
    pub worker_count: usize,
}

impl Default for TaskManagerConfig {
    fn default() -> Self {
        Self {
            worker_count: thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    AlreadyJoined,
    Canceled,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct TaskCancelToken {
    canceled: Arc<AtomicBool>,
}

impl TaskCancelToken {
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct TaskHandle<T> {
    id: TaskId,
    shared: Arc<TaskShared<T>>,
}

impl<T> Clone for TaskHandle<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            shared: Arc::clone(&self.shared),
        }
    }
}

#[derive(Debug)]
struct TaskShared<T> {
    canceled: Arc<AtomicBool>,
    joined: AtomicBool,
    result: Mutex<Option<Result<T, TaskError>>>,
    completed: Condvar,
}

pub struct TaskManager {
    inner: Arc<TaskManagerInner>,
    workers: Vec<thread::JoinHandle<()>>,
}

struct TaskManagerInner {
    next_task_id: AtomicU64,
    shutdown: AtomicBool,
    queues: Mutex<Vec<VecDeque<TaskJob>>>,
    parked: Condvar,
}

impl<T> TaskHandle<T> {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn cancel(&self) -> bool {
        if self
            .shared
            .result
            .lock()
            .expect("task result lock is not poisoned")
            .is_some()
        {
            return false;
        }
        self.shared.canceled.store(true, Ordering::Release);
        true
    }

    pub fn is_canceled(&self) -> bool {
        self.shared.canceled.load(Ordering::Acquire)
    }

    pub fn blocking_join(self) -> Result<T, TaskError> {
        if self.shared.joined.swap(true, Ordering::AcqRel) {
            return Err(TaskError::AlreadyJoined);
        }
        let mut result = self
            .shared
            .result
            .lock()
            .expect("task result lock is not poisoned");
        loop {
            if let Some(result) = result.take() {
                return result;
            }
            result = self
                .shared
                .completed
                .wait(result)
                .expect("task result lock is not poisoned");
        }
    }
}

impl TaskManager {
    pub fn new(config: TaskManagerConfig) -> Self {
        let worker_count = config.worker_count.max(1);
        let inner = Arc::new(TaskManagerInner {
            next_task_id: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
            queues: Mutex::new((0..PRIORITY_LEVELS).map(|_| VecDeque::new()).collect()),
            parked: Condvar::new(),
        });
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let inner = Arc::clone(&inner);
            workers.push(thread::spawn(move || worker_loop(inner)));
        }
        Self { inner, workers }
    }

    pub fn spawn<T, F>(&self, priority: TaskPriority, task: F) -> Result<TaskHandle<T>, TaskError>
    where
        T: Send + 'static,
        F: FnOnce(TaskCancelToken) -> T + Send + 'static,
    {
        if self.inner.shutdown.load(Ordering::Acquire) {
            return Err(TaskError::Shutdown);
        }

        let id = TaskId(self.inner.next_task_id.fetch_add(1, Ordering::Relaxed));
        let shared = Arc::new(TaskShared {
            canceled: Arc::new(AtomicBool::new(false)),
            joined: AtomicBool::new(false),
            result: Mutex::new(None),
            completed: Condvar::new(),
        });
        let token = TaskCancelToken {
            canceled: Arc::clone(&shared.canceled),
        };
        let job_shared = Arc::clone(&shared);
        let job = Box::new(move || {
            let result = if token.is_canceled() {
                Err(TaskError::Canceled)
            } else {
                let value = task(token.clone());
                if token.is_canceled() {
                    Err(TaskError::Canceled)
                } else {
                    Ok(value)
                }
            };
            finish_task(&job_shared, result);
        });

        {
            let mut queues = self
                .inner
                .queues
                .lock()
                .expect("task queue lock is not poisoned");
            queues[priority.index()].push_back(job);
        }
        self.inner.parked.notify_one();

        Ok(TaskHandle { id, shared })
    }

    pub fn shutdown(mut self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.inner.parked.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.inner.parked.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn finish_task<T>(shared: &TaskShared<T>, result: Result<T, TaskError>) {
    let mut slot = shared
        .result
        .lock()
        .expect("task result lock is not poisoned");
    *slot = Some(result);
    shared.completed.notify_all();
}

fn worker_loop(inner: Arc<TaskManagerInner>) {
    loop {
        let job = {
            let mut queues = inner
                .queues
                .lock()
                .expect("task queue lock is not poisoned");
            loop {
                if inner.shutdown.load(Ordering::Acquire) {
                    return;
                }
                if let Some(job) = pop_next_job(&mut queues) {
                    break job;
                }
                queues = inner
                    .parked
                    .wait(queues)
                    .expect("task queue lock is not poisoned");
            }
        };
        job();
    }
}

fn pop_next_job(queues: &mut [VecDeque<TaskJob>]) -> Option<TaskJob> {
    queues.iter_mut().find_map(VecDeque::pop_front)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn task_manager_runs_task_and_blocking_join_returns_result() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let task = manager
            .spawn(TaskPriority::NORMAL, |_| 42)
            .expect("task spawns");
        assert_eq!(task.id(), TaskId(1));

        assert_eq!(task.blocking_join(), Ok(42));
        manager.shutdown();
    }

    #[test]
    fn task_manager_prioritizes_higher_priority_queued_work() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let (block_tx, block_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let blocker = manager
            .spawn(TaskPriority::NORMAL, move |_| {
                let _ = block_rx.recv();
                0
            })
            .expect("blocker spawns");
        let low_done = done_tx.clone();
        let low = manager
            .spawn(TaskPriority::LOW, move |_| {
                low_done.send("low").expect("send succeeds");
                1
            })
            .expect("low task spawns");
        let high_done = done_tx;
        let high = manager
            .spawn(TaskPriority::HIGH, move |_| {
                high_done.send("high").expect("send succeeds");
                2
            })
            .expect("high task spawns");

        block_tx.send(()).expect("unblock succeeds");
        assert_eq!(blocker.blocking_join(), Ok(0));
        assert_eq!(done_rx.recv().expect("first task completes"), "high");
        assert_eq!(high.blocking_join(), Ok(2));
        assert_eq!(low.blocking_join(), Ok(1));
        manager.shutdown();
    }

    #[test]
    fn task_cancel_before_run_suppresses_work() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let (block_tx, block_rx) = mpsc::channel();
        let blocker = manager
            .spawn(TaskPriority::NORMAL, move |_| {
                let _ = block_rx.recv();
                0
            })
            .expect("blocker spawns");
        let task = manager
            .spawn(TaskPriority::NORMAL, |_| 1)
            .expect("task spawns");
        assert!(task.cancel());

        block_tx.send(()).expect("unblock succeeds");
        assert_eq!(blocker.blocking_join(), Ok(0));
        assert_eq!(task.blocking_join(), Err(TaskError::Canceled));
        manager.shutdown();
    }

    #[test]
    fn task_cancel_token_supports_cooperative_cancellation() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let task = manager
            .spawn(TaskPriority::NORMAL, |token| {
                let deadline = Instant::now() + Duration::from_millis(100);
                while Instant::now() < deadline {
                    if token.is_canceled() {
                        return 0;
                    }
                    thread::yield_now();
                }
                1
            })
            .expect("task spawns");
        assert!(task.cancel());

        assert_eq!(task.blocking_join(), Err(TaskError::Canceled));
        manager.shutdown();
    }

    #[test]
    fn task_joining_clone_after_result_reports_already_joined() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let task = manager
            .spawn(TaskPriority::NORMAL, |_| 42)
            .expect("task spawns");
        let clone = task.clone();

        assert_eq!(task.blocking_join(), Ok(42));
        assert_eq!(clone.blocking_join(), Err(TaskError::AlreadyJoined));
        manager.shutdown();
    }

    #[test]
    fn task_cancel_after_completion_does_not_discard_result() {
        let manager = TaskManager::new(TaskManagerConfig { worker_count: 1 });
        let task = manager
            .spawn(TaskPriority::NORMAL, |_| 42)
            .expect("task spawns");

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && !task.is_completed_for_test() {
            thread::yield_now();
        }

        assert!(!task.cancel());
        assert_eq!(task.blocking_join(), Ok(42));
        manager.shutdown();
    }
}

#[cfg(test)]
impl<T> TaskHandle<T> {
    fn is_completed_for_test(&self) -> bool {
        self.shared
            .result
            .lock()
            .expect("task result lock is not poisoned")
            .is_some()
    }
}
