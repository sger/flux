- Feature Name: Interactive REPL — Phase 2 (persistent-compiler / live-VM engine)
- Start Date: 2026-05-24
- Status: Implemented (2026-05-24)
- Proposal PR:
- Flux Issue:
- Depends on: [0175_interactive_repl.md](0175_interactive_repl.md) (Phase 1 — defines the user-facing REPL contract and ships the accumulate-source MVP)
- Builds on: the persistent-`Compiler` model used by the language server ([../../crates/flux-lsp/src/prelude.rs](../../crates/flux-lsp/src/prelude.rs)), the module-interface global preload path ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs)), and the VM globals model ([../../src/vm/mod.rs](../../src/vm/mod.rs))
- Relates to: [0163_flux_language_server.md](0163_flux_language_server.md), [0076_debug_toolkit.md](0076_debug_toolkit.md), [0038_deterministic_effect_replay.md](0038_deterministic_effect_replay.md)

# Proposal 0176: Interactive REPL — Phase 2 (Persistent-Compiler / Live-VM Engine)

## Summary
[summary]: #summary

Replace the REPL engine from [Phase 1 (0175)](0175_interactive_repl.md) — which
re-runs an accumulated source buffer each line — with an **incremental** engine
that compiles each entered line against a **persistent `Compiler`** and runs it on
a **single live `VM`** that retains earlier globals. Earlier lines are never
recompiled and side effects in declarations never re-fire. This converges Flux's
REPL onto the architecture GHC's interactive evaluator (GHCi) uses: a persistent
compile-time environment plus a name-keyed runtime symbol table. The user-facing
behavior is **identical** to Phase 1 (see [0175's Guide-level
section](0175_interactive_repl.md)); this proposal is an engine swap behind that
same contract.

## Motivation
[motivation]: #motivation

[Phase 1 (0175)](0175_interactive_repl.md) ships a working REPL but documents two
costs inherent to re-running the whole session each line:

- **Re-execution of side-effecting declarations.** A top-level
  `let _ = print("loading")` re-prints on every subsequent line, because the entire
  buffer re-runs.
- **O(n²) cost.** A session of *n* lines re-parses and re-compiles the accumulated
  buffer each line, with a fresh VM per line.

For interactive typing these are tolerable, but they make Phase 1 unsuitable for
longer sessions, `:load`-ing real modules, machine-driven input, or any workflow
where declaration side effects matter. Phase 2 removes both by compiling and
linking each line *incrementally*, the way mature REPLs do.

This is also a strategic investment: the same "compiler-and-runtime as a persistent
service" capability underpins richer interactive tooling (notebooks, an
editor-embedded REPL, a debugger prompt). Flux is well-positioned because the
compiler frontend is **already** run as a persistent service by `flux-lsp`.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

There is **no new user-facing surface** in this phase. The prompt, the cumulative
session, the `it` variable, the colon-commands (`:type`, `:reset`, `:list`,
`:help`, `:quit`), multi-line input, and error-rollback behavior are exactly as
specified in [Phase 1's Guide-level section](0175_interactive_repl.md).

The only *observable* differences are improvements:

- **Faster.** Each line does work proportional to that line, not the whole session.
- **No spurious re-runs.** A side effect written in a declaration runs once, when
  the declaration is entered — not again on every later line.

A user upgrading from the Phase 1 engine to the Phase 2 engine should notice only
that the REPL got faster and stopped re-printing declaration side effects.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### The crux: cross-line state, and why Phase 1 re-runs everything

The hard part of any REPL is making line *N* see the names bound by lines *1..N-1*.
Flux's current execution model makes the naive "keep the VM alive and feed it new
chunks" approach unsafe:

- **VM globals are addressed by compile-time slot index**, allocated from 0 on
  every compile. `SymbolTable::define`
  ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs))
  increments `num_definitions` per global; the VM reads `globals[index]`
  ([../../src/vm/mod.rs](../../src/vm/mod.rs)). A *new* independent compile re-issues
  slots 0, 1, 2…, which would collide with a still-live VM's earlier globals.

That single fact is why Phase 1 re-runs the whole buffer (one compile, consistent
slots) instead of keeping state. Phase 2's job is to make per-line compiles agree
on a **stable, monotonic** name→slot mapping and run them on one VM.

### The reference architecture: how GHCi does it

GHCi (studied in the GHC tree at `E:\Github\ghc`) is the canonical solution and
resolves **everything by name, never by fixed offset**, via two persistent pieces
of state:

1. **`InteractiveContext`** (`compiler/GHC/Runtime/Context.hs`) — the *compile-time*
   accumulator. It holds `ic_tythings` (all user-defined things, newest first),
   `ic_gre_cache` (the in-scope reader environment), `ic_instances`, `ic_fix_env`,
   and `ic_mod_index` (the `Ghci1`, `Ghci2`… counter). Each line is
   renamed/typechecked *inside* it: `runTcInteractive` (`compiler/GHC/Tc/Module.hs`)
   seeds the typechecker with `icReaderEnv` and the accumulated `ic_tythings`. After
   a line succeeds, `extendInteractiveContextWithIds` prepends the new bindings and
   bumps the index.

2. **`closure_env`** (`compiler/GHC/Linker/Types.hs`, a `NameEnv (Name,
   ForeignHValue)`) — the *runtime* symbol table. Each line is compiled to bytecode
   objects (BCOs); `linkBCO` (`compiler/GHC/ByteCode/Linker.hs`) resolves references
   to earlier lines **by name** against this persistent table. New closures are
   appended after each line via `extendLoadedEnv` (`compiler/GHC/Linker/Loader.hs`).

A bare expression is auto-wrapped as `it <- expr; print it` (the `it` variable,
`compiler/GHC/Tc/Module.hs`). The decisive property: **GHCi never recompiles
earlier lines** — each line is compiled independently and *linked by name* into
persistent compile-time and runtime environments.

### Flux already has the building blocks

Flux does not have a GHCi-style `NameEnv` linker, but it has the two pieces that
matter, used today by the module system and the language server:

- **A reusable frontend.** `flux-lsp` keeps one persistent `Compiler` with
  accumulating `cached_member_schemes`
  ([../../src/compiler/mod.rs](../../src/compiler/mod.rs)) and resets only per-file
  state between documents via `phase_reset_for_lsp`
  ([../../src/compiler/passes/reset.rs](../../src/compiler/passes/reset.rs)). This is
  Flux's `InteractiveContext` analog for the *type/name* side.
- **Name-keyed cross-unit global resolution.** Imported-module globals are preloaded
  at fixed indices via `define_global_with_index`
  ([../../src/compiler/symbol_table.rs](../../src/compiler/symbol_table.rs)). A REPL
  line is morally a module that imports the session, so this is Flux's analog to
  `closure_env` for the *runtime* side.

### Phase 2 design: module-per-line on a persistent Compiler + live VM

Each entered line becomes a synthetic module `Repl{N}` compiled against persistent
session state:

1. **Persistent `Compiler`.** Keep one `Compiler` alive for the session (as
   `flux-lsp` does — [../../crates/flux-lsp/src/prelude.rs](../../crates/flux-lsp/src/prelude.rs)),
   with the prelude/standard library loaded once. Per-line, reset only transient
   state, preserving accumulated schemes and the session's global mapping.

2. **Session symbol/slot allocator (the new piece).** Maintain a persistent
   `name → global_slot` map that **never resets**: newly defined globals get fresh,
   monotonically increasing slots; each new line's compile resolves references to
   earlier names at their recorded slots (via the `define_global_with_index` preload
   path), exactly as cross-module imports resolve today. This is the direct analog
   of GHCi linking by name into `closure_env`.

3. **Live `VM`.** Keep **one `VM` instance** alive across lines
   ([../../src/vm/mod.rs](../../src/vm/mod.rs)). New compiled chunks execute on the
   live VM and *extend* `vm.globals` rather than replacing it; references to earlier
   globals read the slots populated by earlier lines. A bare expression compiles to
   code that binds `it` (the same `let it = <expr>` shape Phase 1 uses) and prints
   it; the result persists in the session for the next line.

4. **`it`, `:type`, rollback, multi-line, colon-commands** are unchanged from
   [0175](0175_interactive_repl.md). Rollback is now cheaper and cleaner: a line that
   fails to compile/typecheck is discarded *before* its globals are committed to the
   session map and the VM, so no re-run is needed to keep the session intact.

### What is genuinely new vs. reused

| Concern | Status |
|---|---|
| Persistent frontend / accumulated schemes | **Reused** — the LSP pattern (`phase_reset_for_lsp`, `cached_member_schemes`) |
| Cross-line name resolution at compile time | **Reused** — module-interface preload (`define_global_with_index`) |
| Monotonic session slot allocator (never reset) | **New** — small, the heart of this proposal |
| Running additional bytecode chunks on a live VM holding earlier globals | **New** — the main runtime change |
| `it`, commands, UX | **Reused** — defined by Phase 1 (0175) |

Mapping to GHCi: persistent `Compiler` + module-interface preload ≈
`InteractiveContext`; the session slot map + live VM ≈ `closure_env` + the bytecode
interpreter; `it` is identical.

### Acceptance criteria (Phase 2)

- All Phase 1 acceptance criteria still pass (same UX, same tests).
- A side-effecting top-level declaration (e.g. `let _ = print("hi")`) runs its
  effect **once**, at entry, and not again on later lines — asserted by a test.
- Per-line work does not grow with session length: a defined function called on
  line *N* does not recompile lines `1..N-1` (verified structurally, e.g. via a
  compile counter / instrumentation, not wall-clock).
- A live-VM test: define `let x = 5`, then `x + 1` resolves `x` from the persistent
  VM globals without re-executing line 1.
- Engine swap is invisible: the Phase 1 integration session test passes unchanged
  against the Phase 2 engine.

## Drawbacks
[drawbacks]: #drawbacks

- **Largest change in the REPL effort.** It touches the symbol-table slot model and
  the VM's global lifecycle — core compiler/runtime surfaces — so it carries more
  risk than Phase 1.
- **Two engines transiently.** Until Phase 2 is proven, the codebase may carry both
  the accumulate-source path and the incremental path; the swap must be clean.
- **Slot-model invariants.** A monotonic, never-reset allocator introduces new
  invariants (no slot reuse within a session, shadowing creates a new slot) that
  must be upheld everywhere globals are emitted/read.
- **Effect/handler lifetime across lines.** Long-lived effect state on a persistent
  VM (handlers, resources) needs a clear lifetime story that Phase 1 sidesteps by
  starting fresh each line.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why module-per-line rather than a from-scratch GHCi-style `NameEnv` linker?**
  Flux already resolves cross-module globals by name + preloaded index; reusing that
  path is far less risky and less code than inventing a new linker environment.
- **Why a monotonic slot map instead of switching globals to name-keyed lookup
  everywhere?** Keeping slot addressing (just never resetting it per session)
  preserves the VM's fast global access and minimizes churn; only the *allocator*
  becomes session-scoped.
- **Why not stay on Phase 1 forever?** Re-execution of declaration side effects is a
  correctness wart, and O(n²) blocks `:load` and larger sessions.
- **Why not a tree-walking Core interpreter instead?** Simpler state threading, but
  it means a second execution engine to maintain alongside the bytecode VM, with
  divergent semantics — rejected.

## Prior art
[prior-art]: #prior-art

- **GHCi (Haskell)** — the reference design analyzed above: `InteractiveContext`
  (`compiler/GHC/Runtime/Context.hs`), statement evaluation
  (`compiler/GHC/Runtime/Eval.hs`, `compiler/GHC/Driver/Main.hs`), the `it` binding
  and bare-expression wrapping (`compiler/GHC/Tc/Module.hs`), and the name-keyed
  bytecode linker (`compiler/GHC/Linker/Types.hs`,
  `compiler/GHC/Linker/Loader.hs`, `compiler/GHC/ByteCode/Linker.hs`). Lesson:
  persistent compile-time + runtime environments resolved by name; never recompile
  earlier input.
- **OCaml (`ocaml` toplevel / `utop`)** — incremental top-level with a persistent
  environment; `utop` layers editing/completion on top. Lesson: keep the evaluation
  engine separate from the line-editing UX.
- **Rust (`evcxr`)** — REPL for a non-REPL language; compiles each line to a dylib
  and links incrementally. Lesson: per-line compilation + linking gives a good REPL
  even for a compiled language — encouraging for Flux's bytecode VM.
- **Idris / Agda** — incremental, dependently typed REPLs with rich `:type`/`:doc`;
  cheap once the frontend is a persistent service (as Flux's already is via the LSP).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Exact slot-allocator design.** How the monotonic session map interacts with
  shadowing (new slot per redefinition?), with `:reset`, and with `phase_reset` —
  this is the core implementation risk and may warrant a short design note before
  coding.
- **Live-VM chunk execution.** The precise mechanism to append and run a new
  bytecode chunk on a VM that already holds globals, including stack/frame hygiene
  between lines.
- **Effect/handler lifetime** on a persistent VM across lines.
- **Memory growth.** A never-reset slot map and live VM accumulate state for the
  session's lifetime; is `:reset` the only reclamation, and is that enough?
- **Interaction with the native (LLVM) backend.** Phase 2 targets the VM; a JIT REPL
  is explicitly out of scope (see Future possibilities).

## Future possibilities
[future-possibilities]: #future-possibilities

- **Editor-integrated REPL** — a VS Code "Flux: Start REPL" terminal and "send
  selection to REPL", reusing the extension command plumbing
  ([../../editors/vscode/src/extension.ts](../../editors/vscode/src/extension.ts)).
- **`:load <file>` / `:reload`** — now cheap on the persistent engine; bring a
  module's definitions into the session via the module graph.
- **Tab completion and `:doc`** powered by the LSP completion/hover engines
  ([0163_flux_language_server.md](0163_flux_language_server.md)).
- **Notebook / transcript mode** sharing one evaluator with the `/// >>>`
  doc-comment eval feature; deterministic replay of a session relates to
  [0038_deterministic_effect_replay.md](0038_deterministic_effect_replay.md).
- **Debugger at the prompt** — breakpoints and value inspection on the live VM,
  connecting to [0076_debug_toolkit.md](0076_debug_toolkit.md).
- **Native-backend (JIT) REPL** — a much larger, separate effort beyond the VM.

## Implementation notes (2026-05-24)
[implementation-notes]: #implementation-notes

The engine landed as `src/repl/` (`mod.rs` dispatch loop + `engine.rs`
`ReplEngine`). Resolved the unresolved questions above as follows:

- **Per-line compile on a persistent `Compiler`, run the buffer's tail.** Each
  line is parsed against the compiler's interner (the `parse_module_for_goto_def`
  idiom) and compiled alone via `compile_with_opts`, appending to the one
  persistent top-level instruction stream. The engine captures
  `top_level_instruction_len()` *before* compiling, then runs the compiler's
  **full** `bytecode()` from that offset with `VM::run_top_level`. Running the
  whole stream (not an isolated delta) is required because the compiler emits jump
  operands as **absolute** offsets into the stream — a top-level `if` / `match`
  entered at the prompt only resolves its jumps when the full stream is present —
  while starting at the offset skips the prelude and earlier lines so their side
  effects never re-fire. (An earlier delta-only `run_chunk` design mis-jumped on
  top-level control flow precisely because the delta's absolute jump targets
  pointed outside it.)
- **Slot allocation / shadowing.** The symbol table's `num_definitions` is the
  monotonic slot counter; it survives `phase_reset`, so each line's new globals
  get fresh slots and earlier slots stay resolvable. Redefinition is handled by
  `forget_session_binding` (drops the old symbol-table entry + cached scheme), so
  the rebind lands on a new slot — the old value lingers, unreachable, in the VM.
- **Type resolution across lines.** Name resolution works through the persistent
  symbol table, but HM inference needed help: a bare reference to an earlier
  session global isn't in the current line's source, so it inferred as `_`. The
  compiler's REPL mode accumulates each line's `resolved_binding_schemes` into
  `repl_session_schemes`, merged into `build_infer_config`'s base schemes.
- **Rollback.** `Compiler` derives `Clone`; the engine clones it before each line
  and restores the clone on any parse/compile/runtime failure, so a bad line
  never corrupts the session. On a runtime error, partially-written globals become
  unreachable after the compiler rollback (the slot is reused), and the next line
  re-issues the rolled-back compiler's full bytecode, which restores the VM's
  constants pool.
- **Expressions.** A bare expression is first tried as a top-level
  `let __repl_N = <expr>` (so it persists and `it` can reference it) plus a fresh
  `fn main` that prints it. If that fails with E413 (top-level effect), it falls
  back to running inside `main` — the effect happens but the result is not bound
  to `it`.

### Known v1 limitations
- **Cross-line use of `data`-with-named-fields, `effect`, and `class` / `instance`.**
  A declaration of these kinds works within a single line and in a whole file, but
  using it on a *later* line fails: the named-field desugar metadata is collected
  per-program (so the later line doesn't see the earlier `data`'s fields, E430/E082),
  user effect registries are reset by `phase_reset` to the preloaded set each
  compile (so a later `with`/`handle` reports E407 "Unknown Effect"), and class
  methods don't resolve across lines (E004). Enum and **positional** ADTs, `let` /
  `fn`, recursion, `if` / `match`, imports, and `it` all work across lines. Closing
  these needs the same kind of cross-line threading used for binding schemes
  (`repl_session_schemes`), extended to ADT/effect/class collection state — a
  follow-up.
- **Self-referential rebind.** `let x = x + 1` reports `x` as undefined rather
  than reading the previous value, because the old binding is forgotten before
  the line compiles (the alternative — keeping it — silently reads the new,
  uninitialized slot, since the compiler defines the binding before its
  initializer).
- **Effectful-result `it`.** An effectful expression's value isn't captured by
  `it` (it can't be a top-level binding). An effectful *declaration* (e.g.
  `let _ = print("hi")`) does now run its effect once — it is re-run inside a
  synthesized `main` — but a *named* effectful binding still can't persist into
  the session (the REPL prints a note saying so).
- **`:type` / `:list`** use a lightweight parallel record of committed
  declaration sources, re-inferred over a fresh compile, rather than reading
  types back from the persistent compiler (whose post-compile expression IDs are
  keyed to a routed/desugared program clone).
