# Reduced Formal Aether Semantics (Proof Scaffold)

This document defines the proof-oriented formal target for Flux Aether.

It is intentionally narrower than full Flux:

- it covers the current single-threaded Aether Core surface
- it mirrors the two-layer structure described in Perceus
- it does not claim mechanized proofs or backend-equivalence proofs

For the implementation overview, see:
- `docs/internals/aether.md`

For the Aether maturity roadmap, see:
- `docs/proposals/0114_aether_perceus_completion_plan.md`

For the landed architecture foundation, see:
- `docs/proposals/implemented/0084_aether_memory_model.md`

For the Perceus paper text used as the alignment reference, see:
- `/home/sger/Code/docs/perceus-tr-v4.txt`

---

## 1. Purpose

Flux currently has:

- executable Aether transformations
- an Aether contract verifier
- regression and parity coverage

Flux does **not** currently have:

- a mechanized proof artifact
- a theorem-level formal proof that matches the Perceus paper

This document exists to bridge that gap. It defines the reduced formal surface
that future proof work should target first.

Like the Perceus paper, this scaffold is split into two layers:

1. a reduced reference-counted semantics for Aether-shaped Core
2. syntax-directed transformation obligations for the Aether passes

The intent is to prove Flux as implemented today, not to assume Flux is a
verbatim copy of Koka's internal pipeline.

---

## 2. Scope of the Reduced Model

The reduced formal model includes these Core/Aether constructs:

- variables and literals
- `Let`
- `LetRec`
- `Lam`
- `App`
- `AetherCall`
- `Dup`
- `Drop`
- `Reuse`
- `DropSpecialized`
- `field_mask`

The reduced model is single-threaded and assumes:

- immutable values
- reference-counted heap objects
- a uniqueness test corresponding to the current `Rc`-style fast path
- no shared-memory concurrency

The reduced model explicitly excludes:

- actors and actor transfer
- thread-shared flags and atomic reference counting
- mutable references and cycle handling
- backend-specific opcodes or runtime helper ABI details
- full effect-handler semantics
- FBIP proof completeness
- backend equivalence proofs

Effects and handlers may be referenced as proof boundaries, but they are not
formalized in the first proof tranche unless a minimal call/return account
needs them.

---

## 3. Reduced Runtime / Heap Model

The reduced semantics should use an abstract heap model that is faithful to the
current Aether implementation without copying backend details.

### 3.1 Values

Values are either:

- non-heap literals
- heap-allocated constructor values
- closures/function values

The model only needs enough structure to explain:

- ownership transfer
- duplication
- early drop
- constructor reuse
- unique/shared branch splitting

### 3.2 Heap objects

Each heap object has:

- a constructor tag
- zero or more fields
- a reference-count state

The abstract semantics may model uniqueness as a predicate `unique(h)` rather
than baking in `Rc::strong_count`, but it must correspond to the current
single-threaded uniqueness fast path.

### 3.3 Reuse tokens

`Reuse` should be specified using an abstract reuse token / reusable location
concept:

- successful reuse means the rebuilt constructor is written into an existing
  compatible allocation
- failed reuse means the semantics falls back to fresh allocation

The model does not need to expose actual pointer identity or helper ABI calls.

---

## 4. Operational Meaning of Aether Nodes

This section defines the semantic intent that future proofs should justify.

### 4.1 `Dup`

`Dup` increases the usable ownership of a heap value so later evaluation can
consume multiple aliases safely.

Proof-oriented meaning:

- it preserves ordinary program meaning
- it updates the heap/accounting state so later consuming uses remain valid

### 4.2 `Drop`

`Drop` releases ownership of a value exactly at the point Aether has proved it
dead.

Proof-oriented meaning:

- dropping a dead value preserves ordinary meaning
- dropping a live value is invalid

### 4.3 `AetherCall`

`AetherCall` makes argument ownership modes explicit:

- borrowed arguments remain available after the call
- owned arguments may be consumed by the callee

The reduced model only needs the ownership contract; it does not need backend
calling conventions.

### 4.4 `Reuse`

`Reuse` expresses a zero-allocation functional update opportunity:

- if the token refers to a compatible reusable allocation, the result may be
  written in place
- otherwise the result is allocated fresh

The semantics must preserve ordinary constructor meaning on both paths.

### 4.5 `field_mask`

`field_mask` is part of `Reuse` semantics:

- set bits identify fields that must be written
- clear bits identify fields that are preserved from the reusable allocation

This is Flux's current implementation of reuse specialization and belongs in
the formalized surface.

### 4.6 `DropSpecialized`

`DropSpecialized` is the explicit unique/shared split:

- unique branch: may assume the scrutinee shell is uniquely owned
- shared branch: must remain conservative

The semantics should be defined so the whole expression is observationally
equivalent to a conservative drop-based treatment of the same value.

---

## 5. Syntax-Directed Aether Obligations

On top of the reduced semantics, Flux needs proof obligations for the
syntax-directed Aether passes.

The first tranche should target these obligations.

### 5.1 No unsafe `Drop`

Statement:

> If `Drop(x, body)` appears in Aether Core, then `x` is not live in `body`.

Current executable witness:

- `src/aether/verify.rs` rejects dropping a still-live binder.

Known gap:

- this is checked operationally today, not proved from a formal semantics.

### 5.2 No reuse-token escape

Statement:

> If `Reuse(token, tag, fields, ...)` appears, the token does not escape into
> any rebuilt field.

Current executable witness:

- `src/aether/verify.rs` rejects token escape into fields.

Known gap:

- provenance legality is verified syntactically, not theorem-proved.

### 5.3 `field_mask` shape safety

Statement:

> Any `field_mask` used by `Reuse` fits the constructor arity and preserves the
> meaning of unchanged fields.

Current executable witness:

- `src/aether/verify.rs` rejects masks that exceed the constructor arity.

Known gap:

- full semantic equivalence of selective-write reuse is not yet formally proved.

### 5.4 `DropSpecialized` side-condition safety

Statement:

> `DropSpecialized(scrutinee, unique_body, shared_body)` is only well-formed
> when both branches respect the scrutinee use restrictions needed by the
> unique/shared split.

Current executable witness:

- `src/aether/verify.rs` rejects malformed scrutinee usage in both branches.

Known gap:

- branch side conditions are operationally checked, not yet justified by a
  reduced semantic proof.

### 5.5 Aether insertion preserves reduced-Core meaning

Statement:

> The syntax-directed Aether transformation preserves the ordinary meaning of
> the reduced Core program while making ownership actions explicit.

Current executable witness:

- no single checker proves this today; confidence comes from the pass design,
  verifier checks, and regression/parity tests.

Known gap:

- this is the main semantics-preservation theorem target for later proof work.

---

## 6. Verifier Mapping

Flux's current verifier is an executable contract checker, not a proof system.

That distinction must remain explicit:

- verifier checks are implementation guards
- formal proofs would justify why those guards are sufficient

Current verifier-aligned obligations include:

- unresolved Aether binders are invalid
- `Drop` on a still-live binder is invalid
- `Reuse` on an invalid/non-heap tag is invalid
- reuse-token escape is invalid
- invalid `field_mask` is invalid
- malformed `DropSpecialized` scrutinee usage is invalid

Future proof work should treat `src/aether/verify.rs` as the executable
counterpart of these obligations, not as the proof artifact itself.

---

## 7. Paper Alignment and Intentional Differences

This scaffold is paper-aligned, but it does not assume Flux and Koka are
identical.

### 7.1 Reuse pipeline ordering differs

The Perceus paper describes reuse analysis before initial reference-count
emission. Flux currently recognizes reuse after Aether shaping/fusion-oriented
passes have already made ownership actions explicit.

Proof implication:

- future proofs must target Flux's actual pass ordering
- the paper remains the semantic reference point, not a literal algorithmic
  transcript

### 7.2 FBIP checking goes beyond the base paper presentation

Flux has semantic `@fip` / `@fbip` checking over transformed Aether Core.
However, proof coverage is still conservative in higher-order, join-heavy, and
opaque-call cases.

Proof implication:

- FBIP should be treated as a consumer of Aether semantics in the first proof
  tranche
- FBIP proof completeness is deferred

### 7.3 Reuse specialization is part of the formalized surface

Flux already implements selective-write reuse specialization with `field_mask`.
This is not a future optimization placeholder and must be included in the
reduced formal model.

---

## 8. Proof Status

Current status should be described precisely:

- **Implemented now**
  - Aether transformations on Core
  - verifier-enforced local invariants
  - test-backed regression and parity evidence
- **Planned next**
  - reduced written formal semantics for single-threaded Aether
  - named theorem/lemma targets tied to existing verifier obligations
- **Deferred**
  - mechanized proof artifact
  - full FBIP completeness proofs
  - actor/concurrency semantics
  - atomic/thread-shared RC proofs
  - backend equivalence proofs

Flux can truthfully claim a proof scaffold is planned once this document and its
cross-links are in place. Flux cannot yet claim theorem-level proof.

---

## 8.1 FBIP Proof Claims

The Perceus comparison around FBIP/proof should distinguish between:

- semantic proof/checking machinery existing at all
- proof coverage being broad enough for routine programmer reliance

Recommended paper-alignment wording:

| Paper idea | Status | Notes |
|---|---|---|
| Semantic proof/checking machinery exists | `Implemented` | Flux has semantic `@fip` / `@fbip` checking with explicit `Fip`, `Fbip { bound }`, and `NotProvable` outcomes in `src/aether/check_fbip.rs` and `src/aether/fbip_analysis.rs`. |
| Proof coverage is broad enough for routine programmer reliance | `Partial` | Higher-order, indirect-call, and control-flow-join cases still fall back to `NotProvable`; maintained Aether snapshots show blockers like opaque callees and control-flow joins losing precise allocation bounds. |
| Formal theorem proof / mechanization exists | `Not implemented` | Flux does not currently have a theorem-level or mechanized proof artifact for Aether or FBIP. |

This distinction matters:

- a proved result from Flux is meaningful
- `NotProvable` is a conservative incompleteness result, not a semantic counterexample
- the current weakness is proof coverage, not absence of semantic checking machinery

Concretely, the maintained `verify_aether` snapshots already show the split:

- the checker can report `FBIP: fip` and still record `fbip: NotProvable`
- current reasons include:
  - indirect or opaque callee behavior
  - control-flow join imprecision

That means the phrase “Programmer can rely on proof” is too coarse on its own.
If such a row is kept in paper-comparison material, it should be replaced by
the three-way split above or at least rewritten to say:

> Programmer can rely on semantic proof when Flux accepts the contract, but
> proof coverage remains conservative in higher-order and join-heavy cases.

---

## 9. Acceptance Criteria for This Scaffold

This proof scaffold is complete when:

- the reduced Aether surface is explicitly enumerated
- the single-threaded runtime assumptions are explicit
- each current verifier check maps to a named proof obligation
- implementation differences from the Perceus paper are documented
- deferred areas are listed explicitly so future work does not drift in scope

At that point, another proposal or follow-up track can choose whether to add:

- a pen-and-paper theorem appendix
- a Lean/Coq/Rocq mechanization
- stronger formal treatment of effects/handlers
