### Changed
- LSP hover now shows a function's `///` doc comment at every reference, not
  only at its declaration. Hovering a bare-identifier use (`twice(21)`) or an
  identifier pattern resolves the name to its declaration — same-file top-level
  or `module`-nested first, then any cached module (so a Flow-prelude or
  sibling-module export used unqualified also surfaces its doc) — mirroring how
  rust-analyzer surfaces docs at use sites. The resolution is name-based and
  matches goto-definition's order (a top-level declaration wins over a local of
  the same name).
