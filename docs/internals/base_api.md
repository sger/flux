# Base API Classification

> Canonical classification for Proposal 028 Phase 5.
> Related:
> - Base proposal: `docs/proposals/028_base.md`
> - Builtin implementation architecture: `docs/internals/builtins.md`
> - Runtime call routing internals: `docs/internals/primops_vs_builtins.md`

This document defines Base surface governance labels.

## Labels

- `stable-core`: expected long-term prelude surface.
- `provisional-review`: kept in Base for now; reviewed periodically for possible migration to explicit modules.

These labels are governance-only. They do not change runtime behavior by themselves.

## Current Classification

### stable-core

- I/O and fundamental runtime:
  - `print`
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

## Promotion and Demotion Criteria

Promotion to `stable-core` favors:
- ubiquity across programs,
- runtime dependency (not cleanly implementable in Flux alone),
- performance sensitivity requiring native/runtime path,
- high UX cost if explicit import is required.

Demotion to `provisional-review` favors:
- lower-ubiquity convenience wrappers,
- features that can be provided cleanly through explicit library modules.

## Periodic Review Process

Cadence:
- At least once per release cycle for minor/major releases.
- Also triggered by any proposal that adds/removes/migrates Base surface functions.

Review checklist:
1. Usage footprint across first-party examples/tests.
2. Runtime coupling and implementation constraints.
3. Performance sensitivity of common call sites.
4. Language UX impact of moving from implicit Base to explicit imports.
5. Compatibility impact (source, diagnostics, cache/index constraints).

Decision outcomes:
- Keep label unchanged.
- Promote `provisional-review` -> `stable-core`.
- Mark for migration out of Base in a future proposal (no same-cycle silent removal).

Compatibility rule:
- Label changes do not remove behavior immediately.
- Any Base-surface removal/migration requires an explicit proposal and migration plan.
