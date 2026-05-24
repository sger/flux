- Feature Name: Interactive REPL — Phase 1 (accumulate-source MVP)
- Start Date: 2026-05-24
- Status: Draft
- Proposal PR:
- Flux Issue:
- Builds on: the `flux eval "<expr>"` subcommand ([../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs)) and the staged VM compiler pipeline ([../../src/driver/pipeline/program.rs](../../src/driver/pipeline/program.rs))
- Relates to: [0176_interactive_repl_persistent_engine.md](0176_interactive_repl_persistent_engine.md) (Phase 2 — the incremental engine that replaces this phase's evaluator behind the same UX), [0163_flux_language_server.md](0163_flux_language_server.md)

# Proposal 0175: Interactive REPL — Phase 1 (Accumulate-Source MVP)

## Summary
[summary]: #summary

Add an interactive read-eval-print loop, `flux repl`, that lets a user type Flux
expressions and declarations one at a time and see results immediately, with
earlier definitions staying in scope for later lines. **Phase 1** delivers this
by reusing today's `flux eval` pipeline: it keeps a growing buffer of the
session's declarations and, for each expression line, re-runs the accumulated
buffer through the existing compile-and-run pipeline. This ships now with no
compiler or VM changes; its known costs (re-executing side-effecting
declarations, O(n²) recompilation) are documented here and removed by
[Phase 2 (0176)](0176_interactive_repl_persistent_engine.md), which swaps the
engine behind the identical user-facing behavior defined in this proposal.

## Motivation
[motivation]: #motivation

Flux today is run-a-file only ([../../src/main.rs](../../src/main.rs) →
[../../src/cli/](../../src/cli/)). There is one-shot expression evaluation —
`flux eval "1 + 2"`, added for the LSP doc-comment "▶ Eval" lens — but no
*interactive* mode: no way to define `let x = 5`, then ask for `x + 1`, then build
on that incrementally.

A REPL is the single highest-leverage learning and exploration tool a language
can ship. Concrete use cases:

- **Learning the language.** A newcomer types `[1, 2, 3] |> map(\x -> x * 2)` and
  sees the result without creating a file, a `main`, and an `IO` effect row.
- **Exploring the standard library.** "What does `Flow.List.fold` do again?" — try
  it on a literal, read the result.
- **Checking a type quickly.** `:type \x -> x + 1` answers "what does HM infer
  here?" interactively.
- **Debugging a snippet.** Paste a small expression, bind intermediate results,
  poke at them.
- **Teaching and docs.** REPL transcripts are the most readable form of small
  examples; the doc-comment eval feature already leans on this (`/// >>> expr`).

The motivation for Phase 1 specifically is **speed to value**: Flux already has
the building block (`eval`), value rendering is already "REPL-correct" (numbers
unquoted, strings quoted — see
[../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs)), and an
accumulate-source REPL is a legitimate, proven first cut (Elm's `elm repl` shipped
on essentially this model). Phase 1 lets us validate the UX, command set, and
ergonomics before paying for the deeper engine work in
[0176](0176_interactive_repl_persistent_engine.md).

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

> This section defines the **user-facing contract for the REPL as a whole**.
> Phase 1 and [Phase 2 (0176)](0176_interactive_repl_persistent_engine.md) present
> identical behavior here; a user cannot tell which engine is running except by
> performance and by whether a side-effecting top-level declaration re-fires (a
> Phase 1 quirk, fixed in Phase 2).

You start the REPL with:

```sh
flux repl
```

and get a prompt. You type an expression and see its value; you type a declaration
and it is remembered for later lines:

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

Three named concepts:

1. **The session is cumulative.** Every `let`, `fn`, `data`, `alias`, and `import`
   you enter stays in scope for the rest of the session. Re-binding a name shadows
   the old one (most recent wins), like nested scopes in a normal program.

2. **`it` is the last result.** A bare expression's value is bound to `it`, so you
   can build on it without naming it (borrowed from GHCi). `it` updates after every
   expression line.

3. **Colon-commands are meta, not Flux.** Lines beginning with `:` are REPL
   directives:

   | Command | Effect |
   |---|---|
   | `:type <expr>` / `:t` | Show the inferred type of `<expr>` (no evaluation) |
   | `:reset` | Forget all session bindings, start fresh |
   | `:list` / `:l` | Show the accumulated session source |
   | `:help` / `:?` | List commands |
   | `:quit` / `:q` | Exit |

Multi-line input is supported: if a line leaves a brace/paren open, the prompt
continues (`....>`) until the form is complete.

Effects work as in a file: an expression that performs `IO` (`print("hi")`) runs
its effect and shows `()`. Errors — parse, type, or runtime — are printed and the
session continues; a failed line does **not** pollute the session (its bindings
are rolled back).

How to *think* about it: a `flux repl` session is a single growing Flux program you
assemble and observe one statement at a time. Everything you can write at the top
level of a `.flx` file, you can write at the prompt.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Background: the current one-shot pipeline

`flux eval "<expr>"` runs the **entire** pipeline fresh:

1. [../../src/cli/cmdline.rs](../../src/cli/cmdline.rs) parses `CliCommand::Eval`.
2. [../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs) wraps the
   expression in a synthetic `fn main() with IO { println(<expr>) }` and calls
   `run_from_source`.
3. [../../src/driver/pipeline/program.rs](../../src/driver/pipeline/program.rs)
   builds a **fresh** `Compiler`, runs parse → module graph → HM inference → Core →
   CFG/bytecode, then
4. [../../src/driver/run_program/backend/vm.rs](../../src/driver/run_program/backend/vm.rs)
   constructs a **fresh** `VM` and runs it.

Each `eval` is independent with no carried state. Phase 1 turns this into a session
by *re-feeding an accumulated buffer*; it does **not** try to keep state in the
compiler or VM (that is Phase 2's job — see
[0176](0176_interactive_repl_persistent_engine.md)).

### Phase 1 design: accumulate-source

Keep a growing buffer of the session's **declarations**. Each entered line is
classified using the existing parser/lexer ([../../src/syntax/](../../src/syntax/))
— parse the line as a statement; an `Expression` statement is an expression,
anything else (`let` / `public let` / `fn` / `data` / `alias` / `effect` /
`class` / `instance` / `import` / `module`) is a declaration:

- **Declaration**: append to the buffer; print nothing. Still compiled (see
  rollback) so errors surface at entry.
- **Expression**: build a throwaway program
  `"<buffer>\nfn main() with IO { let it = <expr>\n println(it) }"`, run it through
  the existing `run_from_source`, print the captured output, and on success append
  `let it = <expr>` to the buffer (rebinding `it`).
- **Colon-command**: handled by the REPL, never compiled as Flux.

This is exactly the model the doc-comment eval lens proves for single expressions,
generalized to a session.

**`it`** is realized as an ordinary `let it = <expr>` appended to the buffer — no
compiler changes needed.

**Error handling / rollback.** Compile/run the candidate buffer *before* committing
the new line. On any diagnostic or runtime error, print it and discard the line;
the committed buffer is unchanged, so the session never enters a broken state. This
is why declarations are compiled too, GHCi-style.

**`:type`** reuses the inference the LSP hover uses
(`infer_expr_types_for_program` on the compiler —
[../../src/compiler/mod.rs](../../src/compiler/mod.rs)) by wrapping `<expr>` as
`let __t = <expr>` over the buffer and reading back its inferred scheme, rather than
running it.

**Result rendering** reuses `println` on the value (the same REPL-correct rendering
`eval` relies on). ADTs without a displayable form surface as the runtime default;
richer printing ties into `deriving Show` (out of scope).

### Known limitations (motivating Phase 2)

- **Re-execution of side-effecting declarations.** Because the whole buffer re-runs
  each line, a top-level `let _ = print("loading")` re-prints every line. In
  practice declarations are pure (`let`/`fn`/`data`), so normally only the current
  expression's effects fire — but the hazard is real and documented.
- **O(n²) cost.** A session of *n* lines re-parses and re-compiles the buffer each
  line, with a fresh VM per line. Negligible for interactive typing; unsuitable for
  bulk/machine input.

Both are removed by [Phase 2 (0176)](0176_interactive_repl_persistent_engine.md),
which compiles each line incrementally against a persistent `Compiler` and a live
`VM`, behind this same UX.

### Implementation surface (Phase 1)

- `CliCommand::Repl { flags }` in
  [../../src/cli/cmdline.rs](../../src/cli/cmdline.rs), dispatched in
  [../../src/cli/mod.rs](../../src/cli/mod.rs).
- A new `src/driver/pipeline/repl.rs` holding the loop: read line (raw `stdin` for
  v0, or a small line-editor dependency such as `rustyline`), classify, build
  candidate source, call `run_from_source`, print, commit/roll back. Reuses the
  `wrap_expression`-style assembly from
  [../../src/driver/pipeline/eval.rs](../../src/driver/pipeline/eval.rs).
- Multi-line continuation via brace/paren balance reported by the parser.

### Acceptance criteria (Phase 1)

- `flux repl` starts a prompt; expressions echo their value; declarations persist
  and are visible to later lines; `it` holds the last result.
- `:type`, `:reset`, `:list`, `:help`, `:quit` work as specified.
- A failed line (parse/type/runtime) prints an error and leaves the session intact.
- Multi-line forms (open brace/paren) continue the prompt.
- Tests: an integration test driving a scripted session end to end (definitions,
  expressions, `it`, an error line that is rolled back, `:type`).

## Drawbacks
[drawbacks]: #drawbacks

- **Imperfect semantics.** Re-execution of side-effecting declarations and O(n²)
  cost are real, even if rarely hit interactively; users may depend on Phase 1
  quirks before Phase 2 lands.
- **New long-lived entry point** to maintain as the language grows (new top-level
  forms, new effects). Mitigated by building on the existing parser/compiler.
- **Value printing** is only as good as Flux's value rendering; complex ADTs print
  with the runtime default absent a `Show` story.
- **Effects beyond `IO`** can't run at the bare prompt without a handler; the REPL
  must message this clearly rather than emit a raw error.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why accumulate-source first?** It reuses a proven pipeline, ships in days, and
  lets us settle UX and commands before paying for the runtime refactor. The cost
  model is acceptable for interactive typing.
- **Why not jump straight to the persistent engine?** That is a larger change to the
  symbol table and VM core (see [0176](0176_interactive_repl_persistent_engine.md));
  doing it first risks a long, invisible effort with no usable REPL meanwhile.
- **Could this be a library/macro?** No — a REPL drives the compiler and VM and owns
  stdin; it is inherently a tool/driver feature.
- **Impact of not doing this.** Flux stays file-only; the learning and exploration
  cost stays high, and the doc-comment eval feature remains the only interactivity.

## Prior art
[prior-art]: #prior-art

- **Elm (`elm repl`)** — historically an *accumulate-and-recompile* model very close
  to this phase: it wraps prior declarations and re-feeds them to the compiler.
  Direct precedent that accumulate-source is a legitimate shippable first cut for an
  ML-family language.
- **GHCi (Haskell)** — the gold standard, analyzed in depth in
  [0176](0176_interactive_repl_persistent_engine.md). Phase 1 borrows its `it`
  variable and "reject a bad line, keep the session" behavior, but not its
  incremental engine.
- **Python / Node.js** — interpreter-native REPLs; set the baseline user expectation
  ("previous lines in scope, results echo") that this phase meets.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Line editing dependency.** Adopt `rustyline` (history, editing, completion
  hooks) for v1, or ship raw `stdin` first? Affects dependencies and Windows
  behavior.
- **Multi-line continuation heuristic.** Brace/paren balance is simple; do we also
  continue on trailing operators / `->`? Where does the parser distinguish
  "incomplete" from "error"?
- **`:type` fidelity.** Generalized scheme (LSP hover style) or instantiated type?
  Probably the scheme.
- **Effect surface at the prompt.** Which effects are implicitly available beyond
  `IO`, and how do we present an expression that needs an unhandled effect?
- **ADT result rendering** without `deriving Show`.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Phase 2 ([0176](0176_interactive_repl_persistent_engine.md))** — the persistent
  `Compiler` + live-VM engine that removes this phase's limitations.
- **Editor-integrated REPL** — a VS Code "Flux: Start REPL" terminal command and a
  "send selection to REPL" action, reusing the extension's command plumbing
  ([../../editors/vscode/src/extension.ts](../../editors/vscode/src/extension.ts)).
- **`:load <file>` / `:reload`** — bring a module's definitions into the session.
- **Tab completion and `:doc`** powered by the LSP's completion/hover engines
  ([0163_flux_language_server.md](0163_flux_language_server.md)).
- **Notebook / transcript mode** sharing one evaluator with the `/// >>>`
  doc-comment eval feature.
