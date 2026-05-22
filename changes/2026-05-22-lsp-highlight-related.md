### Changed
- LSP document highlight is now relation-aware instead of plain same-name:
  - **Read/write** — occurrences are tagged `READ` or `WRITE`, so bindings,
    parameters, pattern bindings and assignment targets render distinctly from
    reads.
  - **Exit points** — with the cursor on `return` (or `fn`), every `return` in
    the enclosing function plus its tail expression are highlighted.
  - **Effect operations** — with the cursor on a `perform`/`handle` operation
    (or its `effect` declaration), every `perform` site and every matching
    `handle` arm for that `(effect, op)` pair are highlighted together.
