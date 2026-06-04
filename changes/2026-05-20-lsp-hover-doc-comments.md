### Added
- `flux-lsp`: hover now shows a declaration's `///` doc comment above its
  signature. At a declaration site (`fn`, `data`, `effect`, variant) the
  comment is scanned from the current buffer — so it works for any open file,
  user modules included — and at a `Module.member` use site it is scanned from
  that module's cached source. The `///` scanner is shared with
  `completionItem/resolve` (new `doc_comments` module), since the parser drops
  doc comments and both features recover them from source.
