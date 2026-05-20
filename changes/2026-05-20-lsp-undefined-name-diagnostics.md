### Added
- `flux-lsp`: undefined names are now reported in the editor. HM inference
  recovers silently from unknown identifiers (they become fresh type variables),
  so a dedicated lexical-scope pass (`name_resolution`) now flags them with
  `E004` and a "did you mean?" suggestion. It is conservative to avoid false
  positives: only lowercase value identifiers are flagged (uppercase types,
  constructors, and module qualifiers are left to the type checker); a name is
  resolved if it is bound in any scope, a top-level/module-level declaration, a
  class method, an `exposing` import, a prelude/builtin (`print`, `len`, …), or a
  recognized builtin primop spelling (`cmp_eq`, `string_concat_builtin`, …).
  Member access on a known aliased module also reports members the module does
  not export (`Array.frobnicate`). A regression test runs the pass over every
  shipped `examples/guide/**` and `lib/Flow/**` source to keep false positives at
  zero.

### Changed
- `core`: exposed `CorePrimOp::is_builtin_helper_name`, and `compiler`: made the
  `suggestions` module public (`find_similar_names`), so the new LSP pass can
  recognize builtin call targets and offer the same "did you mean?" suggestions
  the VM compiler does.
