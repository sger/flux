### Added
- The REPL now captures **compound** effectful-expression results into `it`, not
  just primitives (proposal 0176). A bare effectful expression runs inside a
  synthesized `fn main() with IO`, and its result is re-bound as a Flux literal so
  `it` resolves to it on the next line without re-running the effect. The literal
  renderer is now recursive over every shape with a faithful literal form — lists,
  tuples, arrays, `Some`/`Left`/`Right`, and user ADT constructors (`Ctor(..)` /
  nullary `Ctor`, whose name the session's accumulated `data` decls keep in scope) —
  in addition to Int / Float / Bool / String. Rendering is bounded by a node budget,
  so a pathologically large result falls back to "not captured" instead of re-binding
  a multi-megabyte literal. `Unit` (the empty tuple returned by a statement-effect
  such as `print(..)`), `None`, maps, closures, improper lists, and non-finite floats
  still run their effect but are not captured, leaving `it` unchanged.
