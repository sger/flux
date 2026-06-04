### Added
- `flux-lsp`: diagnostics now carry a `codeDescription` link. A diagnostic with
  an `E…`/`W…` code gets a "click for more info" href to the error-code
  reference, deep-linked to that code's anchor
  (`docs/internals/error_codes.md#e015`). `docs/internals/error_codes.md` gained
  a per-code HTML anchor on every code row so the links land on the exact code;
  codes without an anchor fall back to the page top.
