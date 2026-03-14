- Feature Name: Thread-per-Actor Runtime Handler (MVP)
- Start Date: 2026-03-01
- Status: Not Implemented
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0066: Thread-per-Actor Runtime Handler (MVP)

## Summary
[summary]: #summary

Implement the minimal correct runtime handler for the `Actor` effect (proposal 0065)
using one OS thread per actor and `crossbeam-channel` mailboxes. This proves the actor
semantics end-to-end with correct isolation, correct message passing, and correct blocking
receive — without any custom scheduler. Limited to approximately 10,000 simultaneous
actors (OS thread limit).

## Motivation
[motivation]: #motivation

Before building an M:N green-thread scheduler (proposal 0071), the actor semantics must
be correct and observable. This is not an optimization step — it is the validation step.
Trying to build the scheduler and the actor semantics simultaneously increases debugging
surface area by an order of magnitude.

The thread-per-actor handler adds approximately 800 lines of Rust across four files. It
is the foundation every subsequent actor proposal builds on.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Running an actor program

```bash
# VM
cargo run -- --no-cache --root lib/ examples/actors/echo.flx

# JIT
cargo run --features jit -- --no-cache --root lib/ examples/actors/echo.flx --jit
```

### Example fixture: `examples/actors/echo.flx`

```flux
import Flow.Actor

fn echo_worker() with Actor, IO {
    let msg = recv()
    print(msg)
    echo_worker()
}

fn main() with Actor, IO {
    let w = spawn(\() with Actor -> echo_worker())
    send(w, "hello")
    send(w, "world")
}
```

Expected output:
```
hello
world
```

### What changes from the user's perspective

- `spawn`, `send`, `recv` become usable in programs.
- Programs with `with Actor` now execute correctly.
- `recv()` blocks until a message arrives.
- A spawned actor's lifetime ends when its function returns.
- Sending to a dead actor is a no-op (the channel is closed; the error is silently dropped
  in Phase 1).

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New files

```
src/runtime/actor/mod.rs          -- Public API: ActorRuntime, ActorId, ActorHandle
src/runtime/actor/sendable.rs     -- SendableValue: cross-thread safe Value representation
src/runtime/actor/registry.rs     -- ActorRegistry: id → sender channel mapping
src/runtime/actor/mailbox.rs      -- Mailbox: blocking/non-blocking receive
```

### `SendableValue` (src/runtime/actor/sendable.rs)

`Value` uses `Rc<T>` throughout, which is `!Send`. Messages cannot cross OS thread
boundaries as raw `Value`. `SendableValue` mirrors `Value` with `Arc` instead of `Rc`.

```rust
// src/runtime/actor/sendable.rs

use std::sync::Arc;
use crate::runtime::value::Value;

#[derive(Debug, Clone)]
pub enum SendableValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    EmptyList,
    // Small strings (≤ 512 bytes): cloned into a new Arc<str>
    Str(Arc<str>),
    // Tuples and arrays: recursively encoded
    Tuple(Arc<Vec<SendableValue>>),
    Array(Arc<Vec<SendableValue>>),
    // Option wrappers
    Some(Arc<SendableValue>),
    Left(Arc<SendableValue>),
    Right(Arc<SendableValue>),
    // Base function index (just a u8, trivially sendable)
    BaseFunction(u8),
}

#[derive(Debug)]
pub enum SendError {
    /// Value::Gc (cons list, HAMT map) cannot cross actor boundaries in Phase 1.
    /// Use to_array() / to_list() before sending if needed.
    GcValueCrossBoundary,
    /// Closures and compiled functions cannot be sent.
    FunctionCrossBoundary,
    /// ADT values are sendable if all fields are sendable.
    AdtFieldNotSendable { constructor: String, field_index: usize },
}

impl SendableValue {
    /// Convert a Value (Rc-based, actor-local) to a SendableValue.
    /// This is the copy-on-send boundary.
    pub fn from_value(v: &Value) -> Result<Self, SendError> {
        match v {
            Value::Integer(n)   => Ok(Self::Int(*n)),
            Value::Float(f)     => Ok(Self::Float(*f)),
            Value::Boolean(b)   => Ok(Self::Bool(*b)),
            Value::None         => Ok(Self::None),
            Value::EmptyList    => Ok(Self::EmptyList),
            Value::String(s)    => Ok(Self::Str(Arc::from(s.as_ref()))),
            Value::BaseFunction(i) => Ok(Self::BaseFunction(*i)),

            Value::Tuple(t) => {
                let fields: Result<Vec<_>, _> = t.iter()
                    .map(Self::from_value)
                    .collect();
                Ok(Self::Tuple(Arc::new(fields?)))
            }
            Value::Array(a) => {
                let elems: Result<Vec<_>, _> = a.iter()
                    .map(Self::from_value)
                    .collect();
                Ok(Self::Array(Arc::new(elems?)))
            }
            Value::Some(inner) => {
                Ok(Self::Some(Arc::new(Self::from_value(inner)?)))
            }
            Value::Left(inner) => {
                Ok(Self::Left(Arc::new(Self::from_value(inner)?)))
            }
            Value::Right(inner) => {
                Ok(Self::Right(Arc::new(Self::from_value(inner)?)))
            }

            // GC-managed cons lists and HAMT maps: Phase 1 limitation
            Value::Gc(_) => Err(SendError::GcValueCrossBoundary),

            // Functions/closures: not sendable
            Value::Function(_)
            | Value::Closure(_)
            | Value::JitClosure(_) => Err(SendError::FunctionCrossBoundary),

            Value::Adt(adt) => {
                let fields: Result<Vec<_>, _> = adt.fields.iter()
                    .enumerate()
                    .map(|(i, f)| Self::from_value(f).map_err(|_| {
                        SendError::AdtFieldNotSendable {
                            constructor: adt.constructor.as_ref().to_string(),
                            field_index: i,
                        }
                    }))
                    .collect();
                // Represent as a tuple with the constructor name as first element
                let mut all = vec![Self::Str(Arc::clone(&adt.constructor))];
                all.extend(fields?);
                Ok(Self::Tuple(Arc::new(all)))
            }

            // Internal VM sentinels: should not be sent
            Value::Uninit
            | Value::ReturnValue(_)
            | Value::Continuation(_)
            | Value::HandlerDescriptor(_)
            | Value::PerformDescriptor(_) => {
                Err(SendError::FunctionCrossBoundary)
            }
        }
    }

    /// Convert a received SendableValue back into a Value on the receiving actor's heap.
    pub fn into_value(self) -> Value {
        match self {
            Self::Int(n)        => Value::Integer(n),
            Self::Float(f)      => Value::Float(f),
            Self::Bool(b)       => Value::Boolean(b),
            Self::None          => Value::None,
            Self::EmptyList     => Value::EmptyList,
            Self::Str(s)        => Value::String(Rc::from(s.as_ref())),
            Self::BaseFunction(i) => Value::BaseFunction(i),
            Self::Tuple(t) => {
                let fields: Vec<Value> = Arc::try_unwrap(t)
                    .unwrap_or_else(|a| (*a).clone())
                    .into_iter()
                    .map(Self::into_value)
                    .collect();
                Value::Tuple(Rc::new(fields))
            }
            Self::Array(a) => {
                let elems: Vec<Value> = Arc::try_unwrap(a)
                    .unwrap_or_else(|a| (*a).clone())
                    .into_iter()
                    .map(Self::into_value)
                    .collect();
                Value::Array(Rc::new(elems))
            }
            Self::Some(inner) => {
                Value::Some(Rc::new(Arc::try_unwrap(inner)
                    .unwrap_or_else(|a| (*a).clone())
                    .into_value()))
            }
            Self::Left(inner) => {
                Value::Left(Rc::new(Arc::try_unwrap(inner)
                    .unwrap_or_else(|a| (*a).clone())
                    .into_value()))
            }
            Self::Right(inner) => {
                Value::Right(Rc::new(Arc::try_unwrap(inner)
                    .unwrap_or_else(|a| (*a).clone())
                    .into_value()))
            }
        }
    }
}
```

### `ActorRegistry` (src/runtime/actor/registry.rs)

```rust
// src/runtime/actor/registry.rs

use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use crossbeam_channel::Sender;
use dashmap::DashMap;
use super::sendable::SendableValue;

pub type ActorId = u64;

#[derive(Clone)]
pub struct ActorHandle {
    pub id: ActorId,
    pub(crate) tx: Sender<Envelope>,
}

#[derive(Debug)]
pub struct Envelope {
    pub sender_id: ActorId,
    pub payload: SendableValue,
}

pub struct ActorRegistry {
    handles: DashMap<ActorId, ActorHandle>,
    next_id: AtomicU64,
}

impl ActorRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            handles: DashMap::new(),
            next_id: AtomicU64::new(1),  // 0 reserved for "main"
        })
    }

    pub fn next_id(&self) -> ActorId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register(&self, handle: ActorHandle) {
        self.handles.insert(handle.id, handle);
    }

    pub fn get(&self, id: ActorId) -> Option<ActorHandle> {
        self.handles.get(&id).map(|h| h.clone())
    }

    pub fn remove(&self, id: ActorId) {
        self.handles.remove(&id);
    }

    /// Send a message. Returns false if actor is dead (channel closed).
    pub fn send(&self, target: ActorId, sender: ActorId, payload: SendableValue) -> bool {
        match self.get(target) {
            Some(handle) => handle.tx.send(Envelope { sender_id: sender, payload }).is_ok(),
            None => false,  // Actor dead or id invalid; silently drop in Phase 1
        }
    }
}
```

### `ActorRuntime` (src/runtime/actor/mod.rs)

```rust
// src/runtime/actor/mod.rs

pub mod sendable;
pub mod registry;
pub mod mailbox;

use std::sync::Arc;
use std::thread;
use crossbeam_channel::{unbounded, Receiver};
use registry::{ActorRegistry, ActorHandle, ActorId, Envelope};
use sendable::SendableValue;
use crate::runtime::value::Value;

/// Global actor runtime. One instance per Flux program execution.
/// Initialized at program startup, dropped at program exit.
pub struct ActorRuntime {
    pub registry: Arc<ActorRegistry>,
}

impl ActorRuntime {
    pub fn new() -> Self {
        Self { registry: ActorRegistry::new() }
    }

    /// Spawn a new actor. The actor runs `func` on a new OS thread.
    /// Returns the ActorId as a Value::Integer.
    pub fn spawn(
        &self,
        func: Value,                    // must be a Function or Closure
        current_actor_id: ActorId,
    ) -> Result<Value, String> {
        let id = self.registry.next_id();
        let (tx, rx) = unbounded::<Envelope>();

        let handle = ActorHandle { id, tx };
        self.registry.register(handle);

        let registry = Arc::clone(&self.registry);
        let func = func.clone();

        thread::Builder::new()
            .name(format!("flux-actor-{}", id))
            .spawn(move || {
                // Each actor gets its own VM instance.
                // The mailbox receiver is stored in a thread-local so that
                // OpPerform ACTOR_RECV can access it without passing it through
                // the entire VM call stack.
                ACTOR_CONTEXT.with(|ctx| {
                    *ctx.borrow_mut() = Some(ActorContext { id, rx, registry });
                });

                // Run the actor function.
                // This is a blocking call; the thread exits when func returns.
                let mut vm = crate::runtime::vm::Vm::new_for_actor(id);
                if let Err(e) = vm.call_value(func, vec![]) {
                    eprintln!("[actor {}] error: {}", id, e);
                }

                // Clean up: remove from registry so sends to this actor fail cleanly.
                ACTOR_CONTEXT.with(|ctx| {
                    if let Some(ctx) = ctx.borrow().as_ref() {
                        ctx.registry.remove(ctx.id);
                    }
                });
            })
            .map_err(|e| format!("failed to spawn actor thread: {}", e))?;

        Ok(Value::Integer(id as i64))
    }

    /// Send a message from `sender_id` to `target_id`.
    pub fn send(
        &self,
        target_id: ActorId,
        sender_id: ActorId,
        payload: &Value,
    ) -> Result<Value, String> {
        let sendable = SendableValue::from_value(payload)
            .map_err(|e| format!("cannot send value: {:?}", e))?;
        self.registry.send(target_id, sender_id, sendable);
        Ok(Value::None)
    }
}

/// Per-actor context stored in a thread-local.
/// Accessed by OpPerform ACTOR_RECV without threading through VM state.
struct ActorContext {
    id: ActorId,
    rx: Receiver<Envelope>,
    registry: Arc<ActorRegistry>,
}

thread_local! {
    static ACTOR_CONTEXT: std::cell::RefCell<Option<ActorContext>> =
        std::cell::RefCell::new(None);
}

/// Called by OpPerform ACTOR_RECV.
/// Blocks the current thread until a message arrives.
pub fn actor_recv_blocking() -> Result<Value, String> {
    ACTOR_CONTEXT.with(|ctx| {
        let ctx = ctx.borrow();
        let ctx = ctx.as_ref()
            .ok_or_else(|| "recv() called outside of an actor context".to_string())?;
        let envelope = ctx.rx.recv()
            .map_err(|_| "actor mailbox closed".to_string())?;
        Ok(envelope.payload.into_value())
    })
}

/// Returns the current actor's id, or 0 for the main thread.
pub fn current_actor_id() -> ActorId {
    ACTOR_CONTEXT.with(|ctx| {
        ctx.borrow().as_ref().map(|c| c.id).unwrap_or(0)
    })
}
```

### PrimOps: ActorSpawn, ActorSend, ActorRecv (src/primop/mod.rs)

Append three new PrimOps. **Never reorder or reuse discriminant values.**

```rust
// src/primop/mod.rs — append after existing variants

pub enum PrimOp {
    // ... existing 40 ops (0–39) ...

    // Actor operations (71–73)
    // Gap left intentionally (40–70) for future non-actor ops.
    ActorSpawn = 71,
    ActorSend  = 72,
    ActorRecv  = 73,
}

impl PrimOp {
    pub const COUNT: usize = 74;  // update from 40

    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            // ... existing arms ...
            71 => Some(Self::ActorSpawn),
            72 => Some(Self::ActorSend),
            73 => Some(Self::ActorRecv),
            _  => None,
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            // ... existing arms ...
            Self::ActorSpawn => 1,  // fn() with Actor -> Unit
            Self::ActorSend  => 2,  // (ActorId, Any)
            Self::ActorRecv  => 0,  // ()
        }
    }

    pub fn effect_kind(&self) -> PrimEffect {
        match self {
            // ... existing arms ...
            Self::ActorSpawn | Self::ActorSend | Self::ActorRecv => PrimEffect::Io,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            // ... existing arms ...
            Self::ActorSpawn => "actor_spawn",
            Self::ActorSend  => "actor_send",
            Self::ActorRecv  => "actor_recv",
        }
    }
}
```

### PrimOp execution (src/primop/mod.rs → execute_actor_primop)

```rust
// In execute_primop(), add arm:
PrimOp::ActorSpawn | PrimOp::ActorSend | PrimOp::ActorRecv => {
    execute_actor_primop(ctx, op, args)
}

fn execute_actor_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    let runtime = ctx.actor_runtime()
        .ok_or("actor operations require the Actor runtime to be initialized")?;

    match op {
        PrimOp::ActorSpawn => {
            // args[0] = the function/closure to run as an actor
            let func = args.into_iter().next()
                .ok_or("ActorSpawn: missing function argument")?;
            let sender_id = current_actor_id();
            runtime.spawn(func, sender_id)
        }
        PrimOp::ActorSend => {
            // args[0] = ActorId (Integer), args[1] = message (Any)
            let mut it = args.into_iter();
            let target = it.next().ok_or("ActorSend: missing target id")?;
            let msg    = it.next().ok_or("ActorSend: missing message")?;
            let target_id = match &target {
                Value::Integer(n) => *n as u64,
                _ => return Err(format!("ActorSend: target must be an Int, got {:?}", target)),
            };
            let sender_id = current_actor_id();
            runtime.send(target_id, sender_id, &msg)
        }
        PrimOp::ActorRecv => {
            // No arguments. Blocks until a message arrives.
            actor_recv_blocking()
        }
        _ => unreachable!(),
    }
}
```

### RuntimeContext extension (src/runtime/vm/mod.rs)

```rust
// Add to the RuntimeContext trait:
pub trait RuntimeContext {
    // ... existing methods ...

    /// Returns the actor runtime if the program was started with actor support.
    fn actor_runtime(&self) -> Option<&ActorRuntime>;

    /// Returns this actor's id (0 for main thread / non-actor VM).
    fn actor_id(&self) -> u64;
}
```

### JIT helpers (src/jit/runtime_helpers.rs)

```rust
// Append to existing extern "C" helpers:

#[no_mangle]
pub extern "C" fn rt_actor_spawn(
    ctx: *mut JitContext,
    func: *mut Value,
) -> *mut Value {
    let ctx = unsafe { &mut *ctx };
    let func = unsafe { &*func }.clone();
    match ctx.actor_runtime().and_then(|r| r.spawn(func, current_actor_id()).ok()) {
        Some(v) => ctx.alloc_value(v),
        None => {
            ctx.set_error("actor_spawn failed".to_string());
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn rt_actor_send(
    ctx: *mut JitContext,
    target: *mut Value,
    msg: *mut Value,
) -> *mut Value {
    let ctx = unsafe { &mut *ctx };
    let target = unsafe { &*target }.clone();
    let msg    = unsafe { &*msg }.clone();
    let target_id = match &target {
        Value::Integer(n) => *n as u64,
        _ => { ctx.set_error("actor_send: target must be Int".to_string()); return std::ptr::null_mut(); }
    };
    match ctx.actor_runtime().and_then(|r| r.send(target_id, current_actor_id(), &msg).ok()) {
        Some(v) => ctx.alloc_value(v),
        None    => { ctx.set_error("actor_send failed".to_string()); std::ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn rt_actor_recv(ctx: *mut JitContext) -> *mut Value {
    let ctx = unsafe { &mut *ctx };
    match actor_recv_blocking() {
        Ok(v)  => ctx.alloc_value(v),
        Err(e) => { ctx.set_error(e); std::ptr::null_mut() }
    }
}
```

### Cargo.toml additions

```toml
[dependencies]
crossbeam-channel = "0.5"
dashmap = "5"
```

### Validation commands

```bash
# Build with actor support (no feature gate needed — always enabled)
cargo build

# Run fixture: VM
cargo run -- --no-cache --root lib/ examples/actors/echo.flx

# Run fixture: JIT
cargo run --features jit -- --no-cache --root lib/ examples/actors/echo.flx --jit

# Run all actor tests
cargo test --test actor_tests
```

### Test fixture: `tests/testdata/actors/echo.flx`

```flux
import Flow.Actor

fn echo_worker() with Actor, IO {
    let msg = recv()
    print(msg)
    echo_worker()
}

fn main() with Actor, IO {
    let w = spawn(\() with Actor -> echo_worker())
    send(w, "hello")
    send(w, "world")
}
```

### Known Phase 1 limitations

| Limitation | Impact | Resolution |
|---|---|---|
| Max ~10K actors | OS thread limit | Proposal 0071 (M:N scheduler) |
| `Value::Gc` not sendable | Cannot send cons lists or HAMT maps | Proposal 0067 gives clean error; proposal 0070 resolves |
| No actor death notification | Dead actor sends silently dropped | Proposal 0065 future: monitors |
| No preemption | Tight loops block scheduler thread | Proposal 0071 adds fuel-based preemption |
| recv() blocks OS thread | Each blocked actor holds a thread | Proposal 0071 resolves with green threads |

## Drawbacks
[drawbacks]: #drawbacks

- One OS thread per actor limits scalability. This is an intentional Phase 1 limitation.
- `dashmap` is a new dependency. Alternatives: `parking_lot::RwLock<HashMap<...>>`.
- The `ACTOR_CONTEXT` thread-local creates implicit state in the VM that is not visible
  in the function signatures. This is an acceptable tradeoff for Phase 1 simplicity.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not tokio?** tokio requires async/await coloring throughout the VM and JIT. The VM
instruction loop is synchronous; making it async is a complete redesign, not an extension.

**Why not a custom green thread scheduler immediately?** Building the scheduler and
semantics together doubles the debugging surface area. Proving semantics on OS threads
first is the correct engineering approach.

**Why `crossbeam-channel`?** It is the Rust standard for efficient, lock-free MPSC
channels. The alternative (`std::sync::mpsc`) has worse performance and less flexible
API. `crossbeam-channel` is already an indirect dependency of several Rust tools.

## Prior art
[prior-art]: #prior-art

- **Erlang/OTP**: thread-per-process was the original BEAM model before green threads.
- **Go**: goroutines started as OS threads before the GMP scheduler was built.
- **Akka (JVM)**: thread-per-actor is a valid deployment mode for small actor counts.
- **Proposal 0065**: defines the Actor effect this proposal implements.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should actor thread panics propagate to the main thread? Decision: no, log and clean
   up silently. Panic propagation is part of the failure model (future proposal).
2. Should the main thread itself be an actor (id=0) with a mailbox? Decision: yes,
   for semantic consistency, but recv() on the main thread is allowed and blocks.
3. Should `spawn` wait for the actor to initialize before returning the id? Decision: no,
   fire-and-forget. The id is valid immediately; the actor may not have started yet.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Proposal 0071**: replace OS threads with M:N green thread scheduler.
- **Actor death notification**: when an actor thread exits, remove from registry and
  optionally notify linked actors.
- **Bounded mailboxes**: `spawn_bounded(fn, capacity)` creates an actor with a bounded
  mailbox; senders block when full (backpressure).
- **Actor introspection**: `actor_status(id)` returns `Running | Dead | Blocked`.
