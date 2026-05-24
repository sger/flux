### Added
- `flux repl` — an interactive read-eval-print loop (proposal 0175, Phase 1).
  Type expressions and declarations one at a time and see results immediately;
  earlier `let`/`fn`/`data`/`import` definitions stay in scope for later lines,
  and a bare expression's value is bound to `it`. Meta-commands: `:type <expr>`
  (show an expression's inferred type without running it), `:reset`, `:list`,
  `:help`, `:quit`. Multi-line forms continue the
  prompt until balanced, and a line that fails to compile or run is reported and
  rolled back without disturbing the session. This first phase re-runs the
  accumulated session source each line (the engine is replaced incrementally in
  proposal 0176), so side-effecting top-level declarations re-execute.
