### Fixed
- `flux-lsp`: renaming a declaration no longer corrupts the source. The
  reference collector records a declaration's whole-statement span (e.g.
  `fn twice(n)` or `let x = 1`), and rename used that span directly as the edit
  range — so renaming `twice` replaced the entire `fn twice(n)` signature with
  the new name. Rename now narrows each occurrence to the identifier itself.
  Find-references and document-highlight shared the same root cause and were
  imprecise (they pointed at the whole declaration statement); they now report
  the name range too. The narrowing is one shared helper, also used by linked
  editing.
