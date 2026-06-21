### Fixed
- **Effectful closures now work.** An un-annotated function literal (closure)
  that performs an effect — e.g. a fiber `yield_now()` / `Channel.send` passed to
  `both`/`fork`/`race`/`Task.spawn`, or a `let`-bound closure — previously failed
  to compile with `E400: Missing Ambient Effect` even though the type checker
  accepted it. The compiler's effect re-check was stricter than inference: it
  built a closure's ambient effect row only from its (empty) syntactic `with`
  clause. Function literals now inherit their enclosing function's ambient effect
  row, matching the type checker. This is sound — a closure can only perform
  effects its defining context permits — and a closure that performs an effect its
  enclosing function lacks is still rejected.

### Added
- Function literals may carry an explicit return type and effect clause:
  `fn(params) -> T with E { ... }`. Both are optional, so `fn(params) { ... }` is
  unchanged. Useful for documenting/constraining a closure's effects where no
  expected type is available (e.g. `let`-bound closures).
