### Added
- `flux-lsp`: type hierarchy — `textDocument/prepareTypeHierarchy`,
  `typeHierarchy/supertypes`, and `typeHierarchy/subtypes`. Put the cursor on a
  `class` and the editor shows its place in the type-class graph: **supertypes**
  are the class's declared superclasses (`class Sup<a> => Sub<a> { … }`), and
  **subtypes** are the classes that name it as a superclass plus every
  `instance` of it (the implementing types). Resolution spans the cursor file's
  module-graph component and looks inside `module { … }` blocks, like call
  hierarchy and go-to-implementation. The `typeHierarchyProvider` capability is
  injected into the initialize response because the `lsp-types` version in use
  has no typed field for it.
