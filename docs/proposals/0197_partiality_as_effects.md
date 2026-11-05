- Feature Name: `partiality_as_effects`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft
- Umbrella: [0195](0195_checked_guarantees.md) (track T2)
- Depends on: [0161](implemented/0161_effect_system_decomposition_and_capabilities.md) (effect labels), [0165](implemented/0165_io_primop_migration_to_effect_handlers.md) (default handlers at the entry point)
- Supersedes: [0135](0135_total_functions_and_safe_arithmetic.md) Phase 3 (operators return `Option`)

# Proposal 0197: Partiality as effects

## Summary
[summary]: #summary

A Flux function that can stop the process type-checks as pure today. This
proposal makes the effect row say so, as Koka does:

- **`Exn`**: the function can fail at run time. Division or modulo by zero,
  `unwrap` of `None`, `panic`, a failing parse.
- **`Diverge`**: the function may not terminate. It is recursive, and the
  compiler cannot see an argument that gets smaller on every call.
- **`total`**: a function annotation meaning neither. The build rejects a
  `total` function whose body can raise `Exn` or `Diverge`.

`x / y` keeps its type and its syntax and gains `Exn`. Today's `Div` label
becomes a deprecated alias of `Exn`. A handler can catch `Exn`, and `main` and
test functions discharge it by default, so programs that crash today still
crash the same way, only now their types say so.

## Motivation
[motivation]: #motivation

### Purity is broken by every division

Proposal 0135 states the problem: "A function that panics is not pure — it has
an observable side effect (process termination) that isn't declared in its
type." Its Phase 3 proposes that `/` return `Option<Int>`, which makes ordinary
arithmetic hard to write. Flux has effect rows, which carry the same
information without changing the result type.

### The labels exist, and are switched off

Checked 2026-11-03 at b21d7f89:

- `Div`, `Exn` and `Panic` are declared (`src/syntax/builtin_effects.rs:52-72`,
  documented in `lib/Flow/Effects.flx:40-42`). `Exn` is reserved and rejected in
  annotations (`is_known_function_effect_annotation_name`, 122).
- `/` and `%` add no effect. `infer_arithmetic_operator`
  (`src/ast/type_infer/expression/operators.rs:177`) adds none to the HM row, and
  `Expression::Infix` in `infer_effects_from_expr` (`src/compiler/mod.rs:5012`) adds
  none to the compiler's effect set. `Num.div` (`lib/Flow/Num.flx:18`) has no row.
  Only `idiv`, `imod`, `index` and `array_get`, called by name, require `with Div`.
- `Panic` is filtered out of eight effect checks:
  `src/ast/type_infer/effects.rs:272,370`, `src/compiler/mod.rs:5333,5985`,
  `src/compiler/expression.rs:2001,3716`, `src/compiler/cfg_bytecode.rs:978,1117`.
  `panic`'s fallback HM scheme is pure on purpose (`src/compiler/mod.rs:4665`): "it
  diverges rather than returning, so no `with Panic` annotation is required
  today".
- An unannotated function infers its body's effects under a fresh open row, but
  publishes its type with `InferEffectRow::closed_empty()`
  (`src/ast/type_infer/function.rs:382`). The compiler's own effect inference
  patches the function's contract instead (`infer_unannotated_function_effects`,
  `src/compiler/mod.rs:4841`). An HM caller sees an empty row.

### The name `Div` means the wrong thing

In Koka, `div` means *divergence*: non-termination. In Flux, `Div` means
division by zero or an out-of-range index, a recoverable failure that is Koka's
`exn`. Adding non-termination under the name `Div` would collide. This proposal
moves division failure to `Exn` and names non-termination `Diverge`.

### Integer arithmetic has two holes beyond division

- Overflow wraps silently. `overflow-checks = false` (`Cargo.toml:74,84`), and
  E1010 `INTEGER_OVERFLOW` is defined but never raised.
- `i64::MIN / -1` stops the VM with a Rust panic, not a Flux error: "attempt to
  divide with overflow" at `src/vm/dispatch.rs:494` (checked 2026-11-03).

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

```flux
fn average(xs) {
    sum(xs) / len(xs)            // inferred: with Exn
}

fn safe_average(xs) {
    match len(xs) {
        0 -> None,
        n -> Some(sum(xs) / n),  // still Exn: the compiler does not track that n != 0
    }
}
```

Hover shows `average : (List<Int>) -> Int with Exn`. Nothing else changes.
`main` and `test_*` functions handle `Exn` by default: the program stops with the
same E1008 message and stack trace as today.

To promise that a function cannot fail, say so:

```flux
fn area(s) total {
    match s {
        Circle(r)  -> 3.14159 * r * r,   // Float division and multiplication: no Exn
        Rect(w, h) -> w * h,
    }
}

fn ratio(a, b) total {
    a / b        // error: `ratio` is declared total, but `/` can fail (Exn)
}
```

The fix is one of: use `safe_div`, take a `NonZero` (0198), or handle `Exn`:

```flux
fn ratio_or_zero(a, b) total {
    (a / b) handle Exn {
        raise(resume, _msg) -> 0     // no resume: the continuation is discarded
    }
}
```

Recursion that the compiler cannot see terminating gets `Diverge`:

```flux
fn length(xs) {               // total: the recursive call takes the tail
    match xs { [_ | t] -> 1 + length(t), [] -> 0 }
}

fn collatz(n) {               // with Diverge: n does not get smaller
    if n == 1 { 0 } else if n % 2 == 0 { 1 + collatz(n / 2) } else { 1 + collatz(3 * n + 1) }
}
```

`collatz` has no `Exn`: `n / 2` and `n % 2` divide by a non-zero literal (the
literal-divisor rule in Phase 3).

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

Every phase changes both effect checkers together: the HM rows
(`src/ast/type_infer/`) and the set-based inference (`src/compiler/mod.rs`). A
phase that changes one is a parity bug between them.

### Phase 1: `Exn` exists and `panic` raises it

- Accept `Exn` in `is_known_function_effect_annotation_name`, and update the unit
  test that asserts it is rejected (`builtin_effects.rs:347-366`).
- Map `Div` and `Panic` to `Exn` as aliases, through
  `seed_builtin_effect_aliases` (`src/compiler/mod.rs:3289`). An annotation that
  names either gets a deprecation warning.
- `primop_fine_effect_label` maps `Div | Mod | IDiv | IMod | Index | Panic` to
  `EXN`, and gains the `Unwrap` primop (`src/vm/core_dispatch.rs:3434`), which it
  does not list today. `SafeDiv` and `SafeMod` stay pure.
- Remove the eight `PANIC` filters listed above, and give `panic` the scheme
  `(String) -> a with Exn`, matching `lib/Flow/Primops.flx:33`.
- Seed one operation, `Exn.raise(String) -> a`, so a handler can name it.
  The runtime behind it is the existing `Try` primop and `flux_try`
  (setjmp/longjmp, `runtime/c/flux_rt.c:2786`).
- Keep `PrimOpEffectClass::CanFail` (`src/core/passes/helpers.rs:282`) for these
  primops, so dead-code elimination behaves as before: a dead division may still
  be dropped. A dropped division then has no `Exn` left to report, which is
  sound, since the failure no longer happens.

### Phase 2: inferred rows reach the type

- `infer_function_declaration` publishes the inferred row of an unannotated
  function (generalised over its tail) instead of `closed_empty()`
  (`src/ast/type_infer/function.rs:382`). This is a prerequisite for everything
  else: without it, a caller of `average` cannot see `Exn`.
- `infer_unannotated_function_effects` then becomes a check against HM, not a
  second source of truth. Where the two disagree, the build reports a compiler
  bug (E998, `CORE_LINT_FAILURE`), not a user error.

### Phase 3: `/` and `%` raise `Exn`

- `infer_arithmetic_operator` adds `Exn` to the ambient row for `/` and `%` on a
  type whose `Num.div` instance can fail (Int). Float division is IEEE and
  stays pure.
- `Expression::Infix` in `infer_effects_from_expr` adds the same label.
- **Literal-divisor rule:** a divisor that is a non-zero integer literal (and not
  `-1`, see Phase 6) adds nothing. This keeps `n % 2` and `x / 10` total, which
  covers most uses in `lib/` and `examples/` (to be measured before the phase
  lands).
- `class Num<a>`: `div` gains a row variable, so the Int instance can say
  `with Exn` and the Float instance can stay pure.

### Phase 4: discharge at the root

- `main` and `test_*` get a default `Exn` handler, installed the way 0165
  installs the IO handlers (`src/ast/route_effectful_primops.rs`). It prints the
  error and stops, as today, so runtime error snapshots
  (`examples/runtime_errors/`) do not change.
- Add `Exn` to the allowed set in `validate_main_root_effect_discharge`
  (`src/compiler/mod.rs:5676`).

### Phase 5: migration, then enforcement

A function annotated `with IO` that divides would now fail with E400 "missing
effect `Exn`". To avoid breaking every program at once:

- **5a.** A missing `Exn` in an annotated row is a warning, with a fix-it that
  adds `Exn` to the row. A `total` annotation is an error from the start.
- **5b.** After one release, a missing `Exn` is an error, like any other
  missing effect.

### Phase 6: `Diverge` and `total`

- Add `Diverge` as a label. For each strongly connected component of the call
  graph, a function gets `Diverge` unless every recursive call passes, in some
  fixed argument position, a value bound by a pattern inside that same
  parameter (structural recursion). Calls through an unknown function value
  carry that value's row, as today.
- `total` is a row annotation meaning the closed empty row minus IO, so it can
  sit next to `with` in the grammar. Parsing goes next to the existing `with`
  parser (`src/syntax/effect_expr.rs`).
- `i64::MIN / -1`: the VM and both native paths return the same Flux error
  as division by zero (E1008 or a new code), not a Rust panic.
- Overflow: either raise E1010 under `Exn` on `+ - *`, or document wrapping as
  the semantics. Pick one before Phase 3, since it decides whether `+` is total.

### Tests

- `tests/type_inference/type_inference_tests/effects.rs`: `Exn` inferred for `/`,
  `%`, `panic`, `unwrap`, and propagated through an unannotated caller (Phase 2).
- `tests/integration/flow_effects_spec_drift.rs`: updated for the new labels and
  aliases.
- Compiler-error fixtures with `//~`: `total` breaking `Exn`, `total` breaking
  `Diverge`, missing `Exn` warning (5a).
- Parity fixtures: `effect_exn_handled.flx` (handle `Exn` and continue),
  `effect_exn_default_root.flx` (root discharge prints E1008), and
  `arith_min_div_neg_one.flx`.
- The runtime error snapshots stay unchanged: that is the test for Phase 4.

## Drawbacks
[drawbacks]: #drawbacks

- Every annotated function that divides by a variable needs `Exn` in its row.
  Phase 5a softens it; the cost is still real.
- Structural recursion misses functions that terminate for a deeper reason
  (accumulators that grow toward a bound, binary search on an index range).
  They get `Diverge`. Koka accepts the same false positives.
- `Exn` on every Int division may push users toward `safe_div` and `Option`
  everywhere, which 0135 Phase 3 wanted and this proposal tries to avoid.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **0135 Phase 3, `/` returns `Option<Int>`.** Rejected: it changes the type of
  every expression that divides, and `/!` would be the operator everyone writes.
  An effect leaves the result type alone and lets one handler cover a whole
  computation.
- **Keep `Div` and add `Exn` beside it.** Rejected: two labels for "this can fail
  at run time" without a reason to handle one and not the other. `Div` survives
  as an alias.
- **Exempt `main` only and keep the filters elsewhere.** Rejected: the filter is
  what makes `panic` invisible to every caller.

## Prior art
[prior-art]: #prior-art

- Koka: `exn`, `div`, `total`, and the `pure = <exn,div>` alias. Koka infers `div`
  for recursion it cannot see terminate, much as Phase 6 proposes.
- Haskell: `error` and `undefined` are untracked. Liquid Haskell adds termination
  checks with a solver; this proposal does not.
- Rust: `checked_div` and `overflow-checks` are the model for the overflow
  choice in Phase 6.
- Flux 0131 (superseded) argued that `IDiv` crashes by accident and `panic` on
  purpose. This proposal gives both the same label, because a caller can handle
  neither differently.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should `Panic` stay a separate label that cannot be handled (0161 asked
  "Should `Panic` be handleable?"), or be an alias of `Exn` as proposed here?
- Overflow: `Exn` on `+ - *`, or documented wrapping?
- Does `Diverge` apply to `Async` code, where a server loop is meant not to
  terminate? Probably `Async` implies `Diverge`.

## Future possibilities
[future-possibilities]: #future-possibilities

- Range facts from `match` guards (`n if n != 0 ->`) that remove `Exn` from a
  division in the arm, a small step toward 0135 Phase 4 without a solver.
- `@fip` (0196) could imply `total`, as Koka's `fip` does.
