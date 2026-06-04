### Changed
- `flux-lsp`: cleared the clippy lints that failed the workspace
  `clippy --workspace --all-targets --all-features -- -D warnings` gate
  (collapsible `if`/`match`, `manual_contains`) across completion, definition,
  hover, inlay-hints and references, and tightened the worker-thread hover test
  to break on timeout instead of leaving a dead initial assignment. Pure
  cleanup — no behavior change.
