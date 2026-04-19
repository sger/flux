- Feature Name: Flux Language Server (LSP)
- Start Date: 2026-04-18
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0127](implemented/0127_type_inference_ghc_parity.md) (Type Inference Completion), [0155](implemented/0155_core_ir_parity_simplification.md) (Core IR Parity), [0160](implemented/0160_static_typing_hardening_closure.md) (Static Typing Hardening)

# Proposal 0163: Flux Language Server (LSP)

## Summary
[summary]: #summary

Ship a first-party Language Server Protocol implementation (`flux-lsp`) so
editors — VS Code, Zed, Neovim, JetBrains — get diagnostics, hover types,
and go-to-definition without each editor reimplementing Flux support.

The proposal is staged in three phases:

1. **Phase 1 — MVP (diagnostics + hover + goto).** Stateful compiler façade,
   `tower-lsp` stdio transport, spans mapped to LSP ranges. Full recompile
   per keystroke is acceptable for small projects. Completion, rename, code
   actions, and inlay hints are explicitly out of scope for this phase.
2. **Phase 2 — Performance.** Measure MVP latency, then add coarser caching,
   module-graph reuse, and bounded reinference. Evaluate `salsa` only if
   simpler invalidation strategies are insufficient.
3. **Phase 3 — Rich editor features.** Completion, signature help, inlay
   hints, workspace-wide symbol search, and code actions for common
   diagnostics. Rename remains deferred until symbol identity is stable enough
   across modules.

The LSP does **not** duplicate compiler logic. It drives
`crate::compiler::Compiler` as a long-lived library, pushing diagnostics and
exposing type-env queries. Every LSP feature maps to data Flux already
computes.

The concrete Phase 1 deliverable is a **`CompilerSession`** wrapper around
the existing compiler library. The session owns open documents, module graph
state, and cached diagnostics/type information for editor queries.

## Motivation
[motivation]: #motivation

### What's missing today

Flux has a CLI and a parser but no editor integration. The current
developer loop is: write code, save, run `cargo run -- path.flx`, read
terminal diagnostics, repeat. Every other statically-typed functional
language a Flux user might come from (Haskell → HLS, OCaml → ocaml-lsp,
Rust → rust-analyzer, Elm → elm-language-server) has LSP. The absence is
visible.

### What exists that we can reuse

The hard parts of an LSP are: structured diagnostics with source spans,
incremental type inference, a symbol table, and a module graph.
Flux already has all four:

- [`src/diagnostics/`](../../src/diagnostics/) — `Diagnostic` carries
  `Span`, `Severity`, `Hint`, `Related`, stable `ErrorCode` — a near 1:1
  map to `lsp_types::Diagnostic`.
- [`src/ast/type_infer/`](../../src/ast/type_infer/) — `expr_types:
  HashMap<ExprId, InferType>` and `binding_schemes_by_span` already answer
  `hover` and `goto_definition` directly.
- [`src/compiler/symbol_table.rs`](../../src/compiler/symbol_table.rs) and
  [`src/compiler/binding.rs`](../../src/compiler/binding.rs) — powers
  completion and rename.
- [`src/compiler/module_interface.rs`](../../src/compiler/module_interface.rs)
  — already serializes per-module schemes and exports into `.fxc` caches.
  Cross-module hover/goto works without re-parsing the world.
- [`src/syntax/module_graph/`](../../src/syntax/module_graph/) — cross-file
  dependency resolution.

### Why now

Four prerequisites just landed:

- 0160 closed static typing (inferred schemes, canonical rendering, E305).
  Hover output is now stable.
- 0155 shipped `core_lint` and the maintained Core IR. Diagnostic stream is
  now authoritative.
- 0127 closed inference correctness gaps. Expression-level type queries
  return the "right" type.
- The [`src/compiler/`](../../src/compiler/) split (separate from
  `bytecode/` and `vm/`) means the compiler is now a consumable library,
  not a VM appendage.

Before these, an LSP would have locked in half-finished semantics. Now
the compiler surface is coherent enough to expose.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What users get

Install a VS Code extension (or Zed/Neovim equivalent); open a `.flx`
file; see:

- **Diagnostics inline** — same text as `cargo run -- file.flx`, with
  squiggles, hover for hint text, quick-fixes for common codes.
- **Hover on any identifier** — shows the inferred type, scheme, and
  effect row. For a function, the full signature with type parameters
  and class constraints.
- **Goto definition** — jumps across modules using
  `module_interface`-resolved exports. Works for ADT constructors, class
  methods, instances, and dict values.

Phase 1 intentionally does **not** promise completion, rename, or code
actions. Those features depend on stronger symbol identity and incomplete-code
handling than the initial LSP needs.

### What the LSP is not

- Not a reimplementation of the type checker. Inference happens in
  `src/ast/type_infer/` and the LSP calls it.
- Not a second parser. The LSP parses with
  [`syntax::parser::Parser`](../../src/syntax/parser/) and keeps the same
  AST the compiler sees.
- Not a new IDE. It's a protocol server; editors remain in charge of UI.

### Incomplete code behavior

The LSP must spend most of its life on files that do not currently parse or
type-check. Phase 1 therefore defines explicit degraded behavior:

- `didChange` on an incomplete file is best-effort; parser recovery may yield a
  partial AST.
- Diagnostics should still publish for the current file whenever parsing or
  module loading can proceed far enough to produce them.
- Hover and goto-definition may return `None` on broken syntax or unresolved
  nodes; they must not fabricate stale or guessed answers.
- A broken current file should not poison unrelated open files. The session may
  degrade cross-module features for directly affected dependents only.

This keeps the MVP honest about editor reality instead of assuming CLI-style
well-formed input.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Crate layout

New top-level crate under `crates/flux-lsp/` (or new binary in the
existing workspace root):

```
crates/flux-lsp/
├── Cargo.toml             ← depends on `flux` (lib), `tower-lsp`; Phase 2 may add `salsa`
├── src/
│   ├── main.rs            ← bin: tower-lsp stdio driver
│   ├── server.rs          ← impl LanguageServer for FluxLsp
│   ├── db.rs              ← optional incremental query layer (Phase 2, only if needed)
│   ├── text_store.rs      ← Map<Url, Rope> + dirty-tracking
│   ├── range.rs           ← Span ↔ lsp_types::Range (UTF-16 aware)
│   ├── providers/
│   │   ├── diagnostics.rs ← Phase 1
│   │   ├── hover.rs       ← Phase 1
│   │   ├── goto.rs        ← Phase 1
│   │   ├── completion.rs  ← Phase 3
│   │   ├── code_actions.rs← Phase 3
│   │   └── rename.rs      ← Future work / follow-up proposal
│   └── lib.rs
└── tests/
    └── integration.rs     ← boot server, drive LSP requests, assert responses
```

### Phase 1 — MVP

#### Transport

[`tower-lsp`](https://github.com/ebkalderon/tower-lsp) on stdio. ~50 lines
of boilerplate. Async handlers called by the runtime for every client
message.

#### Document store

`text_store: DashMap<Url, Rope>` — `ropey::Rope` for efficient incremental
edits. `didOpen`/`didChange`/`didClose` keep it in sync.

#### Compiler façade

**Audit result (2026-04-18):** `src/compiler/`, `src/ast/`, `src/types/`,
`src/syntax/`, and `src/core/` contain **zero `process::exit` calls** and
**zero shared global state** (no `lazy_static`, no `static mut`, no
`OnceCell`). All 27 `process::exit` calls in the workspace live in
`src/driver/` and `src/cli/`. `Compiler::compile` at
[src/compiler/mod.rs:4451](../../src/compiler/mod.rs#L4451) already
returns `Result<(), Vec<Diagnostic>>` and is safe to call repeatedly.
`expr_types` already has a public accessor,
[`infer_expr_types_for_program`](../../src/compiler/mod.rs#L1494), that
runs inference and returns the `HashMap<ExprId, InferType>` directly.
`errors` and `warnings` are already `pub` fields on `Compiler`.

This means Phase 1's compiler-side work is much smaller than initially
scoped and can stay focused on one boundary object:

1. ~~A `Compiler::compile_non_fatal` variant~~ — **not needed**,
   `compile` is already non-fatal and non-abort.
2. Accessors for `binding_schemes_by_span` and resolved module exports —
   `expr_types` is already public via `infer_expr_types_for_program`;
   confirm similar accessors exist for the other two, add them if not.
3. A **`CompilerSession`** wrapper that owns one `Compiler` and one
   module graph, and exposes the editor-facing API:

```rust
pub struct CompilerSession {
    open_documents: HashMap<Url, Rope>,
    graph: ModuleGraph,
    compiler: Compiler,
    interfaces: HashMap<ModuleName, ModuleInterface>,
    diagnostics_by_file: HashMap<Url, Vec<Diagnostic>>,
}

impl CompilerSession {
    pub fn open(&mut self, uri: Url, text: String);
    pub fn change(&mut self, uri: Url, text: String);
    pub fn close(&mut self, uri: &Url);
    pub fn rebuild(&mut self, uri: &Url) -> Result<(), Diagnostic>;
    pub fn diagnostics(&self, uri: &Url) -> &[Diagnostic];
    pub fn type_at(&self, uri: &Url, pos: Position) -> Option<&InferType>;
    pub fn definition_of(&self, uri: &Url, pos: Position) -> Option<Span>;
}
```

This is the **one real compiler-side addition** Phase 1 needs. Everything
else is LSP plumbing. The compiler library is already LSP-ready at the
boundary: no process exits, no hidden global state, all diagnostics flow
through `Result`.

The few library-level `panic!`s that do exist (in
[src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs),
[src/ast/free_vars.rs](../../src/ast/free_vars.rs), etc.) are ICE-style
pattern-match asserts on compiler-internal invariants; they cannot be
triggered by malformed user input that reaches them, because the
parser/checker rejects malformed input first. These remain acceptable
panics for LSP purposes — they'd indicate a compiler bug, not a user
error.

#### Span → LSP Range

Flux's `Span { start: Position, end: Position }` uses byte offsets.
LSP requires UTF-16 code-unit offsets. Utility:

```rust
pub fn span_to_range(span: Span, rope: &Rope) -> lsp_types::Range {
    lsp_types::Range {
        start: offset_to_utf16(rope, span.start.offset),
        end:   offset_to_utf16(rope, span.end.offset),
    }
}
```

Subtle but standard. `ropey` has built-in byte-to-char conversion; char
indices then convert to UTF-16 via the existing `str::encode_utf16`.

#### Provider sketches

```rust
// diagnostics.rs
pub fn publish(session: &CompilerSession, client: &Client, uri: &Url) {
    let diags = session.diagnostics(uri).iter()
        .map(|d| to_lsp_diagnostic(d, session.rope(uri)))
        .collect();
    client.publish_diagnostics(uri.clone(), diags, None);
}

// hover.rs
pub fn hover(session: &CompilerSession, uri: &Url, pos: Position) -> Option<Hover> {
    let ty = session.type_at(uri, pos)?;
    let text = format_scheme(ty);  // reuse src/types/scheme.rs canonical rendering
    Some(Hover { contents: HoverContents::Markup(markdown(text)), range: None })
}

// goto.rs
pub fn definition(session: &CompilerSession, uri: &Url, pos: Position) -> Option<Location> {
    let def_span = session.definition_of(uri, pos)?;
    Some(Location { uri: span_file_uri(def_span), range: span_to_range(def_span, …) })
}
```

#### Workspace and root rules

Phase 1 should make conservative workspace guarantees:

- The current open document plus successfully loaded dependencies are the unit
  of correctness.
- Standalone files outside a Cargo/project root still work in single-file mode.
- Multi-root editor workspaces are supported only insofar as each opened file
  can resolve a valid Flux root set.
- Unsaved buffer contents override on-disk contents for the active document;
  dependencies continue to load from disk in Phase 1.

This keeps editor semantics understandable and avoids overcommitting on
workspace-wide correctness before the session model is proven.

### Phase 2 — Performance and incrementalization

#### Problem

Phase 1 recompiles the full module graph on every `didChange`. For a
100-module project that's hundreds of ms per keystroke — usable, not
great.

#### Strategy

Phase 2 should proceed in this order:

1. measure real MVP latency on small and medium Flux projects,
2. add debounce in the LSP layer,
3. cache parsed AST/module graph data for unchanged files,
4. cache module interfaces aggressively,
5. only then evaluate whether a query system such as `salsa` is justified.

The proposal intentionally does **not** commit Flux to `salsa` up front. The
compiler should not be reorganized around a query engine until Phase 1 proves
that simpler invalidation is insufficient.

#### Optional query surface

If coarse caching and interface reuse are still not enough, the natural
incremental boundaries are:

1. **`parse(file: SourceFile) -> Arc<Program>`**
   Invalidated only when that file's text changes.

2. **`module_interface(module: ModuleName) -> Arc<ModuleInterface>`**
   Invalidated when the file's parse output changes or any of its
   imports' interfaces change.

3. **`diagnostics(file: SourceFile) -> Arc<Vec<Diagnostic>>`**
   Invalidated when the file's parse changes or any imported interface
   changes. Interface-only changes (e.g., editing an unrelated module's
   private body) do not invalidate unrelated files.

These are the three natural boundaries regardless of implementation
strategy. If Flux adopts a query framework later, it should preserve the
existing compiler passes inside those boundaries rather than rewriting
semantic logic around the query engine.

#### If Phase 2 adopts `salsa`

If measured latency shows a need for a query framework, `salsa` remains a
strong option because it matches compiler-shaped dependency graphs and has
clear prior art in the Rust tooling ecosystem. The proposal does not
require it up front.

### Phase 3 — Refactors and workspace features

Once Phase 2 lands:

- **Code actions**:
  - `E013 MODULE_NOT_IMPORTED` → offer "Add import `Foo`"
  - `E007 UNKNOWN_VARIABLE` + close spelling match → "Did you mean `Foo`?"
  - Unused binding warning → "Rename to `_<name>`"
- **Signature help** — on `(` inside a function call, show the parameter
  list with the currently-typed arg highlighted. Uses the function's
  scheme from `binding_schemes_by_span`.
- **Inlay hints** — inferred types after `let` bindings and lambda
  params. Toggle via client setting. Uses `expr_types`.
- **Workspace symbols** — fuzzy search over all `binding_schemes_by_span`
  keys across the graph.
- **Document symbols / outline** — top-level `fn`/`let`/`type`/`class`
  declarations from the parsed `Program`.
- **Rename across modules** — likely worth a follow-up proposal once
  symbol identity, imports, and shadowing behavior are specified
  rigorously enough for safe workspace edits.

## Implementation contract

### Phase 1 (MVP) deliverables

- `crates/flux-lsp/` crate, one `flux-lsp` binary.
- VS Code extension (`editors/vscode-flux/`) that spawns the binary and
  registers `.flx` as the file pattern.
- Compiler changes:
  - `Compiler::compile_non_fatal` — no `process::exit`, returns full
    diagnostic vec.
  - Public accessors for `expr_types`, `binding_schemes_by_span`,
    resolved module interfaces.
  - `CompilerSession` façade that owns mutable graph + compiler state.
- LSP features: `publishDiagnostics`, `hover`, `definition`.
- Tests: `crates/flux-lsp/tests/integration.rs` — start server, issue
  LSP requests against fixtures under `examples/basics/`, assert
  responses.

### Phase 2 deliverables

- Measured latency report for the Phase 1 server on small and medium Flux
  projects.
- Coarser caching and module/interface reuse for unchanged files.
- If still needed, an incremental layer in `crates/flux-lsp/src/db.rs`
  built around the three boundaries above.
- Benchmarks: `criterion` bench harness measuring p50/p99 latency for
  `didChange` → `publishDiagnostics` on a 50-module synthetic project.
  Target: p50 < 50ms, p99 < 200ms.

### Phase 3 deliverables

Listed above. Each feature lands as a separate PR.

## Drawbacks
[drawbacks]: #drawbacks

- **Added maintenance surface.** LSP protocol changes; `lsp-types` bumps;
  client-side extension quirks per editor.
- **Compiler API now has two consumers** (CLI + LSP). Refactors in
  `src/compiler/` need to keep the LSP-facing API stable or break both.
- **Incremental analysis is hard.** Phase 2 is the difference between a
  toy and a production-quality LSP; if Flux adopts `salsa` or another
  query framework, that is a meaningful dependency and a new framework to
  learn.
- **VS Code extension requires TypeScript** — small but non-Rust surface
  to maintain.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why `tower-lsp` over alternatives

- `tower-lsp`: most adoption, simplest ergonomics, good docs. Default
  choice.
- `async-lsp`: newer, cleaner layered architecture. Worth considering if
  we want tracing/cancellation middleware. Tradeoff: fewer examples in
  the wild.
- `lsp-server` (rust-analyzer's): lower-level, sync. Only makes sense if
  we're writing rust-analyzer-scale infrastructure.

### Why not wrap the CLI

Naive alternative: spawn `cargo run -- file.flx` on every change, parse
its output, convert to LSP diagnostics. This is what the original Rust
`rls` did before rust-analyzer, and it's the reason `rls` was retired:
slow (~seconds per keystroke), wasteful (full recompile), and the output
format isn't designed for programmatic consumption. We should skip this
phase entirely.

### Why not commit to `salsa` up front

The proposal should not commit Flux to a query framework before Phase 1
latency is measured. Simpler invalidation may be sufficient for the first
usable server. If it is not, `salsa` is a credible Phase 2 option because
it matches compiler dependency graphs and has strong prior art, but it
should remain an engineering decision informed by data rather than a
premise of the MVP.

### Why not ship Phase 3 upfront

Scope discipline. The MVP is where we learn what editor users actually
want; Phase 3 features should be informed by real usage, not speculative.

## Prior art
[prior-art]: #prior-art

- **rust-analyzer** — the gold standard. Uses salsa, rowan, its own
  parser and HIR. 8+ years of engineering. Flux should copy the
  *architecture* (stateful server, incremental queries, demand-driven
  recompute) but not reimplement its semantic engine.
- **HLS (Haskell Language Server)** — drives GHC as a library. Strongest
  analog for Flux since both are typed functional languages with effect
  systems. The `ghcide` core is a close parallel to what
  `CompilerSession` would be.
- **ocaml-lsp** — drives the OCaml Merlin backend. Demonstrates a minimal
  LSP over a pre-existing analysis engine (similar to our position).
- **elm-language-server** — TypeScript-based, small codebase. Evidence
  that an LSP for a typed FP language doesn't need to be huge.
- **Koka** ([koka-lang/koka](https://github.com/koka-lang/koka)) —
  Flux's closest semantic cousin. Has no first-party LSP today. An
  opportunity for Flux to lead on editor experience in the effect-
  handlers niche.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **Workspace root detection** — does Flux need a `flux.toml`-style
   project manifest, or is the module graph entry point inferred from
   `--root` flags? LSP needs a stable workspace concept.
2. **Error recovery in the parser** — the current parser's recovery is
   good enough for batch compilation. For LSP, we need diagnostics on
   partially-typed code (mid-edit). How bad is today's recovery in
   practice?
3. **Where does `Compiler` own `TypeEnv` vs. where does the caller?**
   Phase 1 assumes the session holds the env. If `Compiler::compile`
   internally mutates shared global state, the session model gets ugly.
4. **Which editor(s) are the launch target?** VS Code first (biggest
   audience), Zed second (native LSP, clean), Neovim third (via
   `nvim-lspconfig` — basically free once the binary exists).

## Future possibilities
[future-possibilities]: #future-possibilities

- **DAP (debug adapter protocol)** after LSP — `flux-dap` for step
  debugging in the VM.
- **`.flx` syntax highlighting grammar** (TextMate / tree-sitter) for
  editors that want highlighting without full LSP.
- **Formatter integration** — the existing `src/syntax/formatter.rs`
  exposed as `textDocument/formatting`.
- **Notebook / REPL** — LSP protocol extensions for interactive
  evaluation; useful for Flux's functional/data-analysis audience.

## Relationship to nearby proposals

- **0126** (Diagnostic Rendering Improvements) — complementary. The LSP
  consumes Flux's diagnostic stream as-is; any rendering improvements to
  the human-facing CLI are orthogonal to the structured LSP output.
- **0127/0155/0160** — static typing prerequisites. All shipped;
  hover output is stable.
- **0145** (Type Classes) — hover on a class method must show the
  class's scheme + instance, not just the call-site type. Requires the
  class env to be a first-class exported surface.

## Exit criteria
[exit-criteria]: #exit-criteria

Phase 1 (MVP):

- `crates/flux-lsp/` crate builds, binary starts on stdio, handshake
  succeeds.
- VS Code extension installs, opens a `.flx` file from `examples/basics/`,
  diagnostics show inline, hover returns inferred type, goto jumps to
  definition.
- Integration test suite under `crates/flux-lsp/tests/` covers the three
  features against 20+ example fixtures.
- No new clippy warnings; `cargo test --all-features` green.

Phase 2 (incremental):

- Coarse caching and interface reuse demonstrably reduce `didChange`
  latency for one-file edits.
- If an incremental query layer is added, it invalidates only the
  edited file's diagnostics plus direct dependents.
- Bench target met: p50 < 50ms, p99 < 200ms on 50-module synthetic
  workload.

Phase 3 (refactors):

- Rename works across modules; at least 3 code actions landed; inlay
  hints + signature help shipped.
- Published VS Code extension on the marketplace.
