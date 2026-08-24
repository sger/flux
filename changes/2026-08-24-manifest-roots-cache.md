### Fixed
- `flux build` and `flux check` wrote VM cache artifacts that a later
  `flux run` could not consume, failing with "missing global mapping for local
  index". A check-only run stops before execution and so compiles serially,
  while a run takes the parallel VM fast path; the two write different module
  artifacts. Check-only runs now compile for their diagnostics without writing
  VM cache entries.

### Performance
- Resolved package roots are cached against the content of every manifest in
  the dependency graph, so a warm package build no longer re-spawns the Flux
  manifest resolver. A no-op `flux build` drops from ~0.32s to ~0.19s. Editing
  any manifest in the graph invalidates the entry, as does a `CACHE_EPOCH` bump.
