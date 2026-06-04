### Changed
- `flux-lsp`: the prelude now eagerly parse-indexes *every* `lib/Flow/*.flx`
  module at startup, not just the auto-prelude plus whatever the buffer
  imports. Completion (`Http.`, `Json.`, `Map.`, …) and goto-definition into
  the Flow stdlib now cover all 23 modules regardless of imports. The
  extra modules are parsed only — no inference — so member listing and
  goto-definition (AST-driven) work immediately; a buffer that actually
  imports one still triggers the full parse+infer for type information.
