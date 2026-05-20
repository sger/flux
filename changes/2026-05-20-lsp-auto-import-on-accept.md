### Added
- `flux-lsp`: accepting a module-name completion for a not-yet-imported module
  now inserts its `import` automatically. Each module item in expression
  completion carries the import as `additionalTextEdits` (`import Flow.Array as
  Array` for a Flow short name, `import Lib.App.Main` for a sibling module's
  full path), so picking `Array` from the popup both writes `Array` and adds
  the import in one step — no separate quick-fix. The same applies to module
  *member* completion: `Array.` lists members even when `Flow.Array` is only
  indexed (not imported), and accepting any member adds `import Flow.Array as
  Array` too. Items for modules already in scope (including via an `import … as
  A` alias) carry no edit. Reuses the auto-import binding rules from the
  `textDocument/codeAction` fix.
