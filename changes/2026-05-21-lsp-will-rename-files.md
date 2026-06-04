### Added
- `flux-lsp`: `workspace/willRenameFiles` — renaming or moving a `.flx` module
  file now keeps the project compiling. A Flux module's dotted name mirrors its
  path under a search root (`module A.B.C` ⇄ `A/B/C.flx`), so when the editor
  renames such a file the server returns a `WorkspaceEdit` (applied before the
  rename) that rewrites, across the whole workspace: the file's own
  `module <old>` declaration, every dependent's `import <old>` path, and — for
  unaliased imports, which bind the full dotted path — every `<old>.member`
  qualified use. Aliased imports rebind to the alias, so their uses are left
  untouched. Renames that aren't module moves (an entry script with no `module`,
  a move outside the root, or into a non-module directory) produce no edit and
  the rename proceeds untouched.
