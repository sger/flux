### Added
- LSP refactor assists (cursor/selection-driven code actions):
  - **Add type annotation** — on an un-annotated `let`, insert `: <inferred
    type>` after the binding name (the same inferred type the inlay hint shows).
  - **Introduce variable** — hoist a selected expression into a `let` on the
    line above and replace the selection with the new name. A bare-identifier
    selection is skipped.
  - **Inline variable** — replace the use of a `let` binding with its value and
    delete the binding. Offered only when the name occurs exactly twice in the
    file (declaration + one use), which rules out shadowing ambiguity and
    re-evaluation; the value is parenthesized when it isn't a single atom.
