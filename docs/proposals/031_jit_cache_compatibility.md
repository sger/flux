# Proposal 031: JIT and Cache Compatibility

**Status:** Proposed  
**Priority:** High  
**Created:** 2026-02-18  
**Related:** Proposal 029 (Cranelift JIT Backend), Proposal 023 (Bytecode Decode Passes)

## Summary

Flux currently has a bytecode cache that is compatible with VM execution, but not with `--jit`.
On cache hit, execution returns early via VM and bypasses the JIT path. This creates confusing behavior where:

- `flux file.flx --jit` may run VM if cache hits
- `flux file.flx --jit --no-cache` runs JIT

This proposal defines a staged plan:

1. Short-term: make semantics explicit and correct (`--jit` never uses VM cache-hit execution)
2. Mid-term: add dedicated JIT cache metadata and keying
3. Long-term: optionally cache JIT-ready artifacts

## Problem

In `src/main.rs`, the bytecode cache load path executes before the JIT branch. On hit, it runs VM and returns.

That means cache behavior changes execution backend, which violates user intent and makes debugging hard.

## Goals

- Preserve backend intent: `--jit` means JIT, always.
- Keep cache correctness: source changes and dependency changes must invalidate stale artifacts.
- Avoid silent backend fallback.
- Provide clear UX in verbose mode.

## Non-Goals

- Full native machine-code persistence in v1.
- Cross-process executable image caching in v1.
- Replacing the existing bytecode cache format immediately.

## Design

### Phase 1: Semantic Fix (Immediate)

When `--jit` is enabled:

- Skip bytecode cache load-and-run fast path.
- Continue full parse/module-graph/compile flow and execute through JIT path.
- Keep existing bytecode cache behavior unchanged for VM mode.

Verbose output should state:

`cache: bypassed (jit mode)`

This is the minimum change that restores correct semantics.

### Phase 2: JIT Cache Keying (Metadata Only)

Introduce a JIT cache namespace and key format separate from bytecode cache:

- Include source hash + roots hash (same as today)
- Include compiler version
- Include target triple / CPU feature fingerprint
- Include JIT feature set/version marker

Store only metadata in v2:

- final module graph dependency hashes
- JIT compile options
- validation stamp

This enables correctness and observability without persisting native code yet.

### Phase 3: Optional Artifact Caching

Evaluate caching one of:

1. Cranelift IR/module serialization
2. Backend-independent lowered representation
3. Native object blobs (platform-specific)

Selection criteria:

- deterministic invalidation
- fast load benefit
- safety (ABI/version compatibility)
- low operational complexity

## CLI Behavior

### Current

- `flux file.flx --jit` may execute VM if bytecode cache hits.

### Proposed

- `flux file.flx --jit`: always JIT execution.
- `flux file.flx --jit --no-cache`: same execution path, but skip all cache reads/writes.
- `flux file.flx` (no `--jit`): current bytecode cache behavior unchanged.

## Diagnostics and UX

Add explicit backend + cache mode logging under `--verbose`:

- `backend: vm` or `backend: jit`
- `cache: hit/miss/store` in VM mode
- `cache: bypassed (jit mode)` in Phase 1
- future: `cache: jit-hit/jit-miss` when JIT cache is implemented

## Risks

- Slight startup regression for JIT mode in Phase 1 (expected and acceptable).
- Additional complexity once dual cache paths exist.
- Potential user confusion during transition if messages are unclear.

Mitigation: clear verbose diagnostics and documentation update.

## Testing Plan

### Unit/Integration

- Assert `--jit` does not execute VM cache-hit path.
- Assert VM mode still uses bytecode cache hit path.
- Assert `--no-cache` bypasses reads in both backends.

### Regression

- Edit a file between runs and verify both VM and JIT observe changes.
- Run with and without cache to confirm backend remains stable.
- Validate module dependency invalidation behavior remains correct.

## Rollout

1. Ship Phase 1 immediately (semantic correctness).
2. Add instrumentation + tests.
3. Implement Phase 2 keying and metadata.
4. Reassess cost/benefit before Phase 3 artifact caching.

## Open Questions

- Should JIT cache be opt-in initially (`--jit-cache`)?
- Should we persist JIT cache under `target/flux/jit/` to avoid format collision?
- Is cross-platform portability required, or can cache be host-specific?

## Acceptance Criteria

- `--jit` always uses JIT execution path regardless of bytecode cache state.
- Existing VM cache behavior remains unchanged.
- Clear verbose messaging for cache/backend decisions.
- Added tests preventing regression of backend-selection semantics.
