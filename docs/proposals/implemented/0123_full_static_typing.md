- Feature Name: Full Static Typing — From Gradual to Haskell-Like Type Safety
- Start Date: 2026-03-25
- Status: Implemented
- Last Updated: 2026-04-15
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0119 (Typed LLVM Codegen), Proposal 0120 (Unified Base Library)

# Proposal 0123: Full Static Typing

## Summary
[summary]: #summary

Historical umbrella for Flux's transition away from `Any`-driven gradual
typing toward maintained static typing.

This proposal is no longer the authoritative roadmap for the remaining work.
Use these instead:

- `0156` for maintained front-end static-typing completion
- `0155` for Core validation follow-on work
- `0157` and `0158` for the semantic-vs-representation split and downstream cleanup
- `0160` for the final hardening and closure framing across proof surfaces

## Implementation status
[implementation-status]: #implementation-status

This proposal is **implemented** as a historical umbrella record.

What landed under this broader track:

- typed Core/Aether/native groundwork
- class infrastructure, solving, deriving, and dictionary elaboration
- operator desugaring
- HKT instance resolution
- base HM signature tightening
- the maintained static-typing closure later recorded in `0156`
- the later hardening and closure follow-through recorded in `0158`, `0159`,
  and `0160`

What remains open is no longer “finish static typing from scratch”. The open
areas are hardening and closure work recorded in narrower follow-on proposals:

- `0155` — Core validation / `core_lint`
- `0160` — final proof-bar and proposal-stack closure framing

## Historical phase snapshot
[historical-phase-snapshot]: #historical-phase-snapshot

| Phase | Feature | Historical status |
|---|---|---|
| 1 | Eliminate `Any` fallback (`--strict-types`) | completed later by `0156` |
| 2 | Public API annotations (`--strict`) | already existed |
| 3 | Type classes and constrained polymorphism | landed through `0145` / `0146` |
| 4 | Constraint solver + dictionaries | landed |
| 5 | Higher-kinded support | landed enough for maintained path |
| 6 | Deriving | landed |
| 7 | Typed Core / typed backend groundwork | landed enough for maintained path |

## Key files
[key-files]: #key-files

Representative implementation anchors from the historical umbrella:

- `src/ast/type_infer/static_type_validation.rs`
- `src/types/class_env.rs`
- `src/types/class_dispatch.rs`
- `src/syntax/type_class.rs`
- `src/diagnostics/compiler_errors.rs`
- `src/core/lower_ast/pattern.rs`
- `src/core/lower_ast/mod.rs`
- `src/bytecode/compiler/adt_registry.rs`
- `src/bytecode/compiler/adt_definition.rs`
- `src/bytecode/compiler/constructor_info.rs`

## Current reading rule
[current-reading-rule]: #current-reading-rule

Use this proposal as:

- a historical umbrella record
- a bridge into the narrower follow-on proposals
- a snapshot of the large body of work that was later split into more precise closures

Do **not** use this proposal as the current status source for:

- maintained static typing
- downstream semantic-vs-representation cleanup
- remaining inference-completeness work

Those belong to `0156`, `0157`, `0158`, `0159`, and `0160`.

## Historical material
[historical-material]: #historical-material

Older design narrative, migration text, and broad motivational sections were
trimmed from this document once `0156` became the maintained source of truth.
