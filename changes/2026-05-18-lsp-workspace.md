### Added
- `flux-lsp`: a workspace/VFS layer that interns every project `.flx` file to a stable `FileId` and tracks open buffers separately from on-disk content.
- `flux-lsp`: cross-file analysis — opening a buffer that imports user modules builds the module graph, infers each module in topological order, and threads schemes through, so diagnostics, hover, and goto-definition work across files.
- `flux-lsp`: cross-file find-references and rename — a top-level symbol is searched/renamed across its whole module-graph component, producing a multi-file `WorkspaceEdit`.
- `flux-lsp`: `workspace/didChangeWatchedFiles` support with dynamic `**/*.flx` watcher registration, so on-disk edits to unopened modules refresh dependents.
- `flux-lsp`: closed and never-opened project files stay queryable — request handlers build a file's `Snapshot` lazily from on-disk content, so references and goto-definition still resolve into a file after its editor tab is closed.
- `lsp_support::stash_module_schemes` to publish an already-inferred user module's schemes into a shared compiler.

### Changed
- `flux-lsp`: documents are keyed by interned `FileId` rather than `Uri`; the server advertises multi-root `workspaceFolders` support and discovers project `.flx` files on initialize.
