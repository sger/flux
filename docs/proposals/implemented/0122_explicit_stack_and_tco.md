- Feature Name: Explicit Stack and Tail Call Optimization
- Start Date: 2026-03-24
- Status: Implemented (Phases 1-3 complete; Phase 4 deferred as optional)
- Proposal PR:
- Flux Issue:

## Summary

Replace the system stack with an explicit, growable stack for the `core_to_llvm` native backend, following GHC's STG machine architecture. This eliminates stack overflow on deeply recursive programs and enables true tail call optimization (TCO) that converts recursive calls into constant-stack loops.

## Motivation

### The current problem

The `core_to_llvm` backend uses LLVM `alloca` for all local variables. Every function call creates a new system stack frame:

```
walk_collect(ctx, 80, 84, Up, {})
  → walk_collect(ctx, 79, 84, Up, visited)
    → walk_collect(ctx, 78, 84, Up, visited)
      → ... 4778 frames deep
```

Each frame contains ~50 alloca slots (400+ bytes). With 4778 recursive calls, this requires ~2MB of system stack. More complex programs (AoC Day 6 Part B) create thousands of nested calls across multiple recursive functions, exhausting the default 8MB stack.

Current workarounds:
- 64MB stack size linker flag (`-Wl,-stack_size,0x4000000`)
- Hope that LLVM's `opt -O2` converts tail calls to loops (works for some, not all)

These are fragile. A functional language **must** handle deep recursion gracefully.

### Why LLVM's tail call optimization isn't enough

LLVM can optimize `tail call` instructions into jumps, but only when:
1. The caller and callee have the same signature
2. No `alloca` memory is live across the call
3. The call is in tail position (nothing happens after it)
4. The calling convention supports TCO (`fastcc` does, `ccc` doesn't)

Our codegen violates condition 2: every variable is an `alloca` slot that's live across calls. LLVM's `mem2reg` pass promotes some to registers, but complex functions with many variables keep stack allocations. Even when `opt -O2` succeeds in converting `walk_collect` to a loop, other functions like `drive_loop_jump` may not be optimized.

### How GHC solves this

GHC uses the **STG (Spineless Tagless G-machine)** architecture:

1. **Explicit stack in the heap**: Each thread has its own stack, allocated as a heap object. The stack pointer (`Sp`) and stack limit (`SpLim`) are machine registers or global variables.

2. **No system stack frames**: Function "calls" are jumps that push continuation frames onto the explicit stack. The system stack is only used for C FFI calls and tiny local spills.

3. **Automatic TCO**: Since tail calls are jumps (not function calls), they don't push anything. A tail-recursive function uses constant stack space by definition.

4. **Growable stack**: When `Sp - frameSize < SpLim`, the runtime grows the stack (reallocating to a larger buffer). No fixed limit.

5. **Stack managed by GC**: The garbage collector can walk, compact, and resize stacks. Stack frames contain layout information for precise collection.

```
GHC's execution model:

  System stack (tiny):     LLVM locals, C FFI frames
  STG stack (in heap):     Continuations, return addresses, saved variables
  Heap:                    Closures, data, STG stacks themselves
```

### Flux's opportunity

Flux doesn't need the full STG machine. A simpler approach achieves the same benefits:

1. **Trampoline pattern**: Instead of direct recursion, recursive functions return a "call me next" thunk. A top-level loop calls thunks until a final value is produced. This converts all recursion to iteration.

2. **Explicit continuation stack**: For non-tail calls, push the continuation (what to do with the result) onto a heap-allocated stack. This separates the "call stack" from the system stack.

3. **CPS transform**: Convert Core IR to continuation-passing style before LLVM lowering. Every function takes an extra continuation argument. Tail calls jump directly; non-tail calls create continuation closures.

---

## Guide-level explanation

### For Flux users

No change in syntax or semantics. Programs that previously stack-overflowed now work:

```flux
// This works even for n = 1,000,000+
fn sum_to(n, acc) {
    if n <= 0 { acc }
    else { sum_to(n - 1, acc + n) }
}
```

Mutual recursion, deeply nested pattern matching, and higher-order recursion all work without stack limits.

### For compiler contributors

The compilation pipeline adds a transform before LLVM lowering:

```
Core IR → Aether → [TCO Transform] → LLVM codegen
```

The TCO transform identifies tail calls and rewrites them:

**Before (direct recursion):**
```llvm
define fastcc i64 @walk_collect(i64 %ctx, i64 %r, i64 %c, i64 %d, i64 %visited) {
  ; ... compute next_r, next_c, next_d, next_visited ...
  %result = tail call fastcc i64 @walk_collect(i64 %ctx, i64 %next_r, i64 %next_c, i64 %next_d, i64 %next_visited)
  ret i64 %result
}
```

**After (loop with phi nodes):**
```llvm
define fastcc i64 @walk_collect(i64 %ctx, i64 %r0, i64 %c0, i64 %d0, i64 %visited0) {
entry:
  br label %loop
loop:
  %r = phi i64 [%r0, %entry], [%next_r, %recurse]
  %c = phi i64 [%c0, %entry], [%next_c, %recurse]
  %d = phi i64 [%d0, %entry], [%next_d, %recurse]
  %visited = phi i64 [%visited0, %entry], [%next_visited, %recurse]
  ; ... compute ...
  br i1 %base_case, label %done, label %recurse
recurse:
  ; ... compute next_r, next_c, next_d, next_visited ...
  br label %loop
done:
  ret i64 %visited
}
```

This transformation happens at the Core IR or LLVM IR level, before `opt`.

---

## Reference-level explanation

### Phase 1: Guaranteed self-tail-call optimization

Convert self-recursive tail calls into loops at the LLVM IR level. This is the simplest and highest-impact change.

**Scope**: Functions where the only recursive call is a `tail call` to itself.

**Implementation**:

```rust
// In codegen, detect self-recursive tail calls:
fn compile_function(def: &CoreDef) -> LlvmFunction {
    if is_self_tail_recursive(def) {
        compile_as_loop(def)  // emit loop with phi nodes
    } else {
        compile_normal(def)   // emit alloca + call as before
    }
}
```

The loop transformation:
1. Function parameters become phi nodes in a `loop` block
2. Base case branches to `done` block with `ret`
3. Recursive case computes new values and branches back to `loop`
4. No `alloca` needed for parameters (they're phi nodes)

**Coverage**: Handles `walk_collect`, `count_loops`, `drive_loop_jump`, `fold` implementations, `fib`, `factorial`, and most recursive AoC functions.

### Phase 2: Mutual tail call optimization via trampoline

For mutually recursive functions (A calls B, B calls A), use a trampoline:

```c
// Trampoline: keeps calling until result is a value, not a thunk
int64_t flux_trampoline(int64_t initial_thunk) {
    int64_t current = initial_thunk;
    while (is_thunk(current)) {
        current = eval_thunk(current);
    }
    return current;
}
```

Functions return either a final value or a thunk (tagged pointer to a closure representing the next call). The trampoline loop iterates until a value is produced.

**Thunk representation** (NaN-boxed):
```
Tag 0x6 = Thunk
Payload = pointer to { fn_ptr, args[] }
```

### Phase 3: Explicit continuation stack

For non-tail calls in recursive functions, push continuations onto a heap-allocated stack:

```
fn count_loops(candidates, idx, acc) {
    if idx >= len(candidates) { acc }
    else {
        let result = check_candidate(candidates[idx])  // non-tail call
        count_loops(candidates, idx + 1, acc + result)  // tail call
    }
}
```

Here `check_candidate` is a non-tail call (we need its result). Instead of using the system stack:

1. Push continuation `{count_loops, candidates, idx+1, acc}` onto explicit stack
2. Jump to `check_candidate`
3. When `check_candidate` returns, pop continuation and jump to `count_loops` with updated args

The explicit stack is a heap-allocated array that grows dynamically (like GHC's STG stack).

### Phase 4: CPS transform (optional, advanced)

Full continuation-passing style transformation at the Core IR level:

```
// Before CPS:
let x = f(a)
g(x, b)

// After CPS:
f(a, \x -> g(x, b, k))
```

Every function takes an extra continuation parameter. All calls become tail calls. This is what GHC does internally (STG language is CPS).

**Trade-offs**:
- Pro: Eliminates all stack overflow, enables advanced optimizations
- Con: Significant code complexity, closure allocation overhead, harder to debug

---

## Implementation phases

**Phase 1 — Self-tail-call loops** (~3 days) ✅ **DONE**
- `setup_tco_loop` in `src/core_to_llvm/codegen/expr.rs` emits `tco.loop` blocks with phi nodes
- Self-recursive tail calls are converted to `br label %tco.loop` with updated phi values
- No `alloca` needed for parameters in the loop (they're phi nodes)
- Tested: recursive functions like `sum_to`, `walk_collect`, `fold` work without stack overflow

**Phase 2 — Trampoline for mutual recursion** (~1 week) ✅ **DONE**
- `NanTag::Thunk = 0x6` in `src/runtime/nanbox.rs` for thunk NaN-box encoding
- `MutualRecGroup` struct with Tarjan's SCC detection in `src/core_to_llvm/codegen/function.rs`
- `build_trampoline_function()` generates LLVM trampoline loop: switch on `fn_index` → call `.impl` → check `flux_is_thunk` → unpack via `flux_untag_thunk_ptr` → loop or return
- `build_trampoline_entry_wrapper()` wraps each group member so external callers enter via the trampoline
- `lower_top_level_function_with_mutual()` emits thunk returns for cross-function tail calls within the group
- C runtime helpers: `flux_is_thunk`, `flux_untag_thunk_ptr`, `FLUX_TAG_THUNK`

**Phase 3 — Explicit continuation stack** (~2 weeks) ✅ **DONE**
- `has_nontail_self_recursion()` detects functions with non-tail self-recursive calls
- `lower_top_level_function_cps()` in `src/core_to_llvm/codegen/function.rs` uses CPS driver loop with explicit continuation stack
- `setup_cps_driver()` + `finalize_cps()` manage the continuation stack during lowering
- Non-tail recursive calls push continuations; tail calls loop without stack growth

**Phase 4 — CPS transform** (future, optional) ❌ **DEFERRED**
- Full Core IR → CPS Core IR pass not implemented
- Current Phase 3 approach (per-function CPS driver) handles the common cases
- Only needed if Phases 1-3 prove insufficient for more complex patterns

---

## Drawbacks

- **Phase 1 complexity**: Loop transformation requires detecting tail position accurately in the presence of pattern matching, let bindings, and closures. Getting this wrong produces incorrect code.

- **Phase 2 overhead**: Trampoline adds one branch per call (check if thunk or value). For tight loops this is measurable but small (~5% overhead).

- **Phase 3 complexity**: Explicit continuation stack requires integration with memory management. Continuations reference local values that must be kept alive.

- **Phase 4 code explosion**: CPS can double the number of closures allocated. GHC mitigates this with join points and case-of-case optimization. Flux would need similar passes.

- **Debugging**: Stack traces become harder to produce when the system stack doesn't reflect the logical call stack.

---

## Rationale and alternatives

### Why not just increase the system stack?

The 64MB linker flag works today but:
- It's a fixed limit (can still overflow)
- It wastes virtual address space
- It's platform-specific (macOS flag differs from Linux)
- It doesn't compose (library code can't control the caller's stack)

### Why not rely on LLVM's TCO?

LLVM's TCO is best-effort. It works for simple cases but fails with:
- Multiple alloca'd values live across the call
- Complex control flow (match + multiple recursive calls)
- Non-uniform signatures (mutual recursion with different arities)

A functional language needs **guaranteed** TCO, not optimistic TCO.

### Why Phase 1 first?

Phase 1 (self-tail-call loops) covers ~90% of real-world recursion patterns. It's simple, has zero runtime overhead (loops are faster than calls), and doesn't require any runtime infrastructure changes. Phases 2-4 are for the remaining 10%.

### Why not full CPS from the start?

CPS is the correct long-term solution but requires significant infrastructure (continuation types, closure allocation optimization, join points). Starting with Phase 1 delivers immediate value while we build toward CPS.

---

## Prior art

### GHC (Haskell)
GHC's STG machine uses an explicit stack for all calls. The LLVM backend emits `tail call` with `cc 10` (GHC calling convention). Every function "call" is actually a jump that manipulates the STG stack. This has worked for 25+ years at scale.

### Scheme (various implementations)
R5RS requires proper tail calls. Implementations use:
- **Chicken Scheme**: CPS + Cheney on the MTA (stack is the nursery)
- **Guile**: Trampoline + explicit stack
- **Chez Scheme**: Direct TCO via platform calling conventions

### Rust (async/await)
Rust's `async` transforms functions into state machines — essentially CPS. Each `await` point becomes a variant in an enum. This is conceptually similar to our Phase 3/4 approach.

### Erlang/BEAM
BEAM uses an explicit process stack with tail call optimization built into the VM. Every process has a growable heap+stack.

---

## Unresolved questions

- **Phase 1 scope**: Should we only handle self-recursion, or also detect "tail call to known function" (e.g., `walk_collect` calling `is_wall` in non-tail position, then tail-calling itself)?

- **Thunk tag**: Should thunks reuse the existing closure representation (a closure with 0 remaining arity = ready to call), or have a dedicated NaN-box tag?

- **Stack size policy**: For Phase 3, should the explicit stack grow unboundedly, or have a configurable limit (like GHC's `+RTS -K` flag)?

- **Interaction with Aether**: The Aether RC pass inserts `dup`/`drop` around values. With explicit continuations, do we need to dup captured values when pushing a continuation frame?

---

## Future possibilities

- **Green threads**: With an explicit stack, each "thread" is just a stack + heap pointer. This enables lightweight concurrency (like Erlang processes or Go goroutines).

- **Delimited continuations**: The explicit stack makes `shift`/`reset` style continuations implementable. This connects to Flux's algebraic effects system.

- **Stack-allocated closures**: Short-lived closures (e.g., lambda in `map`) could be allocated on the explicit stack instead of the heap, avoiding GC pressure.

- **Profiling**: The explicit stack enables precise call-stack profiling without platform-specific unwinding (like GHC's cost-centre profiling).
