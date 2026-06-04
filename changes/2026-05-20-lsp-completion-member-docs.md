### Added
- `flux-lsp`: module-member completion items now resolve their `///` doc
  comment into the popup. Since the AST drops doc comments, `completionItem/
  resolve` scans the member's declaration in the module source (via a new
  `Workspace::member_doc`) and fills `documentation` lazily — so `Either.`
  members show e.g. "Case analysis for Either…" when highlighted. Works for the
  Flow stdlib and for sibling user modules alike (the source is found in the
  prelude, then in any analyzed snapshot's module cache).
