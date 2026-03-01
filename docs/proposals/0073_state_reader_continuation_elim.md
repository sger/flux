- Feature Name: Eliminate Continuation Allocation for State and Reader Effects
- Start Date: 2026-03-01
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0073: Eliminate Continuation Allocation for State and Reader Effects

## Summary
[summary]: #summary

Specialize the evidence passing optimization (proposal 0072) for the two most common
tail-resumptive effect patterns — `State<s>` and `Reader<e>` — by compiling their
handlers to Rust-level mutable references and function parameters respectively. This
eliminates both the continuation allocation *and* the `RefCell` dynamic borrow check,
achieving zero-overhead effect dispatch for these patterns.

## Motivation
[motivation]: #motivation

Proposal 0072 eliminates continuation allocation for tail-resumptive handlers by using
an evidence record (`Rc<RefCell<Vec<Value>>>`). The `RefCell` is necessary for general
tail-resumptive handlers because the VM needs mutable access to the evidence in the
middle of expression evaluation. But for `State` and `Reader` specifically, the access
pattern is always safe at compile time:

- `State`: `get()` only reads, `set(v)` only writes. They never overlap.
- `Reader`: `ask()` is read-only. The environment never changes.

For these patterns, `RefCell` dynamic borrow checking is unnecessary overhead. We can
compile directly to `*mut Value` (for State) and `*const Value` (for Reader), threaded
through the call stack as a hidden function parameter.

This is the model the Koka compiler uses internally. The Koka paper calls this
"monomorphic evidence" — evidence that is specialized to a specific effect at compile time.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What changes for users

Nothing. No syntax changes. The optimization is automatic when the compiler detects a
recognized effect pattern.

### Recognized patterns

The compiler recognizes these standard effects by name:

| Effect name | Pattern recognized | Compiled as |
|---|---|---|
| `State` or `State<s>` | `get() -> resume(s)` + `set(v) -> resume(unit)` | `*mut Value` on call stack |
| `Reader` or `Reader<e>` | `ask() -> resume(env)` | `*const Value` function arg |

All other tail-resumptive effects use the general evidence passing from proposal 0072.

### Performance: the elimination chain

```
Effect operation cost (per perform call):

Phase 1 (proposal 0066 - thread-per-actor):
  Continuation alloc + frame copy → O(stack depth) allocation + copy

Proposal 0072 (evidence passing):
  Rc<RefCell<Vec>> alloc (once per handle) + RefCell::borrow_mut() per op
  → O(1) allocation per handle expression + ~5ns per perform call

Proposal 0073 (raw pointer specialization):
  Pointer push onto call stack (once per handle) + raw dereference per op
  → zero allocation + ~1ns per perform call (register or L1 cache)
```

For a tight loop doing 10M state operations:
- Current: ~50ms (continuation alloc + GC pressure)
- After 0072: ~5ms (RefCell overhead)
- After 0073: ~0.5ms (raw pointer read/write)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Detection: recognizing State and Reader patterns

```rust
// src/bytecode/compiler/tail_resumptive.rs

#[derive(Debug, Clone, PartialEq)]
pub enum HandlerSpecialization {
    /// General tail-resumptive: use evidence struct (Rc<RefCell<...>>)
    General,
    /// State effect: use *mut Value thread-local or call-stack slot
    State { var_count: usize },
    /// Reader effect: use *const Value passed as hidden parameter
    Reader { env_slot_count: usize },
}

pub fn classify_handler(handler: &HandleExpression) -> HandlerSpecialization {
    if !is_tail_resumptive(handler) {
        // Not even tail-resumptive; use full continuation path
        return HandlerSpecialization::General;
    }

    let op_names: Vec<&str> = handler.arms.iter()
        .map(|arm| arm.op_name.as_ref())
        .collect();

    // Recognize State effect: exactly get + set operations
    if op_names == ["get", "set"] || op_names == ["set", "get"] {
        let set_arm = handler.arms.iter().find(|a| a.op_name == "set").unwrap();
        if is_simple_set_resume(set_arm) {
            return HandlerSpecialization::State { var_count: 1 };
        }
    }

    // Recognize Reader effect: exactly ask operation
    if op_names == ["ask"] {
        let ask_arm = handler.arms.iter().find(|a| a.op_name == "ask").unwrap();
        if is_simple_ask_resume(ask_arm) {
            return HandlerSpecialization::Reader { env_slot_count: 1 };
        }
    }

    // Unrecognized: use general evidence passing
    HandlerSpecialization::General
}

/// True if the arm is: set(v) -> resume(unit)
fn is_simple_set_resume(arm: &HandlerArm) -> bool {
    matches!(&arm.body, Expression::Call { callee, .. } if is_resume_call(callee))
}

/// True if the arm is: ask() -> resume(env)
fn is_simple_ask_resume(arm: &HandlerArm) -> bool {
    matches!(&arm.body, Expression::Call { callee, .. } if is_resume_call(callee))
}
```

### New OpCodes: `OpHandleState` and `OpHandleReader`

```rust
// src/bytecode/opcode.rs

pub enum OpCode {
    // ... existing + OpHandleTR + OpPerformTR from 0072 ...

    /// OpHandleState(init_slot)
    /// Compiles a State effect handle expression.
    ///   1. Pops the initial state value from the stack.
    ///   2. Stores it in a call-stack-local slot (a fixed Vm.stack slot index).
    ///   3. Executes the body.
    ///   4. OpPerformState reads/writes directly to this slot.
    OpHandleState = 0xD2,

    /// OpPerformStateGet
    /// Pushes the current state value (reads from state slot).
    /// O(1), no allocation.
    OpPerformStateGet = 0xD3,

    /// OpPerformStateSet
    /// Pops a value, stores it in the state slot.
    /// O(1), no allocation.
    OpPerformStateSet = 0xD4,

    /// OpHandleReader(env_slot)
    /// Compiles a Reader effect handle.
    ///   1. Pops the environment value from the stack.
    ///   2. Stores in a read-only slot.
    ///   3. Body executes with the environment accessible via OpPerformAsk.
    OpHandleReader = 0xD5,

    /// OpPerformAsk
    /// Pushes the current reader environment value.
    /// O(1), no allocation, read-only.
    OpPerformAsk = 0xD6,
}
```

### VM execution: State effect

The state value is stored in a dedicated `state_slots` stack within the VM — a `Vec<Value>`
that parallels the handler stack:

```rust
// src/runtime/vm/mod.rs

pub struct Vm {
    // ... existing fields ...

    /// State effect slots: one per active OpHandleState.
    /// Pushed on OpHandleState, popped when the handle block exits.
    state_slots: Vec<Value>,

    /// Reader effect slots: one per active OpHandleReader.
    reader_slots: Vec<Value>,
}

// src/runtime/vm/dispatch.rs

OpCode::OpHandleState => {
    // Pop the initial state value from the main stack
    let init = self.pop()?;
    self.state_slots.push(init);
    // Mark the current frame so we know to pop state_slots on exit
    self.frames[self.frame_index].state_slot_depth = self.state_slots.len();
    Ok(1)
}

OpCode::OpPerformStateGet => {
    // Read current state: just clone from the slot (no allocation for primitives)
    let state = self.state_slots.last()
        .ok_or("get(): no State handler active")?
        .clone();
    self.push(state)?;
    Ok(1)
}

OpCode::OpPerformStateSet => {
    // Write new state: pop from main stack, store in slot
    let new_val = self.pop()?;
    *self.state_slots.last_mut()
        .ok_or("set(): no State handler active")? = new_val;
    self.push(Value::None)?;   // set() returns Unit
    Ok(1)
}

OpCode::OpHandleReader => {
    let env = self.pop()?;
    self.reader_slots.push(env);
    self.frames[self.frame_index].reader_slot_depth = self.reader_slots.len();
    Ok(1)
}

OpCode::OpPerformAsk => {
    let env = self.reader_slots.last()
        .ok_or("ask(): no Reader handler active")?
        .clone();
    self.push(env)?;
    Ok(1)
}
```

### Frame cleanup on block exit

When a `handle` block exits (the body function returns), the state/reader slots must be
popped:

```rust
// src/runtime/vm/function_call.rs — on return from a handle block

fn exit_handle_block(&mut self, frame: &CallFrame) {
    // Pop state slots back to the depth recorded when this handle was entered
    while self.state_slots.len() > frame.state_slot_depth {
        self.state_slots.pop();
    }
    while self.reader_slots.len() > frame.reader_slot_depth {
        self.reader_slots.pop();
    }
}
```

### Bytecode emission: State effect example

For:

```flux
handle {
    count_to(1000000)
    get()
} with State(0) {
    get()   -> resume(state)
    set(v)  -> do { state = v; resume(unit) }
}
```

The compiler emits:

```
OpConstant 0           ; initial state value
OpHandleState          ; pop 0, push to state_slots

; body:
OpConstant 1000000
OpCall count_to 1      ; count_to will use OpPerformStateGet / OpPerformStateSet

OpPerformStateGet      ; final get() for the result

; cleanup:
; (OpHandleState cleanup is handled by frame exit)
```

Inside `count_to`, every `get()` call compiles to `OpPerformStateGet` (1 instruction,
reads from state_slots.last()) and every `set(v)` compiles to a push + `OpPerformStateSet`
(2 instructions, writes to state_slots.last()). No allocation, no RefCell, no continuation.

### JIT integration

For JIT-compiled code, the state slot is stored in the `JitContext` as a thread-local:

```rust
// src/jit/runtime_helpers.rs

thread_local! {
    static JIT_STATE_SLOTS: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    static JIT_READER_SLOTS: RefCell<Vec<Value>> = RefCell::new(Vec::new());
}

#[no_mangle]
pub extern "C" fn rt_handle_state_push(ctx: *mut JitContext, init: *mut Value) {
    let val = unsafe { (*init).clone() };
    JIT_STATE_SLOTS.with(|s| s.borrow_mut().push(val));
}

#[no_mangle]
pub extern "C" fn rt_perform_state_get(ctx: *mut JitContext) -> *mut Value {
    let ctx = unsafe { &mut *ctx };
    JIT_STATE_SLOTS.with(|s| {
        let val = s.borrow().last().cloned().unwrap_or(Value::None);
        ctx.alloc_value(val)
    })
}

#[no_mangle]
pub extern "C" fn rt_perform_state_set(ctx: *mut JitContext, new_val: *mut Value) -> *mut Value {
    let val = unsafe { (*new_val).clone() };
    JIT_STATE_SLOTS.with(|s| {
        if let Some(slot) = s.borrow_mut().last_mut() {
            *slot = val;
        }
    });
    let ctx = unsafe { &mut *ctx };
    ctx.alloc_value(Value::None)
}
```

### Validation commands

```bash
# Build
cargo build

# Verify State effect correctness
cargo run -- --no-cache examples/effects/state_counter.flx

# Benchmark: compare 0072 evidence passing vs 0073 raw slot
cargo bench --bench state_effect_bench

# Verify Reader effect
cargo run -- --no-cache examples/effects/reader_config.flx

# JIT validation
cargo run --features jit -- --no-cache examples/effects/state_counter.flx --jit

# Full test suite
cargo test --test effect_handler_tests
```

### Benchmark fixture

```flux
-- examples/effects/state_bench.flx

effect State {
    get : () -> Int
    set : (Int) -> Unit
}

fn sum_to(n: Int) with State {
    if n == 0 {
        unit
    } else {
        set(get() + n)
        sum_to(n - 1)
    }
}

fn main() with IO {
    let result = handle {
        sum_to(10000000)
        get()
    } with State(0) {
        get()   -> resume(state)
        set(v)  -> do { state = v; resume(unit) }
    }
    print(result)   -- 50000005000000
}
```

Expected performance: `sum_to(10M)` should complete in under 100ms with this optimization.

## Drawbacks
[drawbacks]: #drawbacks

- Two more specialized opcodes (`OpHandleState`, `OpHandleReader`, `OpPerformStateGet`,
  `OpPerformStateSet`, `OpPerformAsk`) are added to the bytecode format.
- The `state_slots: Vec<Value>` field on `Vm` adds memory to every VM instance, even
  those that never use State effects.
- Pattern matching is fragile: if the user names their State effect differently
  (`effect MyState { read: () -> Int; write: (Int) -> Unit }`) it won't be recognized.
  Mitigation: document the recognized patterns; users who need the optimization use the
  standard names.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why hard-code State and Reader vs general mechanism?** The general mechanism
(proposal 0072) already handles the semantic correctness. This proposal adds performance
by specializing the two most universally useful patterns. Hard-coding two patterns is
a pragmatic engineering decision for a solo implementer — building a fully general
monomorphization framework is much more work with uncertain return.

**Why not use Rust's borrow checker for the state slot?** The state slot must be
accessible across arbitrary function calls (the body of the handle block calls other
functions that may perform State effects). Rust's borrow checker cannot express this
pattern without unsafe. Thread-local `Vec<Value>` with `push`/`pop` is the pragmatic
safe implementation.

**Alternative: CPS transform.** CPS-transforming `State` to `fn(s) -> (a, s)` is
correct but requires transforming the entire body function's type. This would require
exposing the state threading in the function call ABI, changing how `count_to` is called.
The stack-slot approach avoids this by keeping the state invisible in the calling
convention.

## Prior art
[prior-art]: #prior-art

- **Koka** — compiles `State` to a mutable variable passed as evidence. The "var" keyword
  in Koka creates a mutable location in the evidence record, compiled to a Rust-style
  mutable reference under the hood.
- **Haskell's `ST` monad** — eliminates State allocation via the `runST` trick, similar
  in spirit to the stack-slot approach here.
- **MLton** — compiles `ref` to stack-allocated mutable cells when liveness analysis
  permits.
- **Proposal 0072** — the general evidence passing this proposal specializes.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the recognized effect names (`State`, `Reader`) be configured or hard-coded?
   Decision: hard-coded for now. A future annotation (`@tail_resumptive`) could mark any
   effect for this optimization.
2. Should `state_slots` be a fixed-size array (e.g., `[Value; 16]`) rather than a `Vec`
   to avoid heap allocation for the slot stack? Decision: `Vec` for simplicity; optimize
   if profiling shows the push/pop overhead matters.
3. Should `set()` return the new state (for chaining) or always return `Unit`? Decision:
   always `Unit`, matching Koka's `State` effect convention.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Annotation-based specialization**: `@stack_allocated effect MyEffect { ... }` marks
  any user-defined effect for the stack-slot optimization, not just recognized patterns.
- **Multi-cell State**: `State<(Int, String)>` with a tuple of mutable cells, each
  compiled to a separate stack slot.
- **Writer effect**: `tell(v) -> resume(unit)` where the writer accumulates values into
  a list or string. Can also be compiled to a stack-local mutable buffer.
- **Non-monadic State**: when the `with State(init)` handler is at the top level and
  the state never escapes, the compiler can allocate it on the Rust stack entirely
  (no heap allocation for the initial state value).
