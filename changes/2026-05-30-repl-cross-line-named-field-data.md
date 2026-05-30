### Added
- `flux repl` (proposal 0176): a `data` type with **named fields** declared on one
  line is now fully usable on later lines — construction
  (`Person { name: "Alice", age: 30 }`), dot access (`alice.age`), and the
  functional spread-update (`{ ...alice, age: alice.age + 1 }`). The session
  accumulates each committed line's top-level `data` declarations and replays them
  into HM inference (so the constructor's named-field metadata is in scope) and into
  the named-field desugar (so the construction / spread / access lower to positional
  form). Previously these reported E082/E430 (construction) and E464 (spread) across
  lines. An unknown field on an earlier-line record still errors (E463) — the
  accumulated metadata is scoped, not a blanket pass.

### Internal
- HM inference gained a narrow `InferProgramConfig::preloaded_adt_data` channel:
  `Statement::Data` declarations from earlier compilation units are predeclared
  before the program's own constructors (so a same-line ADT rebind still wins).
  The REPL engine drives the accumulation; the compiler holds a passive
  `repl_session_adt_data` store that rides the engine's per-line compiler clone, so
  a failed line rolls it back.
