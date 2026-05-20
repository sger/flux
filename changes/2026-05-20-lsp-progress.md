### Added
- `flux-lsp`: `$/progress` reporting around startup. When the client supports
  `window.workDoneProgress`, the server warms the Flow prelude (parse + infer
  the stdlib, index every `lib/Flow/*.flx`) eagerly at `initialized` and
  brackets it with a work-done progress begin/end — so the one-time startup
  cost shows as "Flux: indexing standard library" instead of a silent hang.
  Without workspace roots, or when the client lacks progress support, the
  prelude still loads lazily as before.
