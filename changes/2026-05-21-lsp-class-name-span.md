### Fixed
- `flux-lsp`: putting the cursor on a class declared with a superclass
  constraint (`class Eq<a> => Ord<a>`) now resolves to the class name. The name
  sits after `=>`, but the editor was looking for it right after the `class`
  keyword (where the superclass constraint is), so type hierarchy,
  go-to-implementation, rename, and document/workspace symbols all mis-targeted
  such a class. `Statement::Class` now carries the parsed name span, and the LSP
  reads it instead of re-deriving the position from the keyword.
