# Proposal 016: Tail-Call Accumulator Optimization

**Status:** Complete
**Priority:** Medium (Performance)
**Created:** 2026-02-08
**Related:** Proposal 010 (GC), Proposal 019 (Zero-Copy Value Passing)
**Implementation Order:** 019 → 016 (this) → 017 → 018

## Overview

Introduce a two-phase optimization that (1) eliminates stack overflow for self-recursive tail calls by reusing frames, and (2) eliminates O(n^2) array copying in accumulator patterns by allowing the VM to mutate arrays in-place when the compiler can prove the source binding is dead.

No GC is required. Immutable semantics are fully preserved.

---

## Goals

1. Prevent stack overflow for deep self-recursion (Phase 1).
2. Reduce O(n^2) array accumulation to O(n) (Phase 2).
3. Preserve immutable semantics — the optimization must be invisible to the user.
4. Keep changes backward-compatible — existing bytecode without tail calls runs unchanged.

### Non-Goals

1. Mutual tail-call optimization (two or more functions calling each other in tail position).
2. General move semantics or linear types at the language level.
3. Garbage collection (this proposal is complementary to Proposal 010, not a replacement).

---

## Problem Statement

### Problem 1: Stack Overflow

Every `OpCall` pushes a new `Frame` onto the frame stack. A self-recursive function that recurses 2000+ times overflows the 2048-slot stack:

```flux
fn countdown(n) {
    if n == 0 { 0; }
    else { countdown(n - 1); }
}

countdown(10000);  // stack overflow
```

This call is in tail position — the current frame is not needed after the call returns. Reusing it instead of pushing a new one would allow unbounded recursion depth.

### Problem 2: O(n^2) Array Accumulation

`builtin_push` always clones the entire array:

```rust
let mut new_arr = arr.clone();  // O(n) clone
new_arr.push(args[1].clone());
Ok(Object::Array(new_arr))
```

The common accumulator pattern:

```flux
fn build(n, acc) {
    if n == 0 { acc; }
    else { build(n - 1, push(acc, n)); }
}

build(10000, []);
```

Each step clones `acc` (growing by one each time), yielding O(1 + 2 + ... + n) = O(n^2) total copies. For n = 10,000 this is ~50 million element copies.

If the compiler can determine that `acc` is dead after the `push` call, the VM can mutate the array in-place, reducing cost to O(n).

---

## Proposed Design

### Phase 1: Self-Recursive Tail Call Elimination

#### New Opcode: `OpTailCall`

Add `OpTailCall = 44` with the same 1-byte operand as `OpCall` (argument count).

#### Compiler: Tail Position Detection

Add an `in_tail_position: bool` flag to the `Compiler`. Set it to `true` before compiling the last expression of a function body. Propagate it through `if`/`else` branches and `match` arms. When `in_tail_position` is true and the call target resolves to `SymbolScope::Function` (self-call via `OpCurrentClosure`), emit `OpTailCall` instead of `OpCall`.

#### VM: Frame Reuse

`OpTailCall` handler:

```rust
fn tail_call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
    let bp = self.current_frame().base_pointer;

    // Copy new arguments over old locals
    for i in 0..num_args {
        self.stack[bp + i] = self.stack[self.sp - num_args + i].clone();
    }

    // Reset stack and instruction pointer
    self.sp = bp + closure.function.num_locals;
    self.current_frame_mut().ip = 0;
    self.current_frame_mut().closure = closure;
    Ok(())
}
```

No new frame is allocated. The current frame's arguments are overwritten and `ip` resets to 0.

#### What Phase 1 Optimizes

```flux
// OPTIMIZED: self-recursive tail call
fn factorial(n, acc) {
    if n == 0 { acc; }
    else { factorial(n - 1, acc * n); }
}

// NOT OPTIMIZED: not in tail position (multiplication after call)
fn factorial_bad(n) {
    if n == 0 { 1; }
    else { n * factorial_bad(n - 1); }
}

// NOT OPTIMIZED: mutual recursion (future work)
fn is_even(n) { if n == 0 { true; } else { is_odd(n - 1); } }
fn is_odd(n) { if n == 0 { false; } else { is_even(n - 1); } }
```

Phase 1 fixes stack overflow but does NOT fix the O(n^2) array copy.

---

### Phase 2: Uniqueness-Aware Array Reuse

#### New Opcode: `OpConsumeLocal`

Add `OpConsumeLocal = 45` with a 1-byte operand (local index). Semantics: move the value from the local slot onto the stack AND replace the local slot with `Object::None`.

```rust
OpCode::OpConsumeLocal => {
    let idx = read_u8(instructions, ip + 1) as usize;
    let bp = self.current_frame().base_pointer;
    let value = std::mem::replace(&mut self.stack[bp + idx], Object::None);
    self.push(value)?;
}
```

The difference from `OpGetLocal`: `std::mem::replace` instead of `clone()`. The stack slot no longer holds a reference to the array, making the pushed value the unique owner.

#### Compiler: Liveness Analysis (Minimal)

For self-recursive tail calls, emit `OpConsumeLocal` instead of `OpGetLocal` when:

1. The local is a parameter of the current function.
2. It is NOT in `SymbolTable.free_symbols` (not captured by a closure).
3. It appears in the tail-call argument list and is not read afterward.

This is a conservative analysis that handles the common accumulator pattern.

#### Builtin Ownership Refactor

Modify `builtin_push` to take ownership instead of cloning:

```rust
pub(super) fn builtin_push(mut args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "push", "push(arr, elem)")?;
    let elem = args.swap_remove(1);
    let arr_obj = args.swap_remove(0);
    match arr_obj {
        Object::Array(mut arr) => {
            arr.push(elem);
            Ok(Object::Array(arr))
        }
        other => Err(type_error(...)),
    }
}
```

Since `args: Vec<Object>` is already owned, `swap_remove` moves the value out without cloning. Combined with `OpConsumeLocal` (which zeroed the local slot), the array `Vec` has exactly one owner — mutation is safe.

Same treatment applies to `builtin_concat`, `builtin_reverse`, `builtin_sort`.

#### Bytecode Comparison

Before:
```
OpGetBuiltin 5         # load push
OpGetLocal 1           # load acc (CLONE — slot keeps a copy)
OpGetLocal 0           # load n
OpCall 2               # push(acc, n) — clones again inside builtin
OpCall 2               # build(n-1, result)
OpReturnValue
```

After:
```
OpGetBuiltin 5         # load push
OpConsumeLocal 1       # MOVE acc (slot zeroed, no clone)
OpGetLocal 0           # load n
OpCall 2               # push(acc, n) — mutates in-place
OpTailCall 2           # reuse frame
```

#### What Phase 2 Optimizes

```flux
// OPTIMIZED (O(n)): acc is dead after push
fn build(n, acc) {
    if n == 0 { acc; }
    else { build(n - 1, push(acc, n)); }
}

// OPTIMIZED (O(n)): same pattern with concat
fn flatten(lists, acc) {
    if len(lists) == 0 { acc; }
    else { flatten(rest(lists), concat(acc, first(lists))); }
}

// NOT OPTIMIZED: acc captured by closure
fn build_closure(n, acc) {
    let f = fn() { acc; };
    if n == 0 { f(); }
    else { build_closure(n - 1, push(acc, n)); }
}
```

---

## Acceptance Criteria

### Phase 1

1. `countdown(1_000_000)` runs without stack overflow.
2. Compiler emits `OpTailCall` for self-recursive calls in tail position.
3. Tail position propagates through `if`/`else` and `match` branches.
4. Non-tail calls remain `OpCall` with correct semantics.
5. All existing tests pass.

### Phase 2

1. `build(100_000, [])` completes in O(n) time (benchmarked).
2. `OpConsumeLocal` emitted for dead locals; `OpGetLocal` for captured locals.
3. `builtin_push`/`concat` take ownership instead of cloning.
4. Closure-captured accumulators are NOT mutated (correctness test).
5. All snapshot tests pass unchanged.

---

## Implementation Checklist

### Phase 1: TCE

1. Add `OpTailCall = 44` to `OpCode` enum, operand widths, `From<u8>`, `Display`
2. Add `in_tail_position: bool` to `Compiler` struct
3. Propagate tail position through `compile_function_literal`, `compile_if_expression`, `compile_match_expression`
4. Emit `OpTailCall` when `in_tail_position && callee == SymbolScope::Function`
5. Add `execute_tail_call()` / `tail_call_closure()` to VM
6. Add `OpTailCall` handler to `dispatch_instruction()`
7. Add integration test: deep recursion (n = 100,000) without overflow
8. Update bytecode cache version
9. Update `--trace` output for tail-call events

### Phase 2: Array Reuse

1. Add `OpConsumeLocal = 45` to `OpCode` enum, operand widths, `From<u8>`, `Display`
2. Add `OpConsumeLocal` handler to `dispatch_instruction()` using `std::mem::replace`
3. Add liveness check: local not in `free_symbols` and dead after tail-call
4. Emit `OpConsumeLocal` for dead locals in tail-call argument expressions
5. Refactor `builtin_push`, `builtin_concat`, `builtin_reverse`, `builtin_sort` to use `swap_remove`
6. Add compiler test: `OpConsumeLocal` emitted / not emitted appropriately
7. Add benchmark: `build(n, [])` linear vs quadratic scaling

---

## Risks

| Risk | Mitigation |
|------|------------|
| Incorrect tail-position detection | Start with self-calls only; extensive test coverage |
| Frame reuse corrupts arguments | Copy all new args before overwriting old locals |
| Liveness marks a live variable as dead | Conservative: only parameters, check `free_symbols` |
| `builtin_push` ownership change breaks callers | Signature `fn(Vec<Object>)` already transfers ownership |
| Bytecode cache incompatibility | Bump cache version number |

---

## Open Questions

1. **`--no-tco` flag?** Useful for debugging. Compiler emits `OpCall` even in tail position.
2. **Extend to `rest(arr)`?** Same ownership principle — `drain(1..)` instead of `to_vec()`. Future extension.
3. **Liveness beyond parameters?** Let-bound locals used exactly once in tail-call args could also be consumed. Requires use-count tracking in symbol table. Future enhancement.
4. **`CompiledFunction.has_tail_calls` flag?** Skip tail-call dispatch logic for functions that don't use it. Minor optimization; defer unless profiling shows overhead.
5. **Interaction with Proposal 010 (GC)?** Complementary. `OpConsumeLocal` drops references earlier, making objects eligible for collection sooner.
