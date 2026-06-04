### Added

- LSP keyword-hover drift gate: [`flux::syntax::token_type::KEYWORDS`](src/syntax/token_type.rs) now exposes a `&[&str]` slice of every lexer-reserved word. Three tests in [`crates/flux-lsp/src/keywords.rs`](crates/flux-lsp/src/keywords.rs) enforce that every lexer keyword has hover documentation (with a small allowlist for the built-in ADT constructors `Some`/`None`/`Left`/`Right` whose hover comes from the AST path), every contextual keyword (`ambient`, `end`, `except`, `exposing`, `resume`) has a doc, and no `KEYWORD_DOCS` entry is orphaned.
- LSP hover content for `ambient`, `end`, `except`, `resume` — the four contextual keywords previously lacked entries.
- LSP goto-definition for import aliases ([`crates/flux-lsp/src/handlers/definition.rs`](crates/flux-lsp/src/handlers/definition.rs)): F12 on the bare alias `A` in `A.map(...)` jumps to its `import` statement; F12 on `A.member` resolves through the alias to the qualified module's source.
- LSP goto-definition for record fields: F12 on `.name` in `alice.name` jumps to the field's declaration in the `data` decl, using the same span synthesis the locator applies to `DataFieldName`.
- LSP goto-definition for effect row variables: F12 on `|e` jumps to its binding occurrence in the enclosing function signature.
- LSP control-flow keyword nav (rust-analyzer-style): F12 on `return` jumps to the enclosing `fn` signature; on `else` to its matching `if`; on `resume` to the enclosing `handle` expression.
- [`crates/flux-lsp/src/navigation_target.rs`](crates/flux-lsp/src/navigation_target.rs): internal `NavigationTarget` abstraction with `full_range` + `focus_range` + `name`, mirroring rust-analyzer's. Used in-process today; the boundary is still `GotoDefinitionResponse::Scalar(Location)` to keep client behavior unchanged. Promoting to `LocationLink` is a single-edit follow-up when the peek-range polish is wanted.

### Changed

- LSP keyword table ([`crates/flux-lsp/src/keywords.rs`](crates/flux-lsp/src/keywords.rs)) no longer carries entries for `Some`/`None`/`Left`/`Right`. Hover on those constructors now surfaces the inferred type (`Option<Int>`, `Either<Error, Value>`) via the AST path, which is strictly more useful than the previous static prose.
- LSP `MemberAccessMember` resolution in goto-def is now alias-aware: the object identifier may be either a loaded module's short name or an `import X.Y as A` alias.

### Deferred (tracked, not implemented in this slice)

- **Instance-method goto-def** (`show(x)` → matching `instance Show<Int> { show(...) }` arm). The type-inference pass uses `class_method_call_info` internally but doesn't surface a per-call-site resolved-instance map on `InferProgramResult`. Implementing this responsibly requires extending inference output; left as TODO.
- **Cross-file user-module goto-def**. Flux's compiler today only resolves `Flow.*` prelude modules via `lsp_support::flow_module_file_for`. There is no user-module-resolution convention (path layout, name-to-file mapping) in the compiler, so an LSP-side resolver would invent semantics that don't match the language. Deferred until the compiler grows user-module support.
- **`GotoDefinitionResponse::Link(Vec<LocationLink>)`** boundary switch. `NavigationTarget` is in place; converting at the LSP boundary requires updating ~5 existing tests and is a stylistic follow-up.
