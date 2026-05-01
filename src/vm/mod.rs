use std::{collections::HashMap, rc::Rc, sync::mpsc};

use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode},
    runtime::{
        r#async::{
            backend::RequestId,
            context::TaskId,
            runtime::{AsyncRuntime, AsyncRuntimeError},
            scheduler::{RequestIdAllocator, SchedulerConfig},
            send_value::{SendValue, SendValueError},
            task::{TaskCancelToken, TaskHandle, TaskManager, TaskManagerConfig},
        },
        closure::Closure,
        compiled_function::CompiledFunction,
        evidence::EvidenceVector,
        frame::Frame,
        hamt,
        handler_frame::HandlerFrame,
        leak_detector,
        value::{AdtFields, AdtValue, Value},
        yield_state::YieldState,
    },
};

#[cfg(feature = "async-mio")]
use crate::runtime::r#async::{
    backend::AsyncError,
    backends::mio::{
        MioBackend, MioBackendHandle, MioDriverBackend, MioReactorRunLimit, MioReactorRunReport,
        spawn_mio_reactor_until_stopped,
    },
};
#[cfg(feature = "async-mio")]
type VmAsyncBackend = MioDriverBackend;
#[cfg(not(feature = "async-mio"))]
use crate::runtime::r#async::backends::ThreadTimerBackend as VmAsyncBackend;

#[cfg(feature = "async-mio")]
pub struct VmMioReactor {
    handle: MioBackendHandle,
    thread: Option<std::thread::JoinHandle<Result<MioReactorRunReport, AsyncError>>>,
}

mod binary_ops;
mod comparison_ops;
mod core_dispatch;
mod dispatch;
mod function_call;
mod index_ops;
mod primop;
pub mod profiling;
pub mod test_runner;
mod trace;

const INITIAL_STACK_SIZE: usize = 2048;
const MAX_STACK_SIZE: usize = 1 << 20; // 1,048,576 slots
const GLOBALS_SIZE: usize = 65536;
const STACK_PREGROW_HEADROOM: usize = 256;
const STACK_GROW_MIN_CHUNK: usize = 4096;

// ── Slot-type abstraction ─────────────────────────────────────────────────────
//
// `Slot` is the element type used in the VM's stack, globals, and constants.
//
// When `nan-boxing` is enabled every slot is a `NanBox` (8 bytes).
// When `nan-boxing` is disabled every slot is a `Value` (no overhead).
//
// All conversions between `Value` and `Slot` go through `slot::to_slot` /
// `slot::from_slot` / `slot::from_slot_ref`.  Callers that only need to
// read-then-own a slot should use `from_slot`; callers that need a clone
// without consuming the slot should use `from_slot_ref`.

mod slot {
    use crate::runtime::nanbox::NanBox;
    use crate::runtime::value::Value;

    pub type Slot = NanBox;

    #[inline(always)]
    pub fn uninit() -> Slot {
        NanBox::from_uninit()
    }

    #[inline(always)]
    pub fn to_slot(v: Value) -> Slot {
        NanBox::from_value(v)
    }

    #[inline(always)]
    pub fn from_slot(s: Slot) -> Value {
        s.to_value()
    }

    #[inline(always)]
    pub fn from_slot_ref(s: &Slot) -> Value {
        s.clone().to_value()
    }
}

use slot::Slot;

pub struct VM {
    constants: Vec<Slot>,
    stack: Vec<Slot>,
    sp: usize,
    last_popped: Slot,
    pub globals: Vec<Slot>,
    frames: Vec<Frame>,
    frame_index: usize,
    trace: bool,
    tail_arg_scratch: Vec<Slot>,
    /// Active effect handlers pushed by OpHandle / popped by OpEndHandle.
    pub(crate) handler_stack: Vec<HandlerFrame>,
    /// Shared evidence vector for the Phase 3 VM effect runtime path.
    pub(crate) evv: EvidenceVector,
    /// In-flight yield state for the Phase 3 VM effect runtime path.
    pub(crate) yield_state: YieldState,
    /// Profiling state — only active when `--prof` is passed.
    pub(crate) profiling: bool,
    pub(crate) cost_centres: Vec<profiling::CostCentre>,
    pub(crate) cc_stack: Vec<profiling::CostCentreStackEntry>,
    task_registry: VmTaskRegistry,
    task_manager: TaskManager,
    async_runtime: AsyncRuntime<VmAsyncBackend>,
    #[cfg(feature = "async-mio")]
    async_mio_reactor: Option<VmMioReactor>,
    async_task_id: TaskId,
    task_await_tx: mpsc::Sender<VmTaskAwaitCompletion>,
    task_await_rx: mpsc::Receiver<VmTaskAwaitCompletion>,
    async_next_scope_id: i64,
    async_scopes: HashMap<i64, Vec<TaskHandle<Result<SendValue, String>>>>,
    async_cancel_token: Option<TaskCancelToken>,
    /// Per-VM channel registry (proposal 0174 Phase 1b). Channels are
    /// VM-local for now; cross-Task delivery promotes to a process-wide
    /// registry once the fiber loop lands.
    channel_registry: VmChannelRegistry,
}

#[derive(Debug, Default)]
pub(crate) struct VmChannelRegistry {
    pub(crate) next_id: i64,
    pub(crate) channels: HashMap<i64, VmChannelRecord>,
}

#[derive(Debug)]
pub(crate) struct VmChannelRecord {
    pub(crate) sender: Option<std::sync::mpsc::SyncSender<SendValue>>,
    pub(crate) receiver: std::sync::mpsc::Receiver<SendValue>,
    pub(crate) closed: bool,
}

#[derive(Debug, Default)]
struct VmTaskRegistry {
    next_id: i64,
    tasks: HashMap<i64, VmTaskRecord>,
}

#[derive(Debug)]
struct VmTaskRecord {
    handle: Option<TaskHandle<Result<SendValue, String>>>,
    canceled: bool,
}

#[derive(Debug)]
struct VmTaskAwaitCompletion {
    request_id: RequestId,
    result: Result<SendValue, String>,
}

impl VM {
    pub fn new(bytecode: Bytecode) -> Self {
        #[cfg(feature = "async-mio")]
        let (async_backend, async_mio_reactor) = Self::new_async_backend();
        #[cfg(not(feature = "async-mio"))]
        let async_backend = Self::new_async_backend();
        #[cfg(feature = "async-mio")]
        return Self::new_with_backend(bytecode, async_backend, Some(async_mio_reactor));
        #[cfg(not(feature = "async-mio"))]
        return Self::new_with_backend(bytecode, async_backend);
    }

    #[cfg(feature = "async-mio")]
    pub fn new_with_backend(
        bytecode: Bytecode,
        async_backend: VmAsyncBackend,
        async_mio_reactor: Option<VmMioReactor>,
    ) -> Self {
        Self::new_with_backend_and_ids(bytecode, async_backend, async_mio_reactor, None)
    }

    /// Variant of `new_with_backend` that lets the caller seed the
    /// `RequestId` allocator. When `request_ids` is `Some`, the new VM
    /// shares that allocator with another VM (typically the parent that
    /// spawned it via `Async.both`/etc.), so completion-routing keys are
    /// unique across the process.
    #[cfg(feature = "async-mio")]
    pub fn new_with_backend_and_ids(
        bytecode: Bytecode,
        async_backend: VmAsyncBackend,
        async_mio_reactor: Option<VmMioReactor>,
        request_ids: Option<RequestIdAllocator>,
    ) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);
        let mut async_runtime = match request_ids {
            Some(ids) => AsyncRuntime::with_request_ids(
                SchedulerConfig { worker_count: 1 },
                async_backend,
                ids,
            ),
            None => AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, async_backend),
        };
        let (async_task_id, _) = async_runtime
            .spawn_task()
            .expect("VM async runtime task allocation cannot fail");
        let (task_await_tx, task_await_rx) = mpsc::channel();

        Self {
            constants: bytecode.constants.into_iter().map(slot::to_slot).collect(),
            stack: vec![slot::uninit(); INITIAL_STACK_SIZE],
            sp: 0,
            last_popped: slot::to_slot(Value::None),
            globals: vec![slot::to_slot(Value::None); GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
            tail_arg_scratch: Vec::new(),
            handler_stack: Vec::new(),
            evv: EvidenceVector::new(),
            yield_state: YieldState::new(),
            profiling: false,
            cost_centres: Vec::new(),
            cc_stack: Vec::new(),
            task_registry: VmTaskRegistry::default(),
            task_manager: TaskManager::new(TaskManagerConfig::default()),
            async_runtime,
            #[cfg(feature = "async-mio")]
            async_mio_reactor,
            async_task_id,
            task_await_tx,
            task_await_rx,
            async_next_scope_id: 1,
            async_scopes: HashMap::new(),
            async_cancel_token: None,
            channel_registry: VmChannelRegistry::default(),
        }
    }

    #[cfg(not(feature = "async-mio"))]
    pub fn new_with_backend(bytecode: Bytecode, async_backend: VmAsyncBackend) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);
        let mut async_runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, async_backend);
        let (async_task_id, _) = async_runtime
            .spawn_task()
            .expect("VM async runtime task allocation cannot fail");
        let (task_await_tx, task_await_rx) = mpsc::channel();

        Self {
            constants: bytecode.constants.into_iter().map(slot::to_slot).collect(),
            stack: vec![slot::uninit(); INITIAL_STACK_SIZE],
            sp: 0,
            last_popped: slot::to_slot(Value::None),
            globals: vec![slot::to_slot(Value::None); GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
            tail_arg_scratch: Vec::new(),
            handler_stack: Vec::new(),
            evv: EvidenceVector::new(),
            yield_state: YieldState::new(),
            profiling: false,
            cost_centres: Vec::new(),
            cc_stack: Vec::new(),
            task_registry: VmTaskRegistry::default(),
            task_manager: TaskManager::new(TaskManagerConfig::default()),
            async_runtime,
            async_task_id,
            task_await_tx,
            task_await_rx,
            async_next_scope_id: 1,
            async_scopes: HashMap::new(),
            async_cancel_token: None,
            channel_registry: VmChannelRegistry::default(),
        }
    }

    fn make_task_value(task_id: i64) -> Value {
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Task".to_string()),
            fields: AdtFields::One(Value::Integer(task_id)),
        }))
    }

    #[cfg(feature = "async-mio")]
    fn new_async_backend() -> (VmAsyncBackend, VmMioReactor) {
        let backend = MioBackend::new().expect("VM mio async backend initialization cannot fail");
        let driver_backend = backend.driver_backend();
        let handle = backend.handle();
        let thread = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: usize::MAX,
                timeout: Some(std::time::Duration::from_millis(10)),
            },
        );
        (
            driver_backend,
            VmMioReactor {
                handle,
                thread: Some(thread),
            },
        )
    }

    #[cfg(not(feature = "async-mio"))]
    fn new_async_backend() -> VmAsyncBackend {
        VmAsyncBackend::new()
    }

    pub(crate) fn make_channel_value(channel_id: i64) -> Value {
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Channel".to_string()),
            fields: AdtFields::One(Value::Integer(channel_id)),
        }))
    }

    pub(crate) fn channel_id_from_value(channel: &Value) -> Result<i64, String> {
        match channel {
            Value::Adt(adt) if adt.constructor.as_ref() == "Channel" => match adt.fields.get(0) {
                Some(Value::Integer(id)) => Ok(*id),
                Some(other) => Err(format!(
                    "Channel handle id must be Int, got {}",
                    other.type_name()
                )),
                None => Err("Channel handle is missing id field".to_string()),
            },
            other => Err(format!(
                "expected Channel handle, got {}",
                other.type_name()
            )),
        }
    }

    fn task_id_from_value(task: &Value) -> Result<i64, String> {
        match task {
            Value::Adt(adt) if adt.constructor.as_ref() == "Task" => match adt.fields.get(0) {
                Some(Value::Integer(id)) => Ok(*id),
                Some(other) => Err(format!(
                    "Task handle id must be Int, got {}",
                    other.type_name()
                )),
                None => Err("Task handle is missing id field".to_string()),
            },
            other => Err(format!("expected Task handle, got {}", other.type_name())),
        }
    }

    fn make_tcp_value(handle: u64) -> Value {
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Tcp".to_string()),
            fields: AdtFields::One(Value::Integer(handle as i64)),
        }))
    }

    fn make_tcp_listener_value(handle: u64) -> Value {
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("TcpListener".to_string()),
            fields: AdtFields::One(Value::Integer(handle as i64)),
        }))
    }

    fn tcp_handle_from_value(conn: &Value) -> Result<u64, String> {
        match conn {
            Value::Adt(adt) if adt.constructor.as_ref() == "Tcp" => match adt.fields.get(0) {
                Some(Value::Integer(id)) if *id >= 0 => Ok(*id as u64),
                Some(Value::Integer(id)) => {
                    Err(format!("Tcp handle id must be non-negative, got {id}"))
                }
                Some(other) => Err(format!(
                    "Tcp handle id must be Int, got {}",
                    other.type_name()
                )),
                None => Err("Tcp handle is missing id field".to_string()),
            },
            other => Err(format!("expected Tcp handle, got {}", other.type_name())),
        }
    }

    fn tcp_listener_handle_from_value(listener: &Value) -> Result<u64, String> {
        match listener {
            Value::Adt(adt) if adt.constructor.as_ref() == "TcpListener" => {
                match adt.fields.get(0) {
                    Some(Value::Integer(id)) if *id >= 0 => Ok(*id as u64),
                    Some(Value::Integer(id)) => Err(format!(
                        "TcpListener handle id must be non-negative, got {id}"
                    )),
                    Some(other) => Err(format!(
                        "TcpListener handle id must be Int, got {}",
                        other.type_name()
                    )),
                    None => Err("TcpListener handle is missing id field".to_string()),
                }
            }
            other => Err(format!(
                "expected TcpListener handle, got {}",
                other.type_name()
            )),
        }
    }

    pub(crate) fn constant_values(&self) -> Vec<Value> {
        self.constants.iter().map(slot::from_slot_ref).collect()
    }

    pub(crate) fn global_values(&self) -> Vec<Value> {
        self.globals.iter().map(slot::from_slot_ref).collect()
    }

    pub(crate) fn send_value_error(error: SendValueError) -> String {
        match error {
            SendValueError::UnsupportedType(ty) => {
                format!("value of type {ty} cannot cross a Task worker boundary")
            }
            SendValueError::UnsupportedMapKey => {
                "map key cannot cross a Task worker boundary".to_string()
            }
            SendValueError::NotAClosure => "Task.spawn expects a closure".to_string(),
        }
    }

    pub(crate) fn async_runtime_error(error: AsyncRuntimeError) -> String {
        format!("async runtime error: {error:?}")
    }

    pub fn set_trace(&mut self, enabled: bool) {
        self.trace = enabled;
    }

    pub fn set_profiling(&mut self, enabled: bool, infos: Vec<profiling::CostCentreInfo>) {
        self.profiling = enabled;
        self.cost_centres = infos
            .into_iter()
            .map(|info| profiling::CostCentre {
                name: info.name,
                module: info.module,
                ..Default::default()
            })
            .collect();
    }

    #[inline(always)]
    fn enter_cost_centre(&mut self, idx: u16) {
        let i = idx as usize;
        if i < self.cost_centres.len() {
            self.cost_centres[i].entries += 1;
            self.cc_stack.push(profiling::CostCentreStackEntry {
                cc_index: idx,
                enter_time: std::time::Instant::now(),
                child_time_ns: 0,
            });
        }
    }

    #[inline(always)]
    fn exit_cost_centre(&mut self) {
        if let Some(entry) = self.cc_stack.pop() {
            let elapsed = entry.enter_time.elapsed().as_nanos() as u64;
            let self_time = elapsed.saturating_sub(entry.child_time_ns);
            let i = entry.cc_index as usize;
            if i < self.cost_centres.len() {
                self.cost_centres[i].time_ns += elapsed;
                self.cost_centres[i].inner_time_ns += entry.child_time_ns;
            }
            // Attribute this function's total time as child time of the parent.
            if let Some(parent) = self.cc_stack.last_mut() {
                parent.child_time_ns += elapsed;
            }
            let _ = self_time; // used for individual time in the report
        }
    }

    pub fn print_profile_report(&self, execute_ns: u64) {
        profiling::print_profile_report(&self.cost_centres, execute_ns);
    }

    /// A closure that acts as the identity function: `fn(x) -> x`.
    /// Used as the `resume` parameter for tail-resumptive `OpPerform` /
    /// `OpPerformDirect`, so that `resume(v)` simply returns `v`.
    ///
    /// Proposal 0162 Phase 1: the identity closure is a thread-local
    /// singleton — every TR perform in a hot loop previously allocated a
    /// fresh `Rc<CompiledFunction>` + `Rc<Closure>` per invocation. The
    /// shared `Rc` is cloned (bumping only the refcount) instead of
    /// rebuilding the bytecode. Safe because the identity closure has no
    /// upvalues and no mutable state. Measured ~15% speedup on a 500k
    /// perform microbench.
    pub(crate) fn make_identity_closure(&self) -> Value {
        thread_local! {
            static IDENTITY: Rc<Closure> = {
                let instructions = vec![OpCode::OpReturnLocal as u8, 0];
                let func = Rc::new(CompiledFunction::new(instructions, 1, 1, None));
                Rc::new(Closure::new(func, vec![]))
            };
        }
        IDENTITY.with(|c| Value::Closure(Rc::clone(c)))
    }

    pub fn run(&mut self) -> Result<(), String> {
        match self.run_inner() {
            Ok(()) => Ok(()),
            Err(err) => {
                let normalized = trace::strip_ansi(&err);
                // Check if error is already formatted (from runtime_error_enhanced / aggregator)
                // Formatted errors may start with a rendered severity header and include an error code.
                let has_code = normalized.contains("[E") || normalized.contains("[e");
                let looks_formatted = has_code
                    && (normalized.starts_with("Error[")
                        || normalized.starts_with("error[")
                        || normalized.starts_with("Warning[")
                        || normalized.starts_with("Note[")
                        || normalized.starts_with("Help[")
                        || normalized.contains("\nError[")
                        || normalized.contains("\nerror[")
                        || normalized.contains("\nWarning[")
                        || normalized.contains("\nNote[")
                        || normalized.contains("\nHelp["));
                if looks_formatted {
                    Err(err)
                } else {
                    // Format unmigrated errors through Diagnostic system
                    Err(self.runtime_error_from_string(&err))
                }
            }
        }
    }

    fn run_inner(&mut self) -> Result<(), String> {
        let mut closure = self.frames[self.frame_index].closure.clone();
        let mut instructions: &[u8] = &closure.function.instructions;

        loop {
            let ip = self.frames[self.frame_index].ip;
            if ip >= instructions.len() {
                if self.frame_index > 0 {
                    let fn_name = "<function>";
                    return Err(format!(
                        "VM bug: IP {} overran instruction boundary ({} bytes) in function '{}' at frame depth {}",
                        ip,
                        instructions.len(),
                        fn_name,
                        self.frame_index,
                    ));
                }
                break;
            }

            let op = OpCode::from(instructions[ip]);
            if self.trace {
                self.trace_instruction(ip, op);
            }

            let frame_before = self.frame_index;
            // Track closure identity so continuation resume which leaves
            // frame_index unchanged numerically but swaps in a different frame
            // triggers an instruction-pointer refresh.
            let closure_ptr_before = Rc::as_ptr(&closure);
            let ip_delta = self.dispatch_instruction(instructions, ip, op)?;
            self.apply_ip_delta(frame_before, ip_delta, None);

            let closure_changed =
                Rc::as_ptr(&self.frames[self.frame_index].closure) != closure_ptr_before;
            if self.frame_index != frame_before
                || matches!(op, OpCode::OpTailCall)
                || closure_changed
            {
                closure = self.frames[self.frame_index].closure.clone();
                instructions = &closure.function.instructions;
            }
        }
        Ok(())
    }

    fn execute_current_instruction(
        &mut self,
        invoke_target_frame: Option<usize>,
    ) -> Result<(), String> {
        let frame_index = self.frame_index;
        let ip = self.frames[frame_index].ip;
        let closure = self.frames[frame_index].closure.clone();
        let instructions: &[u8] = &closure.function.instructions;
        let op = OpCode::from(instructions[ip]);
        if self.trace {
            self.trace_instruction(ip, op);
        }

        let frame_before = self.frame_index;
        let ip_delta = self.dispatch_instruction(instructions, ip, op)?;
        self.apply_ip_delta(frame_before, ip_delta, invoke_target_frame);
        Ok(())
    }

    #[inline(always)]
    fn apply_ip_delta(
        &mut self,
        frame_before: usize,
        ip_delta: usize,
        invoke_target_frame: Option<usize>,
    ) {
        if ip_delta == 0 {
            return;
        }

        match invoke_target_frame {
            None => {
                if self.frame_index > frame_before {
                    // New frame was pushed; advance caller frame IP.
                    self.frames[frame_before].ip += ip_delta;
                } else {
                    self.frames[self.frame_index].ip += ip_delta;
                }
            }
            Some(target) => {
                if self.frame_index > frame_before {
                    // New frame was pushed; advance caller frame IP.
                    self.frames[frame_before].ip += ip_delta;
                } else if self.frame_index == frame_before {
                    self.frames[self.frame_index].ip += ip_delta;
                } else if self.frame_index >= target {
                    // Deeper frame returned; advance resumed frame IP.
                    self.frames[self.frame_index].ip += ip_delta;
                }
                // If frame_index < target, target frame returned; do not advance caller IP.
            }
        }
    }

    fn build_array(&mut self, start: usize, end: usize) -> Value {
        // Move values out of stack to avoid Rc refcount overhead
        let mut elements = Vec::with_capacity(end - start);
        for i in start..end {
            let s = std::mem::replace(&mut self.stack[i], slot::uninit());
            elements.push(slot::from_slot(s));
        }
        leak_detector::record_array();
        Value::Array(Rc::new(elements))
    }

    fn build_tuple(&mut self, start: usize, end: usize) -> Value {
        let mut elements = Vec::with_capacity(end - start);
        for i in start..end {
            let s = std::mem::replace(&mut self.stack[i], slot::uninit());
            elements.push(slot::from_slot(s));
        }
        leak_detector::record_tuple();
        Value::Tuple(Rc::new(elements))
    }

    fn build_hash(&mut self, start: usize, end: usize) -> Result<Value, String> {
        let mut root = hamt::hamt_empty();
        let mut i = start;
        while i < end {
            let key = slot::from_slot(std::mem::replace(&mut self.stack[i], slot::uninit()));
            let value = slot::from_slot(std::mem::replace(&mut self.stack[i + 1], slot::uninit()));

            let hash_key = key
                .to_hash_key()
                .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

            root = hamt::hamt_insert(&root, hash_key, value);
            i += 2;
        }
        leak_detector::record_hash();
        Ok(Value::HashMap(root))
    }

    fn current_frame(&self) -> &Frame {
        &self.frames[self.frame_index]
    }

    fn current_frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.frame_index]
    }

    fn ensure_stack_capacity(&mut self, needed_top: usize) -> Result<(), String> {
        self.ensure_stack_capacity_with_headroom(needed_top, 0)
    }

    fn ensure_stack_capacity_with_headroom(
        &mut self,
        needed_top: usize,
        extra_headroom: usize,
    ) -> Result<(), String> {
        if needed_top <= self.stack.len() {
            return Ok(());
        }
        if needed_top > MAX_STACK_SIZE {
            return Err("stack overflow".to_string());
        }

        let target_top = needed_top
            .saturating_add(extra_headroom)
            .min(MAX_STACK_SIZE);
        let mut new_len = self.stack.len().max(1);
        while new_len < target_top {
            let grow_15 = new_len + (new_len / 2);
            let grow_chunk = new_len.saturating_add(STACK_GROW_MIN_CHUNK);
            new_len = grow_15.max(grow_chunk).min(MAX_STACK_SIZE);
        }
        if new_len < needed_top {
            return Err("stack overflow".to_string());
        }

        self.stack.resize_with(new_len, slot::uninit);
        Ok(())
    }

    #[inline(always)]
    fn clear_stack_range(&mut self, new_sp: usize, old_sp: usize) {
        debug_assert!(new_sp <= old_sp);
        debug_assert!(old_sp <= self.stack.len());
        for i in new_sp..old_sp {
            let _ = std::mem::replace(&mut self.stack[i], slot::uninit());
        }
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
    }

    #[inline(always)]
    fn reset_sp(&mut self, new_sp: usize) -> Result<(), String> {
        if new_sp > MAX_STACK_SIZE {
            return Err("stack overflow".to_string());
        }
        if new_sp > self.stack.len() {
            self.ensure_stack_capacity(new_sp)?;
        }
        let old_sp = self.sp;
        if new_sp < old_sp {
            self.clear_stack_range(new_sp, old_sp);
        }
        self.sp = new_sp;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(())
    }

    #[cfg(debug_assertions)]
    #[inline(always)]
    fn debug_assert_stack_invariant(&self) {
        // Dead stack slots may contain stale values until they are reused.
        // In debug mode, only enforce structural bounds to keep checks O(1).
        debug_assert!(self.sp <= self.stack.len());
    }

    #[inline(always)]
    fn push(&mut self, obj: Value) -> Result<(), String> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.sp <= self.stack.len());
        }
        if self.sp < self.stack.len() {
            self.stack[self.sp] = slot::to_slot(obj);
            self.sp += 1;
            #[cfg(debug_assertions)]
            self.debug_assert_stack_invariant();
            return Ok(());
        }
        self.push_slow(obj)
    }

    #[cold]
    #[inline(never)]
    fn push_slow(&mut self, obj: Value) -> Result<(), String> {
        self.ensure_stack_capacity(self.sp + 1)?;
        self.stack[self.sp] = slot::to_slot(obj);
        self.sp += 1;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) {
        self.frame_index += 1;
        if self.frame_index >= self.frames.len() {
            self.frames.push(frame);
        } else {
            self.frames[self.frame_index] = frame;
        }
    }

    fn pop_frame_return_slot(&mut self) -> usize {
        let return_slot = self.frames[self.frame_index].return_slot;
        self.frame_index -= 1;
        return_slot
    }

    #[inline(always)]
    fn pop(&mut self) -> Result<Value, String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        let new_sp = self.sp - 1;
        self.sp = new_sp;
        // Move out + overwrite without drop glue on the hot pop path.
        let value = unsafe {
            let slot_ptr = self.stack.as_mut_ptr().add(new_sp);
            let out = std::ptr::read(slot_ptr);
            std::ptr::write(slot_ptr, slot::uninit());
            slot::from_slot(out)
        };
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(value)
    }

    #[inline(always)]
    fn pop_and_track(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = slot::to_slot(value.clone());
        Ok(value)
    }

    #[inline(always)]
    fn peek(&self, back: usize) -> Result<Value, String> {
        if back >= self.sp {
            return Err("stack underflow".to_string());
        }
        let idx = self.sp - 1 - back;
        let value = slot::from_slot_ref(&self.stack[idx]);
        if matches!(value, Value::Uninit) {
            return Err("read from uninitialized stack slot".to_string());
        }
        Ok(value)
    }

    fn pop_untracked(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = slot::to_slot(Value::None);
        Ok(value)
    }

    #[inline(always)]
    fn discard_top(&mut self) -> Result<(), String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        self.sp -= 1;
        // Drop the value in-place without the clear_stack_range loop overhead.
        // SAFETY: sp was > 0, so self.sp is now a valid index holding a live Slot.
        unsafe {
            let slot_ptr = self.stack.as_mut_ptr().add(self.sp);
            let _old = std::ptr::read(slot_ptr);
            std::ptr::write(slot_ptr, slot::uninit());
        }
        self.last_popped = slot::to_slot(Value::None);
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(())
    }

    fn pop_pair_untracked(&mut self) -> Result<(Value, Value), String> {
        if self.sp < 2 {
            return Err("stack underflow".to_string());
        }
        let new_sp = self.sp - 2;
        self.sp = new_sp;
        // Move both values out in one pass and overwrite dead slots with uninit.
        // SAFETY: old sp >= 2 guarantees both slots are initialized and in-bounds.
        let (left, right) = unsafe {
            let base = self.stack.as_mut_ptr().add(new_sp);
            let left = std::ptr::read(base);
            let right = std::ptr::read(base.add(1));
            std::ptr::write(base, slot::uninit());
            std::ptr::write(base.add(1), slot::uninit());
            (slot::from_slot(left), slot::from_slot(right))
        };
        self.last_popped = slot::to_slot(Value::None);
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok((left, right))
    }

    // ── Stack/globals/constants accessor helpers ──────────────────────────────
    //
    // Use these in dispatch.rs and function_call.rs instead of direct indexing
    // to keep NanBox conversions in one place.

    /// Clone the Value at stack index `idx`.
    #[inline(always)]
    fn stack_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.stack[idx])
    }

    /// Store `v` at stack index `idx`.
    #[inline(always)]
    fn stack_set(&mut self, idx: usize, v: Value) {
        self.stack[idx] = slot::to_slot(v);
    }

    /// Take the Value at stack index `idx`, leaving `Uninit` in its place.
    #[inline(always)]
    fn stack_take(&mut self, idx: usize) -> Value {
        slot::from_slot(std::mem::replace(&mut self.stack[idx], slot::uninit()))
    }

    /// Take the raw `Slot` at stack index `idx`, leaving `Uninit` in its place.
    ///
    /// Unlike [`stack_take`], this does NOT decode the slot to a [`Value`].
    /// Clone the Value at constants index `idx`.
    #[inline(always)]
    fn const_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.constants[idx])
    }

    /// Clone the Value at globals index `idx`.
    #[inline(always)]
    fn global_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.globals[idx])
    }

    /// Store `v` at globals index `idx`.
    #[inline(always)]
    fn global_set(&mut self, idx: usize, v: Value) {
        self.globals[idx] = slot::to_slot(v);
    }

    /// Returns the last popped value from the stack.
    ///
    /// After a program completes execution, this returns the final result.
    pub fn last_popped_stack_elem(&self) -> Value {
        slot::from_slot_ref(&self.last_popped)
    }

    /// Export this VM's constants pool as a `Vec<Value>`.
    ///
    /// Used by the LIR execution path to transfer CFG-compiled constants
    /// (prelude function closures) into the LIR VM.
    pub fn export_constants(&self) -> Vec<Value> {
        self.constants.iter().map(slot::from_slot_ref).collect()
    }

    /// Swap the VM's globals with an external `Vec<Value>` buffer.
    ///
    /// Used by incremental VM-driven workflows to persist globals across iterations without exposing
    /// the internal `Slot` type.
    pub fn swap_globals_values(&mut self, external: &mut [Value]) {
        // Convert VM slots -> Values into external, and external Values -> slots into VM.
        let vm_len = self.globals.len();
        let ext_len = external.len();
        // Ensure both have the same length (they should; both are GLOBALS_SIZE).
        debug_assert_eq!(vm_len, ext_len);

        // Swap element-by-element.
        for (g, e) in self.globals[..vm_len.min(ext_len)]
            .iter_mut()
            .zip(external.iter_mut())
        {
            let vm_val = slot::from_slot(std::mem::replace(g, slot::uninit()));
            let ext_val = std::mem::replace(e, Value::None);
            *g = slot::to_slot(ext_val);
            *e = vm_val;
        }
    }
}

#[cfg(feature = "async-mio")]
impl Drop for VM {
    fn drop(&mut self) {
        if let Some(mut reactor) = self.async_mio_reactor.take() {
            let _ = reactor.handle.stop();
            if let Some(thread) = reactor.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod binary_ops_test;
#[cfg(test)]
mod comparison_ops_test;
#[cfg(test)]
mod dispatch_test;
#[cfg(test)]
mod function_call_test;
#[cfg(test)]
mod index_ops_test;
#[cfg(test)]
mod trace_test;
