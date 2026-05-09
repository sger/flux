use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

use crate::bytecode::bytecode::Bytecode;
use crate::bytecode::debug_info::FunctionDebugInfo;
use crate::bytecode::op_code::Instructions;
use crate::runtime::r#async::task_scheduler::{TaskHandle, TaskJoinError, TaskScheduler};
use crate::runtime::{
    closure::Closure,
    compiled_function::CompiledFunction,
    cons_cell::ConsCell,
    hamt,
    hash_key::HashKey,
    value::{AdtFields, AdtValue, Value},
};

use super::{VM, slot};

type TaskResult = Result<VmSendValue, String>;

#[derive(Clone)]
enum VmSendValue {
    Uninit,
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(String),
    None,
    EmptyList,
    Some(Box<VmSendValue>),
    Left(Box<VmSendValue>),
    Right(Box<VmSendValue>),
    Function(Box<VmSendFunction>),
    Closure(Box<VmSendClosure>),
    Array(Vec<VmSendValue>),
    Tuple(Vec<VmSendValue>),
    Adt {
        constructor: String,
        fields: Vec<VmSendValue>,
    },
    AdtUnit(String),
    Cons(Box<VmSendValue>, Box<VmSendValue>),
    HashMap(Vec<(HashKey, VmSendValue)>),
    Unsupported(String),
}

#[derive(Clone)]
struct VmSendFunction {
    instructions: Instructions,
    num_locals: usize,
    num_parameters: usize,
    max_stack: usize,
    debug_info: Option<FunctionDebugInfo>,
    contract: Option<crate::runtime::function_contract::FunctionContract>,
}

#[derive(Clone)]
struct VmSendClosure {
    function: VmSendFunction,
    free: Vec<VmSendValue>,
}

struct VmTaskSnapshot {
    action: VmSendValue,
    constants: Vec<VmSendValue>,
    globals: Vec<(usize, VmSendValue)>,
}

struct VmTaskEntry {
    handle: Option<TaskHandle<TaskResult>>,
}

struct CompletionQueue {
    tx: mpsc::Sender<(u64, TaskResult)>,
    rx: Mutex<mpsc::Receiver<(u64, TaskResult)>>,
}

static NEXT_TASK_ID: AtomicI64 = AtomicI64::new(1);
static TASKS: OnceLock<Mutex<HashMap<i64, VmTaskEntry>>> = OnceLock::new();
static CANCELLED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
static SCHEDULER: OnceLock<TaskScheduler> = OnceLock::new();
static COMPLETIONS: OnceLock<CompletionQueue> = OnceLock::new();

fn tasks() -> &'static Mutex<HashMap<i64, VmTaskEntry>> {
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cancelled() -> &'static Mutex<HashSet<i64>> {
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn scheduler() -> &'static TaskScheduler {
    SCHEDULER.get_or_init(|| {
        TaskScheduler::new(default_worker_count()).expect("failed to start VM task scheduler")
    })
}

fn completion_queue() -> &'static CompletionQueue {
    COMPLETIONS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        CompletionQueue {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

fn default_worker_count() -> usize {
    std::env::var("FLUX_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(2)
}

impl VM {
    pub(super) fn spawn_vm_task(&mut self, action: Value) -> Result<i64, String> {
        let snapshot = self.task_snapshot(action)?;
        let id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
        let handle = scheduler().spawn(move || run_task_snapshot(snapshot));
        tasks().lock().expect("VM task table poisoned").insert(
            id,
            VmTaskEntry {
                handle: Some(handle),
            },
        );
        Ok(id)
    }

    fn task_snapshot(&self, action: Value) -> Result<VmTaskSnapshot, String> {
        Ok(VmTaskSnapshot {
            action: VmSendValue::try_from_value(&action)?,
            constants: self
                .constants
                .iter()
                .map(|slot| VmSendValue::from_constant(&slot::from_slot_ref(slot)))
                .collect(),
            globals: self
                .globals
                .iter()
                .enumerate()
                .filter_map(|(idx, slot)| {
                    let value = slot::from_slot_ref(slot);
                    if matches!(value, Value::None | Value::Uninit) {
                        None
                    } else {
                        Some(VmSendValue::try_from_value(&value).map(|v| (idx, v)))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

pub(super) fn blocking_join(id: i64) -> Result<Value, String> {
    if cancelled()
        .lock()
        .expect("VM task cancel set poisoned")
        .remove(&id)
    {
        let _ = take_handle(id);
        return Err("TaskCancelled".to_string());
    }
    match take_handle(id)? {
        JoinTarget::Join(handle) => join_handle(handle),
        JoinTarget::Consumed => Err(format!("task {id} not found (already joined or awaited)")),
    }
}

pub(super) fn cancel(id: i64) -> Result<(), String> {
    cancelled()
        .lock()
        .expect("VM task cancel set poisoned")
        .insert(id);
    let map = tasks().lock().expect("VM task table poisoned");
    if let Some(entry) = map.get(&id)
        && let Some(handle) = entry.handle.as_ref()
    {
        handle.cancel();
    }
    Ok(())
}

pub(super) fn start_await(id: i64, request_id: u64) -> Result<(), String> {
    if cancelled()
        .lock()
        .expect("VM task cancel set poisoned")
        .remove(&id)
    {
        let _ = take_handle(id);
        return Err("TaskCancelled".to_string());
    }
    match take_handle(id)? {
        JoinTarget::Join(handle) => {
            let tx = completion_queue().tx.clone();
            std::thread::Builder::new()
                .name(format!("flux-vm-task-await-{id}"))
                .spawn(move || {
                    let result = match scheduler().blocking_join(handle) {
                        Ok(result) => result,
                        Err(err) => Err(join_error(err)),
                    };
                    let _ = tx.send((request_id, result));
                })
                .map_err(|e| format!("failed to spawn VM task await waiter: {e}"))?;
            Ok(())
        }
        JoinTarget::Consumed => Err(format!("task {id} not found (already joined or awaited)")),
    }
}

pub(super) fn try_recv_completion() -> Option<(u64, Result<Value, String>)> {
    let queue = completion_queue();
    let rx = queue.rx.lock().expect("VM task completion queue poisoned");
    match rx.try_recv() {
        Ok((request_id, result)) => Some((request_id, result.and_then(|v| v.to_value()))),
        Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
    }
}

enum JoinTarget {
    Join(TaskHandle<TaskResult>),
    Consumed,
}

fn take_handle(id: i64) -> Result<JoinTarget, String> {
    let mut map = tasks().lock().expect("VM task table poisoned");
    let Some(mut entry) = map.remove(&id) else {
        return Err(format!(
            "task {id} not found (already joined or never spawned)"
        ));
    };
    Ok(match entry.handle.take() {
        Some(handle) => JoinTarget::Join(handle),
        None => JoinTarget::Consumed,
    })
}

fn join_handle(handle: TaskHandle<TaskResult>) -> Result<Value, String> {
    let result = scheduler().blocking_join(handle).map_err(join_error)??;
    result.to_value()
}

fn join_error(err: TaskJoinError) -> String {
    match err {
        TaskJoinError::Cancelled => "TaskCancelled".to_string(),
        TaskJoinError::Panicked(s) => format!("task panicked: {s}"),
    }
}

fn run_task_snapshot(snapshot: VmTaskSnapshot) -> TaskResult {
    let constants = snapshot
        .constants
        .into_iter()
        .map(VmSendValue::to_constant_value)
        .collect::<Result<Vec<_>, _>>()?;
    let action = snapshot.action.to_value()?;
    let mut vm = VM::new(Bytecode {
        instructions: vec![],
        constants,
        debug_info: None,
    });
    for (idx, value) in snapshot.globals {
        if idx < vm.globals.len() {
            vm.globals[idx] = slot::to_slot(value.to_value()?);
        }
    }
    let result = vm.invoke_value(action, vec![])?;
    VmSendValue::try_from_value(&result)
}

impl VmSendValue {
    fn from_constant(value: &Value) -> Self {
        Self::try_from_value(value).unwrap_or_else(|err| Self::Unsupported(err))
    }

    fn try_from_value(value: &Value) -> Result<Self, String> {
        Ok(match value {
            Value::Uninit => Self::Uninit,
            Value::Integer(v) => Self::Integer(*v),
            Value::Float(v) => Self::Float(*v),
            Value::Boolean(v) => Self::Boolean(*v),
            Value::String(v) => Self::String((**v).clone()),
            Value::None => Self::None,
            Value::EmptyList => Self::EmptyList,
            Value::Some(v) => Self::Some(Box::new(Self::try_from_value(v)?)),
            Value::Left(v) => Self::Left(Box::new(Self::try_from_value(v)?)),
            Value::Right(v) => Self::Right(Box::new(Self::try_from_value(v)?)),
            Value::ReturnValue(_) => {
                return Err("Task.spawn cannot transfer internal return values".to_string());
            }
            Value::Function(f) => Self::Function(Box::new(VmSendFunction::from_function(f))),
            Value::Closure(c) => Self::Closure(Box::new(VmSendClosure::try_from_closure(c)?)),
            Value::Array(v) => Self::Array(
                v.iter()
                    .map(Self::try_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Value::Tuple(v) => Self::Tuple(
                v.iter()
                    .map(Self::try_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Value::Adt(adt) => Self::Adt {
                constructor: (*adt.constructor).clone(),
                fields: adt
                    .fields
                    .iter()
                    .map(Self::try_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Value::AdtUnit(name) => Self::AdtUnit((**name).clone()),
            Value::Continuation(_) => {
                return Err("Task.spawn cannot transfer VM continuations".to_string());
            }
            Value::HandlerDescriptor(_) | Value::PerformDescriptor(_) => {
                return Err("Task.spawn cannot transfer VM effect descriptors".to_string());
            }
            Value::Cons(cell) => Self::Cons(
                Box::new(Self::try_from_value(&cell.head)?),
                Box::new(Self::try_from_value(&cell.tail)?),
            ),
            Value::HashMap(root) => Self::HashMap(
                hamt::hamt_iter(root)
                    .into_iter()
                    .map(|(k, v)| Self::try_from_value(&v).map(|sv| (k, sv)))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }

    fn to_value(self) -> Result<Value, String> {
        Ok(match self {
            Self::Uninit => Value::Uninit,
            Self::Integer(v) => Value::Integer(v),
            Self::Float(v) => Value::Float(v),
            Self::Boolean(v) => Value::Boolean(v),
            Self::String(v) => Value::String(Rc::new(v)),
            Self::None => Value::None,
            Self::EmptyList => Value::EmptyList,
            Self::Some(v) => Value::Some(Rc::new(v.to_value()?)),
            Self::Left(v) => Value::Left(Rc::new(v.to_value()?)),
            Self::Right(v) => Value::Right(Rc::new(v.to_value()?)),
            Self::Function(f) => Value::Function(Rc::new(f.to_function())),
            Self::Closure(c) => Value::Closure(Rc::new(c.to_closure()?)),
            Self::Array(v) => Value::Array(Rc::new(
                v.into_iter()
                    .map(Self::to_value)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Self::Tuple(v) => Value::Tuple(Rc::new(
                v.into_iter()
                    .map(Self::to_value)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Self::Adt {
                constructor,
                fields,
            } => Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new(constructor),
                fields: AdtFields::from_vec(
                    fields
                        .into_iter()
                        .map(Self::to_value)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            })),
            Self::AdtUnit(name) => Value::AdtUnit(Rc::new(name)),
            Self::Cons(head, tail) => {
                Value::Cons(Rc::new(ConsCell::new(head.to_value()?, tail.to_value()?)))
            }
            Self::HashMap(entries) => {
                let mut root = hamt::hamt_empty();
                for (key, value) in entries {
                    root = hamt::hamt_insert(&root, key, value.to_value()?);
                }
                Value::HashMap(root)
            }
            Self::Unsupported(err) => {
                return Err(format!("VM task referenced unsupported constant: {err}"));
            }
        })
    }

    fn to_constant_value(self) -> Result<Value, String> {
        match self {
            Self::Unsupported(_) => Ok(Value::None),
            other => other.to_value(),
        }
    }
}

impl VmSendFunction {
    fn from_function(function: &CompiledFunction) -> Self {
        Self {
            instructions: function.instructions.clone(),
            num_locals: function.num_locals,
            num_parameters: function.num_parameters,
            max_stack: function.max_stack,
            debug_info: function.debug_info.clone(),
            contract: function.contract.clone(),
        }
    }

    fn to_function(self) -> CompiledFunction {
        CompiledFunction {
            instructions: self.instructions,
            num_locals: self.num_locals,
            num_parameters: self.num_parameters,
            max_stack: self.max_stack,
            debug_info: self.debug_info,
            contract: self.contract,
        }
    }
}

impl VmSendClosure {
    fn try_from_closure(closure: &Closure) -> Result<Self, String> {
        Ok(Self {
            function: VmSendFunction::from_function(&closure.function),
            free: closure
                .free
                .iter()
                .map(VmSendValue::try_from_value)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn to_closure(self) -> Result<Closure, String> {
        Ok(Closure::new(
            Rc::new(self.function.to_function()),
            self.free
                .into_iter()
                .map(VmSendValue::to_value)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    #[test]
    fn scheduler_runs_vm_task_jobs_in_parallel() {
        let n = 2;
        let barrier = Arc::new(Barrier::new(n));
        let sched = scheduler();
        let mut handles = Vec::new();
        for i in 0..n {
            let barrier = Arc::clone(&barrier);
            handles.push(sched.spawn(move || {
                barrier.wait();
                std::thread::sleep(Duration::from_millis(25));
                Ok::<VmSendValue, String>(VmSendValue::Integer(i as i64))
            }));
        }

        let mut values = handles
            .into_iter()
            .map(|h| sched.blocking_join(h).expect("join").expect("task result"))
            .map(|v| match v {
                VmSendValue::Integer(i) => i,
                _ => panic!("unexpected task result"),
            })
            .collect::<Vec<_>>();
        values.sort();
        assert_eq!(values, vec![0, 1]);
    }
}
