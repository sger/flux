# Effects Examples

This directory shows the current effect system surface:

- public prelude operations such as `println`, `read_file`, and `now_ms`
- explicit function effects and aliases such as `IO` and `Time`
- modules with effectful public functions
- row-polymorphic callbacks
- user handlers
- parameterized handlers for state, reader-style environments, and captured output
- sealing
- developer tracing via `Flow.Debug` (stderr-routed, modeled on GHC's `Debug.Trace`)
- intentional failures for missing effects, denied sealing, reserved primop names, and reserved primop module imports

The user-facing operations are effect operations. Compiler-synthesized default
handlers at entrypoints delegate to internal `Flow.Primops.__primop_*`
intrinsics.

## Effect-row syntax at a glance

Flux writes effect rows in four different contexts. Each has a consistent
shape; the table gathers them so you can keep the distinctions straight
while reading the examples in this directory.

| Context | Shape | Example | Separator inside |
|---|---|---|---|
| `with` clause on a function | bare list | `fn f() with Console, Clock` | `,` between effect expressions |
| Effect expression (one row) | algebraic | `IO + Clock - Console`, `A + B \| e` | `+` / `-` for atoms, `\| e` for one open row-tail |
| Alias body | angle brackets | `alias IO = <Console \| FileSystem \| Stdin>` | `\|` |
| Sealing an expression | braces or ambient form | `f() sealing { Console \| Clock }`, `f() sealing (ambient - FileSystem)` | `\|` inside `{...}`; algebraic `ambient - E` inside `(...)` |

Reading this top-to-bottom:

- **`,`** only appears at the *outermost* level of a `with` clause,
  separating whole effect expressions.
- **`+` / `-`** are algebraic combinators inside a single effect
  expression — primarily useful when you need row subtraction
  (`A + B - B`) or extension over an open tail (`A + B | e`).
- **`|`** is the row-set separator for the two declaration-style forms
  that enumerate labels: alias bodies (`<A | B>`) and sealing rows
  (`{ A | B }`). Sealing rejects `,`; the parser hint points you at `|`.
- **`| e`** (lowercase single identifier after a `|`) is the explicit
  row-tail syntax for polymorphism; exactly one is allowed per effect
  expression.

A linter warning (**W013 `EFFECT ROW SEPARATOR STYLE`**) fires when a
`with` clause uses `+` without a matching `-` or `| e` — the parser
accepts it, but `,` is canonical for list separation in that position.
Reserve `+` for genuine row arithmetic.

## Entrypoint default handlers

The compiler wraps **`main`** and each **`test_*`** function with default
handlers for the operational prelude effects (`Console`, `FileSystem`,
`Stdin`, `Clock`). Inside those entrypoints you can call `println`,
`read_file`, `now_ms`, etc. without declaring `with Console` / `with
FileSystem` / `with Clock` and without writing a `handle` block.

This convenience is **scoped to entrypoints only**. Ordinary helpers,
module functions, and closures do not get it — they must declare their
effects explicitly, or the compiler rejects them with E400.

| Situation | File | Result |
|---|---|---|
| `println` inside `main` | [01_default_entry_handlers.flx](01_default_entry_handlers.flx) | ✓ compiles — `main` gets Console/Clock defaults |
| `println` inside a helper, no `with Console` | [failing/01_missing_effect_in_helper.flx](failing/01_missing_effect_in_helper.flx) | ✗ E400 — helpers are not entrypoints |
| `println` inside a helper, `with Console` declared | [02_explicit_effect_rows_and_aliases.flx](02_explicit_effect_rows_and_aliases.flx) | ✓ compiles — explicit effect row |

The rule to remember: **if you are writing a helper, declare your
effects.** The entrypoint shortcut exists so that script-style
top-level programs and test bodies do not have to spell `with
Console` to call `println`; it is not a general-purpose effect-
escape mechanism.

The rationale, policy boundary, and future direction for this behaviour
(whether it stays always-on, becomes opt-out, or is replaced with an
explicit capability grant) is tracked in proposal
[0171](../../docs/proposals/0171_effect_system_polish_and_hardening.md)
under the *default-handler policy* heading.

## `Flow.Primops` is the intrinsic layer

`Flow.Primops` is the compiler's intrinsic implementation layer for
effectful prelude operations. It exists so that synthesized default
handlers at entrypoints have a stable target to delegate to — every
`Flow.Primops.__primop_*` is the in-runtime implementation behind a
user-facing call like `println`, `read_file`, or `now_ms`. **User
code does not interact with this module directly.**

What you should learn instead, in order:

1. **Prelude calls.** `println`, `read_file`, `write_file`,
   `delete_file`, `read_line`, `now_ms` — these are auto-imported and
   are what your code actually writes.
2. **`Flow.Effects` labels.** `Console`, `FileSystem`, `Stdin`,
   `Clock`, `Debug` — the effects those calls require, the labels you
   put in `with` clauses, and the targets of `handle` blocks.
3. **`perform` / `handle`.** For user-defined effects (`effect E { ...
   }`) and for intercepting / shadowing the built-in operations
   inside your own scope.

The compiler enforces this boundary in two places:

| Attempt | File | Result |
|---|---|---|
| User function named `__primop_println` | [failing/03_reserved_internal_primop.flx](failing/03_reserved_internal_primop.flx) | ✗ Reserved-name diagnostic — `__primop_*` names cannot appear in user source |
| `import Flow.Primops` from user code | [failing/04_reserved_primops_module_import.flx](failing/04_reserved_primops_module_import.flx) | ✗ Reserved-module diagnostic — `Flow.Primops` is not user-importable |

If you find yourself wanting `Flow.Primops`, you almost certainly
want one of: a prelude call (just write `println(...)`), a `with`
clause on your function, or — for genuinely new behaviour — a
user-defined effect with `perform` and `handle`.

The decision to keep `Flow.Primops` *visible-but-discouraged* (rather
than relocate or hide the module) is recorded in proposal
[0171](../../docs/proposals/0171_effect_system_polish_and_hardening.md)
under Track 3.

## Handler coverage is total

Every `handle E { ... }` block must cover **every** operation declared by `E`,
even if the handled expression only performs a subset. This is the same rule
Koka, OCaml 5, and Eff apply: a handler is a *total* interpretation of an
effect.

Consequence: a handler that only cares about one operation still has to
mention the others. The common pattern is **handle-and-discard** — provide a
trivial arm that consumes the operation and calls `resume(())` to let the
surrounding program continue as if the operation silently succeeded.

[03_user_console_handler.flx](03_user_console_handler.flx) uses this idiom to
capture a `println` count while also neutralizing `print`:

```flux
do {
    println("captured: one")
    println("captured: two")
    2
} handle Console {
    print(resume, _msg) -> resume(())       // discard
    println(resume, _msg) -> resume(())     // discard
}
```

Both arms have identical `resume(())` bodies not because the example is
contrived, but because `Console` declares both operations and the handler
must account for both. If you forget an arm, the compiler (E402) prints a
copy-pasteable skeleton of the missing arms.

### Parameterized handlers and coverage

The same rule applies to parameterized handlers. In
[11_parameterized_console_capture.flx](11_parameterized_console_capture.flx)
the `Capture` effect declares `log` and `total`; the handle block must
supply both arms, each taking the threaded `state` as its final parameter.

### Partial / passthrough handlers

Flux does not currently support partial handlers or a `default`/catch-all
arm. The tradeoffs are tracked in proposal
[0171](../../docs/proposals/0171_effect_system_polish_and_hardening.md)
Track 2 and will be revisited when one of the proposed alternatives is
agreed on.
