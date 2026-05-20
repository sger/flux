### Added
- `flux-lsp`: a module-qualified path into a **not-yet-imported sibling**
  module now squiggles too (`E013`, module not imported), with the existing
  "Import `…`" quick fix on the squiggle. Previously only the Flow stdlib and
  already-loaded siblings squiggled at snapshot-build time; a sibling the buffer
  had never imported was invisible to the snapshot (the module graph only
  follows existing imports) and was covered only on-demand by the cursor-driven
  fix. The `Workspace` now keeps an incremental per-file index of `module`
  declaration names — refreshed for one file whenever its content changes, so it
  costs a single re-parse per keystroke, not a whole-workspace scan — and threads
  that set into `Snapshot::build` → `auto_import::missing_import_diagnostics`.
