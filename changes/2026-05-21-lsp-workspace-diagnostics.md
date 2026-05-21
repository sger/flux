### Added
- `flux-lsp`: project-wide pull diagnostics — `workspace/diagnostic` (LSP 3.17).
  A client can fetch problems for the analyzed working set — the open buffers
  plus the modules they import (a file's component is analyzed together) — in one
  request, not just the focused document. Each file's entry is content-tagged
  with a `resultId` and reported `Unchanged` (just the id) when nothing changed
  since the client's last sweep, exactly like the per-document pull. By default
  the sweep reports only files that have already been analyzed (open buffers plus
  the modules they import) rather than force-analyzing every `.flx` on disk, so a
  project's example/fixture/vendored files don't flood the Problems panel with
  errors from files the user never opened. The `flux.workspaceDiagnostics.scanAllFiles`
  setting opts into the full-project sweep (every discovered `.flx`) for users who
  want all errors at once; changing it restarts the server. Reports are assembled
  off the main thread (`handlers::diagnostics::workspace_gather` /
  `workspace_report`); the `diagnosticProvider` capability advertises
  `workspaceDiagnostics: true`.
