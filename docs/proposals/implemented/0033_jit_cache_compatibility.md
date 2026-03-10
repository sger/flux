- Feature Name: JIT and Cache Compatibility
- Start Date: 2026-02-18
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0033: JIT and Cache Compatibility

## Summary
[summary]: #summary

Flux currently has a bytecode cache that is compatible with VM execution, but not with `--jit`.
On cache hit, execution returns early via VM and bypasses the JIT path. This creates confusing behavior where: Flux currently has a bytecode cache that is compatible with VM execution, but not with `--jit`.
On cache hit, execution returns early via VM and bypasses the JIT path. This creates confusing behavior where:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

- Preserve backend intent: `--jit` means JIT, always.
- Keep cache correctness: source changes and dependency changes must invalidate stale artifacts.
- Avoid silent backend fallback.
- Provide clear UX in verbose mode.

### Non-Goals

- Full native machine-code persistence in v1.
- Cross-process executable image caching in v1.
- Replacing the existing bytecode cache format immediately.

### Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Problem:** In `src/main.rs`, the bytecode cache load path executes before the JIT branch. On hit, it runs VM and returns. - **Phase 1: Semantic Fix (Immediate):** - Skip byt...
- **Problem:** In `src/main.rs`, the bytecode cache load path executes before the JIT branch. On hit, it runs VM and returns.
- **Phase 1: Semantic Fix (Immediate):** When `--jit` is enabled: - Skip bytecode cache load-and-run fast path. - Continue full parse/module-graph/compile flow and execute through JIT path. - Keep existing bytecode cac...
- **Phase 2: JIT Cache Keying (Metadata Only):** Introduce a JIT cache namespace and key format separate from bytecode cache: - Include source hash + roots hash (same as today) - Include compiler version - Include target tripl...
- **Phase 3: Optional Artifact Caching:** Evaluate caching one of: 1. Cranelift IR/module serialization 2. Backend-independent lowered representation 3. Native object blobs (platform-specific)
- **Current:** - `flux file.flx --jit` may execute VM if bytecode cache hits.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

- Full native machine-code persistence in v1.
- Cross-process executable image caching in v1.
- Replacing the existing bytecode cache format immediately.

### Non-Goals

### Risks

- Slight startup regression for JIT mode in Phase 1 (expected and acceptable).
- Additional complexity once dual cache paths exist.
- Potential user confusion during transition if messages are unclear.

Mitigation: clear verbose diagnostics and documentation update.

### Non-Goals

### Risks

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

### Open Questions

- Should JIT cache be opt-in initially (`--jit-cache`)?
- Should we persist JIT cache under `target/flux/jit/` to avoid format collision?
- Is cross-platform portability required, or can cache be host-specific?

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
