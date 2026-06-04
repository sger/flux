### Added
- "Change return type to `<inferred>`" fix for return-type mismatches. When a
  function's declared return type disagrees with its body (`E300`, e.g.
  `fn area() -> Bool { 3 + 4 }`), the diagnostic now carries a structured
  suggestion that rewrites the annotation to the inferred type (`-> Int`). It
  renders in the CLI/JSON diagnostics (rustc-style) and the language server
  surfaces it as an "Change return type to `Int`" quick fix — the Flux analogue of
  the Haskell LSP's change-type-signature plugin. The suggested type is always the
  concrete inferred body type, so it is valid Flux source.
