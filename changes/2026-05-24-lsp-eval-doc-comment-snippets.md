### Added
- Evaluate `>>>` snippets in doc comments. Writing `/// >>> 2 + 2` in a doc
  comment surfaces a **▶ Eval** code lens; clicking it runs the expression and
  splices the result back in as a `/// => 4` comment line just below (re-running
  replaces it, so results never stack up). This is the Flux analogue of the
  Haskell Language Server's eval plugin — handy for the language guide and for
  scratch experiments. Evaluation runs out-of-process via a new
  `flux eval "<expr>"` CLI subcommand (the expression is wrapped in a synthetic
  `fn main` and run on the VM), so user code is isolated from the language server
  and a runaway expression is bounded by a timeout. Values render through the
  universal formatter — numbers, lists, tuples, and ADTs all print without a
  `Show` instance — and parse/type/runtime errors are shown as the result rather
  than crashing.
