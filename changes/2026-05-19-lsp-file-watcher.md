### Added
- `flux-lsp`: server-side filesystem watching via the `notify` crate, used as a
  fallback for LSP clients that do not advertise dynamic registration for
  `workspace/didChangeWatchedFiles` — on-disk `.flx` edits (a `git checkout`, a
  codegen run) now invalidate dependents even with such a client.

### Changed
- `flux-lsp`: on-disk change detection now sits behind a `loader::Handle` trait
  with two backends — the editor's `didChangeWatchedFiles` registration and a
  `notify` server-side watcher — chosen at `initialize` time from the client's
  capabilities.
