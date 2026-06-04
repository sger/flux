### Added
- LSP operator-fixity hover — the Flux analogue of the Haskell LSP's
  explicit-fixity plugin. Hovering an infix or prefix operator symbol (`+`, `==`,
  `&&`, prefix `!`/`-`, …) now shows its associativity and precedence (e.g.
  ``infixl``, precedence level), plus the inferred type of the operator
  expression. Fixity is read from the parser's `OPERATOR_TABLE` (the single
  source of truth) by the operator's source symbol; hovering an operand is
  unchanged. (Operators desugared at parse time, such as `|>`, have no operator
  node and so aren't covered.)
