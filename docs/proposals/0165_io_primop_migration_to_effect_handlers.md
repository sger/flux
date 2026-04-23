- Feature Name: IO Primop Migration to Effect Handlers
- Start Date: 2026-04-20
- Status: Blocked (spike completed 2026-04-23 — see "Spike findings" below; 0170 has since landed, remaining blockers are 0161 and 0162 Phase 1)
- Proposal PR:
- Flux Issue:
- Depends on: [0099](0099_static_purity_completion.md) (Static Purity Completion — umbrella), [0161](implemented/0161_effect_system_decomposition_and_capabilities.md) (Effect System Decomposition), [0162](0162_unified_effect_handler_runtime.md) Phase 1 (Unified Effect Handler Runtime — evidence passing), [0170](implemented/0170_polymorphic_effect_operations.md) (Polymorphic Effect Operations — surfaced by this proposal's spike)

# Proposal 0165: IO Primop Migration to Effect Handlers

## Summary
[summary]: #summary

Rewire Flux's IO primops (`println`, `print`, `read_file`, `write_file`,
`read_stdin`) so they are lowered to `Perform` nodes against the
decomposed effect labels declared by 0161, and wrap every program
entry point in a compiler-synthesized default handler stack whose
handler bodies delegate to the existing C runtime entry points.

This is the concrete execution of Part 1 of 0099. Scope is Core
lowering + a new synthesis pass + test-harness integration. No new
user-visible syntax. No runtime-representation changes. No new labels
(0161 owns those). No handler-runtime changes (0162 owns those).

## Status
[status]: #status

Draft. Blocked on 0161 landing and 0162 Phase 1 landing.
The polymorphic effect-operation blocker identified by the spike is now
closed by [0170](implemented/0170_polymorphic_effect_operations.md).

## Motivation
[motivation]: #motivation

### Today: IO bypasses the effect system

Flux already has working effect syntax — `effect Console { print: String -> () }`,
`perform Console.print(s)`, `handle Console { print(resume, s) -> … }`
all parse and execute today, demonstrated by
[examples/guide_type_system/05_perform_handle_basics.flx](../../examples/guide_type_system/05_perform_handle_basics.flx).

But the IO primops do not go through that machinery. In
[src/core/passes/primop_promote.rs:26-31](../../src/core/passes/primop_promote.rs#L26-L31),
calls like `println(x)` are promoted directly to `CorePrimOp::Println`
and lowered to a direct C call to `flux_println` in
[runtime/c/flux_rt.c:229-237](../../runtime/c/flux_rt.c#L229-L237). The
`with IO` annotations on functions in
[lib/Flow/IO.flx](../../lib/Flow/IO.flx) are tracked by the type
system, but no `perform` is ever emitted, no handler frame exists at
runtime, and there is no way for a test to intercept the call.

### The payoff: structural, testable purity

Once IO is routed through `Perform`/`Handle`:

1. A function without `with Console` in its effect row **cannot** print.
   This is the "statically pure FP language" property 0099 was named
   after.
2. Tests can install their own `Console` handler to capture, redirect,
   or mock output — without touching `lib/Flow/IO.flx` or the C
   runtime.
3. The `CorePrimOp::Println` variant survives, but only as the
   implementation inside the compiler-synthesized default handler.
   Primops become an internal contract (aligning with 0164), not a
   user-facing surface.
4. IO is symmetric with user-defined effects. Handler composition, `with`
   restrictions, and capability grants from 0161 apply uniformly.

### Why a separate proposal from 0099

0099 is now an umbrella. 0156 → 0157 → 0158 set the precedent for
splitting umbrella proposals into focused execution proposals. This
proposal is the 0157/0158-shaped task for 0099 Part 1.

### Why not Koka's model

Investigation of `/Users/s.gerokostas/Downloads/Github/koka` confirmed
that Koka declares `console`, `fsys`, `net`, `ui` as kind-`X` phantom
types with no operations
([koka/lib/std/core.kk:54-78](../../koka/lib/std/core.kk#L54-L78),
[koka/lib/std/core/console.kk:23](../../koka/lib/std/core/console.kk#L23))
and calls `kk_print` directly via `extern`
([koka/kklib/src/string.c:872](../../koka/kklib/src/string.c#L872)).
Mocking is a global redirect ref
([koka/lib/std/core/console.kk:33-35](../../koka/lib/std/core/console.kk#L33-L35)),
not a handler swap. Zero tests in the Koka repo install a `console`
handler.

Flux took a different turn already: 0161 declares real `effect` blocks
with operations, and `effect Console { print: String -> () }` is
legal Flux syntax today. This proposal follows the Effekt/Unison
model (real operations + real handlers) rather than Koka's
phantom-label model. Koka informs label *decomposition* (0161), not
the runtime *handler model* (here).

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The mental model

After this proposal:

- `println("hi")` in user code is sugar for
  `perform Console.println("hi")`.
- Every program has an implicit wrapper synthesized around `main`:

  ```
  user_main() handle Console {
      print   (resume, s) -> { __primop_print(s);   resume(()) }
      println (resume, s) -> { __primop_println(s); resume(()) }
  } handle FileSystem {
      read_file  (resume, p)      -> resume(__primop_read_file(p))
      write_file (resume, p, c)   -> { __primop_write_file(p, c); resume(()) }
  } handle Stdin {
      read_line (resume, _) -> resume(__primop_read_line())
  }
  ```

- The `__primop_*` names are compiler-internal and reach the existing
  C runtime entry points (`flux_println`, `flux_read_file`, …). User
  code cannot name them.
- A test that wants to capture output writes its own `handle Console`
  block around the call site — same syntax as any other effect.

### What users see

No new syntax. `println(x)` still works. `with IO` still works (0161
makes it an alias over the decomposed labels). The only observable
change is that `with IO` now carries its weight: a function without
that row cannot print, checked by the existing effect checker.

### Test mocking example

Works today in principle once this proposal lands:

```flux
fn test_greets_world() {
    let captured = ref [||]
    greet("world") handle Console {
        println(resume, s) -> { captured := push(!captured, s); resume(()) }
    }
    assert_eq(!captured, ["Hello, world"])
}
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Scope boundary

| Concern | Owner |
|---|---|
| Which labels exist (`Console`, `FileSystem`, `Stdin`, `Clock`) | 0161 |
| `effect` declarations in `Flow.Effects` stdlib | 0161 |
| Handler-runtime algorithm (evidence passing, tail-resumptive specialization) | 0162 |
| Capability sealing (`sealing { Console }`) | 0161 |
| **Primop → `Perform` lowering** | **0165 (this proposal)** |
| **Default-handler synthesis around `main`** | **0165 (this proposal)** |
| **Retained primop naming for handler bodies** | **0165 (this proposal)** |

### Changes

#### 1. Primop promotion rewrites to `Perform`

Today in
[src/core/passes/primop_promote.rs:26-31](../../src/core/passes/primop_promote.rs#L26-L31):

```
App(Var("println"),  [s]) → PrimOp(Println,  [s])
App(Var("print"),    [s]) → PrimOp(Print,    [s])
App(Var("read_file"),[p]) → PrimOp(ReadFile, [p])
...
```

After this proposal, calls from user code route through the effect
system instead:

```
App(Var("println"),  [s]) → Perform("Console",   "println",   [s])
App(Var("print"),    [s]) → Perform("Console",   "print",     [s])
App(Var("read_file"),[p]) → Perform("FileSystem","read_file", [p])
App(Var("write_file"),[p,c]) → Perform("FileSystem","write_file",[p,c])
App(Var("read_stdin"),[]) → Perform("Stdin",     "read_line", [])
```

The table driving this mapping lives alongside the existing promotion
table so adding a new IO primitive touches one file.

#### 2. Retained-primop names for handler bodies

The `CorePrimOp::Println`, `::Print`, `::ReadFile`, `::WriteFile`,
`::ReadStdin` variants do **not** disappear. They remain as the
implementation the compiler-synthesized default handler calls. They
become unreachable from user surface code.

Naming convention: existing variant names are retained. The internal
callable name used by the synthesized handler is
`__primop_<lowercase>` (e.g. `__primop_println`). These names are
reserved — user code cannot define or call them.

#### 3. Default-handler synthesis pass

A new pass (placement: after AST construction, before type inference,
so the handler's effect row is visible to the inference engine) wraps
the body of each program entry point in the nested `handle` stack
shown above. Entry points:

- `main` in a normal program build
- every `test_*` function when invoked via `--test`

The synthesis is skipped when:

- a `#[no_implicit_handlers]` attribute is present on the function
  (escape hatch for low-level tests that want the raw runtime)
- `--dump-core`, `--dump-aether`, `--trace-aether` are active, to keep
  dumps readable (matches existing auto-prelude policy in `main.rs`)

The pass is implemented in a new file, e.g.
`src/ast/passes/synthesize_default_handlers.rs`, and registered in
the driver pipeline alongside the auto-prelude injection step.

#### 4. `lib/Flow/IO.flx` migration

[lib/Flow/IO.flx](../../lib/Flow/IO.flx) functions currently declare
`with IO`. After 0161, `IO` is an alias over the decomposed labels. No
function-body edits are required — calls to `println(v)`,
`read_file(path)`, etc. inside these functions route through the
effect system automatically via step 1.

#### 5. VM and native-backend impact

Both backends already lower `Perform`/`Handle` via the unified handler
runtime 0162 defines. Routing IO through `Perform` means both backends
call the same path they already use for user-defined effects. No
backend-specific work. The existing C runtime functions
(`flux_println`, etc.) continue to be the final implementation — they
are simply called from the synthesized handler body rather than
directly from primop lowering.

### Non-goals

- No new effect labels. 0161 owns those.
- No handler-runtime work. 0162 owns that.
- No removal of `CorePrimOp::Println` or its siblings. They are
  retained as the internal handler body.
- No change to NaN-box layout, Aether RC, or memory model.

### Error codes

Two new diagnostics, registered in
[src/diagnostics/compiler_errors.rs](../../src/diagnostics/compiler_errors.rs):

- **E4xx — implicit handler shadowing**: a user `handle Console { … }`
  that omits operations the synthesized default installs emits a
  warning unless the user's handler is inside a function annotated
  `with Console` (in which case they own the row).
- **E4xx — reserved primop name**: user-defined function named
  `__primop_*` is rejected.

## Test plan
[test-plan]: #test-plan

Acceptance tests, each as a new `.flx` file under `examples/effects/`
or `tests/effects/`:

1. **Capture test** — a user handler captures `println` output into an
   array; assertion on the array confirms no stdout emission occurred.
2. **Redirect test** — a user handler reroutes `print` to a log buffer;
   verifies the default handler is not invoked.
3. **Passthrough test** — code without a user handler prints to real
   stdout; verifies the synthesized default handler is wired up.
4. **Mixed test** — `with Console` but not `with FileSystem`; attempt
   to call `read_file` fails to type-check.
5. **Parity test** — the same program under `cargo run -- parity-check`
   produces identical stdout on VM and native backends.

## Exit criteria
[exit-criteria]: #exit-criteria

This proposal is complete when:

- calls to `println` / `print` / `read_file` / `write_file` /
  `read_stdin` in user code lower to `Perform` against the
  labels declared by 0161
- every `main` and every `test_*` function runs inside a synthesized
  default-handler wrapper
- the five acceptance tests above pass on both VM and native backends
- the five retained primop variants are not directly reachable from
  user source code
- 0099's Part 1 status row can be updated to "Delegated to 0165 —
  Implemented"

## Spike findings (2026-04-23)
[spike]: #spike-findings

A Console-only slice of this proposal was prototyped end-to-end before
being reverted. The spike's files were removed rather than kept as
dead code; branch `feature/effects` history up to this commit is the
record. What landed, what worked, and what blocked:

### What the spike built

- `src/ast/route_io_through_perform.rs` — AST pass rewriting
  `Call(Identifier("print"|"println"), [arg])` to
  `Perform { effect: Console, operation: print|println, args: [...] }`.
  Wired into `Compiler::run_pipeline` between alias expansion (Phase 1c)
  and predeclaration (Phase 2). Allocates fresh `ExprId`s via
  `ExprIdGen::resuming_past_program`.
- `src/ast/synthesize_default_handlers.rs` — AST pass wrapping the body
  of `main` and every `test_*` in a `handle Console { print(resume, s)
  -> { __primop_print(s); resume(()) } println(...) -> ... }` block.
- Reserved primop names `__primop_print` / `__primop_println` added to
  `primop_promote.rs`, `CorePrimOp::from_name`, and the base-function
  scheme table, with a carve-out in `try_emit_primop_call` so the
  reserved spellings do not re-raise the `Console` effect inside the
  handler arm.
- `Compiler::seed_builtin_effect_declarations` — registers
  `Console.print: String -> ()` and `Console.println: String -> ()` in
  `effect_ops_registry` + `effect_op_signatures` before user-declared
  effects are collected, so `perform Console.*` resolves without the
  user writing `effect Console { … }`.

The first user program (`examples/basics/print.flx`) executed end-to-end
through the rewritten pipeline: stdlib (`Flow.Assert`, `Flow.IO`)
compiled, `main`'s body was wrapped in the synthesized `handle`, and
runtime reached `flux_println`.

### The blocker: polymorphic effect operations

User-space `println` is polymorphic (`a -> ()`) — callers write
`println(42)`, `println(true)`, `println(some_list)`. An effect
operation signature must be monomorphic in today's compiler:
`Compiler::effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>`.
There is no generalization step that freshens a type variable per
call site, unlike `Scheme` instantiation for ordinary functions.

Two paths were attempted:

1. **Declare `Console.println: a -> ()`.** HM treated the `a` in the
   signature as a rigid skolem — the same `a` at every `perform` site.
   A call inside `assert_eq<a>(a, b)` fails with the infamous "Rigid
   type variable `a` cannot be unified with `a`": the op's `a` and the
   caller's `a` are different rigid variables that happen to share a
   name.
2. **Coerce the argument via `to_string(...)` at routing time.** Each
   `println(x)` was rewritten to `perform Console.println(to_string(x))`
   with `Console.println: String -> ()`. This type-checked, but
   changed observable output. Flux's runtime value printer
   (`flux_print_value` in `runtime/c/flux_rt.c:115–117`) quotes
   `String` values but prints `Int` / `Bool` / numbers bare — the
   distinction being "this came from a Flux string literal" vs "this
   is a number". Pre-stringifying via `to_string` erases that
   distinction, so `println(42)` — which printed `42` — began printing
   `"42"` (with quotes, as if it were a user-typed string literal).
   Matches the Koka/Effekt model only if the runtime stringification
   is taught that user-intent printing is distinct from
   string-representation dumps; that is a runtime-level fix that
   cascades into every debug print surface.

Neither path is a small patch. Path 1 demands real compiler work on
effect-operation generalization. Path 2 demands a second print surface
in the runtime (or moving the printf-style formatting into Flux stdlib,
which changes the value-printing behavior programs depend on).

### What this implies for 0165

The proposal is coherent as a design, but it silently assumes
polymorphic effect operations. That assumption deserves its own
proposal — call it **"Polymorphic effect operations"** — that
generalizes effect-op signatures the way function schemes are
generalized today. 0165 should be re-marked as depending on it, and
the "after 0161 lands" note in the header is insufficient: 0161 does
not supply op polymorphism.

Alternative workable scopes if polymorphic ops remain unavailable:

- **Monomorphic-string-only slice.** Require users to call
  `println(to_string(x))` explicitly. The routing pass becomes a typed
  rewrite that only fires when HM has proved `arg: String`, which
  flips the pass order (must run *after* type inference). Delivers
  handler-based mocking for string-valued prints only. Loses the "no
  user-visible change" guarantee in the proposal's guide-level story.
- **Introduce a new surface function** `emit : String -> ()` (or
  similar) that is routed through `Perform` from day one, while
  `println` / `print` remain direct primop calls. This is a 0099
  Part 1 compromise but preserves existing semantics.

None of these are pursued by this proposal as written. Recommended next
step is a short RFC on polymorphic effect operations before revisiting
0165.

## Unresolved questions
[unresolved]: #unresolved

- **Granularity of the default handler.** Install one handler per
  label (shown above), or a single composite handler with all
  operations? One-per-label composes better with partial user
  overrides; the composite is fewer handler frames at runtime.
  Recommend one-per-label; revisit if 0162's benchmarks show handler
  frame count dominates.
- **Escape hatch scope.** Should `#[no_implicit_handlers]` be per
  function, per module, or a CLI flag? Recommend per function only;
  CLI-level disable is too easy to leave on accidentally.
- **Should `--test` also wrap each `test_*` in an assertion handler?**
  Related but orthogonal; tracked separately if pursued.
