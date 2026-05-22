### Added
- LSP "Go to Type Definition" (`textDocument/typeDefinition`): from an
  expression — or a `let`/pattern binding — jump to the declaration of its
  inferred type's ADT or alias. Resolves same-file declarations first, then any
  cached module (a Flow-prelude or sibling-module type). Built-in types and bare
  type variables yield no target.
