### Changed
- `flux-lsp`: rebuilt semantic highlighting (`textDocument/semanticTokens/full`)
  for rust-analyzer-style coverage. The legend grew from 9 bare token types to
  18 standard types (`namespace`, `type`, `class`, `enum`, `interface`,
  `typeParameter`, `parameter`, `variable`, `property`, `enumMember`,
  `function`, `method`, `keyword`, `comment`, `string`, `number`, `operator`,
  `decorator`) plus 4 standard modifiers (`declaration`, `readonly`,
  `defaultLibrary`, `documentation`). A fresh lexer pass gives every keyword,
  literal, doc-comment, operator and annotation a real span (no more column
  arithmetic, and multi-line strings/comments are split into per-line tokens),
  while identifiers are classified by semantic role from AST-derived name sets
  and the Flow stdlib index: a bare `foo`/`Bar` now resolves to
  function/parameter/variable/enum/namespace/method/… and stdlib references
  carry `defaultLibrary` (e.g. `Array` in `Array.map` is a default-library
  namespace, `map` a default-library method). Declarations get `declaration`
  and immutable bindings get `readonly`.

- `editors/vscode`: force-enable semantic highlighting for `.flx` files and add
  `semanticTokenScopes` fallbacks mapping each semantic token type to a Flux
  TextMate scope, so themes without explicit semantic rules still colour them.
