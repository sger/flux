//! Fibers and tasks as threads (proposal 0188 S8).
//!
//! Thread 1 is the program: outside `run_async`, or the root fiber inside
//! it. Each fiber the program spawns is the next number, in spawn order.
//! A task runs on an OS thread of its own, which the debugger does not
//! pause; it is listed from [`TASK_THREADS`] on, with its state and no
//! stack.
//!
//! Every fiber runs on the one VM while debugging, so the fiber that hit a
//! stop is the VM's stack, cut where the fiber starts. A parked fiber's
//! frames are in its continuation, as a perform's are below its arm (S7).

use std::cell::RefCell;
use std::rc::Rc;

use crate::runtime::continuation::Continuation;
use crate::runtime::value::Value;
use crate::vm::core_dispatch::{self, DebugFiber, DebugFiberState};
use crate::vm::fiber_debug;

use super::Inspector;

/// Thread ids from here on are tasks: `TASK_THREADS + task id`.
pub const TASK_THREADS: u64 = 1000;

/// A thread of the paused program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    pub id: u64,
    pub name: String,
    pub state: ThreadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadState {
    /// The thread the program stopped in.
    Stopped,
    /// Woken, waiting for its turn.
    Ready,
    /// Spawned; its function has not run yet.
    NotStarted,
    /// Parked, waiting for what the text says: `sleep 30 ms`,
    /// `channel #1 recv`, `both (threads 2, 3)`, ...
    Waiting(String),
    /// Its scope was cancelled; it runs its cleanup next.
    Cancelled,
    /// A task on its own OS thread, which the debugger does not pause.
    Running,
    /// A task that has finished.
    Done,
}

impl ThreadState {
    /// `stopped`, `waiting sleep 30 ms`, ...
    pub fn describe(&self) -> String {
        match self {
            ThreadState::Stopped => "stopped".to_string(),
            ThreadState::Ready => "ready".to_string(),
            ThreadState::NotStarted => "not started".to_string(),
            ThreadState::Waiting(what) => format!("waiting {what}"),
            ThreadState::Cancelled => "cancelled".to_string(),
            ThreadState::Running => "running (not paused)".to_string(),
            ThreadState::Done => "done".to_string(),
        }
    }
}

/// What a thread other than the stopped one shows as its stack.
pub(super) enum OtherThread {
    /// A parked or woken fiber: the frames of its continuation. The root
    /// fiber's stack goes on below them, on the VM, from the frame that
    /// called `run_async` down.
    Parked { continuation: Rc<RefCell<Continuation>>, below: Option<usize> },
    /// A fiber that has not started: the function it will run.
    NotStarted(Value),
    /// A task: no frames the debugger can read.
    Task(i64),
}

/// The thread a fiber is: its number, or 1 for the root fiber.
fn fiber_thread(fiber: u64) -> u64 {
    fiber_debug::number(fiber).unwrap_or(1)
}

impl Inspector<'_> {
    /// The thread the program stopped in.
    pub fn stopped_thread(&self) -> u64 {
        core_dispatch::debug_current_fiber().map(|(fiber, _)| fiber_thread(fiber)).unwrap_or(1)
    }

    /// The thread `bt`, `locals` and `print` look at.
    pub fn selected_thread(&self) -> u64 {
        self.control.thread.unwrap_or_else(|| self.stopped_thread())
    }

    /// Looks at thread `id` until the next stop.
    pub fn select_thread(&mut self, id: u64) -> Result<(), String> {
        if !self.threads().iter().any(|thread| thread.id == id) {
            return Err(format!("no thread {id}; `threads` lists them"));
        }
        self.control.thread = (id != self.stopped_thread()).then_some(id);
        Ok(())
    }

    /// The program's threads: its fibers, then its running tasks.
    pub fn threads(&self) -> Vec<ThreadView> {
        let fibers = core_dispatch::debug_fibers();
        let mut threads: Vec<ThreadView> = Vec::new();
        if fibers.is_empty() {
            threads.push(ThreadView {
                id: 1,
                name: "main".to_string(),
                state: ThreadState::Stopped,
            });
        }
        for fiber in &fibers {
            let (number, name, blocked) =
                fiber_debug::fiber(fiber.id).unwrap_or((1, "<fiber>".to_string(), None));
            let name = if number == 1 { "main".to_string() } else { name };
            let state = match fiber.state {
                DebugFiberState::Running => ThreadState::Stopped,
                DebugFiberState::Ready => ThreadState::Ready,
                DebugFiberState::NotStarted => ThreadState::NotStarted,
                DebugFiberState::Cancelled => ThreadState::Cancelled,
                DebugFiberState::Waiting => ThreadState::Waiting(
                    blocked
                        .map(|blocked| blocked.describe(fiber_debug::number))
                        .unwrap_or_else(|| "a completion".to_string()),
                ),
            };
            threads.push(ThreadView { id: number, name, state });
        }
        threads.sort_by_key(|thread| thread.id);
        for (task, name, finished) in fiber_debug::tasks() {
            threads.push(ThreadView {
                id: TASK_THREADS + task as u64,
                name: format!("task {task} ({name})"),
                state: if finished { ThreadState::Done } else { ThreadState::Running },
            });
        }
        threads
    }

    /// The selected thread, when its stack is not the VM's: another fiber
    /// or a task, or the stopped thread itself when the program paused
    /// while every fiber waits.
    pub(super) fn other_thread(&self) -> Option<OtherThread> {
        let id = self.control.thread.unwrap_or_else(|| self.stopped_thread());
        if id >= TASK_THREADS {
            return Some(OtherThread::Task((id - TASK_THREADS) as i64));
        }
        let fiber: DebugFiber =
            core_dispatch::debug_fibers().into_iter().find(|fiber| fiber_thread(fiber.id) == id)?;
        let below = (id == 1).then(core_dispatch::debug_fiber_boundary).flatten();
        match (fiber.parked, fiber.body) {
            (Some(continuation), _) => Some(OtherThread::Parked { continuation, below }),
            (None, Some(body)) => Some(OtherThread::NotStarted(body)),
            (None, None) => None,
        }
    }

    /// The frame index the stopped fiber's stack starts above: the frames
    /// below are `run_async`'s caller's, not the fiber's. None for the root
    /// fiber, which shows them.
    pub(super) fn fiber_base(&self) -> Option<usize> {
        core_dispatch::debug_current_fiber().and_then(|(_, above)| above)
    }
}
