### Changed
- `flux-lsp`: `textDocument/signatureHelp` now shows real parameter names and
  the `///` doc comment for a callee imported *unqualified* via `exposing`
  (`import M exposing (foo)` or `exposing (..)`), not just for buffer-local and
  `M.foo`-qualified callees. The buffer is searched first, then any module whose
  `exposing` clause brings the name into scope (honouring `except`), all from the
  snapshot's cached module programs — no workspace round-trip. Still falls back
  to types only for auto-prelude functions that have no explicit import.
