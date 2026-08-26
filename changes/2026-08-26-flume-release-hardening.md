### Fixed

- Moved registry lock-diff diagnostics into the pure `Flume.Build.Plan` layer,
  so `--locked` reports the first unsatisfied requirement instead of only a
  generic lock-change message.
- Kept Git lock comparisons and compatibility rendering delegated to the pure
  planner.
- Routed package CLI test caches through the shared cache helper and the
  project-local `target/flux` layout, preventing legacy repository-level cache
  leakage.
- Added regression coverage for deterministic request traces, failure
  termination, lock-write suppression, and workspace cache placement.

### Docs

- Marked Proposal 0177 implemented through Phase 3 and moved it under
  `docs/proposals/implemented/`.
- Documented hosted registry transport as intentionally deferred until Flux is
  released, with remaining design questions tracked as KI-042–KI-049.
