- Feature Name: General Tail-Call Elimination (Mutual Recursion and CPS)
- Start Date: 2026-03-18
- Status: Partially Implemented — direct tail calls (incl. mutual recursion) and LLVM `tail call fastcc` promotion landed; indirect closure tail calls on LLVM remain open
- Proposal PR:
- Flux Issue:
- Depends on: 0016 (self-recursive tail calls), 0104 (Flux Core IR)
- Delivery notes:
  - **Phase 1 (VM general `OpTailCall`)** ✅ — `OpTailCall = 44` + `OpTailCall1` superinstruction in [`src/bytecode/op_code.rs`](../../src/bytecode/op_code.rs); frame-replacement logic in [`src/bytecode/vm/function_call.rs::execute_tail_call`](../../src/bytecode/vm/function_call.rs).
  - **Phase 2 (Core IR / compiler tail-position tracking)** ✅ — `in_tail_position` propagation in [`src/cfg/lower.rs`](../../src/cfg/lower.rs) and [`src/bytecode/compiler/statement.rs`](../../src/bytecode/compiler/statement.rs); CLI `analyze-tail-calls` command.
  - **Phase 3 (Cranelift JIT)** — N/A; Cranelift JIT retired, superseded by [0116 LLVM text-IR backend](implemented/0116_llvm_text_ir_backend.md).
  - **Phase 4 (LLVM `tail call fastcc`)** ✅ for direct calls — [`src/lir/lower.rs::promote_tail_calls`](../../src/lir/lower.rs); snapshot verification in [`src/llvm/ir/ppr.rs`](../../src/llvm/ir/ppr.rs).
  - **Mutual recursion** ✅ — [`tests/flux/mutual_recursion.flx`](../../tests/flux/mutual_recursion.flx) covers 2-way, 3-way, captured variables, accumulators.
  - **CPS via effect handlers** ✅ — tail-resumptive fast path lives in [`src/core/passes/evidence.rs`](../../src/core/passes/evidence.rs); further work tracked under [0162](0162_unified_effect_handler_runtime.md).
- Remaining: **indirect closure tail calls on LLVM** — [`src/lir/lower.rs::promote_tail_calls`](../../src/lir/lower.rs) intentionally excludes `flux_call_closure` dispatch because the indirect calling convention triggers bus errors on Apple clang. Fix requires calling-convention work to allow indirect tail calls across Linux/macOS/Windows toolchains. Tracked for v0.0.9 alongside other perf-closure items (0109/0112).

# Proposal 0107: General Tail-Call Elimination

## Summary

Extend Flux's tail-call optimization from self-recursive calls only (proposal 0016) to full general tail-call elimination covering mutual recursion, indirect calls through closures, and continuation-passing style (CPS). This is required for Flux to reach Koka-level purity, where recursion is the primary looping mechanism and mutual recursion patterns (state machines, parser combinators, interpreters) must not overflow the stack.

## Motivation

### The gap

Proposal 0016 implemented self-recursive tail calls: when `fn f(...)` calls itself in tail position, the VM reuses the frame. This handles the most common case (list traversal accumulators, countdown loops) but leaves three important patterns broken:

**1. Mutual recursion overflows the stack:**

```flux
fn is_even(n: Int) -> Bool {
    if n == 0 { true }
    else { is_odd(n - 1) }  // tail call to a DIFFERENT function
}

fn is_odd(n: Int) -> Bool {
    if n == 0 { false }
    else { is_even(n - 1) }  // tail call to a DIFFERENT function
}

// is_even(100000) → stack overflow
```

This is a textbook mutual recursion that every pure FP language handles in constant stack space. Haskell (via STG machine), Koka (via evidence-passing compilation), and OCaml (via native tail calls) all optimize this. Flux currently cannot.

**2. Indirect tail calls through closures:**

```flux
fn trampoline(f: (Int) -> Int with |e, n: Int) -> Int with |e {
    f(n)  // tail call through a closure — not optimized
}
```

Higher-order tail calls are essential for CPS transforms, effect handler compilation, and callback-driven state machines. Without them, effect handlers that resume deeply produce O(n) stack growth.

**3. CPS-style effectful code:**

```flux
fn fold_with_early_exit(xs: List<Int>, acc: Int) -> Int with Abort {
    match xs {
        [] -> acc,
        [h | t] -> if h < 0 { perform Abort.abort(acc) }
                   else { fold_with_early_exit(t, acc + h) }  // self-recursive: OK
    }
}
```

Self-recursive CPS works today, but a refactored version using a helper function would not:

```flux
fn process(x: Int, acc: Int) -> Int with Abort {
    if x < 0 { perform Abort.abort(acc) }
    else { acc + x }
}

fn fold_cps(xs: List<Int>, acc: Int) -> Int with Abort {
    match xs {
        [] -> acc,
        [h | t] -> fold_cps(t, process(h, acc))  // self-recursive: OK
    }
}
```

But a state-machine-style decomposition would fail:

```flux
fn step(state: State, input: List<Int>) -> Result with Abort {
    match state {
        Running(acc) -> process_running(acc, input),  // mutual: NOT optimized
        Done(result) -> Ok(result),
    }
}

fn process_running(acc: Int, input: List<Int>) -> Result with Abort {
    match input {
        [] -> step(Done(acc), []),        // mutual: NOT optimized
        [h | t] -> step(Running(acc + h), t),  // mutual: NOT optimized
    }
}
```

### Why this matters for pure FP

In a pure functional language, recursion replaces loops. If mutual recursion overflows the stack, programmers are forced into imperative workarounds (explicit loop constructs, trampolining libraries) that contradict the language's pure FP identity. Every reference language in Flux's design space handles this:

| Language | Mutual TCO | Mechanism |
|----------|-----------|-----------|
| Haskell  | Yes | STG machine: all calls are tail calls to known entry points |
| Koka     | Yes | Evidence-passing compilation, CPS transform for effect handlers |
| OCaml    | Yes | Native code backend emits tail call instructions |
| Scheme   | Yes | R7RS requires proper tail calls for all tail positions |
| Flux     | **No** | Only self-recursive OpTailCall |

## Guide-level explanation

After this proposal, any call in tail position is optimized, regardless of the callee:

```flux
// Mutual recursion works in constant stack space
fn is_even(n: Int) -> Bool {
    if n == 0 { true }
    else { is_odd(n - 1) }  // optimized: no new frame
}

fn is_odd(n: Int) -> Bool {
    if n == 0 { false }
    else { is_even(n - 1) }  // optimized: no new frame
}

// is_even(1000000) → true (no stack overflow)
```

The optimization is transparent — users do not need to annotate anything. The compiler detects tail position using the same rules as today (last expression in function body, both branches of `if`, all arms of `match`) but now emits the optimization for all callees, not just self-calls.

### What counts as a tail call

A call `f(args...)` is in tail position if:
1. It is the last expression evaluated before the enclosing function returns.
2. No work remains after the call (no `+`, no wrapping in a constructor, no `let` binding of the result that is then used).
3. The call is not inside a `try`/`handle` block that would need to intercept the callee's effects.

Rule 3 is new: effect handlers install continuation marks that must remain on the stack. A tail call inside a `handle` block cannot eliminate the handler's frame. This matches Koka's behavior.

### Diagnostics

When `--trace` is active, tail-call-eliminated frames are shown as `[TCE]` in the trace output:

```
[TCE] is_even(4) → is_odd(3)
[TCE] is_odd(3) → is_even(2)
[TCE] is_even(2) → is_odd(1)
[TCE] is_odd(1) → is_even(0)
      is_even(0) → true
```

## Reference-level explanation

### Phase 1: Bytecode VM — general `OpTailCall`

**Current state:** `OpTailCall` in `src/bytecode/op_code.rs` is emitted only when the compiler detects a self-recursive call. The VM dispatch in `src/runtime/vm/dispatch.rs` handles it by reusing the current frame.

**Change:** Generalize `OpTailCall` to work for any callee:

1. **Compiler change** (`src/bytecode/compiler/expression.rs`): In `compile_call`, when `in_tail_position` is true, emit `OpTailCall` regardless of whether the callee is the enclosing function. Remove the self-call check.

2. **VM change** (`src/runtime/vm/dispatch.rs`): When executing `OpTailCall`:
   - Pop the arguments from the stack.
   - If the callee is the current function (self-call): reuse frame as today.
   - If the callee is a different `CompiledFunction`: deallocate the current frame, push a new frame for the callee with the tail-call arguments, and jump to the callee's entry point. Net frame count: unchanged.
   - If the callee is a `Closure`: extract the closure's function and captured environment, deallocate the current frame, push a new frame with the closure's captures and the tail-call arguments.
   - If the callee is a `BaseFunction`: execute normally (base functions are leaf calls; no frame savings).

3. **Frame transition protocol:**
   ```
   Before:  [caller_frame] [callee args on stack]
   After:   [callee_frame with args installed]
   ```
   The key invariant: the caller's frame is removed before the callee's frame is pushed, so the frame stack does not grow.

### Phase 2: Core IR — tail call annotation

**Change** (`src/core/mod.rs`): Add a `tail_call: bool` field to `CoreExpr::App`:

```rust
CoreExpr::App {
    func: Box<CoreExpr>,
    args: Vec<CoreExpr>,
    tail_call: bool,  // NEW
    span: Span,
}
```

The `lower_ast` pass sets `tail_call = true` using the existing `TailPositionAnalyzer` from `src/ast/tail_position.rs`.

The `to_ir` pass propagates the `tail_call` annotation to the backend IR, enabling the Cranelift and LLVM backends to emit tail call instructions natively.

### Phase 3: Cranelift JIT — native tail calls

Cranelift supports tail calls via `call_indirect` with the `tail` calling convention. After Phase 2, the JIT compiler (`src/jit/compiler.rs`) emits `return_call` / `return_call_indirect` for annotated tail calls instead of `call` + `return`.

### Phase 4: LLVM backend — musttail

The LLVM backend (`src/llvm/compiler/`) emits `musttail call` for annotated tail calls. LLVM guarantees that `musttail` calls reuse the caller's stack frame on all targets.

### Interaction with effect handlers

A call inside a `handle` block is **not** in tail position if the handler needs to remain on the stack. The tail position analysis must be updated:

```flux
fn example() -> Int {
    // This is NOT a tail call — the handler must intercept effects
    foo() handle Console {
        print(resume, msg) -> resume(())
    }
}

fn example2() -> Int with Console {
    // This IS a tail call — no handler boundary
    foo()
}
```

The rule: a call `f(args)` is in tail position only if there is no enclosing `handle` block between the call and the function's return point.

**Implementation:** The `TailPositionAnalyzer` gains a `handle_depth: usize` counter. When entering a `handle` expression, increment it. When leaving, decrement. A call is tail-eligible only when `handle_depth == 0`.

### Interaction with effect continuations

When an effect handler resumes a continuation (`resume(value)`), the resumed computation returns to the handler. This continuation frame must not be eliminated. The `resume` call is never in tail position from the handler's perspective.

However, `resume` in tail position of a handler arm can be optimized to avoid allocating a new continuation frame — this is the "tail-resumptive" optimization from the Koka literature. This is a future possibility, not part of this proposal.

## Drawbacks

1. **Debugging complexity:** General TCE removes frames from the call stack, making stack traces less informative. Mitigation: the `--trace` flag shows `[TCE]` markers, and a `--no-tco` flag (mentioned in 0016's open questions) disables all tail-call optimization for debugging.

2. **VM dispatch complexity:** The current `OpTailCall` handler is simple (reuse frame for self-call). General tail calls require frame deallocation and reallocation for a different function, which is more complex and has more edge cases (different arity, captured variables).

3. **Effect handler interaction:** The `handle_depth` check adds a condition to tail-position analysis. Getting this wrong could either miss optimization opportunities or break effect handler semantics.

## Rationale and alternatives

### Why not trampolining?

A library-level trampoline (returning thunks that are re-invoked in a loop) works but:
- Requires users to restructure their code (return thunks instead of direct calls).
- Allocates a thunk on every "tail call" — O(1) stack but O(n) heap allocations.
- Destroys the functional programming ergonomics that Flux aims for.

General TCE is transparent — the programmer writes natural recursive code and the compiler handles it.

### Why not CPS transform the entire program?

Whole-program CPS (as in SML/NJ) eliminates all stack usage. This is a much larger transformation that changes the compilation model fundamentally. It is disproportionate for the problem: most Flux code does not need CPS, and selective tail-call elimination at call sites is simpler and sufficient.

### Why not limit to known-function mutual calls?

We could detect mutual recursion groups (SCCs in the call graph) and only optimize tail calls within those groups. This would handle the `is_even`/`is_odd` case but miss indirect calls through closures. Since Flux is a functional language where higher-order functions are pervasive, indirect tail calls matter.

## Prior art

- **Scheme (R7RS):** Mandates proper tail calls. All implementations must handle arbitrary tail calls in constant space. This is the gold standard.
- **Haskell (GHC):** The STG machine compiles all function applications as jumps to entry points. Tail calls are a natural consequence of the evaluation model.
- **Koka:** Uses evidence-passing compilation for effect handlers. Tail calls are optimized except through handler boundaries, same restriction as this proposal.
- **OCaml:** The native code compiler emits tail call instructions. OCaml 5's effect handlers interact with tail calls similarly to this proposal's `handle_depth` approach.
- **Lua 5.x:** Supports proper tail calls in the bytecode VM via frame replacement, similar to the Phase 1 approach described here.

## Unresolved questions

1. **Arity mismatch:** When a tail call targets a function with different arity than the caller, the frame slot count changes. Should the VM reallocate the frame or use a maximum-arity frame pool? Need benchmarking.

2. **Closure captures in tail calls:** When tail-calling a closure, the caller's frame is replaced. If the closure captures values from the caller, those captures must be preserved. The implementation must copy captures before deallocating the caller frame.

3. **`--no-tco` granularity:** Should `--no-tco` disable all TCE or just mutual/indirect TCE while keeping self-recursive TCE? The latter is safer for debugging while preserving basic recursion support.

4. **Tail-resumptive optimization:** Should this proposal include the tail-resumptive handler optimization, or defer it? The Koka evidence-passing paper shows significant performance gains from this, but it adds complexity.

## Future possibilities

- **Tail-resumptive handlers:** Optimize `resume(value)` in tail position of a handler arm to avoid continuation allocation. This is the main performance optimization for algebraic effects.
- **Guaranteed tail calls:** A `@tailcall` annotation that produces a compile error if the annotated call is not in tail position. Helps programmers verify their expectations.
- **Tail call statistics:** `--stats` could report how many tail calls were eliminated per function, helping users optimize their recursion patterns.
- **CPS transform for effect handlers:** Full CPS compilation of effect handlers (as in Koka) could subsume this proposal's approach, but is a much larger undertaking.
