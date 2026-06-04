### Added
- `flux-lsp`: auto-import quick fix (`textDocument/codeAction`). When the
  cursor sits on a module-qualified path (`Json.stringify`,
  `Modules.Math.square`) whose module prefix is not yet imported, the server
  offers a code action that inserts the missing `import` — choosing
  `import Flow.Json as Json` for a bare single-segment reference and the
  unaliased `import Modules.Math` for a full-path reference, matching how each
  binding form actually resolves. Works for both the Flow stdlib (always
  indexed) and not-yet-imported sibling user modules (found via a workspace
  file scan). The fix is diagnostic-independent and offered on demand only —
  no error squiggles — and never fires for ordinary record access or names
  already bound in the buffer.
