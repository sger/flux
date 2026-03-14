- Feature Name: Effect Handler Optimizations
- Start Date: 2026-03-14
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0042 (effect rows), 0049 (effect rows completeness)
- Supersedes: 0072 (evidence passing), 0073 (state/reader continuation elim)

# Proposal 0101: Effect Handler Optimizations

## Summary
[summary]: #summary

An incremental three-phase optimization plan for Flux's algebraic effect handlers,
progressing from low-risk additive optimizations to a potential compilation model
change. Each phase delivers measurable performance improvements independently.

- **Phase 1**: Tail-resumptive handler detection + direct dispatch (no continuation allocation)
- **Phase 2**: Static handler resolution (compile-time handler binding when handler and perform are co-visible)
- **Phase 3** (deferred): Evidence passing compilation model (requires typed CFG IR)

This proposal replaces proposals 0072 and 0073, which jumped directly to evidence
passing without the incremental foundation. The phased approach delivers wins sooner
with less risk.

## Motivation
[motivation]: #motivation

Flux's current effect handler mechanism (`OpHandle` / `OpPerform`) allocates a
heap continuation on every `perform` call. This captures the entire call stack
above the handler boundary into a `Vec<Frame>`:

```rust
// Current OpPerform path (dispatch.rs):
let mut captured_frames: Vec<Frame> =
    self.frames[entry_frame_index + 1..=self.frame_index].to_vec();
```

For the most common effect patterns — State, Reader, Writer, logging, IO — the
continuation is immediately resumed and discarded. The allocation is wasted work.

**Cost per `perform` call (current)**:
- `Vec<Frame>` heap allocation (proportional to call depth above handler)
- Frame copy (memcpy of all captured frames)
- GC pressure from short-lived `Continuation` values
- Handler stack linear search

For a program doing 1M state operations (get/set), this means ~2M unnecessary
allocations. In benchmarks, effect-heavy Flux code is 10-50x slower than equivalent
non-effectful code purely due to this overhead.

### Use cases that benefit

| Pattern | Example | Frequency |
|---------|---------|-----------|
| State threading | `get()`, `set(v)` in loops | Very common |
| Environment/config | `ask()` for config values | Very common |
| Logging/tracing | `log(msg)` | Common |
| IO wrapping | `print(x)` via effect | Common |
| Non-determinism | `amb()` with backtracking | Rare (needs full continuations) |
| Exceptions | `raise(e)` with non-resumption | Occasional |

Phases 1-2 optimize the "very common" and "common" cases (90%+ of real usage)
while preserving full continuation support for the rare cases that need it.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What changes for users

Nothing changes in Flux surface syntax. All three phases are compiler/runtime
optimizations that preserve identical semantics. Users write the same `handle`/`perform`
code and get faster execution automatically.

### What "tail-resumptive" means

An effect handler is *tail-resumptive* if every operation arm resumes exactly once
as its final action:

```flux
// Tail-resumptive: resume is the last thing in every arm
handle {
    body()
} with {
    get()    -> resume(state)
    set(v)   -> do { state = v; resume(unit) }
    log(msg) -> do { append(buffer, msg); resume(unit) }
}
```

These are **NOT** tail-resumptive:

```flux
// Multi-shot: resume called twice (backtracking)
amb() -> do { resume(true); resume(false) }

// Non-resuming: continuation discarded (exception-like)
raise(e) -> error(e)

// Stored: continuation passed elsewhere
get_cc() -> resume(resume)
```

### Phase overview

```
Phase 1: Tail-resumptive detection          ← biggest win, moderate effort
  Detect tail-resumptive handlers at compile time.
  Replace OpPerform with OpPerformDirect (no continuation allocation).
  Handler operations become direct function calls.

Phase 2: Static handler resolution          ← medium win, low effort
  When handle and perform are in the same function or
  the handler is the innermost for its effect type,
  resolve the handler at compile time (no linear search).

Phase 3: Evidence passing (deferred)        ← big win, high effort
  Thread handler evidence as implicit function parameters.
  Requires typed CFG IR to know which effects flow where.
  Subsumes phases 1-2.
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1: Tail-Resumptive Handler Detection

#### Analysis pass

Add `is_tail_resumptive()` analysis to the bytecode compiler. This runs on `handle`
expressions during compilation, before opcode emission.

A handler arm is tail-resumptive if its body satisfies:
1. The terminal expression is `resume(expr)` with exactly one argument
2. All intermediate statements are "pure" (no nested `perform` calls)
3. `resume` is called exactly once on every code path

```rust
// src/bytecode/compiler/tail_resumptive.rs (new file, ~100 lines)

/// Returns true if all arms of a handler are tail-resumptive.
pub fn is_tail_resumptive(arms: &[HandleArm]) -> bool {
    arms.iter().all(|arm| arm_is_tail_resumptive(&arm.body))
}

fn arm_is_tail_resumptive(expr: &Expression) -> bool {
    match expr {
        // Terminal: resume(v)
        Expression::Call { callee, .. } if is_resume_ident(callee) => true,

        // Block: { stmts; resume(v) }
        Expression::Block { final_expr, .. } => {
            arm_is_tail_resumptive(final_expr)
        }

        // Let: let x = e in resume(v)
        Expression::Let { body, .. } => {
            arm_is_tail_resumptive(body)
        }

        // Conditional: both branches tail-resumptive
        Expression::If { consequence, alternative, .. } => {
            arm_is_tail_resumptive(consequence)
                && alternative.as_ref().is_some_and(|e| arm_is_tail_resumptive(e))
        }

        // Conservative: anything else is NOT tail-resumptive
        _ => false,
    }
}
```

#### New opcodes

```rust
// src/bytecode/op_code.rs

/// OpHandleDirect(const_idx: u8)
/// Like OpHandle but marks the handler as tail-resumptive.
/// No continuation will be allocated for performs targeting this handler.
/// The handler closures are still pushed, but dispatched via direct call.
OpHandleDirect = 0xD0,

/// OpPerformDirect(const_idx: u8, arity: u8)
/// Like OpPerform but skips continuation capture.
/// Finds the handler, calls the arm closure directly, pushes its return value.
OpPerformDirect = 0xD1,
```

#### VM dispatch for `OpPerformDirect`

```rust
OpCode::OpPerformDirect => {
    let const_idx = Self::read_u8_fast(instructions, ip + 1) as usize;
    let arity = Self::read_u8_fast(instructions, ip + 2) as usize;

    // Look up the perform descriptor (same as OpPerform)
    let descriptor = &self.constants[const_idx];
    let (effect_name, op_name) = extract_perform_descriptor(descriptor)?;

    // Find handler on handler stack (same linear search as OpPerform)
    let (handler_idx, arm_closure) =
        self.find_handler_arm(effect_name, op_name)?;

    // Pop arguments from stack
    let args = self.pop_n(arity)?;

    // CRITICAL DIFFERENCE: call the arm closure directly.
    // No continuation capture. No frame slicing.
    let result = self.call_closure_directly(&arm_closure, args)?;

    // The arm closure ends with resume(v) — extract v as the result.
    // Because the handler is tail-resumptive, resume(v) just returns v.
    self.push(result)?;

    Ok(3) // opcode + const_idx + arity
}
```

The key insight: for tail-resumptive handlers, `resume(v)` is semantically just
`return v`. We compile `resume` as a simple return inside the arm closure, eliminating
the continuation entirely.

#### Compiler changes

In `compile_handle_scope` and `compile_handle_expression`:

```rust
// Before emitting OpHandle, check tail-resumptive:
let opcode = if tail_resumptive::is_tail_resumptive(&handler_arms) {
    OpCode::OpHandleDirect
} else {
    OpCode::OpHandle
};
self.emit(opcode, &[desc_idx]);

// Similarly, performs targeting a tail-resumptive handler use OpPerformDirect.
// This requires tracking which handlers are tail-resumptive during compilation.
```

For `perform` calls, the compiler needs to know whether the target handler is
tail-resumptive. Two approaches:

1. **Conservative**: Track active `handle` blocks during compilation. If the innermost
   handler for an effect is tail-resumptive, emit `OpPerformDirect`.
2. **Runtime fallback**: Always emit `OpPerformDirect` when the handler found at
   runtime is marked tail-resumptive (check a flag on `HandlerFrame`).

Approach 2 is simpler and handles cross-function performs correctly. The handler
frame already exists; adding a `is_direct: bool` flag is trivial.

#### Performance impact

- **Per-perform savings**: eliminates `Vec<Frame>` allocation + memcpy (~500ns → ~50ns)
- **GC pressure**: eliminates `Continuation` values from heap entirely for TR handlers
- **Affected patterns**: State, Reader, Writer, logging — estimated 90% of real usage

### Phase 2: Static Handler Resolution

#### Problem

Even with Phase 1, `OpPerformDirect` still does a linear search up the handler stack
to find the matching handler. For deeply nested handlers or tight loops, this search
is measurable overhead.

#### Solution: compile-time handler binding

When the compiler can prove which handler a `perform` targets, it emits the handler's
stack offset directly:

```rust
/// OpPerformDirectIndexed(handler_offset: u8, arm_index: u8, arity: u8)
/// Like OpPerformDirect but skips the handler stack search.
/// handler_offset: distance from top of handler stack to the target handler.
/// arm_index: which arm in the handler to call.
OpPerformDirectIndexed = 0xD2,
```

#### When static resolution is possible

1. **Same-function handle+perform**: The `perform` is textually inside the `handle`
   block body. The compiler knows exactly which handler is targeted.

2. **Single handler for effect**: When there's only one active handler for an effect
   type in scope, no ambiguity exists.

3. **Innermost handler**: When the perform is for the innermost handler of its effect
   type (the common case), the offset is 0.

```rust
// Compile-time resolution in compile_perform:
if let Some(offset) = self.resolve_handler_statically(effect_name) {
    let arm_idx = self.find_arm_index(effect_name, op_name);
    self.emit(OpCode::OpPerformDirectIndexed, &[offset, arm_idx, arity]);
} else {
    // Fall back to runtime search
    self.emit(OpCode::OpPerformDirect, &[const_idx, arity]);
}
```

#### Implementation scope

This requires the compiler to maintain a "handler scope stack" during compilation
that tracks which `handle` blocks are active and their effect types. The bytecode
compiler already tracks some of this for effect validation.

#### Performance impact

- **Per-perform savings**: eliminates handler stack linear search (~20ns → ~5ns)
- **Cumulative**: significant for tight loops with deeply nested handlers

### Phase 3: Evidence Passing (Deferred)

#### Why defer

Evidence passing (as described in proposals 0072/0073 and the Koka literature)
requires threading handler evidence through function call boundaries as implicit
parameters. This means:

1. **Function signatures change**: every function that may perform an effect needs
   an implicit evidence parameter for each effect in its row.
2. **Typed IR required**: to know which effects a function performs, the IR must
   carry effect type information. Currently, Flux's CFG IR (`src/cfg/`) is untyped.
3. **ABI changes**: the calling convention must accommodate evidence parameters.

Without a typed CFG IR, implementing evidence passing correctly is fragile and
error-prone. Phases 1-2 deliver 80% of the performance benefit with 20% of the
complexity.

#### Prerequisites for Phase 3

- Typed CFG IR: effect annotations on `IrFunction` signatures
- Effect row resolution at IR level (not just during HM inference)
- Evidence parameter insertion pass
- Calling convention update for both VM and JIT

#### What Phase 3 enables beyond Phases 1-2

- **Cross-module optimization**: perform calls in separately compiled modules can
  be optimized without runtime handler search
- **Monomorphization**: effect handlers can be specialized per call site
- **Full Koka-style compilation**: effects compiled away entirely for tail-resumptive handlers

## Implementation plan
[implementation-plan]: #implementation-plan

### Phase 1 (target: 2 weeks)

1. Add `src/bytecode/compiler/tail_resumptive.rs` — analysis pass (~100 lines)
2. Add `OpHandleDirect` and `OpPerformDirect` opcodes
3. Add `is_direct: bool` flag to `HandlerFrame` in VM
4. Implement `OpPerformDirect` dispatch (direct closure call, no continuation)
5. Update `compile_handle_scope` and `compile_handle_expression` to detect TR handlers
6. Add tests: verify TR detection for State/Reader/Writer patterns
7. Add tests: verify non-TR handlers still use continuation path
8. Benchmark: State-effect loop before/after

### Phase 2 (target: 1 week, after Phase 1)

1. Add handler scope stack to bytecode compiler
2. Add `OpPerformDirectIndexed` opcode
3. Implement static resolution for same-function handle+perform
4. Add tests: verify static resolution emits indexed opcode
5. Benchmark: nested handler performance

### Phase 3 (deferred — requires typed CFG IR)

1. Design typed CFG IR extensions (separate proposal)
2. Evidence parameter insertion pass
3. Calling convention update
4. Full evidence passing compilation

## Drawbacks
[drawbacks]: #drawbacks

- **Opcode proliferation**: Phases 1-2 add 3 new opcodes (OpHandleDirect,
  OpPerformDirect, OpPerformDirectIndexed). The opcode space is not constrained
  (u8 allows 256), but each opcode adds a dispatch branch.

- **Two handler paths**: Tail-resumptive and non-tail-resumptive handlers coexist
  in the VM. This is inherent complexity — the two have fundamentally different
  execution models — but it means more code to maintain and test.

- **Conservative analysis**: The tail-resumptive analysis is syntactic and conservative.
  Some semantically tail-resumptive handlers may not be recognized (e.g., when
  `resume` is passed through a helper function). These handlers fall back to the
  continuation path with no performance regression.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not jump directly to evidence passing (proposals 0072/0073)?

Evidence passing requires a typed IR and ABI changes. Flux doesn't have a typed
CFG IR yet. Building one is a significant project (estimated 3-4 weeks). Phases 1-2
deliver most of the performance benefit with ~1 week of work, and they're additive
— Phase 3 can build on the same analysis infrastructure.

### Why not CPS-transform the entire program?

CPS (continuation-passing style) transformation is the canonical approach for
compiling effects (used by Koka, Eff). However, CPS transforms the entire program,
not just effectful code. This increases code size, complicates debugging, and
requires a more sophisticated optimizer to recover performance for non-effectful
code. The handler-stack approach keeps non-effectful code untouched.

### Why not use multi-prompt delimited continuations?

Multi-prompt delimited continuations (as in OCaml 5) are a more general mechanism
that can express all effect patterns. Flux already uses a variant of this
(handler stack + frame capture). The optimization phases proposed here are compatible
with multi-prompt continuations — they specialize the common case while preserving
the general mechanism.

### What if we don't do this?

Effect-heavy code remains 10-50x slower than equivalent non-effectful code.
Users learn to avoid effects in hot paths, defeating the purpose of having
algebraic effects as a language feature.

## Prior art
[prior-art]: #prior-art

- **Koka** (Leijen, 2017) — "Type Directed Compilation of Row-Polymorphic Effects"
  compiles effects via evidence passing. Tail-resumptive handlers are compiled to
  direct calls with evidence parameters. This proposal's Phase 1 achieves the same
  optimization for the VM without requiring typed IR.

- **Eff** (Bauer & Pretnar) — compiles tail-resumptive handlers to direct function
  calls, similar to Phase 1.

- **OCaml 5** — uses runtime handler stack with fibers. Effect handlers that resume
  once are optimized to avoid fiber allocation. Flux's Phase 1 is analogous.

- **Multicore OCaml** (Sivaramakrishnan et al.) — "Retrofitting Effect Handlers onto
  OCaml" describes the runtime handler stack approach with similar optimization
  opportunities for tail-resumptive handlers.

- **libhandler** (Leijen) — C library implementing evidence passing for effects,
  demonstrating that the optimization is achievable without a type-directed compiler.

- **Links** (Hillerström & Lindley) — "Liberating Effects with Rows and Handlers"
  describes row-polymorphic effects similar to Flux's, with optimizations for
  common handler patterns.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **Should `resume` be a keyword or a regular identifier?** Currently `resume` is
   a regular identifier bound in handler arms. Making it a keyword would simplify
   tail-resumptive detection (no need to track shadowing). Decision deferred to
   implementation.

2. **Should Phase 1 use a runtime flag or compile-time decision?** Runtime flag
   (approach 2 in the reference section) is simpler but adds a branch per perform.
   Compile-time decision is zero-cost but requires cross-function analysis for
   performs outside the handle block. Recommend starting with runtime flag.

3. **Should Phase 2 handle nested same-effect handlers?** E.g., inner State handler
   shadowing outer State handler. The static resolution must account for handler
   shadowing. Recommend supporting only the innermost (most common) case initially.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Typed CFG IR**: prerequisite for Phase 3. Would also enable type-directed
  optimizations beyond effects (specialization, monomorphization).

- **JIT integration**: Phases 1-2 apply to the bytecode VM. The JIT compiler
  (`src/jit/compiler.rs`) could implement the same optimizations by emitting
  direct calls instead of runtime helper calls for tail-resumptive handlers.

- **Effect sealing** (proposal 0075): when an effect is sealed (all handlers known
  at compile time), Phase 2's static resolution can be extended to cross-module
  performs.

- **Benchmark suite**: a dedicated effect handler benchmark suite would help
  measure the impact of each phase and guide further optimization.

- **Writer effect optimization**: `tell(v) -> resume(unit)` can be compiled to
  append to a thread-local buffer, similar to the State specialization in
  proposal 0073 but generalized through the Phase 1 mechanism.
