### Changed
- `flux-lsp`: `textDocument/implementation` (go-to-implementation on a `class`)
  now spans the whole module-graph component and looks inside `module { … }`
  blocks, so an `instance` defined in a sibling module — or nested in a module
  block — is listed, not just top-level instances in the current file.

### Fixed
- `flux-lsp`: the name position of a `public` declaration was computed 7 columns
  too far right (`decl_name_start` added `"public "`'s width on top of a span the
  parser already starts at the keyword). The cursor on a `public class` /
  `public instance` name resolved to the wrong span, so go-to-implementation,
  rename, and call hierarchy did nothing there. The synthesized name position is
  now correct for public declarations across all of those features.
