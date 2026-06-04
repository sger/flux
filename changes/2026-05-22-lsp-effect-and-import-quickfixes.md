### Added
- LSP quick fixes for two more situations:
  - **Missing effect (`E400`)** — when a function declares an effect row that
    omits an effect its body performs, offer "Add effect `<Effect>` to the
    enclosing function", appending to the existing `with` row (or opening one).
    The effect name is read from the diagnostic's hint.
  - **Unused import** — offer "Remove unused import `<name>`" on an import the
    linter flags as unused (W003), deleting the whole statement line. This is the
    per-import companion to the existing `source.organizeImports` bulk action.
