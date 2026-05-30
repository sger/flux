### Fixed
- A `match` expression (or any value containing one) used at **top level** — e.g.
  `let x = match opt { Some(n) -> n, _ -> 0 }`, or a `match` nested in a tuple /
  call argument — no longer miscompiles. At file scope there is no enclosing
  function, so the match's scrutinee temp and arm pattern bindings were allocated
  in the global namespace off the main frame's base pointer, aliasing the operand
  stack: a normal compile failed with `missing global mapping for local index N`,
  and the REPL silently produced wrong values (`("X", match Some(7){Some(n)->n})`
  evaluated to `(7, 7)`). Such expressions now compile inside a synthesized frame,
  so the transients are proper locals. An effectful top-level `match` still
  reports E413 as before.
- A top-level tuple destructure (`let (a, b) = (10, 20)`) no longer fails the
  cached/parallel compile path with `missing global mapping`; its anonymous
  transient slot is now made resolvable to the module linker.
