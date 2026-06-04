### Added
- `flux-lsp`: `textDocument/linkedEditingRange` — put the cursor on an
  identifier and the editor links every same-file occurrence so they edit in
  lockstep (type once, all update) without opening the rename dialog. Like
  document highlight, it resolves occurrences by interned id and is current-file
  only — a symbol with cross-file uses is only linked within this file, so use
  Rename (F2) for a project-wide change. Each declaration span is narrowed to
  the identifier name so all linked ranges have identical text, and a word
  pattern keeps the edit scoped to a valid identifier.
