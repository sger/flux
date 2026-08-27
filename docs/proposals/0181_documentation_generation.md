- Feature Name: `flux doc` — documentation generation
- Start Date: 2026-08-27
- Proposal PR:
- Flux Issue:

# Proposal 0181: Documentation Generation

## Summary
[summary]: #summary

Add `flux doc`, a command that reads Markdown documentation comments from Flux
source and generates a navigable static HTML site. The syntax layer gains two
module-level comment forms — `//!` and `/*! */` — to complement the existing
`///` and `/** */` declaration forms. A new `src/doc/` module extracts a
structured documentation model from the AST and renders it. The generator is a
read-only pass over parsed source: it does not depend on type inference, does
not touch either backend, and does not write documentation into `.flxi`
interfaces.

## Motivation
[motivation]: #motivation

Flux already has documentation. It has nowhere to put it.

The standard library carries **665 `///` comment lines across 21 of its 30
modules**, written in Markdown by convention — fenced code blocks, inline
backticks, bullet lists, paragraph breaks. The LSP surfaces them on hover, in
completion resolve, and in signature help. But there is no way to read the
standard library's API without opening its source files, and no way for a Flux
package to publish an API reference at all.

This is the gap that blocks Flux from being usable as a library ecosystem
rather than a language you read the source of. Proposal 0180 covers how Flume
packages are *distributed*; nothing covers how a consumer learns what a package
*offers*.

Three concrete cases:

1. **A user evaluating `Flow.Result`** must open `lib/Flow/Result.flx` and read
   past private helpers to find the public surface. A generated page would list
   the public API with signatures and prose, in declaration-kind sections.

2. **A package author publishing to a registry** has no artifact to point at.
   Today the only answer to "what does this package do?" is "read the source."

3. **A contributor writing a module header** has no syntax for it. Every
   stdlib file today opens with a plain `//` block that the lexer discards:

   ```flux
   // Flow.List — List operations for the Flux standard library.
   //
   // Functions operate on cons lists ([...]) using [h | t] pattern matching.
   ```

   That text is exactly module documentation, written as an ordinary comment
   because no doc form accepts it. 23 of 30 stdlib files open this way. The
   information exists and is thrown away at tokenization.

The cost of *not* doing this rises with the package ecosystem. Every package
published without a documentation story sets the expectation that Flux packages
are read, not referenced.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Writing documentation

Flux has four documentation comment forms. Two exist today and are unchanged:

```flux
/// Apply `f` to every element, returning a new list of the results.
///
/// Effect-row polymorphic: `f` may perform effects, and they propagate to
/// the caller.
public fn map<a, b>(xs: List<a>, f: ((a) -> b with |e)) -> List<b> with |e {
```

```flux
/** Apply `f` to every element. */
public fn map(xs, f) { ... }
```

Two are new, and document the *module* rather than the declaration below:

```flux
//! # Flow.List
//!
//! Utilities for working with cons lists.
//!
//! Argument order follows Flux convention: collection first, function second.

module Flow.List {
    ...
}
```

```flux
/*!
# Flow.List

Utilities for working with cons lists.
*/
```

The bodies are Markdown. Headings, paragraphs, bullet lists, fenced code
blocks, horizontal rules, and tables all work. A blank line separates
paragraphs, exactly as in Markdown.

The rule for which declaration a `///` run attaches to is unchanged: a
contiguous run of doc comments attaches to the declaration on the line
immediately below it, and a blank line ends the run. `//!` runs do not attach to
a declaration at all — they attach to the file's module.

### Generating documentation

```sh
flux doc .                    # document the package in the current directory
flux doc lib/Flow/List.flx    # document a single file
flux doc . --open             # generate, then open in a browser
```

Output lands in `target/doc/`, mirroring cargo:

```
target/doc/
├── index.html
├── Flow/
│   ├── index.html
│   ├── List/index.html
│   └── Result/index.html
└── static/style.css
```

Flags:

| Flag | Default | Meaning |
|---|---|---|
| `--output <dir>` | `target/doc` | Output directory |
| `--private` | off | Include non-public items |
| `--open` | off | Open `index.html` after generating |
| `--check` | off | Parse and extract, emit nothing; non-zero exit on error |

`--check` is intended for CI: it verifies that documentation parses and that
every doc comment attaches to something, without producing output.

### What appears on a page

A module page shows the module's `//!` documentation, then the public items
grouped by kind — Types, Effects, Classes, Functions, Constants. Each item
shows its declared signature, its documentation, and its children: data
variants, class methods, effect operations. Undocumented public items still
appear, with their signature and no prose.

### How contributors should think about it

Documentation extraction is a **read-only pass over the parsed AST**. It runs
before type inference and is independent of both backends. A file that fails to
typecheck still documents. This is deliberate: documentation is often how you
navigate code that does not yet compile.

The corollary is that signatures shown are the **declared** ones, taken from
the AST's `TypeExpr` and rendered with the existing
`TypeExpr::display_with(&Interner)`. An unannotated `public fn` shows
`fn f(x)` with no types. Nearly all stdlib functions are fully annotated, so
this is rarely visible; §"Future possibilities" covers adding inferred
fallback.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Syntax layer

`//!` and `/*!` are currently swallowed silently. Verified by lexing a file
containing all three forms:

```
//! Module docs here.     →  (no token — matched as a `//` comment)
/*! block module doc */   →  (no token — matched as a `/*` comment)
/** block decl doc */     →  DOC_COMMENT " block decl doc "
```

A corpus grep for `//!` and `/*!` across `lib/`, `examples/`, and `tests/`
returns **0 hits**, so this is a pure extension: no existing file changes
meaning.

Changes:

- `src/syntax/token_type.rs` — add `TokenType::ModuleDocComment`.
- `src/syntax/lexer/mod.rs:181-186` — two dispatch arms, `//!` and `/*!`.
- `src/syntax/lexer/mod.rs:358-379` — two matching arms in `skip_ignorable`, so
  the new forms are not consumed as ordinary comments.
- `src/syntax/lexer/comments.rs` — `read_module_doc_line_comment` and
  `read_module_doc_block_comment`, mirroring the existing readers at `:56` and
  `:89` including the unterminated-block diagnostic at `:157`.
- `src/syntax/parser/mod.rs:205` — collect module doc tokens into a buffer
  separate from `doc_comments`.
- `src/syntax/program.rs:20` — add `pub module_doc: Option<String>`, kept out of
  the manual `Debug` impl at `:61-68` alongside `doc_comments`, so AST snapshots
  are unchanged.

**Ordering matters.** `//!` must be tested before `//`, and `/*!` before `/*`,
or the general arm wins. A regression test asserting that `//` and `/* */` are
still skipped guards this.

`read_doc_line_comment` strips the marker and **one** optional leading space,
so indentation beyond that is preserved — which is what makes fenced code
blocks inside doc comments work. The new readers keep that behavior.

### Documentation model

The parser's existing index is `HashMap<usize, String>` keyed by declaration
line (`src/syntax/program.rs:20`). It is lossy in four ways: a key is a line
rather than a declaration identity; there is no item kind; per-line positions
are gone (which already forced `code_lens.rs:74` to re-scan raw buffer text);
and it cannot be serialized to a consumer that never sees the source.

It remains a fine *intermediate*. The structured model is built on top:

```rust
pub struct ModuleDoc {
    pub module_name: String,
    pub source_path: PathBuf,
    pub module_doc: Option<Markdown>,
    pub items: Vec<ItemDoc>,
}

pub struct ItemDoc {
    pub name: String,
    pub kind: ItemKind,        // Fn | Data | Class | Effect | TypeAlias | Const
    pub signature: String,     // via TypeExpr::display_with
    pub doc: Option<Markdown>,
    pub is_public: bool,
    pub span: Span,            // for source links
    pub children: Vec<ChildDoc>,
}

pub struct ChildDoc {
    pub name: String,
    pub kind: ChildKind,       // Variant | Field | Method | EffectOp
    pub signature: String,
    pub doc: Option<Markdown>,
    pub span: Option<Span>,    // None for record fields — see Drawbacks
}

/// Raw Markdown. Never rendered in the compiler.
pub struct Markdown(pub String);
```

`Markdown` is a newtype over `String` so that "this is raw, unrendered text" is
a type-level fact. No compiler code can accidentally emit HTML.

### Module layout

```
src/doc/
├── mod.rs        # build_docs(&[ModuleNode], &Interner, opts) -> DocPackage
├── model.rs      # ModuleDoc, ItemDoc, ChildDoc, Markdown
├── extract.rs    # AST + doc index -> model (no rendering)
├── render/
│   ├── html.rs
│   ├── markdown.rs
│   └── assets.rs
└── search.rs
```

The `extract` / `render` split is load-bearing: extraction is dependency-free
and unit-testable; only `render/` touches the Markdown crate, and only behind
the `doc` feature.

This lives in the root crate, not a workspace crate, because it needs
`Program`, `Statement`, `TypeExpr`, `Interner`, and `ModuleGraph` — all
root-crate types. A separate crate would either depend on the root crate anyway
or force those types public. `crates/flux-lsp` earns separate-crate status by
being a separate binary; `flux doc` is a subcommand, like `fmt` and `lint`.

### Attachment algorithm

Given a `Program` and its `doc_comments`:

1. Walk statements. For `Statement::Module`, record the module name and recurse
   into `body.statements`.
2. For each declaration, look up `doc_comments.get(&stmt.span().start.line)`.
3. For children, look up by `DataVariant.span`, `EffectOp.span`, and
   `ClassMethod.span` — all three carry spans.
4. Module documentation comes from `Program.module_doc`, keyed separately.

Verified against the real standard library by simulating the parser's
attachment rule over `lib/Flow/*.flx`:

```
public decls:                   477
  doc run directly above:       291  (61%)
  undocumented:                 186
"orphan" doc runs:               16
```

Two conclusions. First, 61% coverage means the generator must render
undocumented items gracefully rather than assume documentation exists. Second,
**all 16 apparent orphans are data variants** — for example `Flow/Async.flx:26-41`,
where every `AsyncError` constructor already carries a `///`. They are not
orphans; they resolve through `DataVariant.span`. Child-level documentation is
already written in the standard library and is supported from Phase 2.

Step 2 depends on `stmt.span().start.line` being the declaration's first line.
The probe confirms this holds for 291 real declarations, but the invariant is
currently implicit. Phase 1 adds a regression test pinning it, since a future
parser change to span start would silently break every attachment.

### Module enumeration

`ModuleGraph::build_with_entry_and_module_roots`
(`src/syntax/module_graph/mod.rs:115`) is seeded only with an entry file and
grows by following imports. Counting importers for each `lib/Flow/*.flx`:

```
0 importers: Flow.Debug, Flow.Effects, Flow.Either, Flow.Env, Flow.Numeric
```

**Five of thirty standard library modules have no importer at all.** An
entry-rooted documentation build omits them silently — no error, just missing
pages — and `Flow.Either` is a documented public API.

`flux doc` therefore **enumerates `.flx` files on the filesystem** and parses
each as its own root. This is the single most consequential design constraint
in this proposal, and it is the reason the command does not reuse the existing
graph loader.

Traversal must not follow symlinks out of the tree; each candidate is
canonicalized and verified to be under the root. Module names come from source
and are treated as untrusted when mapped to output paths: `..` and absolute
components are rejected.

### CLI wiring

Per the pattern traced through `lint`:

| Step | Location |
|---|---|
| Enum variant | `src/cli/cmdline.rs:87` |
| Match arm | `src/cli/cmdline.rs:436` |
| Exempt own flags | `src/cli/cmdline.rs:227` — `OWN_FLAGS` |
| Dispatch | `src/cli/mod.rs:60` |
| Implementation | `src/driver/command/doc.rs` (new) |
| Help line | `src/cli/render/text.rs:27-43` |

The `OWN_FLAGS` entry is required, not optional: `reject_unknown_flag_tokens`
rejects `--output` and `--open` without it.

Dispatch uses `exit_status` (`src/cli/mod.rs:66`) so `--check` reports failure
cleanly. `fmt` calls `std::process::exit(1)` directly at `inspect.rs:142`; that
pattern is not followed here.

Package metadata (name, version) comes from Flume via `call_flume`
(`src/cli/package.rs:35`), parsing the `ok<TAB>` reply. The repository's
convention is that `flux.toml` is parsed only by Flume, in Flux — the single
Rust reader at `cache_paths.rs:137` is explicitly documented as "deliberately
conservative… avoids making cache-root discovery a second manifest parser."
Phase 3 may use the directory name and defer the Flume call to Phase 4.

### Markdown rendering

`pulldown-cmark`, feature-gated:

```toml
[features]
doc = ["dep:pulldown-cmark"]

[dependencies]
pulldown-cmark = { version = "0.12", optional = true, default-features = false }
```

Three reasons over `comrak`. It is a pull parser, so raw HTML arrives as
`Event::Html` and is dropped by a filter on an iterator rather than a post-hoc
scrub of a string. It is effectively dependency-free, which matters in a
`Cargo.toml` with six runtime dependencies that already vendors a stub to
remove one transitive dep. And it is what rustdoc uses.

Gating matches `llvm` and `repl`, keeping the dependency out of the staticlib
that native builds link — relevant given the documented Windows Defender
sensitivity to linked imports (`Cargo.toml:5-9`).

### Output determinism

Non-negotiable, and the main implementation trap. `Program.doc_comments` is a
`HashMap`, and several compiler maps are too. Every collection is sorted before
emission: items by (kind, name), modules by name. No timestamps, no absolute
paths, no hash-map iteration order in output. `DocIndex` uses `BTreeMap`.

This repository has been bitten before — `AetherEnv` HashSet iteration once
leaked into Aether snapshots. Deterministic output is also what makes HTML
snapshot tests viable at all.

### Cross-linking

There is no global symbol table spanning modules: `Identifier` is interned and
unique only within an `Interner`, and `ModuleId` is a string. The doc layer
builds its own index:

```rust
struct DocIndex {
    by_path: BTreeMap<String, String>,       // "Flow.List.map" -> "/Flow/List/index.html#fn.map"
    by_name: BTreeMap<String, Vec<String>>,  // "map" -> ["Flow.List.map", "Flow.Array.map"]
}
```

Resolution order: exact qualified path, then same module, then a child of a
same-module item, then an explicitly imported module (via
`ModuleNode.imports`), otherwise unlinked.

Two rules keep links honest. **Ambiguity is never guessed** — `map` exists in
both `Flow.List` and `Flow.Array`, so a bare `map` renders unlinked; a
confidently wrong link is worse than none. And **only explicit ``[`name`]``
links are generated**, never every backtick span: the 665 existing `///` lines
use backticks for prose emphasis (`` `f` ``, `` `acc` ``) that must not become
links.

### Security

- **Raw HTML is stripped.** `Event::Html` and `Event::InlineHtml` are dropped
  from the stream. Doc comments are source text; a generated site must not let
  source inject markup. Stricter than rustdoc, deliberately.
- **Link schemes are allowlisted** to `http`, `https`, `mailto`, and relative
  paths. Comparison happens after lowercasing and stripping ASCII whitespace
  and control characters, since `java\tscript:` and `JaVaScRiPt:` are both real
  evasions.
- **Signatures are escaped through a single helper.** `List<a>`,
  `(a) -> b with |e`, and `Map<String, Int>` all contain markup characters; one
  missed escape corrupts every generic type's page.
- **Stale output is pruned conservatively.** Written files are tracked and
  unknown ones deleted — but only inside the output directory, only matching
  known patterns, and only when a marker file confirms the directory is a
  documentation output. `--output` is user-supplied; `flux doc --output ~/src`
  must not delete a source tree.
- **A parse error in one file is reported and skipped**, the rest are
  documented, and the exit code is non-zero. One bad file does not abort the
  build.

### Interfaces and caching

**Documentation is not written to `.flxi`.** `flux doc` re-parses sources.

`ModuleInterface` (`src/types/module_interface.rs:101-188`) carries no
documentation data today and gains none. Three reasons:

1. Documentation is derivable from source, and `flux doc` has the source.
   Package dependencies resolve to on-disk paths via
   `manifest_roots::resolve_project_roots`. Parsing them is strictly more
   accurate than reading a cached copy.
2. `.flxi` is a *semantic* interface. `CLAUDE.md` requires `bytecode/` stay "a
   narrow leaf… must not gain compile-time or execution logic"; prose in a
   semantic interface is the same category error.
3. It would create a staleness hazard with no fix. `interface_fingerprint`
   (`src/compiler/module_interface.rs:601-661`) excludes `source_hash`, so a
   doc-only edit does not invalidate dependents — a dependent's cached view
   would serve stale documentation indefinitely. Adding documentation *to* the
   fingerprint would fix staleness by making every typo fix rebuild the world.

Existing cache behavior needs no change. A doc edit changes `source_hash` (raw
bytes), invalidating that module's own `.flxi` and forcing a recompile, but not
dependents. That is already correct. `CACHE_EPOCH` is not bumped.

Should binary-only package distribution ever require serialized documentation,
the path is: add `#[serde(default)] pub docs: Option<ModuleDocRecord>`
(matching the convention on 11 of 17 existing fields, pinned by the test at
`src/types/module_interface.rs:369`), populate it in `build_interface` — which
already receives `ast_program: Option<&Program>` at `:98`, so no signature
change is needed — keep it out of `CanonicalInterface`, add a separate
`doc_hash`, and bump `CACHE_EPOCH`.

### Phases

**Phase 1 — module doc syntax.** The lexer, parser, and `Program` changes
above, plus lexer and parser tests including the `span().start.line` invariant.
No dependencies. Self-contained and independently mergeable. Done when `//!`
lexes to its own token, `Program.module_doc` is populated, LSP hover /
completion / code-lens behavior is unchanged, and `cargo test --all` is green.

**Phase 2 — public API extraction.** `src/doc/{mod,model,extract}.rs`;
`pub mod doc;` in `src/lib.rs`. Covers functions, data and variants, classes
and methods, effects and operations, type aliases, constants; visibility
filtering; signatures; nested module ownership. Done when extracting `lib/Flow`
yields every public item with correct signatures, all 30 modules present, and
identical ordering across runs.

**Phase 3 — HTML MVP.** `src/doc/render/`, `src/driver/command/doc.rs`, the six
CLI edits, the `doc` feature and `pulldown-cmark`. Covers the index page, module
pages, sections, signatures, children, breadcrumbs, CSS, intra-module links,
and all four flags. Done when `flux doc .` documents all 30 standard library
modules **including the five unimported ones**, output is byte-identical across
two runs, and snapshots pass.

**Phase 4 — dependencies and cross-links.** Multi-root enumeration via
`resolve_project_roots`; `DocIndex`; cross-module link resolution; package
metadata via `call_flume`, degrading to the directory name if the subprocess
fails. Done when a package with a path dependency documents both, with working
cross-package links.

**Phase 5 — rustdoc-like features.** Independent and individually shippable,
ordered by value: search (`search-index.json` + `search.js`); source pages with
line anchors; `--format markdown`; documentation tests; themes; inferred
signatures via `hm_expr_types`; incremental builds if measurement warrants
them; record-field documentation once spans exist.

Phases 1 → 2 → 3 are strictly sequential. Phase 4 and each Phase 5 item are
independent of one another.

### Testing

Following repository conventions: stage directories, explicit `[[test]]`
targets in `Cargo.toml` (autodiscovery is off), `insta` snapshots.

- **Lexer** (`tests/lexer/lexer_tests.rs`): `//!` and `/*! */` token text
  including multi-line; `//!` at EOF without a trailing newline; unterminated
  `/*!` diagnostic; `////` is not a doc comment; `//!` inside a string literal
  is not a comment; and the regression that `//` and `/* */` are still skipped
  while `///` and `/**` are unchanged.
- **Parser** (`tests/parser/parser_tests.rs`): extend the existing
  `doc_comments_index_keys_the_declaration_below_each_run` and
  `doc_comments_index_attaches_to_module_nested_methods`. Add: `//!` collects
  into module doc and does not attach to the declaration below; a blank line
  and a plain `//` each end a run; Markdown preserved verbatim including
  indentation past the one stripped space; and the `span().start.line`
  invariant per declaration kind.
- **Extraction** (`tests/doc/extract_tests.rs`, new stage): visibility
  filtering with and without `--private`; data variants via `DataVariant.span`,
  using `Flow/Async.flx`'s `AsyncError` and its eight documented variants;
  effect operations; class methods; nested module ownership; undocumented items
  yielding `doc: None`; signature rendering for generics and effect rows; and
  identical ordering across two extractions.
- **Rendering** (`insta` snapshots under `tests/snapshots/doc/snapshots/`): a
  module page; Markdown to HTML for headings, fences, `---`, tables, lists;
  HTML escaping of signatures; raw HTML stripped; a `javascript:` link dropped;
  search-index shape; and byte-identical output across two runs. Snapshots
  cover a small hand-written fixture, not `lib/Flow` — otherwise every
  documentation typo becomes snapshot churn.
- **CLI** (`tests/integration/doc_cli_tests.rs`, new, plus its `[[test]]`
  stanza): spawning `env!("CARGO_BIN_EXE_flux")` with `Scratch` and
  `NO_COLOR=1` per `package_cli_tests.rs:15-30`. Covers `flux doc .` writing
  `target/doc/index.html`; `--output`; `--check` emitting nothing; **a module
  no one imports still being documented**; stale-file cleanup; and no stray
  cache directories.

## Drawbacks
[drawbacks]: #drawbacks

1. **A new runtime dependency.** `pulldown-cmark` is the first addition to a
   deliberately lean six-dependency tree. Feature-gating confines it, but it is
   still a dependency the project must track.

2. **A fourth comment form.** Flux gains `//!` and `/*!` alongside `///` and
   `/** */`. Four forms is what Rust has, and the mapping is familiar, but it
   is more syntax to learn and more lexer paths to maintain.

3. **Record fields cannot be documented.** `DataVariant.field_names` holds bare
   identifiers with no spans (`src/syntax/data_variant.rs:23`), so a doc comment
   above a record field is unreachable by line lookup. Zero such comments exist
   in `lib/Flow` or `lib/Flume` today, so nothing is lost now — but the
   limitation is real and fixing it is a parser change with wide match-arm blast
   radius.

4. **Effects cannot be marked public.** `Statement::EffectDecl` and
   `EffectAlias` have no `is_public` field, unlike every other declaration kind.
   This proposal treats effect declarations as always public, matching current
   module-system behavior, but that is a workaround for a gap in the language.

5. **Declared signatures only.** An unannotated `public fn` documents as
   `fn f(x)`. Using inferred types would couple documentation to type inference
   and prevent documenting code that does not compile — a trade this proposal
   declines, at the cost of weaker output for unannotated code.

6. **Filesystem enumeration documents everything it finds.** Unlike an
   import-rooted walk, a directory walk has no notion of reachability. A stray
   `.flx` file in a package directory gets a page.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why filesystem enumeration rather than the module graph?** Because the module
graph provably loses modules: five of thirty standard library modules have no
importer, and an entry-rooted walk drops them with no diagnostic. Reusing
`ModuleGraph` would be less code and would produce silently incomplete
documentation — the worst failure mode for a reference tool, because the output
looks correct.

**Why not store documentation in `.flxi`?** The brief for this work assumed it
was necessary to document dependencies. It is not: dependencies resolve to
on-disk paths, so the source is available and parsing it is more accurate than
a cached copy. Worse, `interface_fingerprint` excludes `source_hash`, so cached
documentation would go stale in dependents with no invalidation path, and the
fix — putting documentation in the fingerprint — would make every typo
correction rebuild the world.

**Why the root crate rather than a `flux-doc` workspace crate?** The extractor
needs `Program`, `Statement`, `TypeExpr`, `Interner`, and `ModuleGraph`. A
separate crate would depend on the root crate anyway, gaining nothing, or force
those types public, which is worse. The `extract` / `render` split inside
`src/doc/` provides the isolation a crate boundary would, and the `doc` feature
provides the dependency isolation.

**Why `pulldown-cmark` over `comrak`?** Security structure, dependency weight,
and precedent — detailed above. The decisive one is that dropping raw HTML is a
filter on an event stream rather than a sanitizer over a string; correctness by
construction rather than by diligence.

**Why explicit ``[`name`]`` links rather than auto-linking backticks?** The 665
existing `///` lines predate this feature and use backticks for prose emphasis.
Auto-linking would turn `` `f` `` and `` `acc` `` — parameter names in prose —
into links or, worse, into wrong links. Rustdoc requires explicit intra-doc
link syntax for the same reason.

**Alternative considered: extend the LSP instead.** The LSP already surfaces
documentation on hover. But hover requires an editor, an open file, and knowing
what to hover over — it answers "what is this?", not "what does this package
offer?". They are complementary.

**Alternative considered: a Flux-language documentation tool**, in the style of
Flume. Attractive for dogfooding, but it would need to parse Flux, which means
either reimplementing the frontend in Flux or exposing the AST across the
boundary. Proposal 0180 already documents the friction in the `flux → flume`
inversion; adding a second such tool compounds it.

**Impact of not doing this:** the standard library stays readable only as
source, published packages have no reference artifact, and the 665 existing
documentation lines and 23 module header blocks continue to be discarded at
tokenization.

## Prior art
[prior-art]: #prior-art

**Rustdoc** is the direct model, and the mapping is close: `///` and `//!` mean
the same things, output is a static site under `target/doc`, and Markdown is
the body language. Two lessons taken directly. First, rustdoc requires explicit
intra-doc link syntax rather than auto-linking code spans — adopted here.
Second, rustdoc's doc tests are widely cited as its best feature; this proposal
defers them (see Unresolved questions) rather than shipping a weak version.
Rustdoc permits some raw HTML in doc comments; this proposal is stricter and
strips it, since Flux has no legacy of HTML-in-docs to preserve.

**Haddock** (Haskell) is the closest analogue for a language with type classes
and higher-kinded types, and demonstrates that rendering class methods and
instances is tractable. Its `-- |` and `-- ^` distinction — documentation
before versus after the item — is not adopted: Flux has no established
after-the-item convention, and one attachment rule is easier to teach.

**Godoc** (Go) takes the opposite approach: plain comments, minimal markup, no
separate doc syntax. Its lesson is that low-ceremony documentation gets
written. Flux has already chosen the `///` marker, so that fork is behind us,
but it motivates rendering undocumented items rather than hiding them — at 61%
coverage, hiding would conceal most of the standard library API.

**Cargo's `target/doc`** is the layout precedent, and `resolve_cache_root`
already establishes `target/flux` in this repository, so `target/doc` sits
naturally beside it.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

To resolve before this proposal merges:

- **Effect visibility.** Treat `effect` declarations as always public
  (recommended for Phases 1–3, matching current behavior), or add `is_public`
  to `EffectDecl` and `EffectAlias`? The latter is a language change and
  deserves its own proposal rather than arriving as a side effect of
  documentation work.
- **`//!` in a multi-module file.** Every standard library file has exactly one
  `module` declaration, so `//!` unambiguously documents that module. A file
  with two module blocks has no correct answer. Recommendation: reject `//!` in
  such files with a diagnostic rather than guessing or silently attaching to
  the first.

To resolve during implementation:

- Whether `--check` should warn on doc comments that attach to nothing. The
  probe found 16 apparent orphans, all of which resolve as variant
  documentation, so the true orphan rate is near zero and the warning would not
  be noisy — but this is worth confirming against `lib/Flume` before enabling.
- Whether the package index page needs Flume metadata in Phase 3 or can use the
  directory name until Phase 4.

Out of scope for this proposal:

- Documentation tests (`/// >>>`). The LSP supports the syntax at
  `code_lens.rs:74`, but a corpus grep finds **0 uses** in `lib/`. It is not an
  established convention, and needs its own design: what is the expected value,
  how is failure reported, how do effects interact with an evaluated example.
- Record-field documentation, pending spans on `field_names`.
- Consolidating the LSP's duplicate `render_type_expr` (`hover.rs:791`) onto the
  root crate's `TypeExpr::display_with` (`src/syntax/type_expr.rs:78`). The
  duplication exists today; this proposal uses the root-crate method and leaves
  the LSP untouched.

## Future possibilities
[future-possibilities]: #future-possibilities

**Search** is the highest-value Phase 5 item — a static `search-index.json` and
a small `search.js`, with no server component. Rustdoc demonstrates this works
well for a static site.

**Source pages** with line anchors are nearly free: `ItemDoc.span` already
carries the position, so linking a declaration to its rendered source is
plumbing rather than analysis.

**`--format markdown`** reuses the entire model and skips only the HTML layer.
It would let a package embed generated API reference into a README or a
documentation site built with another tool.

**Documentation tests** are the natural long-term extension and the reason
`/// >>>` exists in the code lens today. A design would need to settle expected
values, failure reporting, and effect handling — and would interact with the
test harness described in `run_tests.rs`.

**Inferred signature fallback** via `Compiler.hm_expr_types` would let
unannotated public functions document with real types. It would make
documentation depend on successful inference, so it should be a fallback used
only when the declared type is absent, never a replacement.

**A documentation registry integration** is the endpoint this points at: Flume
publishes a package, and generated documentation is hosted alongside it. That
depends on registry work outside this proposal, but the output format here — a
self-contained static site with deterministic paths — is chosen to make it
possible.

**Cross-package linking to the standard library** would let a third-party
package's documentation link `List<a>` to `Flow.List`. Phase 4 builds the index
that makes it feasible; publishing a stable URL scheme for standard library
documentation is the remaining piece.
