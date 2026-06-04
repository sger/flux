### Added

- LSP goto-definition now returns `GotoDefinitionResponse::Link(Vec<LocationLink>)` instead of `Scalar(Location)`, carrying both `target_range` (the whole declaration) and `target_selection_range` (the identifier only). VS Code's peek-definition view highlights just the name in the destination, matching the rust-analyzer / haskell-language-server UX.
- [`crates/flux-lsp/src/symbol_index.rs`](crates/flux-lsp/src/symbol_index.rs) `Entry` now carries `full_span` (whole declaration) and `focus_span` (identifier only) as separate fields. Models GHC's `NameAnn` / `EpAnn` split where the outer anchor and the inner identifier sub-span are first-class and distinct (`compiler/GHC/Parser/Annotation.hs:581-635`). The `focus_span` is synthesized via the locator's existing `decl_name_start` helper, now `pub(crate)`.
- [`origin_selection_range`](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#locationLink) is populated with the source-side cursor word so VS Code underlines just the identifier the user clicked on (not the whole line) in the peek view.
- Two integration tests assert the focus/full split is preserved end-to-end: `goto_definition_returns_link_with_distinct_focus_and_full_range_for_let` checks `target_range` covers `let answer = 42` (cols 0..15) while `target_selection_range` covers only `answer` (cols 4..10); `goto_definition_origin_selection_covers_cursor_word` checks the source-side `origin_selection_range` width matches the cursor word's length.

### Changed

- [`crates/flux-lsp/src/capabilities.rs`](crates/flux-lsp/src/capabilities.rs) advertises `definitionProvider` as `DefinitionOptions { .. }` (the "with options" shape) instead of the bare boolean, opting into the modern definition-with-LocationLink contract. Clients negotiate `LocationLink` support via the `textDocument.definition.linkSupport` client capability; VS Code advertises it.
- [`crates/flux-lsp/src/handlers/definition.rs`](crates/flux-lsp/src/handlers/definition.rs) `goto_definition` returns `Option<NavigationTarget>` instead of `Option<Location>`. Every branch now produces both ranges where the AST supports it (record fields, top-level decls, local lets); branches collapsed `focus = full` where the underlying parser data exposes only one span (effect ops, data variants, control-flow keywords).
- [`crates/flux-lsp/src/navigation_target.rs`](crates/flux-lsp/src/navigation_target.rs) now stores LSP-coordinate `Range` values (pre-converted using the destination file's `PositionMap`) instead of Flux-coordinate `FluxSpan`. Lets cross-module goto-def use the correct map at the point of conversion. New `into_location_link(origin)` helper for the LSP boundary.

### Closed deferrals

- `GotoDefinitionResponse::Link(Vec<LocationLink>)` boundary switch (previously deferred in `2026-05-15-lsp-keyword-and-goto-def-coverage.md`) is now landed.
