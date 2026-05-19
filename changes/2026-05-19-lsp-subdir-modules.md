### Added
- `flux-lsp`: goto-definition on an `import` statement now jumps into the
  imported module's file (to its `module` declaration) — on the module name
  or on an `as`-alias alike — instead of pointing the import statement at
  itself.

### Fixed
- `flux-lsp`: cross-file analysis now resolves modules in a project that
  lives in a subdirectory of the workspace folder (e.g. an entry and its
  `A/B/C.flx` package both under `examples/type_classes/`). Module-graph
  import resolution previously searched only the workspace roots, so
  `import A.B.C` from such a file never resolved — the file fell back to
  single-file analysis and cross-file goto-definition / completion into its
  modules went dead. The entry's ancestor directories (up to the enclosing
  workspace root) are now added as module search roots.
