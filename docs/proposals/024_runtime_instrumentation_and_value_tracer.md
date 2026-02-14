# Proposal 024: Runtime Instrumentation Hooks & Value Graph Tracer

**Status:** Proposed
**Priority:** Medium
**Created:** 2026-02-12
**Related:** Proposal 017 (Persistent Collections and GC), Proposal 022 (AST Traversal Framework), Proposal 023 (Bytecode Decode & Passes)

## Overview

Two complementary runtime infrastructure pieces:

- **Part A — VM Observer Hooks:** A trait-based observer that receives callbacks for instruction execution, function calls/returns, and errors — without changing execution semantics.
- **Part B — Value Graph Tracer:** A `Trace` trait and `Tracer` struct that walk the live value graph from roots (stack, globals, frame closures), suitable for future GC/leak analysis.

## Value & Use Cases

### What This Enables

| Use Case | Part | How it uses the API |
|----------|------|-------------------|
| **Instruction profiling** | A | `on_instruction` callback counts opcode frequencies, measures time per opcode category |
| **Call graph construction** | A | `on_call` / `on_return` callbacks build a dynamic call tree with timing |
| **Step debugging** | A | `on_instruction` callback with breakpoint table; pause and inspect state |
| **Coverage analysis** | A | `on_instruction` records which (file, line) pairs execute |
| **Runtime error logging** | A | `on_error` callback captures structured error context before formatting |
| **Heap snapshot** | B | `trace_roots` walks all live values, counts by type, measures Rc sharing |
| **Leak detection** | B | Compare `trace_roots` results before/after a scope to find retained values |
| **Future GC integration** | B | Mark phase uses `Trace` to find live objects (Proposal 017) |
| **Closure analysis** | B | Trace from a closure to enumerate all transitively captured values |

### Why a Trait Instead of the Existing `trace: bool` Flag?

The VM currently has a `trace: bool` field and a `trace_instruction()` method that prints to stdout ([vm/mod.rs:30](src/runtime/vm/mod.rs#L30), [vm/trace.rs:171](src/runtime/vm/trace.rs#L171)). This is hardcoded to one output format (println) and cannot be extended for profiling, debugging, or coverage without modifying the VM itself. A trait-based observer decouples instrumentation from execution.

## Existing Architecture

### VM Structure ([vm/mod.rs](src/runtime/vm/mod.rs))

```rust
pub struct VM {
    constants: Vec<Value>,
    stack: Vec<Value>,          // growable, max 1,048,576 slots
    sp: usize,
    last_popped: Value,
    pub globals: Vec<Value>,    // 65,536 slots
    frames: Vec<Frame>,
    frame_index: usize,
    trace: bool,                // existing trace flag (to be replaced)
}
```

### Execution Loop

```
execute_current_instruction()
  → read ip and op from current frame
  → if trace: trace_instruction(ip, op)      ← HOOK POINT: on_instruction
  → dispatch_instruction(ip, op)
    → OpCall / OpTailCall                     ← HOOK POINT: on_call
    → OpReturnValue / OpReturn                ← HOOK POINT: on_return
    → error path                              ← HOOK POINT: on_error
```

### Value Memory Model ([value.rs](src/runtime/value.rs))

14 variants. Primitives unboxed; containers use `Rc<T>`:

| Variant | Inner type | Contains Values? |
|---------|-----------|-----------------|
| `Integer(i64)` | primitive | no |
| `Float(f64)` | primitive | no |
| `Boolean(bool)` | primitive | no |
| `None` | unit | no |
| `String(Rc<str>)` | Rc | no |
| `Some(Rc<Value>)` | Rc | **yes** (1 child) |
| `Left(Rc<Value>)` | Rc | **yes** (1 child) |
| `Right(Rc<Value>)` | Rc | **yes** (1 child) |
| `ReturnValue(Rc<Value>)` | Rc | **yes** (1 child) |
| `Function(Rc<CompiledFunction>)` | Rc | no (bytecode only) |
| `Closure(Rc<Closure>)` | Rc | **yes** (free vars: `Vec<Value>`) |
| `Builtin(BuiltinFunction)` | fn pointer | no |
| `Array(Rc<Vec<Value>>)` | Rc | **yes** (N children) |
| `Hash(Rc<HashMap<HashKey, Value>>)` | Rc | **yes** (N children) |

**No-Cycle Invariant:** Values form DAGs, never cycles. `Rc` cannot handle cycles (would leak). The language design enforces this through immutability and lack of mutable reference cells.

### Roots (entry points for value graph traversal)

| Root set | Location | How to access |
|----------|----------|--------------|
| Stack | `vm.stack[0..vm.sp]` | Live values currently on stack |
| Globals | `vm.globals[..]` | Module-level bindings |
| Frame closures | `vm.frames[0..=vm.frame_index]` | Each frame holds `Rc<Closure>` with free vars |
| Constants | `vm.constants[..]` | Compiled-in constant values |

## Design

### Part A — VM Observer Hooks

#### `src/runtime/vm/observer.rs`

```rust
use crate::bytecode::op_code::OpCode;
use crate::runtime::value::Value;

/// Observer trait for VM execution events.
/// All methods default to no-ops. Override only the events you need.
pub trait VmObserver {
    /// Called before each instruction is dispatched.
    fn on_instruction(&mut self, pc: usize, op: OpCode) {}

    /// Called when a function/closure is about to be invoked.
    fn on_call(&mut self, callee: &Value, num_args: usize) {}

    /// Called when a function returns a value.
    fn on_return(&mut self, value: &Value) {}

    /// Called when the VM encounters a runtime error.
    fn on_error(&mut self, err: &str) {}
}

/// Default no-op observer. All hooks are inlined away by the compiler.
pub struct NoopObserver;
impl VmObserver for NoopObserver {}
```

#### Generic VM with Default Observer

```rust
pub struct VM<O: VmObserver = NoopObserver> {
    // ... existing fields ...
    observer: O,
}
```

Using a default type parameter means:
- `VM::new(bytecode)` creates `VM<NoopObserver>` — **fully backward compatible**
- `VM::with_observer(bytecode, observer)` creates `VM<O>` for custom observers
- The compiler monomorphizes `VM<NoopObserver>` and eliminates all no-op observer calls — **zero cost when not used**
- All 7 existing `impl` blocks gain `<O: VmObserver>` parameter

**Trade-off vs `Box<dyn VmObserver>`:** Dynamic dispatch adds a vtable lookup per instruction (millions/sec). The generic approach has zero overhead for the default case. The cost is adding `<O: VmObserver>` to all impl blocks (~7 files).

#### Hook Insertion Points

| Hook | File | Location | When |
|------|------|----------|------|
| `on_instruction` | [vm/mod.rs](src/runtime/vm/mod.rs) | `execute_current_instruction()` | After reading op, before dispatch. **Replaces** existing `if self.trace { self.trace_instruction(ip, op) }` |
| `on_call` | [vm/function_call.rs](src/runtime/vm/function_call.rs) | `execute_call()` | After resolving callee, before creating frame |
| `on_return` | [vm/dispatch.rs](src/runtime/vm/dispatch.rs) | `OpReturnValue` / `OpReturn` handlers | After popping return value, before restoring caller frame |
| `on_error` | [vm/mod.rs](src/runtime/vm/mod.rs) | `run()` error path | Before formatting through diagnostic system |

#### Built-in TraceObserver

Replaces the existing `trace: bool` + `trace_instruction()` approach:

```rust
pub struct TraceObserver;

impl VmObserver for TraceObserver {
    fn on_instruction(&mut self, pc: usize, op: OpCode) {
        println!("IP={:04} {}", pc, op);
    }

    fn on_call(&mut self, callee: &Value, num_args: usize) {
        println!("  CALL {} args={}", callee.type_name(), num_args);
    }

    fn on_return(&mut self, value: &Value) {
        println!("  RET {}", value.type_name());
    }
}
```

The existing `trace_instruction` method (which also prints stack/locals state) can be preserved as a richer observer variant or kept as an internal helper that `TraceObserver` delegates to.

### Part B — Value Graph Tracer

#### `src/runtime/value_trace.rs`

```rust
use std::collections::HashSet;
use std::rc::Rc;

use crate::runtime::value::Value;
use crate::runtime::closure::Closure;

/// Pointer identity for Rc-wrapped values.
type PtrId = usize;

/// Tracks visited Rc pointers during value graph traversal.
pub struct Tracer {
    visited: HashSet<PtrId>,
    pub value_count: usize,
}

/// Trait for types that can be traced through the value graph.
pub trait Trace {
    fn trace(&self, tracer: &mut Tracer);
}
```

#### Tracer Implementation

```rust
impl Tracer {
    pub fn new() -> Self { ... }

    /// Returns true if this is the first visit to this Rc pointer.
    /// Uses Rc::as_ptr() cast to usize for identity.
    pub fn visit_rc<T>(&mut self, rc: &Rc<T>) -> bool {
        self.visited.insert(Rc::as_ptr(rc) as *const () as usize)
    }

    pub fn visited_count(&self) -> usize {
        self.visited.len()
    }
}
```

#### Trace Implementations

**Value:**
```rust
impl Trace for Value {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.value_count += 1;
        match self {
            // Primitives — no children, no Rc
            Value::Integer(_) | Value::Float(_) | Value::Boolean(_)
            | Value::None | Value::Builtin(_) => {}

            // Rc<str> — track pointer, no children
            Value::String(rc) => { tracer.visit_rc(rc); }

            // Single-child wrappers
            Value::Some(rc) | Value::Left(rc) | Value::Right(rc)
            | Value::ReturnValue(rc) => {
                if tracer.visit_rc(rc) {
                    rc.as_ref().trace(tracer);
                }
            }

            // Function — Rc<CompiledFunction>, no Value children
            Value::Function(rc) => { tracer.visit_rc(rc); }

            // Closure — Rc<Closure>, contains free vars
            Value::Closure(rc) => {
                if tracer.visit_rc(rc) {
                    rc.as_ref().trace(tracer);
                }
            }

            // Array — Rc<Vec<Value>>
            Value::Array(rc) => {
                if tracer.visit_rc(rc) {
                    for elem in rc.iter() {
                        elem.trace(tracer);
                    }
                }
            }

            // Hash — Rc<HashMap<HashKey, Value>>
            Value::Hash(rc) => {
                if tracer.visit_rc(rc) {
                    for value in rc.values() {
                        value.trace(tracer);
                    }
                }
            }
        }
    }
}
```

**Closure:**
```rust
impl Trace for Closure {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.visit_rc(&self.function);  // compiled function (no Value children)
        for free_var in &self.free {
            free_var.trace(tracer);       // trace captured variables
        }
    }
}
```

#### Entry Points

```rust
/// Trace a single value and all transitively reachable values.
pub fn trace_value(v: &Value, tracer: &mut Tracer) {
    v.trace(tracer);
}

/// Trace from multiple roots (stack, globals, etc.).
pub fn trace_roots<'a>(roots: impl Iterator<Item = &'a Value>, tracer: &mut Tracer) {
    for root in roots {
        root.trace(tracer);
    }
}
```

### Module Structure Changes

```
src/runtime/
├── vm/
│   ├── observer.rs       # NEW: VmObserver trait, NoopObserver, TraceObserver
│   ├── mod.rs            # MODIFY: add observer field, generic param, hook calls
│   ├── dispatch.rs       # MODIFY: add on_return hooks at OpReturnValue/OpReturn
│   ├── function_call.rs  # MODIFY: add on_call hooks at execute_call/execute_tail_call
│   ├── trace.rs          # MODIFY: remove trace_instruction (moved to TraceObserver)
│   └── ...               # binary_ops, comparison_ops, index_ops unchanged
├── value_trace.rs        # NEW: Trace trait, Tracer, trace_value, trace_roots
├── mod.rs                # MODIFY: add pub mod value_trace, re-export observer types
└── ...                   # value.rs, closure.rs etc. unchanged
```

## Files Summary

| Action | File | Changes |
|--------|------|---------|
| Create | `src/runtime/vm/observer.rs` | VmObserver trait, NoopObserver, TraceObserver |
| Create | `src/runtime/value_trace.rs` | Trace trait, Tracer, trace_value, trace_roots |
| Create | `tests/vm_observer_tests.rs` | Observer hook tests |
| Create | `tests/value_trace_tests.rs` | Value graph traversal tests |
| Modify | `src/runtime/vm/mod.rs` | Add generic `<O: VmObserver>`, observer field, hook calls, remove `trace: bool` |
| Modify | `src/runtime/vm/dispatch.rs` | Add `<O: VmObserver>`, on_return hooks |
| Modify | `src/runtime/vm/function_call.rs` | Add `<O: VmObserver>`, on_call hooks |
| Modify | `src/runtime/vm/trace.rs` | Add `<O: VmObserver>`, remove/refactor trace_instruction |
| Modify | `src/runtime/vm/binary_ops.rs` | Add `<O: VmObserver>` to impl block |
| Modify | `src/runtime/vm/comparison_ops.rs` | Add `<O: VmObserver>` to impl block |
| Modify | `src/runtime/vm/index_ops.rs` | Add `<O: VmObserver>` to impl block |
| Modify | `src/runtime/mod.rs` | Add `pub mod value_trace`, re-exports |

## Tests

### Part A — VM Observer Tests (`tests/vm_observer_tests.rs`)

| Test | What it verifies |
|------|-----------------|
| **NoopObserver default** | `VM::new(bytecode)` compiles and runs unchanged (backward compat) |
| **on_instruction fires** | Custom observer counts instructions; run simple program; count > 0 |
| **on_call / on_return** | Observer records call/return events; run program with function calls; verify call tree |
| **on_error** | Observer captures error string; run program that errors; verify on_error was called |
| **TraceObserver output** | Run simple program with TraceObserver; verify no panic (output goes to stdout) |
| **Observer with invoke_value** | Higher-order builtin (map/filter) triggers on_call for user callbacks |

### Part B — Value Trace Tests (`tests/value_trace_tests.rs`)

| Test | What it verifies |
|------|-----------------|
| **Primitive values** | Trace Integer/Float/Boolean/None; value_count increments, visited_count stays 0 |
| **Rc-wrapped values** | Trace String/Array/Hash; visited_count matches number of distinct Rc pointers |
| **Diamond sharing** | Two Arrays sharing the same `Rc<Value>` child; trace both; child visited exactly once |
| **Nested containers** | Array of Arrays of Integers; trace_value visits all transitively |
| **Closure free vars** | Build Closure with captured values; trace it; free vars reachable |
| **trace_roots** | Provide multiple root values; tracer visits union of all reachable values |
| **Empty roots** | trace_roots with empty iterator; value_count = 0, visited_count = 0 |

## Design Decisions

### Why Generic VM Instead of `Box<dyn VmObserver>`?

| | Generic `VM<O>` | `Box<dyn VmObserver>` |
|-|----------------|----------------------|
| **Default overhead** | Zero (NoopObserver inlined away) | vtable lookup per instruction |
| **Backward compat** | Yes (default type param) | Yes |
| **Runtime swappable** | No (fixed at construction) | Yes |
| **Code churn** | ~7 impl blocks gain `<O>` | None |

The VM loop runs millions of instructions per second. The generic approach ensures zero overhead for the default case (no observer). Recommended approach.

### Why `on_error(&mut self, err: &str)` Instead of `&VmError`?

Flux runtime errors are currently plain `String` values (no structured error type). Introducing a `VmError` enum would be a separate proposal. The observer receives `&str` to match current conventions.

### Why a Separate `value_trace.rs` Instead of Methods on Value?

Adding `trace()` as an inherent method on `Value` would couple the value type to the tracing infrastructure. A separate `Trace` trait keeps concerns separated and allows the tracer to be an optional import. This also aligns with how GC systems typically work — the trace logic is provided by the GC infrastructure, not embedded in the object types.

### Why `Rc::as_ptr()` for Identity Instead of Value Equality?

Two `Rc` pointers can point to equal-but-distinct allocations. The tracer needs pointer identity (same allocation), not value equality. `Rc::as_ptr()` gives the allocation address, which is used as the `PtrId` for the visited set. This ensures diamond-shared nodes are visited exactly once.

## Future Extensions

- **`on_alloc` hook** — call observer when Rc-wrapped values are created (at `leak_detector::record_*` sites)
- **`on_gc` hook** — if Proposal 017 (GC) is implemented, observer receives collection events
- **Structured `VmError` type** — replace `String` errors with an enum for richer `on_error` callbacks
- **Profiling observer** — timing per opcode category, call graph with durations, hot-path analysis
- **Heap size estimation** — extend `Tracer` to sum approximate byte sizes of visited values

## Verification

```bash
cargo test --test vm_observer_tests                    # New observer tests
cargo test --test value_trace_tests                    # New tracer tests
cargo test --test vm_tests                             # Existing VM tests (no regression)
cargo test --test tail_call_tests                      # Tail call (uses VM)
cargo test                                             # Full suite
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```
