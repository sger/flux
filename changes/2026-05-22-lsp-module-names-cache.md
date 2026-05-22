### Changed
- `flux-lsp`: the workspace-wide module-name list is now memoized instead of
  rescanned on every keystroke. `Workspace::workspace_module_full_names` drives
  the unimported-module squiggle and auto-import quick fix, and `build_snapshot`
  calls it once per component member — so re-analyzing a K-file module component
  on a single edit ran the O(total-symbols) scan across every file K times. The
  result is cached and invalidated only when the symbol index actually changes
  (`index_symbols`), so the scan now happens at most once per edit and is reused
  by the rest of that edit's per-member snapshot builds. Lazily rebuilt, so
  initial project discovery stays O(total symbols), not O(symbols × files).
