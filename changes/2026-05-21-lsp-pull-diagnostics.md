### Added
- `flux-lsp`: pull-model diagnostics — `textDocument/diagnostic` (LSP 3.17).
  Editors that prefer to fetch diagnostics on demand (on open, on focus, at an
  interval) can now ask the server for a document's problems instead of waiting
  for a push. The report is content-tagged with a `resultId`; re-pulling an
  unchanged document returns a tiny `Unchanged` report (just the id) rather than
  resending the whole set. `handlers/diagnostics.rs::report` reuses the same
  `Snapshot` diagnostics that power push, and runs on the worker thread via
  `dispatch_document_diagnostic`. The capability is advertised with
  `interFileDependencies: true` since editing one module changes its dependents.

### Changed
- `flux-lsp`: push diagnostics (`textDocument/publishDiagnostics`) are now
  suppressed for clients that opt into pulling, so the two sources never paint a
  squiggle twice. After a cross-file edit such a client is nudged with one
  `workspace/diagnostic/refresh` (when it supports it) to re-pull the affected
  dependents — a plain pull only re-fetches the focused document. Clients that
  don't pull keep the existing push behaviour unchanged.
