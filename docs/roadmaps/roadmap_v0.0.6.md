# Flux v0.0.6 Implementation Plan

## Overview

**Theme: Finish What We Started + Name the Next Feature**

v0.0.6 has two jobs:

1. **Closure work** — five proposals landed Phase 1 during v0.0.5 and stopped. Each is a real user benefit stuck behind the final phase. Ship them.
2. **One new language feature** — named fields for data types (Proposal 0152). Deferred since v0.0.5; the static-typing closure now makes it safe to land.

No new architectural bets. No new IRs. No new backends. v0.0.5 delivered a huge compiler overhaul; v0.0.6 consolidates.

---

## Current State (v0.0.5 — Complete)

**Foundations delivered in v0.0.5:**

- Static typing closure (0127, 0155, 0156, 0158, 0159, 0160): numeric defaulting, signature-directed checking with rigid skolems (E305), runtime-boundary hardening for ADTs and closures, core_lint (E998) enforced after every simplification round, deterministic scheme rendering, five new Core passes (algebraic, const_fold, canonicalize, specialize, disciplined_inline).
- Explicit Core types + runtime-representation split (0157/0158): `CoreType::Dynamic` and `IrType::Dynamic` removed from the maintained pipeline.
- HKT instance resolution (0150) and type-class dictionary elaboration (0145) landed for the main cases.
- LLVM text-IR backend shipped (0116); VM/LLVM parity check is the regression gate.

**Observable gaps after v0.0.5:**

- No named fields on data types — users still destructure positionally: `Student(name, scores)`.
- Diagnostic rendering improvements proposal (0126) shipped Phase 1 only; Phases 2 and 3 are still painful.
- Total-functions proposal (0135) shipped `safe_div`/`safe_mod`; division operator `/` still panics unless you rewrote the call site.
- Module-scoped type classes (0151) parse but do not enforce scoping semantics end-to-end.
- Package workflow (0015) absent: no `flux init`, no dependency resolution, no publishing story.

---

## Version Goals for v0.0.6

**Primary objectives:**

1. **Named fields for data types** (0152) — dot-access, functional update, named-field pattern matching.
2. **Finish 0151 module-scoped type classes** — Phase 1a and Phase 1b, so classes declared in modules actually respect their scope at inference time.
3. **Finish 0126 Phase 2** — narrow function-definition label spans in multi-site diagnostics.
4. **Finish 0135 Phase 2** — `NonZero<T>` refinement type, so `x / nonzero_y` can skip the `Option` unwrap.

**Secondary objectives:**

5. Package workflow MVP (0015): `flux init`, `flux.toml`, local path dependencies. No registry yet.
6. Finish 0126 Phase 3 — dedup shared related notes across clustered diagnostics.
7. Fix stale example files that still use positional constructor syntax for records.

**Success criteria:**

- `type Point = Point { x: Int, y: Int }` parses, type-checks, and gives `p.x` / `{ p | x: 5 }` access.
- `class Eq` declared inside `module Foo { … }` does not leak to importers that did not name it.
- Running a diagnostic across two distant call sites elides lines between labels and no longer dumps 300 irrelevant rows.
- `x / nonzero(y)` returns `Int` (not `Option<Int>`) under Phase 2.
- `flux init foo && cd foo && flux run` produces a compiling scaffold.
- All v0.0.5 closure tests (`tests/static_typing_closure.rs`, parity sweep) remain green.

---

## Timeline: 6 weeks

```
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 1-2: Named Fields (0152)                                  │
│   ✓ Parser: `Point { x: Int, y: Int }` constructor syntax       │
│   ✓ HM: field-type inference, named-field patterns              │
│   ✓ Core: field-access lowering, functional update `{ p | … }`  │
│   ✓ Runtime boundary: ADT contract carries field names          │
│   ✓ Fixtures: named_field_*.flx + snapshots                     │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 3: Module-Scoped Type Classes Closure (0151)               │
│   ✓ Phase 1a: class visibility threaded through module graph    │
│   ✓ Phase 1b: instance resolution respects module scope         │
│   ✓ Diagnostic: E453 "class not in scope at this import"        │
│   ✓ Update `tests/module_scoped_classes_tests.rs` beyond Step 1 │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 4: Diagnostic Rendering Closure (0126)                     │
│   ✓ Phase 2: narrow signature-only labels on function defs      │
│   ✓ Phase 3: dedup related notes with identical span+message    │
│   ✓ Snapshot refresh across existing diagnostic fixtures        │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 5: NonZero Refinement Type (0135 Phase 2)                  │
│   ✓ Introduce `NonZero<Int>` as a type-level guard              │
│   ✓ `nonzero : Int -> Option<NonZero<Int>>` constructor         │
│   ✓ `/` and `%` accept `NonZero<Int>` and return `Int`          │
│   ✓ Propagate refinement through `let`-bindings where trivial   │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 6: Package Workflow MVP + Release                          │
│   ✓ `flux init <name>` scaffold                                 │
│   ✓ `flux.toml` with name, version, [dependencies] (path only)  │
│   ✓ Module resolution respects `flux.toml` dependency roots     │
│   ✓ Fix stale examples, docs updates                            │
│   ✓ Release sign-off                                            │
└─────────────────────────────────────────────────────────────────┘
```

---

## Milestone Details

### M1: Named Fields (0152) — Weeks 1-2

**Proposal:** [0152_named_fields_for_data_types.md](../proposals/0152_named_fields_for_data_types.md)

**Scope:**

```flux
type Point = Point { x: Int, y: Int }

let origin = Point { x: 0, y: 0 }
let shifted = { origin | x: 10 }
let p_x = origin.x

fn describe(p) {
    match p {
        Point { x: 0, y: 0 } -> "origin",
        Point { x, y }       -> "(" + to_string(x) + ", " + to_string(y) + ")",
    }
}
```

**Implementation steps:**
1. Parser: struct-variant arms in `type` declarations; `{ field: val, … }` in expressions and patterns.
2. HM: field-name table per ADT; constructor synthesis produces named-field schemes.
3. Core: lower field access to `TupleField` keyed by index; keep field-name metadata for display.
4. Runtime contract ([runtime_type.rs](../../src/runtime/runtime_type.rs)): `AdtConstructorContract.fields` gains per-field names for boundary diagnostics.
5. Diagnostics: new codes for duplicate field names, missing fields, unknown-field update.
6. Supersedes 0048 — confirm the old proposal stays in `superseded/`.

**Validation:**
- New fixture family `examples/named_fields/*.flx` + VM/LLVM parity.
- `tests/named_fields_tests.rs` for unit coverage.
- All existing ADT tests remain green (positional constructors unchanged).

---

### M2: Module-Scoped Type Classes Closure (0151) — Week 3

**Proposal:** [0151_module_scoped_type_classes.md](../proposals/0151_module_scoped_type_classes.md)

**Problem:** The module-body validator already accepts `class` / `instance` / `import` inside `module { … }` blocks (Phase 1 Step 1, [tests/module_scoped_classes_tests.rs:1-10](../../tests/module_scoped_classes_tests.rs)). End-to-end resolution still treats those declarations as global.

**Deliverables:**
1. **Phase 1a** — class-env carries `module_path: Option<Identifier>`; `lookup_class(name, caller_module)` respects it.
2. **Phase 1b** — instance resolution rejects instances from modules the caller has not imported; dictionary elaboration emits `E453` when coverage is incomplete.
3. **Phase 2** (stretch) — re-export qualifiers for classes and instances.

**Validation:**
- Extend `tests/module_scoped_classes_tests.rs` past Step 1 to assert: (a) class in-scope in declaring module, (b) class out-of-scope in sibling module without import, (c) instance visible only where both class and type are visible, (d) E453 shape.

---

### M3: Diagnostic Rendering Closure (0126) — Week 4

**Proposal:** [0126_diagnostic_rendering_improvements.md](../proposals/0126_diagnostic_rendering_improvements.md)

**Phase 2 — Narrow signature labels:**
- `FnDecl` grows a `signature_span` (name through parameter list) alongside `name_span`.
- Multi-label diagnostics anchoring on a function use `signature_span` instead of the full body span.

**Phase 3 — Dedup related notes:**
- In `DiagnosticsAggregator`, related notes with identical `(span, message)` tuples collapse. Different messages at the same span do not collapse.

**Validation:**
- Snapshot diff across `tests/snapshots/compiler_error_fixtures/*` should be all shrinkage, no content loss. Manually verify on the five worst offenders listed in the proposal.

---

### M4: NonZero Refinement Type (0135 Phase 2) — Week 5

**Proposal:** [0135_total_functions_and_safe_arithmetic.md](../proposals/0135_total_functions_and_safe_arithmetic.md)

**Scope:**

```flux
fn safe_divide(x: Int, d: Int) -> Option<Int> {
    match nonzero(d) {
        Some(nz) -> Some(x / nz),   // returns Int, not Option<Int>
        None     -> None,
    }
}
```

**Implementation steps:**
1. Introduce `NonZero<T>` as a nominal phantom type built on a private constructor.
2. `nonzero: Int -> Option<NonZero<Int>>` as a total conversion.
3. Overload `/` and `%` to accept `NonZero<Int>` on the right; fall through to `Option<Int>` otherwise.
4. Add the `NonZero<Int>` → `Int` path to the primop dispatcher on both VM and LLVM.
5. Keep Phase 3 (edition change switching `/` to always return `Option`) explicitly out of scope.

**Validation:**
- `tests/non_zero_type_tests.rs` grows refinement cases.
- Parity sweep confirms VM and LLVM agree.

---

### M5: Package Workflow MVP (0015) — Week 6

**Proposal:** [0015_package_module_workflow_mvp.md](../proposals/0015_package_module_workflow_mvp.md)

**MVP scope (explicitly narrow):**

- `flux init <name>` — scaffold a project with `flux.toml`, `src/main.flx`, `tests/`.
- `flux.toml`:
  ```toml
  [package]
  name = "demo"
  version = "0.1.0"

  [dependencies]
  # path deps only in MVP
  shared = { path = "../shared" }
  ```
- Module graph respects `[dependencies]` roots; `--root` flag stays usable as an override.
- No registry, no publish, no version solver. Those go to v0.0.7.

**Validation:**
- `tests/package_workflow_tests.rs`: init → build → run on a two-package scaffold.

---

## Out of Scope for v0.0.6

Explicitly deferred:

- **0040** Macro system — design surface still moving.
- **0109 Phase 1** (computed goto) / **0109 Phase 3** (lazy compilation) — VM perf work, not user-facing.
- **0112 Phase 2** (SSA CFG) — optimization groundwork, large architectural change.
- **0099 Part 1** (IO as algebraic effect) — unblocked by static-typing closure but still needs a design pass.
- **0099 Part 3** (monomorphization) — same.
- **0143** Actor concurrency roadmap Phase 0 — deferred to v0.0.7 or later.
- **0075** Effect sealing, **0082** Effect-directed pipelines, **0083** Typed holes — all active drafts, not yet scheduled.

These stay in the backlog with accurate `Partially Implemented` / `Draft` status in [`0000_index.md`](../proposals/0000_index.md).

---

## Risks and Mitigations

- **Named fields touch the runtime contract layer.** Risk: regressing v0.0.5's boundary-hardening guarantees (see `tests/static_typing_closure.rs`). Mitigation: closure test remains the gating check; add named-field cases before declaring M1 done.
- **Module-scoped classes are a visibility overhaul.** Risk: breaking existing imports that relied on global class lookup. Mitigation: default to permissive lookup and gate strict mode behind a `--strict-class-scope` flag initially.
- **NonZero refinement sets the shape for later refinements.** Risk: over-generalizing before Phase 4 (refinement types at large). Mitigation: keep the mechanism ad hoc and specifically `NonZero`-only; do not introduce an `Refinement<T>` trait yet.
- **Package workflow is easy to over-scope.** Risk: getting pulled into a dependency resolver before v0.0.6 ships. Mitigation: path-only dependencies are the whole MVP; defer version resolution.

---

## Exit Criteria

v0.0.6 ships when:

- Five milestones delivered or explicitly rescheduled with a one-line rationale in this file.
- `cargo test --all --all-features` green, including new M1–M5 test files.
- `cargo run -- parity-check tests/parity` at 100%.
- VM and LLVM parity holds on the full `examples/` corpus.
- Proposal statuses in [`0000_index.md`](../proposals/0000_index.md) updated: 0126 / 0135 / 0151 moved to `Implemented` (where applicable); 0152 moved to `implemented/`; 0015 either moved or set to `Partially Implemented` with a pointer to the MVP scope.
- Changelog fragment in `changes/` per release procedure.

---

## Post-v0.0.6 — What Becomes Next

After v0.0.6, the natural v0.0.7 shape is:

- **Actor/concurrency MVP** (0143) — Phase 0: actor declaration syntax + single-thread scheduler.
- **IO as algebraic effect** (0099 Part 1) — unblocks symmetric effect handling.
- **Macro system Phase 1** (0040) — if the design is ready.
- **Effect sealing** (0075) — capability gating.

These are all proposal-owned work already in backlog; v0.0.7 picks the ones with the cleanest design docs.
