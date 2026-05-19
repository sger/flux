### Changed
- `flux-lsp`: keyword hover docs now share one uniform shape — a bold
  ``**`kw`** — summary`` header, a prose paragraph, and a ```flux fenced
  example. The `true` / `false` entries (previously a bare
  `**Boolean literal.**` with no header or example) and `exposing` (missing
  its prose paragraph) were brought into line. A new test
  (`keyword_docs_share_a_uniform_shape`) enforces the template so future
  entries cannot drift.
