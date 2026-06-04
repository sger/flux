# REPL

> Source: `src/repl/` (`mod.rs`, `engine.rs`, `completion.rs`, `info.rs`, `browse.rs`)
> Proposals: [0175](../proposals/0175_interactive_repl.md) (Phase 1),
> [0176](../proposals/0176_interactive_repl_persistent_engine.md) (Phase 2, persistent engine)

`flux repl` is an interactive read-eval-print loop. The current implementation is the
**Phase 2 persistent engine**: a single prelude-loaded `Compiler` and a single live `VM`
live for the whole session, and each entered line compiles to a *delta* whose freshly
appended tail runs on the live VM. Earlier declarations never recompile and their side
effects never re-fire — eliminating the O(n²) recompile/re-execution cost of the Phase 1
accumulate-source engine.

```sh
cargo run -- repl          # interactive (rustyline)
echo ':type map' | cargo run -- repl   # piped / non-interactive
```

The `repl` feature (default-on) pulls in `rustyline`. Native builds drop it
(`--no-default-features --features llvm`) so the linked staticlib stays clean — see
[CLAUDE.md](../../CLAUDE.md) for why.

## The persistent engine

`ReplEngine` (`engine.rs`) owns the whole mutable session:

- **`compiler: Compiler`** — prelude-loaded once at `bootstrap`; each line compiles as a
  delta against it, so prior session globals stay resolvable through its symbol table (e.g.
  `let y = x + 1` resolves `x` to its existing slot).
- **`vm: VM`** — kept live across lines via `VM::run_top_level`, which preserves `globals`.
  Only the freshly-compiled tail runs.
- **`committed: Vec<SessionDecl>`** — a lightweight parallel record of top-level declarations
  in entry order. Used **only** for `:type` (re-inference over a fresh compile), `:list`, and
  to identify the user's own bindings for `:browse`/completion. Execution itself never reads
  it. A rebind replaces the earlier entry **in place** (matched by `name`) rather than
  appending, so the record stays a duplicate-free, compilable snapshot.
- **`result_counter` / `last_result`** — `it` can't be re-`let`, so each expression result
  binds to a fresh `__repl_N` global; `last_result` is the name `it` currently resolves to.
- **`show_type` / `show_timing` / `optimize` / `analyze`** — runtime-toggleable settings
  (see [Settings](#settings)).
- **`loaded`** — the file most recently `:load`'d, replayed by `:reload`.

### Bootstrap

`ReplEngine::bootstrap` → `bootstrap_repl_session` (in the driver) loads and compiles the
Flow prelude into one compiler and populates the live VM's globals from it. Module caching is
disabled (`no_cache`) and per-module progress chatter silenced (`quiet`) because the prelude
compiles exactly once against a synthetic entry path.

## Line routing

`run_repl` splits on whether stdin is a terminal:

- **Interactive** (`run_interactive`) — the full `rustyline` editor: history, in-line editing,
  Ctrl-R search, tab completion. History persists across sessions.
- **Piped** (`run_piped`) — a plain line reader; prompts go to **stderr** so stdout stays
  clean and deterministic for scripts and the integration tests.

Each logical input is classified (`LineKind`):

| Kind   | Trigger                                                        | Handling |
|--------|---------------------------------------------------------------|----------|
| `Skip` | blank / comment-only                                          | ignored |
| `Decl` | `let` / `fn` / `data` / `import` / `effect` / `class` / `instance` / `module` / `alias` | compiled at file scope → persists as a session global |
| `Expr` | a bare expression                                            | bound to `it` and printed |

A `:`-prefixed line is a **command** (`handle_command`), dispatched before classification.

### Expression evaluation

A bare expression takes one of two paths, because **Flux rejects top-level effects (E413)**:

- **Pure** — wrapped as `let __repl_N = <expr>` (value persists, `it` references it) plus a
  fresh `fn main` that prints it.
- **Effectful** — can't be a top-level binding, so it runs inside `main`. When the result has
  a faithful literal form (a primitive, or a list/tuple/ADT of such), it is **re-bound** so
  `it` captures it without re-running the effect (`eval_self_rebind` /
  `eval_effectful_named_binding`). Resultless effects (`Unit`/`None` from `print(..)`) and
  unrenderable values (maps, closures) leave `it` unchanged.

### Rollback

A line that fails to compile or run is rolled back by restoring a **cheap clone of the
compiler** taken before the attempt, so the session never enters a broken state. Globals a
failed chunk partially wrote are left in the VM but become unreachable once the compiler rolls
back, and the slot is reused. This is why the session "never breaks": the compiler clone is
the transaction boundary.

## Commands

Dispatched by `handle_command` (`mod.rs`). `:!<cmd>` is matched before the word split so the
whole tail reaches the shell verbatim.

| Command | Aliases | Purpose |
|---------|---------|---------|
| `:quit` | `:q` | exit |
| `:reset` | | re-bootstrap a fresh session |
| `:help` | `:?` | command list |
| `:list` | `:l` | echo the session's declaration *sources* (from `committed`) |
| `:type <expr>` | `:t` | inferred type of an expression (fresh re-inference) |
| `:info <name>` | `:i` | value type + origin / type's constructors / effect's operations |
| `:browse [prefix]` | `:b` | every in-scope name with its type, grouped Session / Prelude |
| `:set <opt>` / `:unset <opt>` | | toggle `+t` `+s` `optimize` `analyze` |
| `:!<cmd>` / `:shell <cmd>` | | run a shell command (stdio inherited) |
| `:cd [dir]` | | change working directory (`~` expanded; bare → home) |
| `:edit [file]` | `:e` | open in `$EDITOR`; reload if it's the loaded file |
| `:script <file>` | | feed a file's lines through the dispatcher |
| `:load <file>` | | compile a file into the session |
| `:reload` | | re-run the last `:load` |

### `:info` and `:browse` data sources

These read directly from the persistent compiler — **no per-name re-inference** — via
`pub(crate)` accessors on `Compiler`:

- **`:info`** (`engine::info`, formatted by `info.rs`) resolves a name in five steps: registry
  ADT → built-in type → effect → constructor (registry, then built-in) → value. Flux
  constructors aren't first-class unapplied (error E082 on a bare `Some`), so a constructor's
  signature is obtained by **eta-expansion** — `fn(__p0){ Some(__p0) }` — using the
  constructor's arity from its `AdtDefinition`, with the synthetic open-effect-row suffix
  (` with |_`) stripped. Built-in types and their constructors live in `info.rs`'s
  `BUILTIN_TYPES` table (`Option`, `Either`, `List`, …).
- **`:browse`** (`engine::browse`, formatted by `browse.rs`) pulls types in bulk:
  `repl_inferred_binding_types()` for the session and `repl_prelude_value_types()` (filtered to
  the Flow prelude modules) for the prelude. **Caveat worth knowing:** the session's scheme map
  is polluted with prelude bindings (the prelude is compiled in repl-mode), so the Session group
  is narrowed to exactly the names in `committed`. A session binding that shadows a prelude name
  appears only under Session.

## Tab completion

`ReplHelper` (`completion.rs`) is a rustyline `Helper` mirroring GHCi's two-stage dispatch:

- a line starting with `:` completes the **command name**, or — past the command word — that
  command's **argument** (file paths for `:load`, an identifier for `:type`);
- anything else completes an **identifier**.

Candidates come from `ReplEngine::completion_names()`: the bare names a user can actually type
(exposed members, builtin primops, ADT constructors, session bindings, `it`, keywords) plus
fully-qualified names for `.`-prefix completion. The set is shared with the loop via
`Rc<RefCell<Vec<String>>>` and refreshed after each entered line. `.` is **not** a word-break
char, so `Flow.Array.map` completes as one word. An empty word yields nothing (never dump the
whole namespace).

**Completion mode:** the editor uses `CompletionType::Circular` (menu-complete) rather than
`List`. With `List`, when the typed word is already the longest common prefix (e.g. `ma` for
`map`/`match`/`max`), rustyline needs a *second* Tab to draw the candidate list — which read as
"unresponsive". `Circular` inserts a match on the first Tab and cycles on each subsequent one
(Shift-Tab backward).

> The `COMMANDS` array in `completion.rs` is the completion source and currently lists only the
> original commands (`quit reset help list type load reload`). The newer commands
> (`info browse set unset shell cd edit script`) dispatch fine but aren't offered by name
> completion — keep this array in sync when adding commands.

## Settings

`:set +t` / `+s` and `:set optimize` / `analyze` (with `:unset` to clear) toggle the engine's
booleans via `ReplEngine::set_option`; `settings()` backs `:set` with no argument (prints the
table). `+t` prints the inferred type after each evaluation (via `infer_type`); `+s` prints
elapsed wall-clock time (an `Instant` around the eval). `optimize`/`analyze` surface the
compile flags carried from the session and affect **subsequent lines only**.

## Typed holes

Writing `_` (or a named `_foo`) anywhere an expression is expected reports a `TYPED HOLE`
diagnostic (**E469**): `found hole _ : T` plus the in-scope bindings whose type fits. This is
**not** REPL-specific code — it lives in HM inference
([`src/ast/type_infer/holes.rs`](../../src/ast/type_infer/holes.rs)) and is surfaced as an
inference diagnostic, so it powers the REPL *and* the LSP from one place (both consume
`InferProgramResult.diagnostics`). Fits are found by trial-unifying each in-scope binding
against the hole's type with `unify_core` and discarding the returned substitution. See the
module docs there for the detection rule (bare `_` always a hole; `_name` only when unbound;
never `__`-prefixed internals) and ranking. The double-diagnostic guard in
`src/compiler/expression.rs` (emit a placeholder for hole names instead of an E004
Undefined-Variable error) keeps codegen from also flagging the hole.

## File layout

| File | Responsibility |
|------|----------------|
| `mod.rs` | entry point, interactive/piped loops, line classification, command dispatch, shell conveniences |
| `engine.rs` | `ReplEngine` — the persistent compiler+VM, eval paths, rollback, `:info`/`:browse`/`:type` backing |
| `completion.rs` | `ReplHelper` — rustyline tab completion |
| `info.rs` | `:info` formatting + built-in type/constructor table |
| `browse.rs` | `:browse` formatting (grouped, aligned) |

## Testing

- **REPL behavior** — `tests/integration/test_runner_cli.rs` drives the piped path with
  scripted input (`repl_info_*`, `repl_browse_*`, `repl_set_*`, `repl_script_*`, typed-hole
  cases, …).
- **Completion / formatting** — unit tests in `completion.rs`, `info.rs`, `browse.rs`.
- **Typed holes** — inference-level in `tests/type_inference/static_type_validation_tests.rs`
  (assert `diag.code() == Some("E469")`); LSP-level in `crates/flux-lsp/tests/integration.rs`.
