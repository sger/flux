- Feature Name: Aether Perceus Completion Plan
- Start Date: 2026-03-20
- Status: Draft
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

The plan is organized around four gaps:

1. **Backend parity** — Aether semantics must be identical across VM, Cranelift JIT, and LLVM.
2. **Ownership analysis quality** — dup/drop insertion and borrowing must be driven by explicit ownership and liveness reasoning, not only local heuristics.
3. **Reuse sophistication** — reuse and drop specialization must be generalized and profitability-aware.
4. **FBIP rigor** — `@fip` / `@fbip` checking must become semantic rather than count-based.

This proposal does **not** replace 0084. It is the execution plan for finishing
0084 to a level that is comparable in spirit and behavior to Koka's Perceus.

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

Compared to Koka, Flux currently has these weaknesses:

1. **Borrowing is too heuristic**
   - Current borrow inference is mostly per-function body scanning.
   - Call-site ownership decisions only handle limited direct-callee cases.

2. **Dup/drop insertion is too local**
   - Current insertion relies heavily on use counting and owned-position checks.
   - This is weaker than Koka's environment-driven reverse-liveness approach.

3. **Reuse is narrower than Koka**
   - Flux mostly recognizes syntactic `Drop -> Con` patterns.
   - Koka has a richer reuse token discipline and a separate reuse specialization pass.

4. **FBIP checking is underpowered**
   - Flux mostly checks constructor/reuse counts.
   - Koka's checker reasons about calls, borrowing, capabilities, and control flow.

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

## Non-goals

This proposal does not:

- replace Flux's shared `Core -> cfg` architecture with a C-backend-specific path
- reintroduce AST fallback into JIT or backend paths
- add a second semantic IR beside `core`
- require actor transfer semantics; that remains future work
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

#### Primary files

- `src/aether/`
- `src/aether/mod.rs`
- `src/aether/verify.rs`
- `src/core/mod.rs`
- `docs/proposals/0084_aether_memory_model.md`

#### Acceptance criteria

- each Aether node has a stable lowering contract
- verifier catches unsafe drop/token patterns
- 0084 documentation reflects actual implementation status

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

## Unresolved questions

1. Should borrow metadata become explicit syntax/IR metadata on functions, or remain compiler-internal only?
2. Should reuse specialization stay purely in Core, or should CFG gain a richer notion of reusable constructor layout?
3. How much of Koka's FBIP surface should Flux mirror directly versus adapting to Flux-specific effects/runtime structure?
4. Should `@fip` eventually support bounded forms like `fip(n)` / `fbip(n)`?

## Success metrics

This proposal is succeeding if:

- Aether parity bugs stop appearing between VM/Cranelift JIT/LLVM
- maintained Aether examples show more `Reuse` and `DropSpecialized` nodes
- `Dup`/`Drop` counts on representative examples go down after analysis improvements
- FBIP diagnostics explain optimization blockers in actionable terms
- proposal 0084 can be updated from "partially implemented" to a status that accurately reflects Koka-comparable Aether maturity
