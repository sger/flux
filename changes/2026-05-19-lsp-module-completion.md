### Added
- `flux-lsp`: `Module.` completion now works for user modules, not just the
  Flow stdlib — typing `M.` (where `M` aliases or names a sibling module)
  lists that module's public functions, `let`s, `data`, classes, effects, and
  type aliases, each with its own `CompletionItemKind` and a rendered
  signature. An `import X.Y as A` alias is followed back to the module.
- `flux-lsp`: goto-definition now resolves an unqualified reference to an
  imported module's member — a Flow-prelude function used bare (`len`,
  `print`) or a sibling user module's export — by searching every cached
  module program when no in-buffer definition matches.
- `flux-lsp`: completion and goto-definition now handle deeply-qualified
  module paths (`A.B.C.member`), not just a single segment — completing after
  `A.B.C.` lists that module's members, completing after a proper prefix
  (`A.`, `A.B.`) lists the next path segment, and F12 on `member` jumps into
  the module file.

### Changed
- `flux-lsp`: `Module.` completion items are now built by walking the target
  module's parsed program rather than the prelude's scheme-derived name list,
  so they carry per-kind icons and signatures; the name-list path remains a
  fallback.
