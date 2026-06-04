### Added
- `flux repl` (proposal 0176): a self-referential rebind such as `let x = x + 1`
  now reads the binding's **previous** value (so `x` goes 10 → 11), instead of
  failing because the freshly-defined slot was read back uninitialized. The REPL
  detects a single top-level `let name = init` whose initializer makes a free use
  of an already-bound session `name`, and evaluates it as capture-then-rebind:
  `init` is bound to a fresh hidden global while the previous `name` is still in
  scope, then `name` is rebound to that capture. The two steps are atomic — a
  failure in either restores the pre-attempt session. Being value-based, the rebind
  may also change the binding's type (e.g. `xs: [Int]` → `xs: Int` via `len(xs)`).
  A *fresh* `let y = y + 1` with no prior `y` is unchanged: the self-use is still a
  genuine unbound reference.

### Internal
- Self-rebind detection uses a scope-aware free-variable check (`collect_free_vars`)
  on the parsed initializer, so an inner binding that shadows the name (e.g.
  `let x = match v { x -> x }`) is not mistaken for a self-reference.
