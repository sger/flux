### Changed
- `flux-lsp`: `workspace/symbol` and the auto-import quick fix now read a cached
  per-file declaration index instead of re-parsing every workspace file on each
  request. The `Workspace` keeps a `symbols` map (one `Arc<FileSymbols>` per
  file) refreshed incrementally when a file changes — so a symbol-search
  keystroke or a code-action request is a pure filter, not a whole-workspace
  parse. `workspace/symbol` resolves each declaration's range at index time too,
  so queries do no per-result `PositionMap` work. The `module` entries in the
  same index replace the older per-file module-name index that fed the
  unimported-sibling squiggle, so module-name lookups share one parse with
  symbol search.
