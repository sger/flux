### Added
- `flux repl` (proposal 0176): a bare **effectful** expression now captures its
  result into `it` when that result is a primitive value. Because top-level
  effects are rejected (E413), such an expression runs inside a synthesized
  `fn main() with IO` — the only context with the root IO handler — and `main`'s
  return value is re-bound as a literal (a `read_line()` / `read_file(..)` String,
  a `now()`-style Int, a Float or Bool). The literal re-bind is pure, so the
  original effect runs exactly once. Previously an effectful expression's value was
  always discarded. A statement-effect that returns nothing (`print(..)`) keeps its
  output but is still not captured, so `it` is left unchanged; a compound result
  (list / record / ADT) is likewise not yet captured.
