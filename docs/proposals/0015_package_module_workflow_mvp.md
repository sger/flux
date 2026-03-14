- Feature Name: Package/Module Workflow MVP
- Start Date: 2026-02-07
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0015: Package/Module Workflow MVP

## Summary
[summary]: #summary

This proposal defines a simple, deterministic package/module workflow for Flux focused on reliability and fast adoption.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Deterministic builds across machines/CI.
2. Simple project/dependency UX (`init`, `add`, `build`, `test`, `run`).
3. Offline-capable workflow once dependencies are cached.
4. Stable module layout/import rules.
5. Actionable diagnostics for dependency/module resolution failures.

### Non-Goals (MVP)

1. Hosted public package registry.
2. Full semver solver complexity.
3. Workspace/monorepo orchestration beyond basic path dependencies.

### Goals

### Non-Goals (MVP)

### Non-Goals (MVP)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **`flux.toml`:** Project manifest with metadata and dependency declarations. - **`flux.lock`:** Generated lock file containing: - exact resolved package graph - source details...
- **`flux.toml`:** [dependencies] utils = { path = "../utils" } jsonx = { git = "https://github.com/org/jsonx", rev = "abc123" }
- **`flux.lock`:** Generated lock file containing: - exact resolved package graph - source details (`path`/`git`) - pinned commit/revision - checksums
- **`.flux/`:** Local cache/build metadata directory (git-ignored).
- **Standard Project Layout:** 1. `src/main.flx` for binary entrypoint. 2. `src/lib.flx` for library entrypoint. 3. `tests/` for integration tests. 4. Module mapping: `foo::bar` -> `src/foo/bar.flx`.
- **CLI Commands (MVP):** 1. `flux init` Create project scaffold (`flux.toml`, `src/main.flx`). 2. `flux add <name> --path <path>` or `flux add <name> --git <url> --rev <rev>` Update manifest + lock. 3....

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals (MVP)

1. Hosted public package registry.
2. Full semver solver complexity.
3. Workspace/monorepo orchestration beyond basic path dependencies.

### Non-Goals (MVP)

### Non-Goals (MVP)

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
