### Added
- `flux-lsp`: read requests (hover, completion, goto-definition, references, rename, signature help, inlay hints, document symbols, semantic tokens) now run on a worker thread, so a slow query no longer blocks the edit → diagnostics pipeline on the main loop.
- `flux-lsp`: a workspace generation counter — a queued read whose document changed before the worker reached it is answered with `ContentModified` instead of being computed against stale state — and `$/cancelRequest` is now honored.

### Changed
- `flux::diagnostics::Diagnostic` stores its source file path as `Arc<str>` instead of `Rc<str>` (the `with_file`/`set_file`/`make_*` builders now take `impl Into<Arc<str>>`), and the type-inference `file_path` plumbing follows suit. This makes `Diagnostic` — and the LSP `Snapshot` — `Send + Sync` so analysis results can cross to the worker thread.
