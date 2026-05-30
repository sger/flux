### Added
- `flux repl` (proposal 0176): a user **`effect`** declared on one line can now be
  used (`with` / `perform` / `handle`) on later lines, and a user **`class`** plus
  its **`instance`**s declared on earlier lines stay in scope so a later line can
  dispatch the method. Both previously failed across lines (E407 for effects, E004
  for class methods) because the registries were rebuilt from the prelude/import
  set on every compile; the REPL now accumulates a successful line's effects and
  classes/instances into that set so subsequent lines see them. A failed line is
  rolled back wholesale, so the session is never left inconsistent.

### Internal
- Cross-line REPL tests: a user effect used across three lines, and a class with
  two instances dispatched on a later line.
