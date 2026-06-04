### Added
- `flux-lsp`: a module-qualified path whose module isn't imported is now flagged
  in the editor with `E013` (module not imported), turning the previously
  on-demand auto-import into a squiggle-driven one — the existing "Import `…`"
  quick fix is offered on the squiggle. A buffer-wide scan
  (`auto_import::missing_import_diagnostics`) reports each unbound prefix
  (`List` in `List.reverse`, `Modules.Math` in `Modules.Math.square`) using the
  same conservative binding rules as the quick fix, so it only fires for a
  known-but-unbound module (the Flow stdlib is always indexed). The
  cursor-driven fix still additionally covers not-yet-imported sibling modules.

### Changed
- `flux-lsp`: the unknown-module-member diagnostic now uses `E012`
  (UNKNOWN MODULE MEMBER, "Module `X` has no member named `Y`.") instead of the
  generic `E004`, matching what the compiler reports for the same mistake.
