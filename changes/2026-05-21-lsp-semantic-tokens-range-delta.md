### Added
- `flux-lsp`: incremental semantic highlighting — `textDocument/semanticTokens/range`
  and `textDocument/semanticTokens/full/delta`. A `range` request colours just
  the requested span (e.g. the viewport) instead of the whole file. After an
  edit, a `full/delta` request returns a minimal splice (the changed run of
  tokens) against the client's last result instead of re-sending the entire
  token stream; full responses now carry a `resultId` so the client can ask for
  those deltas, and the server keeps the last stream per document (evicted on
  close) to diff against. When the client's baseline is unknown, the delta
  request transparently falls back to a full response.
