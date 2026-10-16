//! What the debugger knows about fibers and tasks (proposal 0188 S8).
//!
//! The scheduler keeps fibers by id and request; to show each as a thread
//! the debugger also needs a name, a number that stays put, and what a
//! parked fiber waits for. These records are kept only while a debugger is
//! attached, on the thread that runs the program: nothing is recorded, and
//! nothing costs more than one flag read, otherwise.
//!
//! Fibers are numbered in the order they are spawned, from 1 for the root
//! fiber of the outermost `run_async`; with one worker that order is the
//! program's own, so the VM and native number them alike.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

use crate::runtime::value::Value;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static RECORDS: RefCell<Records> = RefCell::new(Records::default());
}

#[derive(Default)]
struct Records {
    fibers: HashMap<u64, FiberRecord>,
    next_number: u64,
    tasks: BTreeMap<i64, TaskRecord>,
}

struct FiberRecord {
    number: u64,
    name: String,
    blocked: Option<Blocked>,
}

struct TaskRecord {
    name: String,
    finished: Box<dyn Fn() -> bool>,
}

/// What a parked fiber waits for. Fibers are named by scheduler id; the
/// thread view turns them into thread numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Blocked {
    Sleep { ms: u64 },
    Yield,
    Task { id: i64 },
    ChannelRecv { channel: i64 },
    ChannelSend { channel: i64 },
    Event { id: i64 },
    Both { fibers: Vec<u64> },
    Race { fibers: Vec<u64> },
    FirstOf { fibers: Vec<u64> },
    Try { fiber: u64 },
    Timeout { ms: u64, fiber: u64 },
    Io { what: &'static str },
}

impl Blocked {
    /// `sleep 30 ms`, `channel #1 recv`, `both (threads 2, 3)`, ...
    pub(crate) fn describe(&self, number_of: impl Fn(u64) -> Option<u64>) -> String {
        let threads = |fibers: &[u64]| {
            let numbers: Vec<String> = fibers
                .iter()
                .filter_map(|fiber| number_of(*fiber))
                .map(|number| number.to_string())
                .collect();
            match numbers.len() {
                0 => String::new(),
                1 => format!(" (thread {})", numbers[0]),
                _ => format!(" (threads {})", numbers.join(", ")),
            }
        };
        match self {
            Blocked::Sleep { ms } => format!("sleep {ms} ms"),
            Blocked::Yield => "yield".to_string(),
            Blocked::Task { id } => format!("task #{id}"),
            Blocked::ChannelRecv { channel } => format!("channel #{channel} recv"),
            Blocked::ChannelSend { channel } => format!("channel #{channel} send"),
            Blocked::Event { id } => format!("event #{id}"),
            Blocked::Both { fibers } => format!("both{}", threads(fibers)),
            Blocked::Race { fibers } => format!("race{}", threads(fibers)),
            Blocked::FirstOf { fibers } => format!("first_of{}", threads(fibers)),
            Blocked::Try { fiber } => format!("try{}", threads(&[*fiber])),
            Blocked::Timeout { ms, fiber } => format!("timeout {ms} ms{}", threads(&[*fiber])),
            Blocked::Io { what } => what.to_string(),
        }
    }
}

/// Whether a debugger is attached on this thread.
pub(crate) fn on() -> bool {
    ON.with(|on| on.get())
}

/// A debugger is attached: keep records from here on.
pub(crate) fn enable() {
    ON.with(|on| on.set(true));
}

/// The outermost `run_async` starts: number its fibers from 1.
pub(crate) fn start_run() {
    if !on() {
        return;
    }
    RECORDS.with(|records| {
        let mut records = records.borrow_mut();
        records.fibers.clear();
        records.next_number = 1;
    });
}

/// Fiber `fiber` will run `body`.
pub(crate) fn note_spawn(fiber: u64, body: &Value) {
    if !on() {
        return;
    }
    let name = body_name(body);
    RECORDS.with(|records| {
        let mut records = records.borrow_mut();
        if records.fibers.contains_key(&fiber) {
            return;
        }
        let number = records.next_number.max(1);
        records.next_number = number + 1;
        records.fibers.insert(
            fiber,
            FiberRecord {
                number,
                name,
                blocked: None,
            },
        );
    });
}

/// Fiber `fiber` parks, waiting for `blocked`.
pub(crate) fn note_block(fiber: u64, blocked: Blocked) {
    RECORDS.with(|records| {
        if let Some(record) = records.borrow_mut().fibers.get_mut(&fiber) {
            record.blocked = Some(blocked);
        }
    });
}

/// Task `id` runs `action` on a thread of its own; `finished` says when it
/// has.
pub(crate) fn note_task(id: i64, action: &Value, finished: Box<dyn Fn() -> bool>) {
    if !on() {
        return;
    }
    let name = body_name(action);
    RECORDS.with(|records| {
        records
            .borrow_mut()
            .tasks
            .insert(id, TaskRecord { name, finished });
    });
}

/// Fiber `fiber`'s thread number, name and what it last parked for.
pub(crate) fn fiber(fiber: u64) -> Option<(u64, String, Option<Blocked>)> {
    RECORDS.with(|records| {
        records
            .borrow()
            .fibers
            .get(&fiber)
            .map(|record| (record.number, record.name.clone(), record.blocked.clone()))
    })
}

/// The thread number of fiber `fiber`.
pub(crate) fn number(fiber: u64) -> Option<u64> {
    RECORDS.with(|records| {
        records
            .borrow()
            .fibers
            .get(&fiber)
            .map(|record| record.number)
    })
}

/// The tasks the program has started, by id, with their names and
/// whether each has finished.
pub(crate) fn tasks() -> Vec<(i64, String, bool)> {
    RECORDS.with(|records| {
        records
            .borrow()
            .tasks
            .iter()
            .map(|(id, task)| (*id, task.name.clone(), (task.finished)()))
            .collect()
    })
}

/// The function a fiber or task runs, as its thread is named.
fn body_name(body: &Value) -> String {
    let function = match body {
        Value::Closure(closure) => Some(&closure.function),
        Value::Function(function) => Some(function),
        _ => None,
    };
    function
        .and_then(|function| function.debug_info.as_ref())
        .and_then(|info| info.name.as_deref())
        .map(super::debug::display_name)
        .unwrap_or_else(|| "<fiber>".to_string())
}
