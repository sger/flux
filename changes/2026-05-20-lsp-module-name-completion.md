### Fixed
- `flux-lsp`: expression-position completion now offers known module names
  (the Flow stdlib plus imported/sibling modules), so typing `Arr` surfaces
  `Array` ready for a `Array.member` access. Previously the expression list
  held only the buffer's own declarations, locals, and keywords, so a bare
  module prefix produced no popup. Picking a name then `.` flows into
  module-member completion; if the module isn't imported yet, the auto-import
  quick fix offers the `import`.
