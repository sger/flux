### Changed
- `flux-lsp`: richer `textDocument/signatureHelp`. The parameter hint now shows
  the callee's name and real parameter names (`add(x: Int, y: Int) -> Int`) when
  the function is declared in the buffer, plus its `///` doc comment. Each
  parameter is reported as `[start, end)` label offsets so the client highlights
  the exact active parameter even when two share a type, the active index is
  clamped into range (a trailing comma keeps the last parameter highlighted; a
  no-arg call has none), and `,`/`)` are advertised as retrigger characters so
  the popup updates in place as you move between arguments.
