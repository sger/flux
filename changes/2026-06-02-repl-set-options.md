### Added
- The interactive REPL now has **`:set` / `:unset`** options (GHCi-style). `:set +t`
  echoes each expression's inferred type (as `it : <type>`) after its value; `:set +s`
  prints the elapsed wall-clock time of each evaluation. The session's existing
  `optimize` / `analyze` compile flags are surfaced as options too (`:set optimize`,
  `:set analyze`), affecting subsequently-compiled lines. Bare `:set` lists every
  option and its current state; `:unset <opt>` disables an option. Multiple options
  can be set at once (`:set +t +s`). Type echoes print to stdout (alongside values);
  timing and confirmations go to stderr.
