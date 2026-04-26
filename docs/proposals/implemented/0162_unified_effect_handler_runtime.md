- Feature Name: Unified Effect Handler Runtime
- Start Date: 2026-04-18
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: [0084](0084_aether_memory_model.md) (Aether memory model), [0086](0086_backend_neutral_core_ir.md) (Backend-neutral Core IR)
- Supersedes: [0072](../superseded/0072_evidence_passing_handlers.md) (Phase 1), [0073](../superseded/0073_state_reader_continuation_elim.md) (Phase 2), [0141](../superseded/0141_unified_effect_handlers.md) (Phase 3)

# Proposal 0162: Unified Effect Handler Runtime

## Summary
[summary]: #summary

Replace Flux's two-algorithm handler runtime with a single Koka-style
evidence-passing implementation shared by the VM and LLVM backends. Add
compile-time specialization for tail-resumptive handlers (no continuation
allocation) and monomorphic evidence for `State`/`Reader` (no `RefCell`).

This closes three pre-existing proposals (0072, 0073, 0141) into one runtime
closure proposal. The result is: one algorithm, two implementations in
different host languages, parity by construction.

Scope: **runtime / codegen only.** No user-visible syntax changes. No changes
to effect declarations or capability grants (those live in [Proposal
0161](0161_effect_system_decomposition_and_capabilities.md)).

## Motivation
[motivation]: #motivation

### Today: two algorithms, parity-by-luck

| | VM (Rust) | LLVM (C runtime) |
|---|---|---|
| **perform** | Copy `Vec<Frame>` + `Vec<Value>` into `Continuation` | `setjmp` saves C stack |
| **resume** | Restore frames + stack slice | `longjmp` back to perform site |
| **Multi-shot** | Clone frames (expensive) | Not supported |
| **Nested handlers** | Full support | Limited by `jmp_buf` nesting |
| **Tail-resumptive** | `is_direct` flag skips capture | Not optimized |

These are **fundamentally different algorithms**. Parity is maintained by
test-by-test vigilance, not by construction. Edge cases (nested handlers,
multi-shot, deep stacks) are the most likely place for silent divergence —
which is exactly where users are least able to diagnose it.

### Continuation allocation on the hot path

Tail-resumptive effect patterns (`State`, `Reader`, `Writer`) are common and
allocate a continuation on every `perform`. For a `fold` over a list of N
elements using `State`, that's 2N continuation allocations — each capturing
the full call frame stack — all immediately resumed. The overhead dominates
the arithmetic.

### Koka already solved this

Koka's runtime uses **neither** frame copying nor setjmp. Instead:

1. `perform` sets a yield flag in the thread context.
2. Every function, as it returns, checks the yield flag and (if set) adds its
   own frame to a continuation array.
3. The handler prompt checks if the yield is targeted at it (marker matching).
4. If yes, the handler composes the accumulated continuations into a single
   function.
5. `resume` calls the composed continuation.

No stack frames are copied. Continuations are built incrementally as the stack
unwinds normally via function returns. This is portable (works in C, JS,
Wasm), efficient (one branch per function return), and supports multi-shot
effects natively. See Xie & Leijen, _Generalized Evidence Passing for Effect
Handlers_, ICFP'21.

### Why three phases

Rolling out the full yield algorithm as a single step is risky — the VM and
LLVM backends would both be rewritten simultaneously. Instead, three phases
that each ship measurable value:

1. **Evidence passing for tail-resumptive handlers** — eliminates the hot-path
   continuation allocation without changing the general handler mechanism yet.
2. **Monomorphic evidence for `State`/`Reader`** — further specializes the
   common case to a direct pointer; zero-overhead effect dispatch.
3. **Unified yield algorithm across both backends** — retires the two
   algorithms; parity becomes structural.

Each phase is releasable on its own and the parity gate protects correctness
throughout.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What changes for users

**Nothing syntactically.** `effect`, `perform`, `handle`, `resume` all work
exactly as today.

**Performance visibly improves.** Tight `State`/`Reader` loops get significant
speedups (measured via `benches/aether_bench.rs` and new effect-dispatch
microbenchmarks). Multi-shot effects become first-class on both backends.

**Error messages about handler runtime get cleaner.** Today, some runtime
traces mention VM-internal `Continuation(Rc<RefCell<_>>)` types and C-level
`jmp_buf` names depending on backend. After Phase 3, the runtime vocabulary is
shared: evidence vector, marker, clause, continuation composition.

### Mental model

After this proposal:

- Every handler installs an **evidence** record in a thread-local evidence
  vector.
- Every `perform` yields: it sets a marker (identifying the target handler)
  and begins the unwind.
- Every function return is a tiny check: "is a yield in progress? if so, add
  my frame to the continuation."
- The target handler composes the accumulated frames into a resume closure.

The same algorithm runs in Rust (VM) and C (LLVM-linked runtime). Same data
structures, same control flow, different host language — the way arithmetic
already works (Rust `i64 + i64` vs LLVM `add`, but nobody worries about
divergence).

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1 — Evidence passing for tail-resumptive handlers (absorbs 0072)

**Definition.** A handler is *tail-resumptive* if every operation arm resumes
exactly once and that resume is the arm's final expression. Example:

```flux
handle {
    count(xs)
} with {
    get()    -> resume(state_ref)       -- tail-resumptive
    set(v)   -> do { state_ref = v; resume(()) }    -- tail-resumptive
}
```

Detection: static analysis in [`src/aether/tail_resumptive.rs`](../../src/aether/tail_resumptive.rs) (already exists for the `is_direct` flag).

**Compilation.**
- Tail-resumptive handlers emit an **evidence record** (`Rc<RefCell<Vec<Value>>>`
  for VM, heap-allocated struct for LLVM) instead of installing a continuation-
  based handler.
- `perform` on a tail-resumptive op becomes a direct indirect call to the arm,
  with the evidence record as an implicit parameter.
- `resume` becomes a return.

**Files affected.**
- [`src/core/passes/evidence.rs`](../../src/core/passes/evidence.rs) — extend the existing evidence pass.
- [`src/bytecode/vm/mod.rs`](../../src/bytecode/vm/mod.rs) — new opcode `OpPerformEvidence` (direct call with evidence parameter).
- [`runtime/c/effects.c`](../../runtime/c/effects.c) — mirror in C.

**Validation.**
- `tests/effect_evidence_tests.rs` — new integration test family.
- Parity sweep maintained.
- Microbenchmark: tight `State` loop should allocate zero continuations per
  iteration (measured via allocation counters).

### Phase 2 — Monomorphic evidence for `State` and `Reader` (absorbs 0073)

**Patterns recognized.**

| Effect name | Pattern recognized | Compiled as |
|---|---|---|
| `State` / `State<s>` | `get() -> resume(s)` + `set(v) -> resume(())` | `*mut Value` on call stack |
| `Reader` / `Reader<e>` | `ask() -> resume(env)` | `*const Value` function arg |

Compiler detects these patterns by shape (not just name — use structural
matching so `Counter` can specialize too if it fits the pattern).

**Compilation.**
- State: allocate a mutable slot on the caller's frame; thread `*mut Value` to
  the body; `get()` loads, `set(v)` stores, both inline at the perform site.
  No `RefCell`, no heap allocation.
- Reader: thread `*const Value` as a hidden function argument. `ask()` loads
  the environment pointer. Zero overhead beyond the argument pass.

**Files affected.**
- [`src/aether/tail_resumptive.rs`](../../src/aether/tail_resumptive.rs) — add pattern-shape detection alongside existing tail detection.
- [`src/core/passes/evidence.rs`](../../src/core/passes/evidence.rs) — specialization path for State/Reader shapes.
- [`src/lir/lower.rs`](../../src/lir/lower.rs) and [`src/bytecode/compiler/expression.rs`](../../src/bytecode/compiler/expression.rs) — emit direct pointer ops for recognized patterns.

**Validation.**
- `tests/effect_monomorphic_tests.rs` — assert that `State<Int>` in a tight
  loop compiles with no continuation allocation and no `RefCell`.
- Parity sweep maintained.
- Microbenchmark: `State`-driven counter should hit the same throughput as an
  explicit `&mut Int` loop.

### Phase 3 — Unified yield algorithm across backends (absorbs 0141)

For handlers that are *not* tail-resumptive (multi-shot, non-trivial control
flow, conditional resumes), the current VM and LLVM runtimes diverge. Phase 3
replaces both with a single yield-based algorithm.

**Core data structures (identical on both backends):**

```
Evidence       { htag, marker, handler, parent_evv }
EvidenceVector = sorted array of Evidence
YieldState     { yielding, marker, clause, conts[] }
```

**Algorithm:**

1. `handle` installs an `Evidence` record in the vector, with a fresh marker.
2. `perform` sets `YieldState.yielding = 1`, writes the target marker and
   operation clause, and begins returning.
3. Every function return checks `YieldState.yielding`. If set, the function
   pushes a continuation closure (its own remaining work) onto `conts[]` and
   returns.
4. When the return reaches the handler's frame, the handler sees its own
   marker, composes `conts[]` into a single continuation, and invokes the
   operation clause with it.
5. `resume(v)` invokes the composed continuation.

**Implementation mapping.**

| Structure | VM (Rust) | LLVM (C runtime) |
|---|---|---|
| `EvidenceVector` | `vm.evv: Vec<Evidence>` | `ctx->evv: FluxBoxed*` (NaN-boxed pointer to array) |
| `YieldState` | `vm.yield_state: YieldState` | `ctx->yield: YieldState` |
| Function-return check | `OpReturnCheck` opcode | Inlined at every function return in LLVM IR |
| Continuation composition | `Continuation::compose` in Rust | `flux_compose_conts` in `runtime/c/effects.c` |

**Migration.**
- Phase 1 and Phase 2 remain the fast paths for recognized shapes.
- Phase 3 covers every handler that didn't qualify.
- The old VM handler implementation ([`src/bytecode/vm/dispatch.rs` perform/handle paths](../../src/bytecode/vm/dispatch.rs)) and the setjmp/longjmp C path are removed.

**Files affected.**
- [`src/bytecode/vm/dispatch.rs`](../../src/bytecode/vm/dispatch.rs) — rewrite `OpPerform`/`OpHandle` paths.
- [`runtime/c/effects.c`](../../runtime/c/effects.c) — complete rewrite; new `flux_yield`, `flux_compose_conts`, `flux_install_evidence`.
- [`src/llvm/codegen/effects.rs`](../../src/llvm/codegen/) — emit function-return yield checks.
- [`runtime/c/flux_rt.h`](../../runtime/c/flux_rt.h) — expose the shared `EvidenceVector`, `YieldState` types.

**Validation.**
- **Parity-by-construction test.** `tests/effect_runtime_parity_tests.rs` —
  every handler shape (single-shot, multi-shot, nested, non-local resume,
  conditional resume) runs under both VM and LLVM; results must be
  bit-identical. This is the load-bearing regression gate.
- `tests/effect_multi_shot_tests.rs` — new tests for multi-shot effects on
  both backends.
- Full VM/LLVM parity sweep on `examples/` maintained.

### Phase 3 implementation status (2026-04-22)

Current todo list:

- [x] Define shared `Evidence` / `EvidenceVector` / `YieldState` data
  structures (VM Rust + C runtime).
- [x] Add LIR liveness analysis (backward over `LirFunction`).
- [x] Slice 3a: emit yield-check branch + abort-stub at `Call` sites
  (opt-in via `FLUX_YIELD_CHECKS` env).
- [x] Slice 3b-i: pre-pass `cont_split.rs` — synthesize `LirFunction`s from
  `cont` + live vars, populate `Call.yield_cont`.
- [x] Slice 3b-ii: wire `cont_split` into emit pipeline + replace stub with
  `flux_make_closure` + `flux_yield_extend`.
- [x] Slice 3b-iii: use `.closure_entry` wrapper for zero-capture synthesized
  continuations.
- [x] Slice 4-prereq: add `suppress_yield_check` flag to
  `LirTerminator::Call`; set it on handle-body-final calls so
  `flux_yield_prompt` catches the yield.
- [x] Slice 4: flip native `lower_perform` from `PerformDirect` to `YieldTo`
  when `FLUX_YIELD_CHECKS=1`; keep `PerformDirect` as the fallback while the
  Phase 3 path remains env-gated.
- [ ] Rewrite VM `OpHandle` / `OpPerform` to install evidence and yield.
- [ ] Add `OpReturnCheck` opcode and emit at function returns.
- [ ] Implement `Continuation::compose` (VM) for `conts[]` composition.
- [ ] Rewrite `runtime/c/effects.c`: `flux_yield`, `flux_compose_conts`,
  `flux_install_evidence`; remove `setjmp`/`longjmp` path.
- [ ] Remove old VM perform/handle dispatch paths.
- [ ] Write `effect_runtime_parity_tests.rs` (all handler shapes, both
  backends bit-identical).
- [ ] Write `effect_multi_shot_tests.rs`.
- [ ] Flip parity fixtures back to `parity: vm, llvm` + `expect: success`.
- [ ] Update `--dump-aether` to show yield+evidence vocabulary.

Honest status for slice 4:

- Slice 4-prereq is now landed: the handle body's final `Call` can suppress
  its post-call yield check, which lets the yield sentinel flow into
  `flux_yield_prompt` instead of escaping past `main`.
- Slice 4 is also landed, but intentionally only behind `FLUX_YIELD_CHECKS=1`.
  In that mode, native `lower_perform` now uses `flux_yield_to` and the
  after-call yield checks capture continuations as the stack unwinds.
- The old `PerformDirect` path remains the fallback when
  `FLUX_YIELD_CHECKS=0`, because unconditional `YieldTo` would regress the
  default native path until the full Phase 3 runtime is always-on.
- This is a real semantic improvement, not just plumbing: native now executes
  non-tail-resumptive discard/conditional-resume shapes correctly under the
  gate instead of reporting `E1200`.

What slice 4 now proves:

- The old blocker was real and is now resolved: the handle-body-final call no
  longer preempts `flux_yield_prompt`.
- The native yield path is coherent enough for single-shot non-TR handlers.
- Multi-shot is still not solved. `tests/parity/effect_multi_shot.flx` under
  `FLUX_YIELD_CHECKS=1` still fails, which is expected until the later
  continuation-composition/runtime slices land.

Files touched in slices 4-prereq / 4:

- `src/lir/mod.rs` — `LirTerminator::Call` now carries
  `suppress_yield_check: bool`.
- `src/lir/lower.rs` — `lower_handle` marks handle-body-final calls for
  suppression; `lower_perform` flips to `YieldTo` under
  `FLUX_YIELD_CHECKS=1` and falls back to `PerformDirect` otherwise.
- `src/lir/cont_split.rs` — suppressed calls are excluded from continuation
  synthesis.
- `src/lir/emit_llvm.rs` — suppressed calls skip the post-call yield-check
  branch.
- `src/lir/emit_llvm.rs` — `build_yield_block_instrs` now handles
  zero-capture synthesized continuations by using the `.closure_entry`
  wrapper and a null captures pointer.

Verification from these sessions:

- `cargo test --lib lir::` passed.
- `tests/parity/effect_deep_nesting` produced `"7"` in both
  `FLUX_YIELD_CHECKS=0` and `FLUX_YIELD_CHECKS=1` native runs.
- `tests/parity/effect_non_tr_discard.flx` produced `"-1"` on native with
  `FLUX_YIELD_CHECKS=1`.
- `tests/parity/effect_conditional_resume.flx` produced `"100"` on native with
  `FLUX_YIELD_CHECKS=1`.
- `tests/parity/effect_multi_shot.flx` still fails on native with
  `FLUX_YIELD_CHECKS=1`; that remains tracked work for the later
  continuation-composition slices.

## Exit Criteria
[exit-criteria]: #exit-criteria

Phase 1 ships when:
- Tail-resumptive handlers emit evidence-passing code on both backends.
- Microbenchmark shows zero continuation allocations per `perform` in the hot
  path.
- Existing effect tests remain green.

Phase 2 ships when:
- `State<T>` and `Reader<T>` patterns compile to direct pointer operations.
- Benchmarks show throughput matching hand-written `&mut T` / argument-passing
  code within 10%.

Phase 3 ships when:
- The old VM perform/handle path and the setjmp/longjmp C path are removed.
- `tests/effect_runtime_parity_tests.rs` covers all handler shapes and
  passes on both backends.
- Multi-shot effects are first-class on LLVM.
- `--dump-aether` shows the new yield+evidence vocabulary on both backends.

Overall closure:
- The two-algorithm runtime is gone. One algorithm, two implementations, same
  data vocabulary.
- Parity sweep across full `examples/` corpus: 100%.

## Drawbacks
[drawbacks]: #drawbacks

- Phase 3 is a runtime rewrite. Risk of subtle regressions on unusual handler
  shapes (deep nesting, interleaved handlers, non-local resumes). Mitigation:
  parity-by-construction test is a build gate.
- LLVM runtime needs to emit a yield check at every function return. This is
  one load + branch per return — a small constant overhead. Acceptable given
  the multi-shot payoff; benchmarked to verify.
- Three phases means three release windows with partial work visible. Phase 1
  ships without Phase 3; Phase 2 ships without Phase 3; during those phases
  the old handler algorithm still exists as the general fallback.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why not land Phase 3 first and skip 1 and 2?** Because most effect
  performs in real Flux code are tail-resumptive, and most of those are
  `State`/`Reader`. Phase 1 and Phase 2 deliver 80% of the performance win
  with a fraction of the runtime-rewrite risk. Phase 3 is strictly for
  correctness (multi-shot, algorithm unification), not performance.
- **Why not keep setjmp/longjmp on LLVM?** Because multi-shot resumes break it
  and because parity debugging across two fundamentally different algorithms
  is a perpetual cost. Unifying on one algorithm amortizes.
- **Why Koka's algorithm and not continuation-passing style throughout?**
  Koka's is the most carefully engineered effect runtime in a language with
  Flux's cost model (strict evaluation, RC memory, FFI boundary). Inventing a
  new algorithm has no upside.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Yield-check granularity.** Emit the check at every function return, or
  only at returns that cross a handler boundary? The former is simpler; the
  latter is faster but requires handler-boundary metadata in LIR. Decision:
  every return for Phase 3, optimization for later.
- **Evidence-vector representation on LLVM.** Inline array vs heap with
  doubling. Phase 3 starts with inline array of fixed size (4 or 8); spill
  to heap only when exceeded.
- **Interaction with Aether's borrow checker.** Phase 2's `*mut Value` for
  State must not alias the borrowed surface exposed to user code. Requires a
  tightening of borrow rules at the State-evidence boundary; fixture coverage
  planned.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Handler inlining across crates.** Once evidence passing is the norm,
  whole-program analysis could inline the handler clause at the perform site
  for cross-module tail-resumptive effects.
- **JS/Wasm backend.** The yield algorithm is portable. Flux gaining a
  third backend is significantly easier if the effect runtime is already
  language-agnostic.
- **Cost modelling for effect-heavy code.** With evidence passing as a single
  dispatch path, reasoning about performance becomes tractable: one load,
  one indirect call, one return — uniformly.
