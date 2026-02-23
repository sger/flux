# Proposal 030: Flow — Explicit Standard Library Modules

**Status:** Proposed  
**Priority:** High  
**Created:** 2026-02-12  
**Scope:** Flow stdlib module architecture (separate from Base prelude semantics)  
**Related:** Proposal 003 (Flow Stdlib), Proposal 017 (Persistent Collections + GC), Proposal 028 Base (`docs/proposals/028_base.md`)

## Summary

`Flow.*` is Flux's explicit standard library namespace (e.g., `Flow.List`, `Flow.Option`, `Flow.Either`).

Boundary:
- `Base` remains auto-injected language-core prelude surface.
- `Flow.*` remains explicit import library surface.

This proposal does not redefine Base resolution/aliasing/exclusion semantics; those are canonical in `docs/proposals/028_base.md`.

## Design

## Module model

- `Flow` modules are regular module APIs from the user perspective.
- Imports are explicit (`import Flow.List`, `import Flow.Option as Opt`, etc.).
- Flow is where combinators and expandable library APIs live.

Example:

```flux
import Flow.List
import Flow.Option as Opt

let nums = list(1, 2, 3, 4, 5)
let result = nums
    |> List.take(3)
    |> map(\x -> x * 2)
    |> List.find(\x -> x > 4)
    |> Opt.unwrap_or(0)
```

## Infrastructure model

- Flow sources may be embedded and exposed via virtual module resolution.
- Resolver should support deterministic precedence and clear override policy.
- Bytecode caching should treat Flow modules as normal compilation units.

## Initial module families

- `Flow.List`
- `Flow.Option`
- `Flow.Either`
- `Flow.Func`
- `Flow.Math`
- `Flow.String`
- `Flow.Dict`

## Dependency on Base

Flow modules depend on Base as foundational runtime vocabulary. Base behavior remains external and canonical in `028_base.md`.

## Implementation phases

1. Add/confirm virtual Flow module registry and resolution policy.
2. Publish/maintain initial Flow module set.
3. Add integration tests and cache compatibility coverage.
4. Expand module APIs without expanding Base by default.

## Risks

- API overlap with Base if module boundaries are not enforced.
- Performance variance for pure-Flux helpers versus runtime-native operations.
- Coupling between Flow modules if dependency boundaries are not disciplined.

## Open questions

1. Should local filesystem Flow modules override embedded Flow by default?
2. Which future helpers should remain Flow-only versus candidates for Base promotion?
3. Should Flow namespace remain `Flow` or be revisited in a later naming proposal?

## References

- Base canonical proposal: `docs/proposals/028_base.md`
- Existing combined historical context: `docs/proposals/029_base_and_flow.md`
