### Added
- `flux-lsp`: `textDocument/documentLink` — `import` module paths are now
  ctrl/cmd-clickable and jump to the module's `.flx` file. Every `import A.B.C`
  whose module is loaded (the Flow stdlib, always indexed, plus sibling user
  modules in the file's component) gets a link over its dotted path, resolved
  eagerly from the same module-path cache goto-definition uses — so no
  `documentLink/resolve` round-trip is needed. Imports that don't resolve to a
  loaded module get no link.
