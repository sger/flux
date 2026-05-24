- Feature Name: Interactive REPL (`flux repl`)
- Start Date: 2026-05-24
- Status: Draft
- Proposal PR:
- Flux Issue:
- Builds on: the `flux eval "<expr>"` subcommand ([../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs)), the staged VM compiler pipeline ([../../src/driver/pipeline/program.rs](../../src/driver/pipeline/program.rs)), and the persistent-`Compiler` model already used by the language server ([../../crates/flux-lsp/src/prelude.rs](../../crates/flux-lsp/src/prelude.rs))
- Relates to: [0163_flux_language_server.md](0163_flux_language_server.md) (shares the "reuse the compiler frontend as a service" theme), [0076_debug_toolkit.md](0076_debug_toolkit.md)

# Proposal 0175: Interactive REPL (`flux repl`)

## Summary
[summary]: #summary

Add an interactive read-eval-print loop, `flux repl`, that lets a user type Flux
expressions and declarations one at a time and see results immediately, with
**earlier definitions staying in scope for later lines**. The proposal delivers
this in two phases: a small **Phase A** that reuses today's `flux eval` pipeline
by re-running an accumulated source buffer each line (works now, quadratic, fine
for interactive use), and a later **Phase B** that compiles each line as a
synthetic module against a persistent `Compiler`, reusing the cross-module global
machinery the language server already exercises — so earlier lines are *not*
recompiled. The design is grounded in a study of GHC's interactive evaluator
(GHCi), whose `InteractiveContext` + name-keyed bytecode linker is the reference
architecture and the model Phase B converges toward.

## Motivation
[motivation]: #motivation

Flux today is run-a-file only ([../../src/main.rs](../../src/main.rs) →
[../../src/cli/](../../src/cli/)). There is one-shot expression evaluation —
`flux eval "1 + 2"` — added for the LSP doc-comment "▶ Eval" lens, but no
*interactive* mode: no way to define `let x = 5`, then ask for `x + 1`, then
build on that incrementally.

A REPL is the single highest-leverage learning and exploration tool a language
can ship. Concrete use cases:

- **Learning the language.** A newcomer types `[1, 2, 3] |> map(\x -> x * 2)`
  and sees the result and its type without creating a file, a `main`, and an
  `IO` effect row.
- **Exploring the standard library.** "What does `Flow.List.fold` do again?" —
  try it on a literal, read the result.
- **Checking a type quickly.** `:type \x -> x + 1` answers "what does HM infer
  here?" interactively — the same inference the LSP hover uses, surfaced at a
  prompt.
- **Debugging a snippet.** Paste a small expression, bind intermediate results,
  poke at them.
- **Teaching and docs.** REPL transcripts are the most readable form of small
  examples; the doc-comment eval feature already leans on this (`/// >>> expr`).

Flux is unusually well-positioned: the `eval` building block exists, value
rendering is already "REPL-correct" (numbers unquoted, strings quoted — see
[../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs)), and the
compiler frontend is already used as a reusable service by `flux-lsp`. The only
genuinely new problem is **state persistence across lines**, which this proposal
analyzes in depth.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

You start the REPL with:

```sh
flux repl
```

and get a prompt. You type an expression and see its value; you type a
declaration and it is remembered for later lines:

```text
flux> 1 + 2
3
flux> let xs = [1, 2, 3]
flux> xs |> map(\x -> x * 2)
[2, 4, 6]
flux> fn double(n: Int) -> Int { n * 2 }
flux> double(21)
42
flux> it + 1
43
```

Three named concepts a Flux programmer should learn:

1. **The session is cumulative.** Every `let`, `fn`, `data`, `alias`, and
   `import` you enter stays in scope for the rest of the session. Re-binding a
   name shadows the old one (the most recent definition wins), exactly like
   nested scopes in a normal program.

2. **`it` is the last result.** A bare expression's value is bound to `it`, so
   you can build on it without naming it (borrowed directly from GHCi). `it`
   updates after every expression line.

3. **Colon-commands are meta, not Flux.** Lines beginning with `:` are REPL
   directives, not Flux source:

   | Command | Effect |
   |---|---|
   | `:type <expr>` / `:t` | Show the inferred type of `<expr>` (no evaluation) |
   | `:reset` | Forget all session bindings, start fresh |
   | `:list` / `:l` | Show the accumulated session source |
   | `:help` / `:?` | List commands |
   | `:quit` / `:q` | Exit |

Multi-line input is supported: if a line leaves a brace/paren open (or ends in a
trailing operator), the prompt continues (`....>`) until the form is complete.

Effects work the way they do in a file: an expression that performs `IO`
(`print("hi")`) runs its effect and shows `()`. A pure expression just shows its
value. Errors — parse, type, or runtime — are printed and the session continues;
a failed line does **not** pollute the session (its bindings are rolled back).

How to *think* about it: a `flux repl` session is a single growing Flux program
that you are assembling and observing one statement at a time. Everything you can
write at the top level of a `.flx` file, you can write at the prompt.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Current architecture: one-shot, no carried state

`flux eval "<expr>"` runs the **entire** pipeline fresh:

1. [../../src/cli/cmdline.rs](../../src/cli/cmdline.rs) parses `CliCommand::Eval`.
2. [../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs) wraps
   the expression in a synthetic `fn main() with IO { println(<expr>) }` and
   calls `run_from_source`.
3. [../../src/driver/pipeline/program.rs](../../src/driver/pipeline/program.rs)
   builds a **fresh** `Compiler` ([../../src/compiler/mod.rs](../../src/compiler/mod.rs)),
   runs parse → module graph → HM inference → Core → CFG/bytecode, then
4. [../../src/driver/run_program/backend/vm.rs](../../src/driver/run_program/backend/vm.rs)
   constructs a **fresh** `VM` and runs it.

So each `eval` is independent with no carried state. Two facts decide how hard a
REPL is:

- **VM globals are addressed by compile-time slot index**, allocated from 0 on
  every compile. `SymbolTable::define` ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs))
  increments `num_definitions` per global; the VM reads `globals[index]`
  ([../../src/vm/mod.rs](../../src/vm/mod.rs)). A *new* independent compile would
  re-issue slot 0, 1, 2…, colliding with a still-live VM's earlier globals.
- **The frontend is already reusable as a service.** `flux-lsp` keeps one
  persistent `Compiler` with accumulating `cached_member_schemes`
  ([../../src/compiler/mod.rs](../../src/compiler/mod.rs)) and resets only
  per-file state between documents via `phase_reset_for_lsp`
  ([../../src/compiler/passes/reset.rs](../../src/compiler/passes/reset.rs)).
  Imported-module globals are *preloaded at fixed indices* via
  `define_global_with_index` ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs)).
  That preload mechanism is the closest existing analog to a persistent runtime
  symbol table.

### The crux, and how GHCi solves it

The hard part of any REPL is making line *N* see the names bound by lines
*1..N-1*. GHCi (studied in the GHC tree at `E:\Github\ghc`) is the canonical
solution and resolves **everything by name, never by fixed offset**, via two
persistent pieces of state:

1. **`InteractiveContext`** (`compiler/GHC/Runtime/Context.hs`) — the
   *compile-time* accumulator. It holds `ic_tythings` (all user-defined things,
   newest first), `ic_gre_cache` (the in-scope reader environment),
   `ic_instances`, `ic_fix_env`, and `ic_mod_index` (the `Ghci1`, `Ghci2`…
   counter). Each line is renamed/typechecked *inside* it (`runTcInteractive` in
   `compiler/GHC/Tc/Module.hs` seeds the typechecker with `icReaderEnv` and the
   accumulated `ic_tythings`). After a line succeeds,
   `extendInteractiveContextWithIds` prepends the new bindings and bumps the
   index.

2. **`closure_env`** (`compiler/GHC/Linker/Types.hs`, a `NameEnv (Name,
   ForeignHValue)`) — the *runtime* symbol table. Each line is compiled to
   bytecode objects (BCOs); `linkBCO` (`compiler/GHC/ByteCode/Linker.hs`)
   resolves references to earlier lines **by name** against this persistent
   table. New closures are appended after each line via `extendLoadedEnv`
   (`compiler/GHC/Linker/Loader.hs`).

A bare expression is auto-wrapped as `it <- expr; print it` (the `it` variable,
`compiler/GHC/Tc/Module.hs`), and results print via `Show`. The decisive
property: **GHCi never recompiles earlier lines** — each line is compiled
independently and *linked by name* into a persistent environment.

Flux's slot-index globals are the opposite of GHCi's name-keyed linker. That gap
is exactly what the two-phase plan below bridges.

### Phase A — accumulate-source REPL (ships first)

Keep a growing buffer of the session's **declarations**. Each entered line is
classified:

- **Declaration** (`let` / `public let` / `fn` / `data` / `alias` / `effect` /
  `class` / `instance` / `import`, and `module`): append it to the buffer. Do not
  print a value. Optionally evaluate to surface errors immediately (see error
  handling below).
- **Expression**: build a throwaway program `"<buffer>\nfn main() with IO {
  let it = <expr>\n println(it) }"` and run it through the existing
  `run_from_source` pipeline; print the captured output; on success, append
  `let it = <expr>` (rebinding `it`) to the buffer.
- **Colon-command**: handled by the REPL, never compiled as Flux.

This is the model the doc-comment eval lens already proves works for single
expressions; Phase A generalizes it to a session.

Classification is done with the existing parser/lexer ([../../src/syntax/](../../src/syntax/)):
parse the line as a statement; if it is an `Expression` statement, treat it as an
expression, otherwise a declaration. (No new grammar — the top-level grammar
already accepts all these forms.)

**`it`** is realized as an ordinary `let it = <expr>` appended to the buffer, so
no compiler changes are needed for it.

**Error handling / rollback.** Compile/run the candidate buffer *before*
committing the new line. If it produces diagnostics or a runtime error, print
them and discard the line — the committed buffer is unchanged, so the session
never enters a broken state. (This is why declarations are also evaluated, not
just appended: a `fn` with a type error should be rejected at entry, GHCi-style.)

**Effects and re-execution — the known cost.** Because Phase A re-runs the whole
buffer each line, any *side effect performed inside a declaration* re-fires every
line. In practice declarations are `let`/`fn`/`data` (pure or lazy), so the only
effects normally executed are those in the current expression line, which is
correct. But a top-level `let _ = print("loading")` would re-print each line.
Phase A documents this; Phase B removes it.

**Cost.** O(n²) parse+compile over a session of *n* lines, and a fresh VM per
line. Negligible for interactive use (hundreds of short lines); not suitable for
machine-driven bulk input. Acceptable for v1.

**Implementation surface (Phase A):**

- `CliCommand::Repl { flags }` in [../../src/cli/cmdline.rs](../../src/cli/cmdline.rs),
  dispatched in [../../src/cli/mod.rs](../../src/cli/mod.rs).
- A new `src/driver/pipeline/repl.rs` holding the loop: read line (stdin; line
  editing via a small dependency such as `rustyline`, or raw `stdin` for v0),
  classify, build candidate source, call `run_from_source`, print, commit/roll
  back. Reuses `wrap_expression`-style assembly from
  [../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs).
- `:type` reuses the same inference the LSP hover uses
  (`infer_expr_types_for_program` on the compiler — [../../src/compiler/mod.rs](../../src/compiler/mod.rs))
  by wrapping `<expr>` as `let __t = <expr>` and reading back its inferred scheme,
  rather than running it.
- Result rendering reuses `println` on the value (the same REPL-correct rendering
  `eval` relies on). ADTs without a displayable form surface as the runtime's
  default value rendering; richer printing ties into `deriving Show` (out of
  scope, see Future possibilities).

### Phase B — module-per-line against a persistent `Compiler`

Phase B removes re-execution and the O(n²) cost by treating each line as a
synthetic module compiled against a **persistent** `Compiler`, reusing the same
machinery the LSP and the module graph already use:

- Keep one `Compiler` alive for the session (as `flux-lsp` does —
  [../../crates/flux-lsp/src/prelude.rs](../../crates/flux-lsp/src/prelude.rs)),
  with the prelude/standard library loaded once.
- Each line becomes module `Repl{N}` that *imports* the accumulated session
  symbols. Cross-line names resolve through the existing module-interface path:
  prior globals are preloaded at their established indices via
  `define_global_with_index` ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs)),
  the same way imported-module globals are resolved today — this is Flux's
  name-keyed analog to GHCi's `closure_env`.
- Keep **one `VM` instance alive** across lines. The session maintains a
  persistent `name → global_slot` map; newly defined globals get fresh,
  monotonically increasing slots (never reset), and new bytecode references
  earlier globals at their recorded slots. New compiled chunks are executed on
  the live VM, extending `vm.globals` rather than replacing it.

The required work is the part Flux does not have yet: a **session symbol/slot
allocator** that survives across compiles (so indices are monotonic, not
reset-to-zero), and the plumbing to run additional bytecode chunks on a live VM
that already holds earlier globals. The inference side is largely already solved
by the persistent-`Compiler` + `phase_reset_for_lsp` pattern; the runtime side is
the new piece.

Phase B maps onto GHCi as: persistent `Compiler` + module-interface preload ≈
`InteractiveContext`; the session slot map + live VM ≈ `closure_env` + the
bytecode interpreter; `it` is unchanged.

### Phase relationship

Phase A and Phase B present the *same* user-facing behavior (the Guide section).
A user cannot tell which is running except by performance and by whether a
side-effecting declaration re-fires. Phase A is shippable immediately and
validates the UX, command set, multi-line handling, and `:type`; Phase B is a
drop-in engine swap behind the same loop.

## Drawbacks
[drawbacks]: #drawbacks

- **Maintenance surface.** A REPL is a new long-lived entry point that must track
  language growth (new top-level forms, new effects). Mitigated by building on
  the existing parser/compiler rather than a parallel evaluator.
- **Phase A semantics are imperfect.** Re-execution of side-effecting
  declarations and O(n²) cost are real, even if rarely hit interactively. Risk of
  users depending on Phase A quirks before Phase B lands.
- **Value printing is only as good as Flux's value rendering.** Without a `Show`
  story, complex ADT results print with the runtime default, which may be terse.
- **Effect handling at the prompt.** Expressions that require ambient effects
  beyond `IO` (e.g. a custom effect with no handler) can't run at the bare
  prompt; the REPL must give a clear message rather than a raw error.
- **Scope creep.** REPLs invite endless features (tab completion, `:load`,
  history search, multiline editor). The phased plan must hold the line.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why accumulate-source first (Phase A)?** It reuses a proven pipeline, ships
  in days, and lets the team settle UX and commands before paying for the runtime
  refactor. The cost model is acceptable for the actual workload (interactive
  typing).
- **Why not jump straight to a persistent VM (Phase C-equivalent)?** Decoupling
  globals from reset slot indices and keeping a live VM is the largest change and
  touches the symbol table and VM core. Doing it first risks a long, invisible
  effort with no usable REPL meanwhile. Phase B reaches most of the benefit by
  reusing the module-interface preload path instead of a from-scratch linker.
- **Why module-per-line for Phase B rather than a brand-new linker?** Flux
  already resolves cross-module globals by name+preloaded-index; a REPL line is
  morally a module that imports the session. Reusing that path is far less risky
  than inventing a GHCi-style `NameEnv` linker.
- **Could this be a library/macro instead?** No — a REPL needs to drive the
  compiler and VM and own stdin; it is inherently a tool/driver feature.
- **Impact of not doing this.** Flux remains file-only; the learning curve and
  exploration cost stay high, and the doc-comment eval feature remains the only
  taste of interactivity.

## Prior art
[prior-art]: #prior-art

- **GHCi (Haskell)** — the reference design, studied in depth above:
  `InteractiveContext` (`compiler/GHC/Runtime/Context.hs`), statement evaluation
  (`compiler/GHC/Runtime/Eval.hs`, `compiler/GHC/Driver/Main.hs`), the `it`
  binding and bare-expression wrapping (`compiler/GHC/Tc/Module.hs`), and the
  name-keyed bytecode linker (`compiler/GHC/Linker/Types.hs`,
  `compiler/GHC/Linker/Loader.hs`, `compiler/GHC/ByteCode/Linker.hs`). Lesson:
  resolve cross-line references by name into persistent compile-time and runtime
  environments; never recompile earlier input; bind results to `it`.
- **OCaml (`ocaml` toplevel / `utop`)** — incremental top-level that keeps a
  persistent environment; `utop` adds editing/completion as a layer. Lesson:
  separate the evaluation engine from the line-editing UX.
- **Elm (`elm repl`)** — historically an *accumulate-and-recompile* model very
  close to Phase A: it wraps prior declarations and re-feeds them to the
  compiler. Lesson: accumulate-source is a legitimate, shippable first cut for an
  ML-family language, exactly the Phase A bet.
- **Rust (`evcxr`)** — REPL for a non-REPL language; compiles each line to a dylib
  and links incrementally. Lesson: even a "compiled, no-REPL" language can get a
  good REPL with per-line compilation + linking — encouraging for Flux's bytecode
  VM.
- **Python / Node.js** — interpreter-native REPLs with trivial state persistence
  (one mutable namespace). Lesson: the bar users expect is "previous lines are in
  scope and results echo"; both phases meet it.
- **Idris / Agda** — dependently typed REPLs with rich `:type`/`:doc` commands.
  Lesson: `:type` and friends are high-value and cheap once the frontend is a
  service (which Flux's already is, via the LSP).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Line editing dependency.** Adopt `rustyline` (history, editing, completion
  hooks) for v1, or ship raw `stdin` first and add editing later? Affects
  dependencies and Windows behavior.
- **Multi-line continuation heuristic.** Brace/paren balance is simple; do we
  also continue on trailing operators / `->`? Where exactly does the parser tell
  us "incomplete" vs "error"? (Tie into the parser's existing recovery.)
- **`:type` fidelity.** Should `:type` show the generalized scheme (LSP hover
  style) or the monomorphic instantiated type? Probably the scheme, matching
  hover.
- **Effect surface at the prompt.** Which effects are implicitly available
  (`IO`)? How do we present an expression that needs an unhandled effect?
- **Shadowing vs redefinition semantics in Phase A** (append-and-shadow) vs
  Phase B (new module symbol) — confirm they are observationally identical.
- **Result rendering for ADTs** without `deriving Show`.
- **Phase B slot model** — exact design of the monotonic session slot allocator
  and live-VM chunk execution; this is the main implementation risk and may want
  its own follow-up proposal.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Editor-integrated REPL.** A VS Code "Flux: Start REPL" terminal command and a
  "send selection to REPL" action, reusing the extension's command plumbing
  ([../../editors/vscode/src/extension.ts](../../editors/vscode/src/extension.ts));
  natural follow-on to the run/eval lenses.
- **`:load <file>` / `:reload`.** Bring a module's definitions into the session
  (GHCi `:load`), reusing the module graph.
- **Tab completion and `:doc` at the prompt**, powered by the LSP's existing
  completion and hover engines ([0163_flux_language_server.md](0163_flux_language_server.md)).
- **Notebook / transcript mode.** Persist and replay sessions; tie into the
  doc-comment eval feature so `/// >>>` snippets and the REPL share one evaluator.
- **Debugger integration.** Breakpoints and value inspection at the prompt,
  connecting to [0076_debug_toolkit.md](0076_debug_toolkit.md).
- **Native-backend REPL.** Phase B targets the VM; an LLVM-backed REPL (JIT) is a
  much larger, separate effort.
- **Deterministic replay.** A recorded REPL session as a reproducible script,
  relating to [0038_deterministic_effect_replay.md](0038_deterministic_effect_replay.md).
