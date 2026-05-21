### Added
- `flux-lsp`: `inlayHint/resolve` — inferred-type inlay hints now resolve lazily.
  The initial `textDocument/inlayHint` response stays small (just the `: T`
  label); hovering a hint fills in a tooltip, and a `let`/parameter hint also
  resolves to a text edit that inserts the inferred type as an explicit `: T`
  annotation in the source (accept the hint to make the type concrete).
  Destructuring-pattern hints, where an inline annotation isn't valid syntax,
  resolve to a tooltip only.
