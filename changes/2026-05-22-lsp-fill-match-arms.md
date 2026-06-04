### Changed
- The non-exhaustive-match (`E015`) quick fix now offers "Fill missing match
  arms" — one real arm per uncovered variant of the scrutinee's ADT
  (`Circle(_) -> panic("todo")`, `Point { .. } -> panic("todo")`,
  `Red -> panic("todo")`), in declaration order. Bodies use `panic("todo")`
  (a polymorphic, effect-exempt diverging call) so the filled match still type
  checks regardless of the arms' result type. The bare `_ -> ()` catch-all is
  still offered alongside, and remains the fallback when the scrutinee's type
  isn't a buffer-declared ADT. Guarded arms don't count a variant as covered,
  matching the compiler's exhaustiveness check.
