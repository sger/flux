### Fixed
- `flux-lsp`: audit of span→range conversions for declaration imprecision (a
  follow-up to the rename declaration-span fix) found and fixed two issues:
  - The cached symbol index synthesized a type-alias's name range using the
    keyword `type` (4 chars), but the keyword is actually `alias` (5) — so
    goto-definition's focus landed one column off for `alias Foo = …`
    declarations. It now skips the right keyword.
  - Document symbols reported the whole declaration statement as a symbol's
    `selectionRange`, so selecting one in the outline highlighted the entire
    declaration. Each symbol's `selectionRange` is now just its name (the
    AST name span for class/instance heads, located textually otherwise),
    while `range` still covers the full declaration.
