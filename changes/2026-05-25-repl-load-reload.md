### Added
- The interactive `flux repl` gained **`:load <file>`** and **`:reload`**
  (proposal 0175). `:load` resets the session to a fresh prelude-loaded state and
  then compiles a `.flx` file's whole source as **one** delta — so every
  intra-file reference resolves in a single compile — keeping its top-level
  definitions in scope for the rest of the session and reporting how many it
  brought in. `:reload` re-bootstraps and re-applies the last `:load`'d file,
  picking up edits on disk. Both replace the live session only on success, so a
  failed load (missing file, parse/type/runtime error) prints the diagnostic and
  leaves the current session intact. `:type` and `:list` see loaded definitions.

### Notes
- `:load` uses replace (not additive) semantics — it resets first — because a
  user `data` / `class` / `effect` re-applied on `:reload` would otherwise collide
  with its earlier definition. Loading a file that defines `fn main` runs that
  `main` once as part of the load.
