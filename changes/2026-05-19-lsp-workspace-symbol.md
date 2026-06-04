### Added
- `flux-lsp`: `workspace/symbol` support — project-wide symbol search ("Go to
  Symbol in Workspace"). A query matches, case-insensitively, every
  declaration across all discovered project files: functions, `let`s,
  `data`, classes, instances, effects, type aliases, and modules — including
  declarations nested one level inside a `module` block (with the module as
  their container). `(uri, text)` pairs are gathered on the main thread and
  parsed off it, so the search never blocks the edit loop.
