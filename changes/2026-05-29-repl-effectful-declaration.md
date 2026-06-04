### Fixed
- `flux repl`: a top-level declaration whose initializer is effectful (e.g.
  `let _ = print("hi")`) now runs its effect once instead of failing with E413 /
  E414. The effect is run inside a synthesized `main`; a *named* effectful binding
  runs the effect but does not persist (effectful results aren't captured yet) and
  the REPL now says so.

### Internal
- Added an in-process test asserting the persistent REPL compiler is append-only —
  compiling a later line never recompiles earlier lines (proposal 0176 acceptance
  criterion).
- Filled in REPL test coverage: `:list`, `:help` / `:?`, `:reset` after `:load`,
  and that an effectful expression does not rebind `it`.
