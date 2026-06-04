### Fixed
- `flux-lsp`: putting the cursor on the head class name of a constrained
  `instance` (`instance Eq<a> => Eq<List<a>>`) now resolves to that name. The
  head sits after `=>`, but the editor was looking for it right after the
  `instance` keyword (where the context constraint is), so hover,
  go-to-definition, and type hierarchy mis-targeted such an instance.
  `Statement::Instance` now carries the parsed head-name span (the twin of the
  `Statement::Class` fix), and the LSP reads it instead of re-deriving the
  position from the keyword.
