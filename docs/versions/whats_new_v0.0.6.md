# What's New in Flux v0.0.6

Flux v0.0.6 is a **developer-experience** release.

Where v0.0.5 hardened the typed/effectful core, this version is about *using* Flux: a full-featured Language Server with a VS Code extension, an interactive REPL, GHC-style typed holes, and a much more capable asynchronous runtime. The language semantics are largely unchanged — the tooling around them is dramatically better.

## Highlights

- **Language Server Protocol** — a real `flux-lsp` server plus a VS Code extension: diagnostics, hover types, go-to-definition, completion, code actions, semantic tokens, inlay hints, call/type hierarchy, rename, and more
- **Interactive REPL** (`flux repl`) — a persistent compiler + live VM with `:load`/`:reload`, `:type`, `:info`, `:browse`, `:set`, tab completion, and shell conveniences
- **Typed holes** — write `_` (or `_name`) anywhere an expression is expected and inference reports the required type plus the in-scope bindings that fit; surfaced in both the REPL and the LSP
- **Async runtime maturation** (proposal 0174) — fiber work-stealing scheduler, cooperative cancellation, configurable worker pools, first-class events with `select`, and cross-worker fiber migration on both backends
- **Number literals** — radix (`0x`, `0o`, `0b`) and underscore digit separators
- **Reproducible builds** — native intermediates and test scratch moved out of `%TEMP%`

## Language Server

The biggest single theme in this release. Flux now ships a Language Server (`crates/flux-lsp`) that **reuses the compiler's own frontend** — the same lexer, parser, module graph, and HM inference the CLI uses — rather than reimplementing parsing or typing. A minimal VS Code extension (`editors/vscode/`) wires it up.

Capabilities that landed:

- **Diagnostics** on open / change / save, including undefined-name, unimported-module, sibling-import, and import-cycle diagnostics, plus pull and workspace diagnostics
- **Hover** — inferred types, declaration schemes, effect and row-variable labels, keyword docs, module/field labels, constructor-pattern types, effect-operation signatures, and doc-comments at use sites
- **Navigation** — go-to-definition (including class-method calls resolving to the matching `instance` arm, cross-module, and aliased sub-directory modules), go-to-type-definition, call hierarchy, type hierarchy, and document/workspace symbols
- **Completion** — module members, record fields, `with`-clause effect labels, named-constructor fields, stdlib names, and auto-import on accept, with resolve-time docs
- **Code actions & assists** — fill match arms, add missing instance methods, organize/make-imports-explicit, effect and import quick-fixes, change-return-type-to-inferred, convert number format, prefix-unused-let, and refactor assists
- **Editor niceties** — semantic tokens (with range/delta), inlay hints, document highlight, linked editing, selection/folding ranges, on-type and range formatting, document links, signature help, CodeLens runnables ("Run test" / "Run all tests"), prepare-rename, and will-rename-files
- **Robustness** — a worker-thread pool with panic guards, debounced analysis, module-name caching, and inference-panic logging so a single bad buffer can't take the server down

The VS Code extension adds a **"Restart Language Server"** command. See [editors/vscode/README.md](../../editors/vscode/README.md) for install instructions and [docs/internals/](../internals/) for architecture.

## Interactive REPL

`flux repl` is a persistent read-eval-print loop (proposals 0175/0176). It keeps **one** prelude-loaded compiler and **one** live VM for the session: each entered line compiles as a *delta* and runs on the live VM, so earlier declarations never recompile and their side effects never re-fire. A line that fails to compile or run is rolled back, so the session never breaks.

What it offers:

- bare expressions evaluate, bind to `it`, and print; declarations persist as session globals
- `:load <file>` / `:reload`, `:type`, `:list`
- `:info <name>` — type + origin for a value, constructors/fields for a type, operations for an effect
- `:browse [prefix]` — every in-scope name with its type, grouped session / prelude
- `:set +t` / `+s` (type echo, timing) and `:set optimize` / `analyze`
- `:!` / `:shell`, `:cd`, `:edit` (`$EDITOR`), `:script <file>`
- line editing, history, and identifier/command tab completion via `rustyline`

See [docs/internals/repl.md](../internals/repl.md) for how the persistent engine works.

## Typed holes

Writing `_` — or a named `_foo` — anywhere an expression is expected now reports a `TYPED HOLE` diagnostic (**E469**): `found hole _ : T`, where `T` is the type required at that position, together with the in-scope bindings whose type fits. For example, `map([1, 2, 3], _)` reports `found hole _ : (Int) -> a` and lists `even`, `odd`, `abs`, … as candidates.

Because holes are emitted as ordinary inference diagnostics, they work in **both** the REPL (type `_` in any expression) and the **LSP** (shown inline / in Problems as you type) with no surface-specific handling. A `_`-prefixed name that *is* in scope remains an ordinary variable, matching GHC.

## Async runtime

This release continues proposal 0174's asynchronous story on both the VM and native backends:

- **Work-stealing scheduler** — a fiber worker pool with least-loaded / load-aware spawn placement, lightweight task spawning, a root reaper, and cross-OS-worker fiber migration
- **Cooperative cancellation** — `Async.check_cancelled()` and `Async.bail_if_cancelled()` let long pure compute loops between `await` points poll the cancel flag instead of running to completion under a cancelled scope
- **Configurable runtime** — `Async.RuntimeConfig`, `with_worker_count(n)`, and `run_async_with(cfg, action)` give explicit per-`run_async` knobs for worker/pool sizes, plus a `FLUX_WORKERS` env-var fallback and `Async.current_worker_count()` introspection
- **First-class events** — `Flow.Event` and `select { recv / send / after -> ... }` for channel and timer selection
- **Cooperative scheduling** — `yield_now` reschedules the current fiber; `Async.first_of` / `select` race multiple sources
- continuation capture and an `Arc`-mirrored fiber representation underpin migration across workers

## Other changes

- **Number literals** — radix prefixes (`0x1F`, `0o17`, `0b1010`) and underscore digit separators (`1_000_000`) now parse
- **Top-level codegen fix** — corrected top-level `match` and tuple-destructure codegen via a synthetic frame
- **Build hygiene** — native compile intermediates, `src/` unit-test scratch, and integration-test fixtures now write under `target/` instead of `%TEMP%`, and Windows native binaries link `user32`/`gdi32` explicitly
- **VM fix** — deep-baseline continuations are no longer stolen across worker VMs

## Migration notes

- **No breaking language changes.** Existing `.flx` programs compile and run as before.
- **The LSP reuses the CLI frontend**, so diagnostics in the editor match `cargo run -- <file>`. If editor diagnostics look wrong, they almost certainly reproduce on the CLI.
- **The prebuilt VS Code `.vsix` bundles a Windows x64 server only.** macOS/Linux users build `flux-lsp` once (`cargo install --path crates/flux-lsp`) and the extension finds it on `PATH` or via `flux.serverPath`.

## Recommended first things to try

Start the REPL:

```bash
cargo run -- repl
```

In the REPL, try a typed hole and `:browse`:

```text
flux> map([1, 2, 3], _)
flux> :browse map
```

Install the VS Code extension (Windows):

```powershell
./scripts/rebuild-vsix.ps1
```

Run an async example:

```bash
cargo run -- examples/async/16_current_worker_count.flx
```

## In short

Flux v0.0.6 makes Flux pleasant to *work in*:

- a real Language Server and editor integration
- an interactive REPL with rich introspection
- typed holes that turn the checker into a "what goes here?" tool
- a maturing async runtime with cancellation, configuration, and events

It is a tooling and developer-experience release on top of the v0.0.5 typed/effectful core.
