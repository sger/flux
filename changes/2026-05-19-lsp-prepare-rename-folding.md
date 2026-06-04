### Added
- `flux-lsp`: `textDocument/prepareRename` support — the rename provider now
  advertises `prepareProvider`, so the editor validates the cursor and gets
  the editable identifier range before showing the rename box; a cursor that
  is not on an identifier (a keyword, literal, or blank line) reports rename
  as unavailable.
- `flux-lsp`: `textDocument/foldingRange` support — declaration-aware code
  folding. Every multi-line top-level declaration (and the members nested
  inside a `module` block) gets a folding region; a function's region is
  widened to cover its body block, not just the signature.
