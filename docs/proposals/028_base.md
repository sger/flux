# Proposal 028: Base — Auto-Imported Prelude Module

**Status:** Proposed  
**Priority:** High  
**Created:** 2026-02-12  
**Canonical scope:** Base only (prelude/core module architecture)  
**Related:** Proposal 003 (Flow Stdlib), Proposal 008 (Base Functions Module Architecture), Proposal 017 (Persistent Collections + GC), Proposal 026 (Concurrency Model)

> Related docs:
> - Flow stdlib track: `docs/proposals/030_flow.md`
> - Runtime routing internals: `docs/internals/primops_vs_base_functions.md`
> - Base API classification: `docs/internals/base_api.md`

## Summary

This proposal defines `Base` as Flux's synthetic, auto-injected prelude module and makes `Base` the single architectural boundary for language-level core functions.

Goals:
- Replace brittle hard-coded base registration with a canonical Base registry.
- Keep source behavior compatible for existing base calls.
- Formalize name resolution, exclusions, qualification, and shadowing semantics.
- Keep runtime execution strategy (primop/fastcall/generic) as an internal concern.

Non-goals in this phase:
- No user-visible semantic change to existing core function behavior.
- No immediate removal of current Base-surface functions.
- No redesign of Flow module semantics (handled in `030_flow.md`).

## Problem Statement

Current core base functions have three structural issues:

1. **Index coupling risk**
   Base IDs are coupled across compiler and runtime registration points. This creates synchronization risk and hard-to-debug mismatches.

2. **Weak module-level control**
   Core names are globally injected by implementation convention, with no first-class prelude control in the language model.

3. **No explicit prelude contract**
   There is no formal distinction between language-core surface and regular library growth.

## Normative Semantics

## 1) Base identity

- `Base` is a **synthetic module** (no `.flx` file required).
- `Base` is reserved; user-defined modules cannot use this name.
- Base members are Rust-backed runtime functions exposed as language-core APIs.

## 2) Prelude injection

For every script/module compilation unit:

1. The compiler injects Base bindings.
2. If `import Base except [...]` exists, listed names are excluded from unqualified injection.
3. `import Base` is a semantic no-op (already injected).
4. `import Base as X` is invalid.

## 3) Unqualified name resolution precedence

| Priority | Resolution source | Notes |
|---|---|---|
| 1 | Local bindings / params / local functions | Always wins if present. |
| 2 | Explicitly imported symbols/modules | Normal module import behavior. |
| 3 | Injected Base bindings | Fallback core prelude surface. |

## 4) Qualified Base access

- `Base.name(...)` is always resolved through the synthetic Base registry.
- It does not require filesystem module lookup.

## 5) Rejection/error contracts

- `import Base as X` -> compile error (invalid Base aliasing).
- `Base.unknown(...)` -> compile error (unknown Base member).
- Bare call to excluded Base symbol from `except` -> undefined identifier error.

## 6) Shadowing + lint

- Shadowing Base names is legal by default.
- No hard warning is emitted by default.
- Optional lint `W011 SHADOWS BASE FUNCTION` may be enabled to flag accidental shadowing.

## Base Surface Classification

Base retains the current proposal surface, classified as:
- `stable-core`: expected long-term prelude members.
- `provisional-review`: kept in Base now, but reviewed periodically for potential migration to explicit library surfaces.

## Classification criteria

Promotion to `stable-core` favors:
- Ubiquity across programs.
- Runtime dependency (cannot be implemented cleanly in Flux alone).
- Performance sensitivity requiring native/runtime path.
- High UX cost if explicit import were required.

Demotion to `provisional-review` favors:
- Convenience wrappers with lower ubiquity.
- Features that can be reasonably provided via explicit library modules.

## Current Base classification

### stable-core

- I/O and fundamental runtime: `print`
- Core polymorphic collection/type/runtime ops:
  - `len`, `first`, `last`, `rest`, `contains`, `slice`
  - `type_of`, `to_string`
  - `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`, `is_map`
- Core map/hash ops:
  - `keys`, `values`, `has_key`, `merge`, `delete`, `put`, `get`
- Core higher-order pipeline vocabulary:
  - `map`, `filter`, `fold`
- Core string transformations and predicates:
  - `trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, `chars`, `substring`
- Core numeric primitives:
  - `abs`, `min`, `max`

### provisional-review

- Collection convenience/perf helpers:
  - `push`, `concat`, `reverse`, `sort`
- String/container convenience:
  - `split`, `join`

Notes:
- `stable-core` vs `provisional-review` is a governance label, not a behavior change.
- All names above remain available as Base surface in this proposal phase.

## Compatibility and Migration Contract

## Source compatibility

- Existing programs using current core base names continue to compile and run.
- This phase changes architecture and registration model, not user-facing function semantics.

## Runtime/bytecode compatibility requirements

At migration cutover:

1. Base registry becomes the canonical source of truth for core function identity.
2. Index assignment must be deterministic (ordered registry).
3. Bytecode cache version must be bumped to prevent stale index assumptions.

## Explicit non-goals for this phase

- No semantic change to result values/error behavior of existing core functions.
- No forced migration of current Base names into Flow.
- No changes to public language syntax beyond Base-specific directive behavior already specified (`except` and alias rejection).

## Execution Transparency

`Base` defines the language surface. Runtime dispatch strategy remains an implementation detail.

A Base call may execute through:
- primop lowering,
- base fastcall,
- generic base call path.

This proposal does not require users to reason about those paths. Runtime routing specifics are documented in `docs/internals/primops_vs_base_functions.md`.

## Important API / Interface / Type Changes

## AST / parser shape

`Statement::Import` includes Base directive support:

```rust
Statement::Import {
    name: Symbol,
    alias: Option<Symbol>,
    except: Vec<Symbol>,
    span: Span,
}
```

Constraint:
- `except` is valid for Base directive behavior.
- `import Base as X` remains invalid.

## Compiler integration (conceptual)

```rust
inject_base_bindings(except: &[Symbol])
```

Responsibilities:
- Inject Base names into compile scope.
- Apply `except` exclusions deterministically.

## Synthetic qualification path

`Base.<member>` resolution is handled by synthetic Base registry lookup, independent of filesystem modules.

## Error contracts

- Undefined excluded Base symbol (unqualified call).
- Invalid aliasing of Base.
- Unknown qualified Base member.

## Phased Implementation

## Phase 1 — Canonical Base registry

- Introduce deterministic ordered Base registry as source of truth.
- Derive lookup/index structures from that registry.
- Replace duplicated compiler/runtime registration paths.
- Keep behavior unchanged.

## Phase 2 — Base module layering (no behavior change) (Completed)

- Introduced `runtime/base` as the architectural Base layer (registry/policy surface).
- Kept `runtime/base functions/*` as implementation modules during migration.
- Routed Base-facing compiler/runtime registration through the Base layer.
- Deferred mechanical renaming/moves of `base functions` modules until directive semantics were stable.

Rationale:
- Reduces large rename churn during semantic changes.
- Preserves import/test stability while Base behavior is being validated.

Note: The temporary `runtime/base functions/*` migration state was retired in Phase 6.

## Phase 3 — Base directives and qualification (Completed)

- Support `import Base except [...]` behavior.
- Reject `import Base as X`.
- Add synthetic `Base.name(...)` resolution.
- Keep Base-shadowing lint plumbing as optional/follow-up work.

## Phase 4 — Compatibility hardening (Completed)

- Deterministic index assignment is covered by registry/compiler tests.
- Bytecode cache format version was bumped at cutover (`FORMAT_VERSION = 6`).
- VM/JIT parity coverage includes representative Base directive/qualification calls.

## Phase 5 — Documentation and stabilization (Completed)

- Base API classification is published in `docs/internals/base_api.md`.
- Periodic review cadence and criteria for provisional items are documented in `docs/internals/base_api.md`.

## Phase 6 — Base Functions module retirement (mechanical follow-up) (Completed)

- Implementation modules were moved from `runtime/base functions/*` to `runtime/base/*`.
- `runtime/base functions` public surface was removed after call sites/imports/tests migrated.
- Function behavior and diagnostics remained unchanged through the move.

Entry criteria:
- Phase 3 semantics are complete and stable.
- Phase 4 compatibility/parity checks are green.
- No unresolved Base directive semantics remain.

## Phase 7 — Base-only terminology transition (follow-up) (Completed)

- Runtime/value/symbol/opcode terminology migrated to Base-first naming (`BaseFunction`, `SymbolScope::Base`, `OpGetBase`, `OpCallBase`).
- Compatibility aliases were removed (`BuiltinFunction`, `BASE_FUNCTIONS`, `get_base_base*`).
- Bytecode cache format was bumped for the terminology cutover (`FORMAT_VERSION = 7`).
- Compiler/VM/JIT/tests/docs callsites were updated to Base-first APIs.

## Test Cases and Acceptance Criteria

| Area | Scenario | Expected outcome |
|---|---|---|
| Resolution + shadowing | Local `len` shadows Base `len` | Bare `len(...)` resolves local; `Base.len(...)` resolves Base |
| Resolution + exclusions | `import Base except [print]` then `print(...)` | Undefined identifier diagnostic |
| Directive semantics | `import Base` only | No behavior change |
| Directive semantics | `import Base as X` | Deterministic compile error |
| Directive semantics | duplicate/invalid names in `except` | Deterministic diagnostics/handling |
| Synthetic qualification | `Base.name(...)` without file module | Resolves via synthetic registry |
| Synthetic qualification | `Base.unknown(...)` | Deterministic compile error |
| Compatibility | Existing base-based programs | Compile/run unchanged |
| Runtime parity | Representative Base calls in VM and JIT | Same outputs/errors |
| Index/cache safety | Registry/order changes across versions | Cache invalidation on version bump |
| Module retirement safety | `runtime/base functions` removed after migration | No behavior or diagnostic regressions |

## Acceptance Checklist

| Item | Status target |
|---|---|
| Canonical Base-only proposal published in `028_base.md` | Done |
| Name-resolution precedence table finalized | Done |
| Alias rejection + synthetic qualification semantics documented | Done |
| Base classification (`stable-core`/`provisional-review`) documented | Done |
| Periodic review cadence/criteria documented | Done |
| Base Functions ownership retired into `runtime/base` | Done |
| Migration/compat contract documented | Done |
| Acceptance matrix documented | Done |
| Base Functions retirement phase and entry criteria documented | Done |
| Base-only terminology transition completed (aliases removed + cache bump) | Done |

## Open Questions

1. Should `except` eventually support non-Base imports?
2. Should `--no-base` be part of this proposal phase or a follow-up proposal?
3. Should any `provisional-review` names be moved out of Base in a later proposal cycle?
4. Should Base include first-wave concurrency helpers (`spawn`, `send`, `receive`) or leave those to a dedicated concurrency module surface?

## References

- Elixir Kernel (`import ... except` pattern)
- Haskell Prelude (implicit prelude with explicit hiding)
- Flux internals: `docs/internals/primops_vs_base_functions.md`
