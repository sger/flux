### Added
- `flux-lsp`: hovering the top-level `fn main` now shows an entry-point card —
  a ``**`main`** — Program entry point`` header, a prose explanation, and the
  inferred signature in a ```flux block — the same shape as keyword-hover
  docs. It is matched on both the `main` symbol and the declaration span, so a
  nested or module-scoped `fn main` (not an entry point) is not annotated.
