- Feature Name: Aether Perceus Completion Plan
- Start Date: 2026-03-20
- Status: Implemented (Phases A-U complete; Phase S deferred by design)
- Proposal PR:
- Flux Issue:
- Depends on: 0084 (Aether memory model), 0086 (backend-neutral core IR)

# Proposal 0114: Aether Perceus Completion Plan

## Summary

This proposal defines the remaining work needed to bring Flux's Aether memory
model from a Perceus-inspired implementation to a closer match for Koka's
Perceus pipeline, while preserving Flux's backend-neutral architecture and full
support for Flux's three maintained execution backends:

```text
AST -> Core -> cfg -> VM / Cranelift JIT / LLVM
```

The original plan was organized around four gaps:

1. **Backend parity** — Aether semantics must be identical across VM, Cranelift JIT, and LLVM.
2. **Ownership analysis quality** — dup/drop insertion and borrowing must be driven by explicit ownership and liveness reasoning, not only local heuristics.
3. **Reuse sophistication** — reuse and drop specialization must be generalized and profitability-aware.
4. **FBIP rigor** — `@fip` / `@fbip` checking must become semantic rather than count-based.

Phases A-J established that base. The post-M tranche through Phases N-U then
pushed Aether through higher-order precision, broader reuse/drop-spec
coverage, stronger interprocedural summaries, a larger maintained FIP/FBIP
corpus, forwarding/wrapper reuse work, and maintained workload evidence. After
rechecking the current implementation against the Perceus paper and the local
Koka sources, the remaining gap is now narrower and more concrete:

5. **Ownership precision** — borrow/liveness reasoning must become less conservative across recursive, higher-order, imported, and effect-sensitive cases.
6. **Reuse/drop-spec coverage** — more realistic transformed Core shapes must trigger profitable reuse and drop specialization.
7. **FBIP maturity** — semantic FBIP must become less conservative and more Flux-native without expanding the public annotation surface yet.
8. **Forwarding/wrapper reuse** — transparent wrapper and forwarding-child style reuse cases should be recognized more often, closing a specific gap that Koka's C-level reuse still covers better today.
9. **Workload/performance maturity** — Aether progress should be measured not only by transformed Core and parity, but also by maintained Koka-style workloads and A/B performance cases.

This proposal does **not** replace 0084. It is the execution plan for finishing
0084 to a level that is comparable in spirit and behavior to Koka's Perceus.

For the actor/concurrency roadmap and the merged local-vs-shared runtime split
that follows after 0114's local single-threaded Aether completion work, see:
- `docs/proposals/0115_actor_concurrency_roadmap.md`

All new compiler-side ownership, reuse, specialization, verification, and FBIP
logic introduced by this proposal should live in `src/aether/` unless there is
a clear cross-cutting reason it belongs elsewhere. Backend-specific lowering and
runtime realization will still live in backend/runtime modules, but the Aether
analysis and transformation logic itself should remain centralized in the
Aether module.

## Motivation

Flux already has a meaningful Aether implementation:

- Core IR supports `Dup`, `Drop`, `Reuse`, and `DropSpecialized`
- CFG lowering and all maintained backends understand Aether operations
- runtime reuse helpers exist
- `--dump-aether` and basic Aether verification exist

That is enough to validate the architecture, but it is not yet enough to claim
that Flux matches Koka's Perceus implementation.

### Why Flux still falls short of Koka

Compared to Koka, Flux still has these weaknesses:

1. **Borrow precision still trails Koka**
   - Borrow metadata exists, but recursive, imported, and higher-order cases remain conservative.
   - Call-site ownership decisions are explicit now, but still miss some interprocedural precision.

2. **Ownership flow is not yet Koka-grade**
   - Env-based insertion exists, but branch joins and older interprocedural paths still have rough edges.
   - Borrow-aware closure capture has landed, and unused closure captures are now pruned during IR lowering.
   - Flux still emits avoidable `Dup`/`Drop` pairs in some recursive and higher-order examples.

3. **Reuse coverage is still narrower than Koka**
   - Provenance-driven reuse and reuse specialization now exist, but more transformed Core shapes should still be recognized.
   - `DropSpecialized` works on realistic cases, but not yet on all useful branchy/admin shapes.

4. **FBIP remains conservative**
   - Semantic FBIP checking exists, but still falls back to conservative unknown/non-provable outcomes too often.
   - Flux-specific effects/runtime calls should participate more precisely in proof summaries.

5. **Backend parity is not complete**
   - Some reuse paths differ across backends today.
   - Aether semantics cannot be considered complete until parity is strict.

### Why finish this work

Without this completion plan:

- Aether remains difficult to trust as a semantic optimization layer
- backend divergence risks silent behavioral skew
- `@fip` / `@fbip` annotations remain weaker than advertised
- Flux cannot claim Koka-grade Perceus behavior even when the syntax looks similar

With this work complete:

- Aether becomes a stable, testable optimization model
- reuse legality is compiler-guided, not mostly pattern-driven
- backend-neutral Core retains the main architectural advantage over Koka
- Flux gets much closer to zero-allocation purely functional updates on unique paths

## Current status after Phases A-J

The first tranche of 0114 has established the Aether base:

- Aether is real and backend-shared across `Core -> cfg -> VM / Cranelift JIT / LLVM`
- the verifier rejects malformed Aether before lowering
- explicit borrowed/owned call sites, reuse specialization, stronger drop specialization, and semantic FBIP exist
- dedicated Aether core/snapshot/parity/diagnostic suites exist

The remaining work is no longer architectural bootstrapping. It is primarily
about precision, coverage, maturity, and validation:

- fewer conservative ownership decisions
- broader reuse and `DropSpecialized` coverage on transformed Core
- more credible interprocedural FBIP summaries and diagnostics
- better forwarding/wrapper reuse coverage
- explicit workload and performance tracking on maintained Aether examples

## Non-goals

This proposal does not:

- replace Flux's shared `Core -> cfg` architecture with a C-backend-specific path
- reintroduce AST fallback into JIT or backend paths
- add a second semantic IR beside `core`
- require actor transfer semantics; that remains future work
- add atomic/thread-shared reference counting or shared-memory concurrency support; that remains outside 0114 and should be handled by a future actor/concurrency proposal
- require every possible Perceus optimization before progress can ship

## Design principles

The completion work should preserve these constraints:

1. **Core remains the semantic optimization surface**
   - ownership and reuse decisions belong at Core first
   - CFG lowering should realize those decisions, not invent them

2. **Backend parity is mandatory**
   - VM, Cranelift JIT, and LLVM may differ internally
   - they must not differ in observable Aether semantics

3. **Fallback safety beats optimization coverage**
   - missed reuse is acceptable
   - unsound ownership reasoning is not

4. **Aether nodes are semantic commitments**
   - once emitted in Core, each Aether node must have clear lowering rules

5. **Aether compiler logic stays in `src/aether/`**
   - ownership analysis should not be reimplemented independently in each backend
   - backend code should lower Aether decisions, not invent competing ones

## Phased completion plan

### Phase A: Backend parity

**Goal:** make existing Aether behavior identical across the three maintained
backends: VM, Cranelift JIT, and LLVM.

#### Scope

- align VM reuse behavior with Cranelift JIT/LLVM/runtime helper behavior
- audit all Aether ops across VM/Cranelift JIT/LLVM:
  - `Dup`
  - `Drop`
  - `Reuse`
  - `DropSpecialized`
  - `AetherDrop`
  - `DropReuse`

#### Primary files

- `src/aether/`
- `src/bytecode/vm/dispatch.rs`
- `src/runtime/native_helpers.rs`
- `src/jit/compiler.rs`
- `src/llvm/compiler/expressions.rs`
- `src/cfg/mod.rs`

#### Acceptance criteria

- unique/shared reuse paths behave identically across VM, Cranelift JIT, and LLVM
- wrapper reuse (`Some`, `Left`, `Right`) is parity-correct
- Aether parity tests pass for all maintained backends

#### Tests

- `ReuseSome` unique/shared
- `ReuseLeft` unique/shared
- `ReuseRight` unique/shared
- `ReuseCons` with and without field mask
- `ReuseAdt` with and without field mask
- `DropSpecialized` unique/shared behavior

### Phase B: Aether semantics spec and verifier hardening

**Goal:** define and enforce the exact meaning of every Aether node.

#### Scope

- document Core semantics for:
  - `Dup`
  - `Drop`
  - `Reuse`
  - `DropSpecialized`
  - `field_mask`
- strengthen verifier checks to reject malformed Aether shapes before lowering
- define a reduced formal single-threaded Aether semantics as the proof target
- map verifier checks to explicit proof obligations without claiming theorem
  completion yet

#### Primary files

- `src/aether/`
- `src/aether/mod.rs`
- `src/aether/verify.rs`
- `src/core/mod.rs`
- `docs/proposals/implemented/0084_aether_memory_model.md`
- `docs/internals/aether_formal_semantics.md`

#### Acceptance criteria

- each Aether node has a stable lowering contract
- verifier catches unsafe drop/token patterns
- 0084 documentation reflects actual implementation status
- reduced formal semantics exists for the current single-threaded Aether nodes
- proof obligations are stated explicitly and aligned with verifier checks

### Phase C: Borrow metadata infrastructure

**Goal:** replace purely heuristic borrow discovery with explicit compiler-owned metadata.

#### Scope

- introduce a richer borrow registry that supports:
  - user functions
  - imports
  - externals
  - base/runtime functions
  - recursive groups
- preserve borrow information as compiler metadata instead of rediscovering it everywhere

#### Primary files

- `src/aether/`
- `src/aether/borrow_infer.rs`
- `src/core/mod.rs`
- `src/syntax/statement.rs`
- relevant builtin/base function metadata sites

#### Acceptance criteria

- borrow info is available for direct calls, imported calls, and known runtime/base calls
- conservative defaults are explicit for unknown callees
- recursive groups have defined borrow behavior

### Phase D: Environment-based dup/drop insertion

**Goal:** move Phase 5 closer to Koka's Perceus algorithm.

#### Scope

- replace use-count-dominant insertion with ownership/liveness environments
- model:
  - borrowed environment
  - owned/live environment
  - branch joins
  - closure capture
  - effect/handler boundaries
  - recursive lets

#### Primary files

- `src/aether/`
- `src/aether/analysis.rs`
- `src/aether/insert.rs`

#### Acceptance criteria

- dup/drop placement is driven by ownership flow, not only local counts
- closures and branches produce more precise RC insertion
- false positive/negative `Dup`/`Drop` cases are reduced

### Phase E: Explicit borrowed call handling

**Goal:** make borrowed calls and owned calls distinct at transformation time.

#### Scope

- model borrowed arguments explicitly at call sites
- preserve evaluation order while allowing precise post-call drops
- treat indirect or unknown callees conservatively

#### Primary files

- `src/aether/`
- `src/aether/analysis.rs`
- `src/aether/insert.rs`

#### Acceptance criteria

- borrowed and owned call-site behavior is explicit in transformed Core
- nontrivial borrowed expressions are handled safely
- RC overhead around borrowed calls drops on common patterns

### Phase F: Generalized reuse analysis

**Goal:** make reuse less dependent on narrow syntax.

#### Scope

- recognize reuse through simple let-spine transformations
- preserve constructor field provenance further through intermediate bindings
- reduce missed-reuse cases detected by verifier

#### Primary files

- `src/aether/`
- `src/aether/reuse.rs`
- `src/aether/drop_spec.rs`

#### Acceptance criteria

- reuse fires through common let restructuring
- ADT/list updates trigger reuse more often in realistic Core
- verifier reports fewer missed reuse opportunities on maintained examples

### Phase G: Reuse specialization pass

**Goal:** add a dedicated profitability-aware specialization stage.

#### Scope

- split specialization logic from basic reuse insertion
- specialize only when enough writes or metadata updates are avoided
- generalize selective writes beyond the current field-mask embedding

#### Primary files

- `src/aether/`
- `src/aether/reuse.rs`
- new `src/aether/reuse_spec.rs`
- `src/aether/mod.rs`

#### Acceptance criteria

- a dedicated specialization pass exists
- specialization has explicit profitability logic
- partial-write fast paths are emitted only when beneficial

### Phase H: Stronger drop specialization

**Goal:** make `DropSpecialized` trigger often enough to matter in real programs.

#### Scope

- support more RHS shapes than the current narrow spine form
- preserve lowering simplicity across backends
- integrate better with generalized reuse analysis

#### Primary files

- `src/aether/`
- `src/aether/drop_spec.rs`

#### Acceptance criteria

- `DropSpecialized` appears in realistic examples
- unique-path dup elimination is measurable on maintained fixtures
- shared path remains correct and backend-lowerable

### Phase I: Semantic FBIP checker

**Goal:** replace count-based FBIP checking with capability/flow reasoning.

#### Scope

- reason about:
  - allocations
  - deallocations
  - non-FIP calls
  - effect/handler boundaries
  - control-flow joins
  - constructor-token availability
- improve diagnostics so failures explain *why* a function is not FIP/FBIP

#### Primary files

- `src/aether/`
- `src/aether/check_fbip.rs`
- new supporting analysis module(s)

#### Acceptance criteria

- `@fip` / `@fbip` diagnostics are semantic, not just structural counts
- diagnostics point to concrete causes of failure
- no-constructors and count-only checks become secondary, not primary

### Phase J: Aether regression suite

**Goal:** make Aether progress measurable and non-regressing.

#### Scope

- add focused tests for:
  - borrowing
  - reuse
  - reuse specialization
  - drop specialization
  - backend parity
  - FBIP diagnostics
- snapshot representative `--dump-core` / `--dump-aether` surfaces

#### Primary files

- `tests/ir_pipeline_tests.rs`
- new `tests/aether_*.rs`
- `examples/aether/verify_aether.flx`
- new `examples/aether/*.flx`

#### Acceptance criteria

- dedicated Aether regression corpus exists
- representative fixtures assert transformed Core structure, not only runtime output
- parity slices run in CI for maintained backends

### Phase K: Ownership precision

**Goal:** improve borrow/liveness precision toward Koka-style behavior across recursive, higher-order, and imported/base-call cases.

#### Scope

- strengthen borrow metadata propagation and fixpoint behavior for recursive groups
- improve environment/liveness precision in:
  - closures and captures
  - branch joins
  - handler scopes
  - imported/direct-call boundaries
- tighten interprocedural ownership/effect reasoning where older compiler subsystems still fall back to conservative behavior
- keep all logic in `src/aether/` and existing Core metadata surfaces

#### Primary files

- `src/aether/borrow_infer.rs`
- `src/aether/analysis.rs`
- `src/aether/insert.rs`
- small supporting touches in `src/core/` only where metadata plumbing requires it

#### Acceptance criteria

- fewer spurious `Dup`/`Drop` insertions on recursive and higher-order examples
- borrowed call behavior is stable across direct local, imported, and known base/runtime calls
- closure capture and branch joins produce more precise ownership flow than current conservative cases
- no new CFG/backend ownership logic is introduced

### Phase L: Reuse and drop-specialization expansion

**Goal:** broaden the set of realistic transformed Core shapes that trigger profitable reuse and drop specialization.

#### Scope

- extend provenance-driven reuse through more administrative Core rewrites, branch joins, and alias-preserving lets
- broaden `DropSpecialized` candidate recognition to additional safe branchy/admin shapes without complicating lowering
- keep specialization profitability explicit and deterministic
- improve maintained examples so reuse/drop-spec show up on more realistic Aether fixtures

#### Primary files

- `src/aether/reuse_analysis.rs`
- `src/aether/reuse.rs`
- `src/aether/reuse_spec.rs`
- `src/aether/drop_spec.rs`

#### Acceptance criteria

- more maintained fixtures emit `Reuse` and `DropSpecialized`
- masked/selective-write reuse appears only when profitable and exact
- shared-path conservatism remains intact
- backend lowering remains unchanged and parity stays green

### Phase M: FBIP and interprocedural maturity

**Goal:** make semantic FBIP less conservative and more Flux-native without expanding source syntax.

#### Scope

- strengthen interprocedural summaries used by `fbip_analysis`
- model Flux-native call/effect/runtime causes more precisely
- reduce unknown/non-provable outcomes caused only by incomplete ownership/effect summary plumbing
- improve diagnostics to distinguish:
  - unknown/indirect call
  - known but non-provable callee
  - effect boundary
  - token unavailability
  - control-flow precision loss
- explicitly keep `@fip` / `@fbip` surface unchanged in this phase

#### Primary files

- `src/aether/fbip_analysis.rs`
- `src/aether/check_fbip.rs`
- supporting call/effect metadata plumbing only where needed

#### Acceptance criteria

- fewer false-negative `@fip` / `@fbip` failures on maintained examples
- diagnostics are more actionable and less generic conservative fallback
- Flux builtins/effects are treated as Flux-native proof inputs, not opaque unknowns
- no `fip(n)` / `fbip(n)` syntax is added

## Post-M maturity track

Phases K-M establish the broad maturity categories for the current Aether
implementation: ownership precision, reuse/drop-spec coverage, and FBIP
maturity. The next tranche should turn those broad categories into a concrete
execution order for closing the remaining gap with Koka/Perceus.

Phases N-U therefore define the practical post-M implementation sequence. The
goal is not to clone Koka mechanically, but to close the remaining precision,
coverage, benchmark-corpus, and workload-validation gaps while keeping Aether
Flux-native and preserving Flux's Core/Aether/CFG architecture.

### Phase N: Higher-order and recursive borrow/FBIP precision

**Goal:** reduce conservative outcomes that still appear in recursive and higher-order functions.

#### Scope

- improve recursive and mutually recursive borrow precision
- improve higher-order borrowed-call precision
- reduce higher-order FBIP false negatives like `my_map` and `option_map`
- tighten the interaction between borrow summaries and FBIP summaries

#### Primary files

- `src/aether/borrow_infer.rs`
- `src/aether/analysis.rs`
- `src/aether/fbip_analysis.rs`
- `src/aether/insert.rs`

#### Acceptance criteria

- fewer spurious `Dup` / `Drop` operations in recursive higher-order cases
- fewer `@fip` / `@fbip` false negatives caused only by higher-order conservatism
- maintained higher-order fixtures show more precise Aether shape without backend changes

### Phase O: Missed reuse coverage

**Goal:** recover more profitable `Reuse` sites that are still being missed in realistic transformed Core.

#### Scope

- higher-order recursive rebuilds
- more branch-sensitive list and ADT rebuilds
- more transformed/Core-admin shapes where provenance is exact but current coverage still misses reuse
- join-aware provenance and cross-arm token-forwarding reuse where exact branch results can be normalized without speculation

#### Primary files

- `src/aether/reuse_analysis.rs`
- `src/aether/reuse.rs`
- `src/aether/reuse_spec.rs`

#### Acceptance criteria

- more maintained fixtures emit plain `Reuse`
- examples like `bench_reuse` become explicit targets for closing the current reuse gap
- no new ambiguous or speculative reuse is introduced

### Phase P: Missed drop-specialization coverage

**Goal:** increase `DropSpecialized` coverage on structurally safe cases that still stay conservative today.

#### Scope

- deeper admin-let and branch-normalized cases
- recursive update patterns where unique/shared separation is still safe
- more mixed-path cases where unique-path optimization is possible while shared-path conservatism remains intact
- stronger unique-path dup/drop cleanup after specialization

#### Primary files

- `src/aether/drop_spec.rs`
- `src/aether/insert.rs`
- `src/aether/reuse_analysis.rs`

#### Acceptance criteria

- more maintained fixtures emit `DropSpecialized`
- shared-path behavior remains conservative
- backend lowering remains unchanged

### Phase Q: Stronger interprocedural summaries

**Goal:** strengthen interprocedural ownership and FBIP summaries so more direct/internal/imported cases compose precisely.

#### Scope

- direct internal callee summaries
- imported/base/runtime summary plumbing
- ownership/effect summary cleanup in older compiler subsystems
- fewer “known but still conservatively flattened” cases

#### Primary files

- `src/aether/borrow_infer.rs`
- `src/aether/fbip_analysis.rs`
- `src/aether/check_fbip.rs`

#### Acceptance criteria

- direct known callees compose more reliably in ownership and FBIP analysis
- imported/base/runtime metadata is used consistently across Aether analyses
- summary quality improves without adding new source syntax

### Phase R: Larger FIP/FBIP benchmark corpus

**Goal:** make Aether maturity measurable on a broader workload closer to Koka’s benchmark/test style.

#### Scope

- add more maintained Aether fixtures for:
  - higher-order recursion
  - tree/list rebuilds
  - reuse-heavy ADT updates
  - FBIP-annotated success/failure cases
- expand parity and snapshot coverage around those fixtures
- explicitly test claimed optimizations against transformed Core

#### Primary files

- `examples/aether/`
- `tests/aether_core_regressions.rs`
- `tests/aether_cli_snapshots.rs`
- `tests/aether_backend_parity.rs`

#### Acceptance criteria

- every maintained Aether optimization claim is backed by a transformed-Core assertion
- a larger corpus exists for FIP/FBIP and reuse-heavy cases
- no maintained benchmark/fixture comment overstates what actually fires

### Phase T: Forwarding and wrapper reuse coverage

**Goal:** close the remaining reuse gap on transparent-wrapper and forwarding-child style patterns that Koka's reuse pipeline still handles more often today.

#### Scope

- wrapper-like constructor rebuilds where the outer shape is preserved through forwarding
- transparent/named-ADT update paths where exact field provenance survives but current Core-level reuse still misses the opportunity
- more identity-like rebuilds through admin lets and forwarding bindings
- branch-joined forwarded shapes where exact child provenance survives control-flow merges and should still enable wrapper/child reuse
- preserve exactness: no speculative reuse, no ambiguous provenance joins, no new shared-path reuse

#### Primary files

- `src/aether/reuse_analysis.rs`
- `src/aether/reuse.rs`
- `src/aether/reuse_spec.rs`

#### Acceptance criteria

- more wrapper and forwarding fixtures emit real `Reuse`
- at least one maintained fixture captures a forwarding/wrapper case that previously stayed fresh
- Koka-style forwarding-child gaps shrink without changing verifier or backend semantics

### Phase U: Aether workload and performance maturity

**Goal:** make Aether maturity measurable on maintained workloads, not only on transformed Core dumps and parity suites.

#### Scope

- add Koka-style workload fixtures for trees, queues, heaps, and reuse-heavy updates where practical in Flux
- keep A/B performance fixtures for reuse-friendly versus reuse-blocked shapes
- track Aether-specific metrics alongside runtime:
  - `Dup`
  - `Drop`
  - `Reuse`
  - `DropSpecialized`
  - fresh allocations
- keep backend parity and performance tracking separate so Aether wins are not confused with backend speed differences

#### Primary files

- `examples/aether/`
- `tests/aether_core_regressions.rs`
- `tests/aether_backend_parity.rs`
- `tests/aether_cli_snapshots.rs`
- release/benchmark scripts where needed

#### Acceptance criteria

- maintained Aether workloads exist beyond micro-fixtures
- at least one list workload, one tree/ADT workload, and one A/B control benchmark measure real Aether effects
- optimization claims in docs/examples are supported by both transformed-Core assertions and runnable workload evidence

### Phase S: Bounded FBIP forms, deferred until after N-U

**Goal:** keep `fip(n)` / `fbip(n)` explicitly deferred until post-N-U evidence shows that bounded forms are semantically justified and worth a separate proposal.

#### Scope

- proposal-level evaluation only
- no syntax commitment until N-U materially reduce conservative gaps on maintained fixtures
- if pursued later, bounded forms must be grounded in the semantic FBIP checker, not a count-based shortcut

#### Evaluation gates

Bounded forms are not eligible for reconsideration until all of the following are true:

- Phases N-U have reduced higher-order and imported-call conservatism materially on maintained fixtures
- the maintained Aether corpus added through Phase R shows stable, representative FBIP success and failure categories
- the maintained workload and performance corpus from Phase U shows that the remaining gaps are semantic precision limits rather than missing benchmark coverage
- the remaining false negatives are understood as semantic limits of the current model, not analysis or corpus immaturity
- `@fip` / `@fbip` remain the only supported surface until those gates are met

Bounded forms are therefore not the “next syntax”. They are a later optional reconsideration once the post-M maturity work has settled.

#### Decision checklist

Before any future bounded-FBIP syntax proposal is written, it must answer:

- Semantics:
  - what `fip(n)` / `fbip(n)` mean in Flux's semantic FBIP model
  - whether `n` counts fresh allocations, upper bounds, constructor sites, or another semantic quantity
- Soundness:
  - whether the semantic checker can justify numeric bounds compositionally
  - whether imported/runtime/effect boundaries can participate without collapsing the model into conservative fallback
- Diagnostics:
  - how a failed bounded proof would be explained without regressing to vague count-based messages
- Value:
  - which real maintained examples become meaningfully more expressible than today's `@fip` / `@fbip`

If those questions cannot be answered cleanly, bounded forms remain deferred.

#### Reconsideration inputs

Any future Phase S reconsideration must be grounded in the post-M maturity evidence from:

- Phase N:
  - higher-order and recursive precision
- Phase O/P:
  - broader reuse and drop-specialization coverage
- Phase Q:
  - stronger interprocedural summaries
- Phase R:
  - larger maintained FIP/FBIP corpus
- Phase T:
  - forwarding and wrapper reuse coverage
- Phase U:
  - maintained workload and performance evidence

The evaluation trigger is therefore concrete: revisit bounded forms only after those phases provide stable evidence that the remaining FBIP gaps are principled and measurable.

#### Not part of this tranche

Phase S does **not** authorize:

- parser support for `fip(n)` / `fbip(n)`
- AST/Core annotation changes
- numeric-bound implementation in the FBIP checker
- new diagnostics, snapshots, or syntax fixtures for bounded forms

#### Primary files

- proposal/docs only for this phase

#### Acceptance criteria

- bounded syntax remains explicitly deferred in the current proposal
- proposal text states that this phase is contingent on N-U success and the evaluation gates above
- proposal text states that Phase S is evaluation-only
- no implementation work on `fip(n)` / `fbip(n)` is part of the current execution tranche

## Recommended implementation order

The phases should land in this order:

1. Phase A — Backend parity
2. Phase B — Semantics spec and verifier hardening
3. Phase C — Borrow metadata infrastructure
4. Phase D — Environment-based dup/drop insertion
5. Phase E — Explicit borrowed call handling
6. Phase F — Generalized reuse analysis
7. Phase G — Reuse specialization pass
8. Phase H — Stronger drop specialization
9. Phase I — Semantic FBIP checker
10. Phase J — Aether regression suite
11. Phase K — Ownership precision
12. Phase L — Reuse and drop-specialization expansion
13. Phase M — FBIP and interprocedural maturity
14. Phase N — Higher-order and recursive borrow/FBIP precision
15. Phase O — Missed reuse coverage
16. Phase P — Missed drop-specialization coverage
17. Phase Q — Stronger interprocedural summaries
18. Phase R — Larger FIP/FBIP benchmark corpus
19. Phase T — Forwarding and wrapper reuse coverage
20. Phase U — Aether workload and performance maturity
21. Phase S — Bounded FBIP evaluation

## Milestones

### Milestone 1: Correctness base

Includes:

- Phase A
- Phase B

Exit criteria:

- Aether has stable backend semantics
- malformed Aether Core is rejected early
- 0084 docs stop overstating implementation completeness

### Milestone 2: Ownership model

Includes:

- Phase C
- Phase D
- Phase E

Exit criteria:

- borrow information is explicit and interprocedurally usable
- dup/drop insertion is ownership-driven
- call-site ownership behavior is no longer mostly heuristic

### Milestone 3: Reuse quality

Includes:

- Phase F
- Phase G
- Phase H

Exit criteria:

- reuse and drop specialization appear on realistic programs
- specialization is profitability-aware
- missed reuse rates drop on maintained examples

### Milestone 4: FBIP credibility

Includes:

- Phase I
- Phase J

Exit criteria:

- `@fip` / `@fbip` diagnostics are semantically meaningful
- Aether behavior is regression-tested at IR and backend levels

### Milestone 5: Ownership maturity

Includes:

- Phase K

Exit criteria:

- borrow/liveness precision is noticeably less conservative on recursive and higher-order examples
- imported/base/runtime call ownership behavior is stable and metadata-driven

### Milestone 6: Reuse coverage maturity

Includes:

- Phase L

Exit criteria:

- maintained fixtures show more `Reuse` and `DropSpecialized` sites on realistic transformed Core
- reuse/drop-spec coverage expands without changing backend lowering contracts

### Milestone 7: FBIP maturity

Includes:

- Phase M

Exit criteria:

- semantic FBIP is materially less conservative on maintained examples
- diagnostics distinguish Flux-native causes instead of collapsing into generic conservative failure

### Milestone 8: Higher-order and recursive precision

Includes:

- Phase N

Exit criteria:

- recursive and higher-order ownership/FBIP behavior is materially less conservative on maintained examples
- higher-order false negatives drop without changing backend lowering

### Milestone 9: Reuse and drop-specialization closure

Includes:

- Phase O
- Phase P

Exit criteria:

- additional maintained fixtures emit real `Reuse` and `DropSpecialized`
- missed optimization sites drop on realistic transformed Core shapes without losing shared-path conservatism

### Milestone 10: Interprocedural and corpus maturity

Includes:

- Phase Q
- Phase R

Exit criteria:

- direct/internal/imported summaries compose reliably across Aether analyses
- maintained Aether fixtures form a broader FIP/FBIP and reuse benchmark corpus with explicit shape assertions

### Milestone 11: Bounded FBIP evaluation

Includes:

- Phase S

Exit criteria:

- the proposal explicitly defines when bounded FBIP forms may be reconsidered
- Phase S remains evaluation-only and authorizes no implementation work
- no bounded surface is added unless N-U materially reduced conservative gaps and the Phase S checklist can be answered cleanly

### Milestone 12: Forwarding reuse and workload maturity

Includes:

- Phase T
- Phase U

Exit criteria:

- forwarding/wrapper reuse coverage improves on maintained fixtures
- Aether optimization wins are tracked on representative workloads, not only on dumps and parity
- release-facing evidence exists for both transformed-Core quality and runtime impact

## Testing strategy

Every phase must add both:

1. **Shape tests**
   - assert expected Core/CFG Aether structure
   - use `--dump-core` / `--dump-aether` or direct IR inspection

2. **Behavior tests**
   - run VM, Cranelift JIT, and LLVM parity slices
   - assert unique/shared fast-path equivalence

Recommended test categories:

- borrowed direct call
- borrowed imported/base function call
- closure capture with shared and unique values
- list map/filter reuse
- option/either wrapper reuse
- ADT partial update reuse
- drop specialization on constructor patterns
- FBIP success and failure diagnostics
- recursive and mutually recursive borrowed-call cases
- higher-order borrowed argument propagation
- imported/base/runtime callee summary composition
- recursive higher-order map/filter/fold cases
- currently non-reusing recursive rebuild fixtures
- deeper branch/admin-let drop-specialization fixtures
- expanded maintained FIP/FBIP benchmark corpus
- wrapper/forwarding-child reuse fixtures
- maintained A/B performance fixtures that isolate Aether reuse/drop-spec effects from backend speed
- maintained examples must not regress into `E999` internal Aether failures

## Alternatives considered

### Recreate Koka's C-backend Perceus pipeline directly

Rejected because Flux's architecture already commits to backend-neutral Core and
CFG IR shared by VM, JIT, and LLVM.

### Keep the current heuristic approach and only add more rewrite rules

Rejected because this would improve benchmarks locally but leave the ownership
model under-specified and difficult to trust.

### Defer FBIP until after all reuse work is done

Partially rejected. A fully semantic checker can wait until later phases, but
parity and verifier hardening must come first so the optimization work has a
stable target.

## Candidate enhancements beyond the current tranche

The phases above define the current execution plan. The following items are
explicitly recorded as high-value Aether enhancements that may become later
proposal work or a post-U extension of 0114 once the current tranche settles.

These are not authorized implementation work by default, but they capture the
most likely next areas for improvement after the current maturity sequence.

### 1. Precision enhancements

Potential next precision work:

- wrapper and forwarding-child reuse recognition
- better imported/base/runtime summary precision
- more exact higher-order call reasoning when callee identity is known exactly
- fewer conservative ownership degradations at branch joins

Why this matters:

- this is where Aether still trails Koka most concretely on some realistic
  transformed shapes
- many remaining false negatives now come from conservatism rather than missing
  Aether architecture

### 2. Optimization coverage enhancements

Potential next optimization-coverage work:

- broader `DropSpecialized` candidate extraction on deeper admin-let shapes
- more selective-write reuse on named ADTs and tree updates
- better reuse through forwarding wrappers and exact field passthrough
- more profitable fusion around borrowed recursive traversals and post-specialization unique paths

Why this matters:

- the remaining gap is increasingly about whether profitable shapes are
  recognized often enough in realistic workloads
- these are the kinds of cases that distinguish "working Aether" from
  "mature Aether"

### 3. Validation and workload enhancements

Potential next validation work:

- workload-level Aether benchmarks, not only micro-fixtures
- maintained A/B fixtures for reuse-enabled versus reuse-blocked shapes
- per-fixture tracked Aether metrics:
  - `Dup`
  - `Drop`
  - `Reuse`
  - `DropSpecialized`
  - fresh allocations
- tighter consistency checks between dumps, traces, and backend parity

Why this matters:

- transformed-Core evidence is necessary but not sufficient
- release confidence improves when Aether wins are visible on maintained
  workloads and not confused with backend speed differences

### 4. Tooling enhancements

Potential next tooling work:

- richer `--trace-aether` filtering, such as by function name
- optional summary-only trace/report mode
- machine-readable Aether report output, such as JSON
- more explicit verifier/FBIP reason categories in CI and snapshot surfaces

Why this matters:

- as Aether grows, the debugging and validation surfaces need to stay usable
- richer tooling can make maturity work faster without changing semantics

### 5. Longer-term architectural enhancements

Potential longer-term work, explicitly outside the current tranche:

- actor-boundary ownership/sendability rules on top of Aether
- bounded `fip(n)` / `fbip(n)` forms only after the Phase S gate
- shared-memory concurrency or thread-shared/atomic RC in a separate proposal

Why this matters:

- these are plausible future directions, but they should not dilute the
  current single-threaded Aether maturity plan
- concurrency and actor-transfer semantics need their own design space rather
  than being smuggled into 0114 implicitly

## Resolved design decisions

1. **Borrow metadata remains compiler-internal**
   - borrow facts should stay as explicit compiler/Core metadata
   - Flux does not expose borrow annotations in source syntax at this stage

2. **Reuse specialization remains in Core/Aether**
   - CFG/backends lower `Reuse` decisions but should not gain a second reuse-semantics layer

3. **FBIP should mirror Koka semantically, but adapt architecturally**
   - Flux should preserve Koka's semantic intent around fresh allocation, reuse, calls, and joins
   - the checker should adapt to Flux-specific effects, runtime builtins, and backend-neutral lowering structure

4. **Bounded source forms are deferred**
   - `@fip` and `@fbip` remain the only source-level FBIP forms for now
   - numeric bounded forms like `fip(n)` / `fbip(n)` may only be reconsidered after the post-N-U gates and checklist in Phase S are satisfied
   - this proposal makes no syntax commitment for bounded forms and authorizes no implementation work for them

5. **Concurrency remains outside 0114**
   - the Perceus paper and Koka runtime both include thread-shared/atomic RC concerns, but Flux Aether remains intentionally single-threaded today
   - actor transfer semantics and any future `Arc`/thread-shared RC design should be handled in a separate proposal once Aether's single-threaded maturity work has settled

6. **Proof work should target Flux as implemented**
   - the Perceus paper is the semantic reference point, but Flux does not share Koka's exact pass ordering in every detail
   - in particular, Flux reuse recognition/specialization should be proved in the order it actually runs today rather than by assuming the paper's exact pre-insertion reuse pipeline
   - Flux already includes semantic `@fip` / `@fbip` checking and `field_mask` reuse specialization, so those differences must be documented rather than erased

## Success metrics

This proposal is succeeding if:

- Aether parity bugs stop appearing between VM/Cranelift JIT/LLVM
- maintained Aether examples show more `Reuse` and `DropSpecialized` nodes
- `Dup`/`Drop` counts on representative examples go down after analysis improvements
- FBIP diagnostics explain optimization blockers in actionable terms
- false-negative `@fip` / `@fbip` failures on known-good examples go down as ownership/reuse precision improves
- higher-order recursive false negatives continue to fall after post-M precision work
- maintained fixtures with actual `Reuse` / `DropSpecialized` coverage continue to grow
- the Aether benchmark corpus grows toward Koka-style FIP/FBIP coverage with explicit Core-shape assertions
- forwarding/wrapper reuse gaps relative to Koka continue to shrink
- performance evidence exists for Aether-specific wins on maintained workloads, not only backend-to-backend timing comparisons
- precision improvements do not reintroduce backend parity regressions
- proposal 0084 can be updated from "partially implemented" to a status that accurately reflects Koka-comparable Aether maturity
