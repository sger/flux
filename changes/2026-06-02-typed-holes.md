### Added
- **Typed holes** (GHC-style). Writing `_` — or a named `_foo` — anywhere an
  expression is expected now reports a `TYPED HOLE` diagnostic (**E469**): `found
  hole _ : T`, where `T` is the type required at that position, together with the
  in-scope bindings whose type fits. For example, `map([1, 2, 3], _)` reports
  `found hole _ : (Int) -> a` and lists `even`, `odd`, `signum`, `abs`, … as fits.
  A `_`-prefixed name that *is* in scope is an ordinary variable, not a hole
  (matching GHC). Because holes are surfaced as inference diagnostics, they work in
  both the **REPL** (type `_` in any expression) and the **LSP** (shown inline / in
  Problems as you type) with no surface-specific handling. Fits are found by
  trial-unifying each in-scope binding against the hole's type.
