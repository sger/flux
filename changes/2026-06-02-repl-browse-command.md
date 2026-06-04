### Added
- The interactive REPL has a new **`:browse [prefix]`** command (`:b` for short)
  that lists every in-scope value binding paired with its type, grouped into the
  session's own `let` / `fn` definitions and the auto-exposed prelude. Unlike
  `:list`, which only echoes your session's declaration *sources*, `:browse` shows
  the inferred type of each name — including the prelude's library surface (`map`,
  `filter`, `abs`, …). An optional prefix filters the listing (e.g. `:browse map`).
  Types are read in bulk from the persistent compiler's accumulated schemes, so no
  per-name re-inference is needed.
