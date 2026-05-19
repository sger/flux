### Added
- `flux-lsp`: `textDocument/documentHighlight` support — placing the cursor
  on an identifier highlights every occurrence of that symbol within the
  current file. It reuses the find-references symbol resolution and walker,
  scoped to the one file.
