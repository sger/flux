### Added
- `flux-lsp`: `textDocument/selectionRange` support — smart "expand
  selection" (Shift+Alt+→ / ←). For each cursor position the server returns a
  chain of nested ranges, innermost first: the AST node under the cursor, its
  enclosing expression, statement, block, and the whole `fn`/`module`. Spans
  are collected by walking statements, blocks, and expressions, then ordered
  by source length.
