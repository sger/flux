### Changed
- `flux-lsp`: `textDocument/signatureHelp` now shows real parameter names and
  the `///` doc comment for callees declared in *other* modules, not just the
  current buffer. A qualified call `M.foo(..)` resolves `foo`'s declaration in
  module `M`'s cached program and source (`Snapshot::module_programs`, already
  in hand — no workspace round-trip on a per-keystroke request); a direct call
  is still resolved in the buffer. Falls back to types only when the declaration
  can't be located (e.g. a function imported unqualified via `exposing`).
