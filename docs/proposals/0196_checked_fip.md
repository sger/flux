- Feature Name: `checked_fip`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft
- Umbrella: [0195](0195_checked_guarantees.md) (track T1)
- Depends on: [0084](implemented/0084_aether_memory_model.md) (Aether)
- Relates to: [0189](0189_aether_owned_moves.md) (F3-3 arms claim their value, F3-5 shell-only free, KI-164), [0193](0193_parameter_ownership_annotations.md) (`@own` / `@borrow`)

# Proposal 0196: `@fip` is a checked promise

## Summary
[summary]: #summary

`@fip` and `@fbip` already parse, reach Aether and have an analysis. This
proposal makes them mean what Koka's do: a function marked `@fip` is rejected
unless, on the path where its argument is unique, every constructor reuses a
cell the function was given, it calls only other `@fip` functions, and it
consumes each owned parameter exactly once. Five steps:

1. Check the Aether program, not the Core before it.
2. Judge the unique path only, and count what really allocates.
3. Check that owned parameters are consumed once.
4. A registered error code, and a failed `@fip` is an error.
5. Optionally, native code drops the runtime fallback that the check has
   just shown is dead.

Line numbers below are as of b21d7f89.

## Motivation
[motivation]: #motivation

### The build checks the wrong program

`lower_core_to_aether_program` runs `check_fbip::check_fbip(&semantic_core, ..)`
(`src/aether/mod.rs:971`). That is the Core *before* `insert_reuse_aether`, so
no `Reuse` node exists yet, and every heap constructor counts as a fresh
allocation (`fbip_analysis.rs`, the Core arm of the fixpoint). The Aether-level
check, `check_fbip_aether`, runs only for `--dump-aether`
(`src/compiler/mod.rs:6621`).

The two disagree on the regression fixture that says every annotation in it is
"expected to remain provable today" (`examples/aether/fbip_success_cases.flx`).
In `tests/snapshots/aether/aether__aether__fbip_success_cases__dump_aether.snap`:

| Function | Dump header (Aether) | Build diagnostic (Core) |
|---|---|---|
| `rebuild_list` | `FBIP: fbip(1)` | `Fbip { bound: 1 }` |
| `rebuild_some` | `FBIP: fip` | proved |
| `set_black` | `FBIP: fip` | `NotProvable`: "calls imported or name-only function `Node`" |

`set_black` is the standard red-black tree example from the FP² paper. It
reuses its own cell, and the build still warns about it.

`docs/internals/aether.md:282` states that annotated functions "are checked
after Aether transformation, not before". The code does the opposite.

### Even on the Aether program, the analysis is imprecise

`analyze_expr_aether` (`src/aether/fbip_analysis.rs:477`):

- **It joins the two bodies of `DropSpecialized`** (563). The shared body
  rebuilds a fresh cell by design, so `rebuild_list` reads as `fbip(1)`, not `fip`.
  Koka judges a `fip` function on the unique path. The shared path is the
  copy-on-write fallback and is allowed to allocate.
- **A named constructor in the shared path is an `aether_call Node(...)`** and is
  classified as a call to an imported function, so it is reported `NotProvable`.
- **Static constructors count as allocations.** `is_heap_tag`
  (`src/aether/mod.rs:712`) is true for every named tag, but natively a nullary
  constructor or one whose fields are all constants is one static object
  (4cac2164, bbaebee0; `src/lir/emit_llvm.rs:668-699`). `Black` and `Leaf` cost
  nothing.
- **Some allocations are not counted.** `Lam` (480) builds a closure, and
  allocating primops are not counted either.
- **Any `Drop` followed by constructors marks the result `TokenUnavailable`**
  (559), even a drop of an unrelated field.
- **Owned parameters are never checked for linear use.** `Dup` is skipped (551),
  so a function that duplicates its owned argument and then "reuses" one copy
  passes, though the copy keeps the cell shared at run time.

### A failed promise is a warning

A failed `@fip` is `Diagnostic::warning` with category `Internal`
(`src/aether/check_fbip.rs:107`). A failed `@fbip` is an error, but with the
unregistered code `E999` through `make_error_dynamic` (114), the same ad hoc code
the Aether verifier uses. Neither is in the error-code registry, so
neither is listed in `docs/internals/error_codes.md` or stable across releases.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

```flux
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

@fip fn set_black(t) {
    match t {
        Node(_, l, k, r) -> Node(Black, l, k, r),   // reuses t's cell
        _ -> t,
    }
}
```

`@fip` promises: **when `t` is unique, this function allocates nothing.** `Black`
is a static object, and the new `Node` takes `t`'s cell. If `t` is shared, the
function still works, and copies.

If a change breaks the promise, the build says where:

```flux
@fip fn set_black(t) {
    match t {
        Node(_, l, k, r) -> Node(Black, l, k, Node(Red, Leaf, k, r)),
        _ -> t,
    }
}
```

The inner `Node(Red, ..)` has no cell to reuse, so the build fails with the new
code and points at that constructor. The fix is to restructure the function, or
to write `@fbip` (bounded allocation) and accept the cost.

The rules a `@fip` function must follow, on its unique path:

1. Every constructor that allocates takes a cell freed by a `match` in the same
   function, with the same size (exact tag match today).
2. It calls only functions that are themselves `@fip`, plus primops that do not
   allocate.
3. Each owned parameter is used exactly once: matched and reused, passed on,
   or returned.
4. It builds no closure.

`@fbip` relaxes rule 1 to a constant bound on fresh allocations, and allows
`@fbip` callees.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Step 1: check the Aether program

- In `lower_core_to_aether_program`, replace `check_fbip(&semantic_core, ..)`
  with `check_fbip_aether` over the transformed defs, after `insert_reuse_aether`
  and `specialize_reuse_aether` and before `annotate_owned_after`.
- Keep the Core-level fixpoint only to classify *callees* whose Aether is not
  available yet (imports), through their recorded `FbipSummary`.
- Test: `tests/aether/cli_snapshots.rs` `fbip_success_cases` no longer contains a
  `NotProvable` diagnostic. The dump header and the build agree on every function
  in both fixtures.

### Step 2: judge the unique path; count what allocates

In `analyze_expr_aether`:

- `DropSpecialized`: the fact for a `@fip` function is the fact of
  `unique_body`. The `shared_body` must only be well-formed (it may allocate).
  This mirrors `uses_shell_on_every_path` in `src/aether/reuse.rs`, which already
  keeps the split only when every unique path reuses.
- An `AetherCall` whose callee is a constructor of a local or imported data type
  is a constructor, not a call.
- A tag that lowers to a static object (nullary, or all fields constant) is not
  an allocation. Share one predicate between `is_heap_tag` and the LIR rule in
  `emit_llvm.rs`, so the two cannot drift apart again.
- `Lam` counts as an allocation. Primops are classified from a table next to
  `primop_effect_class` (`src/core/passes/helpers.rs:279`).
- `TokenUnavailable` is reported only when the dropped variable is the token a
  later constructor needed. 0189 F3-5's `FreeShell` node, a shell freed without
  its fields, is treated like a dropped token: the token is given up.

### Step 3: owned parameters are used once

- From the def's `borrow_signature`, take the owned parameters.
- On the unique path, count consuming uses with
  `analysis::owned_use_count` (`src/aether/analysis.rs:397`), extended to Aether.
  A `Dup` of an owned parameter, or of a field that a `Reuse` then claims, is a
  failure. Zero uses is a failure too unless the parameter is dropped, which
  for a unique cell is a free.
- With 0193, `@borrow x` on a `@fip` function is allowed. `x` is then read-only,
  and no reuse may claim it.

### Step 4: diagnostics

- Register a code for "fip contract broken" (next free compiler code: E088)
  in `src/diagnostics/compiler_errors.rs` and `src/diagnostics/registry.rs`, and
  a second for "fbip bound exceeded", or one code with two messages.
- A failed `@fip` and a failed `@fbip` are both errors. Their category is not
  `Internal`, which marks compiler bugs: add an `Ownership` variant to
  `DiagnosticCategory` (`src/diagnostics/types/diagnostic_category.rs`), which today
  has none for user-facing ownership errors.
- Each diagnostic carries a primary span on the offending constructor, call,
  closure or duplicate use, and a note pointing at the `@fip`. Today the span is
  the whole def.
- Migrate `examples/compiler_errors/fbip_fail_nonfip_call.flx` from `//~ E999` to
  the new code, and its snapshot.
- The message text is new diagnostic text: the maintainer writes it.

### Step 5 (optional): native code trusts the check

A `Reuse` inside the unique body of a `DropSpecialized` is still lowered with an
`IsUnique` test and a fresh `MakeCtor` fallback (`src/lir/lower.rs:3047-3154`).
For a function that passed the check, the fallback is dead. This is 0189 F3-3's
`claimed_tokens`. It is a performance change, not part of the guarantee, and
lands after F3-3.

### Tests

- `tests/aether/fbip_diagnostics.rs`: one test per rule broken (fresh
  constructor, non-fip call, closure, duplicated owned parameter), each asserting
  the code and the span.
- `examples/aether/fbip_success_cases.flx`: add a red-black `balance`, the FP²
  `map`, and a `reverse` with an accumulator, all proved `fip` both in the dump and in
  the build.
- A parity fixture per success case, so both backends run the code the check
  proved.
- Native: under `FLUX_HEAP_STATS=1`, a `@fip` function applied to a unique list
  reports no allocation beyond the input's (`tests/debug/native_debug_heap_tests/`).

## Drawbacks
[drawbacks]: #drawbacks

- A function that passes today, through the imprecise Core check, could fail the
  new check. That is the intent, but existing `@fip` users (the stdlib, examples)
  will need a pass. `grep -rn '@fip\|@fbip' lib examples` lists them.
- The check depends on Aether's reuse decisions. If a later Aether change
  stops reusing in some shape, a user's build breaks. The tests in step 4 pin
  the shapes that must keep working.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **A separate FIP type system** (FP²'s own calculus). Rejected: Aether already
  computes tokens and uniqueness splits. Checking its output reuses all of it,
  and the check cannot disagree with what the backends run.
- **Keep the warning.** Rejected: a promise that only warns is a lint, and its
  output today is wrong for `set_black`.

## Prior art
[prior-art]: #prior-art

- Koka `fip` / `fbip(n)`: Lorenzen, Leijen, Swierstra, "FP²: Fully in-Place
  Functional Programming", ICFP 2023. Koka checks on the unique path and allows
  the shared path to copy.
- Lean 4's reset/reuse (Ullrich and de Moura, "Counting Immutable Beans",
  IFL 2019), which Perceus builds on, reuses cells but gives the user no
  promise about it. This proposal adds one.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Does cross-constructor reuse (a cell of the same size but a different tag) come
  first? Without it, `fip` rejects some FP² examples (0189 future possibilities).
- Should a `@fip` function's guarantee be recorded in its module interface, so
  an importer's check does not need its body?

## Future possibilities
[future-possibilities]: #future-possibilities

- `@own` combined with `@fip` as a checked calling convention for libraries
  (0193 future possibilities).
- A hover in the language server that shows the reuse site for each
  constructor in a `@fip` function.
