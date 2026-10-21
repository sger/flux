- Feature Name: `aether_owned_moves`
- Start Date: 2026-10-17
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0084](implemented/0084_aether_memory_model.md) (Aether), [0114](implemented/0114_aether_perceus_completion_plan.md) (Perceus completion), [0188](0188_flux_debugger.md) S10c (the debug heap, used here as the oracle)
- Relates to: [0153](implemented/0153_decouple_aether_from_core_ir.md) (Aether as its own IR), [0174](0174_async_effect_concurrency.md) (values shared across worker threads)

# Proposal 0189: Owned arguments are moved

## Summary
[summary]: #summary

Native code copies every owned call argument: the LIR emits a `dup` before
the call (`dup_owned_call_args`, `src/lir/lower.rs:3964`), and nothing ever
releases that extra reference. This proposal makes owned arguments **moves**:
the caller hands the callee exactly one reference, which Aether already
plans for.

Two things follow:

- **In-place reuse comes back natively.** A function such as `map` or
  `bump` can rewrite the cells of a list it was given its only reference to
  (FBIP), instead of allocating new ones.
- **The leak goes.** Each owned call today leaves one reference behind.

The copy cannot simply be removed. It currently hides seven places that hand
out references nobody counted (§Motivation), and removing it first would turn
each one into a use after free. The work is therefore done in six steps,
ordered so that every step before the one that removes the copy can only add
leaks, never crashes. The debug heap (0188 S10c) checks each step.

**Where the tree is (2026-10-21).**
- The copy is already gone. 0190 S15 removed `dup_owned_call_args` for Flux
  calls and for C primops (8e7000c9, KI-120), ahead of F3-1 to F3-3.
- One hazard below was live on that commit (V6). A Flux function returns an
  array element, the array is then given to a call that releases it, and the
  element reads as freed memory (`fn get0(a) { a[0] }`, then `discard(a)`).
  F3-1 adds that program as its failing test. The `owned_moves` fixture
  `array_element_outlives_array.flx` is a different shape and passes today.
- The other hazards stay hidden only because native code still leaks the
  values that would expose them (KI-119, KI-121).
- A second pin exists: the native match copies every field a constructor
  arm binds (`emit_match_ctor`, `src/lir/emit_llvm.rs`), and nothing
  releases those copies.

The steps below are ordered around these facts.

## Motivation
[motivation]: #motivation

### Where this comes from

The debug heap found two reference-count bugs in Aether, fixed on
`feat/debugger`:

- **F1** (b0c230d8): a value dropped twice in nested matches.
- **F2** (7645f84f): about 51 stdlib functions were inferred to *borrow* a
  parameter they actually released, by reusing it in place or dropping it. The
  caller's guard released it again.

F2 made the contract sound:

- a parameter a function may release is inferred `Owned`;
- a `Borrowed` parameter is never released;
- the verifier rejects one that is (`BorrowedParamReleased`).

That fix had a cost. Before F2, the only way native code reused a cell in
place was the borrowed channel, which was unsound. After F2, those parameters
are owned, and native code copies every owned argument. So the parameter is
never unique on entry (its count is at least 2), and the function allocates.

```flux
fn bump(xs) { match xs { [h | t] -> [h + 1 | bump(t)], _ -> [] } }
fn main() { print(bump(build(5000))) }   // allocates a fresh cell per element
```

**Observed after F2:**
- Aether's own totals over `examples/aether/bench_reuse.flx`: reuses 17 → 15,
  fresh allocations 26 → 32.
- The lldb reference count of a list inside an owned callee reads one
  higher than the program's own references (`native_debug_tests::lldb_counts_references`
  expects 3 where the program holds 2).

### Why the copy cannot just go

Aether plans owned arguments as moves already: `plan_var` inserts `dup x in x`
when `x` is still live after the call (`src/aether/insert.rs`). The LIR copy
is therefore an extra pin.

Reading the code shows seven places that rely on that pin, because they hand
out a reference that no count records. These are predicted from the code; the
F3-0 fixtures confirm each before it is fixed.

| | Source | Where | What goes wrong without the pin |
|---|---|---|---|
| V1 | Pattern fields | `bind_pattern` loads fields without a copy (`src/lir/lower.rs:5589`); `plan_var` copies only tuple and nested fields, fields of a borrowed parent, or lent values | `[_ \| t] -> bump(t)` moves `t` out of a cell another holder still sees; the callee finds it unique and rewrites it |
| V2 | Closure captures | loaded from the environment without a copy (`src/lir/emit_llvm.rs:1178`); the body is planned with a fresh environment | the last owned use of a capture moves it, so a closure called twice uses a freed or rewritten value |
| V3 | Handler values | the runtime passes the operation's argument borrowed (`runtime/c/effects.c:838`), but the handler body is planned as if it owned it | a handler that passes its argument on releases what the performer still holds |
| V4 | Continuation captures | stored without a copy (`src/lir/emit_llvm.rs:3029`); `cont_split` reuses the parent's blocks with an empty entry (`src/lir/cont_split.rs:244`) | a value moved after a `perform` is moved again on each resume of a multi-shot handler |
| V5 | Globals and dictionaries | `plan_var` emits no copy for a binder not in scope; one closure is cached per block (`src/lir/lower.rs:1663`) | a global list passed to an owned parameter twice is released twice |
| V6 | Results from the C runtime | `flux_array_at`, `flux_array_get` (`runtime/c/array.c:103`, `:116`), `flux_hamt_get` (`runtime/c/hamt.c:424`), `flux_wrap_some`, `flux_unwrap`, `flux_unwrap_or`, `flux_ho_find` (`runtime/c/flux_rt.c`) return values without counting them; `set`, `push`, `concat` and `slice` copy elements with `memcpy` (`array.c:127-155`) | a value taken from an array or map and passed on is released while the collection still holds it |
| V7 | Owned C primops | `ArrayPush` and `ArrayConcat` take their source owned but never release it | without the pin, the source is freed while the new array still shares its elements |

**Corrections, from reading the tree for F3-0:**
- **V2 and V4.** Captures are counted when they are stored:
  `flux_make_closure` retains them, continuations included. What goes wrong
  is that the closure's one reference is consumed on every call or resume.
- **V5.** Natively, a top-level value is lowered afresh at each use. The one
  shared reference is the closure a function used as a value gets, cached
  per block (`func_closure_in_block`, `src/lir/lower.rs`). Planning a `dup`
  of a global does not work: LIR lowering looks the binder up before it is
  bound.
- **V7.** Primop nodes lend all their arguments. So `push`, `concat`, `set`
  and a map's `set` store values and share elements that the caller still
  holds and will release. The runtime has to count both.
- **V1.** It is real only where a match loads fields without the match-time
  copy: guarded arms, single-arm matches, arms with refutable field checks,
  and nested sub-patterns.

**Already sound:**
- C higher-order functions copy before calling a closure (`call1`/`call2` in
  `flux_rt.c`).
- Partial application copies its stored arguments
  (`src/llvm/codegen/closure.rs`).
- `perform` already moves its arguments.
- Worker mode emits no reference counting at all.

## The ownership contract
[contract]: #contract

One statement, enforced by the planner and the verifier:

1. **A borrowed argument stays with the caller.** The callee never releases
   it (F2, `BorrowedParamReleased`).
2. **An owned argument hands over exactly one reference.** If the caller still
   needs the value, Aether copies it before the call; otherwise the caller
   gives up its own.
3. **A view is copied before any owned use.** Views are:
   - a pattern field;
   - a tuple field or member read from a value;
   - a closure capture;
   - a handler's parameter, state or `resume`;
   - a global;
   - a value a runtime call returns without counting it (until F3-1
     counts those).
4. **A match arm may claim its value** when all of these hold:
   - the matched value is a plain owned variable, not lent and not a view;
   - the pattern is a constructor whose sub-patterns are variables or
     wildcards;
   - the value is dead after the arm.

### The claim (Perceus)

In a claiming arm, the planner:
- copies every field the arm uses;
- then drops the matched value.

Drop specialization (`src/aether/drop_spec.rs`) then splits the arm on
uniqueness, **before** any field is consumed:

- **Unique path:** the cell is the arm's alone. The field copies cancel against
  the cell's own references, and a rebuild of the same shape rewrites the cell
  in place, with no check left at the rewrite.
- **Shared path:** the fields were copied, so dropping the cell gives up only
  the arm's reference. This path keeps a real drop. Today `rewrite_mode`
  removes it in both modes (`drop_spec.rs:570`), which leaks on the shared
  path; F3-3 fixes that.

`bump` after this proposal (predicted):

```
case xs of Cons(h, t) -> drop_specialized xs {
  unique -> let a = IAdd(h, 1); let r = bump(t); reuse xs as Cons(a, r)
  shared -> dup h in dup t in drop xs in let a = IAdd(h, 1); let r = bump(t); Cons(a, r) }
```

These arms fall back to rule 3 plus today's release logic, which is sound but
does not reuse:
- arms with nested or tuple patterns;
- arms whose value is lent, a view, or live afterwards;
- arms containing a `perform`, a handler, a `letrec` or a lambda, which drop
  specialization already declines.

### Why it is sound

| Caller | What happens |
|---|---|
| Linear (gives its last reference) | no copy and no pin: the callee holds the only counted reference, claims it and rewrites in place; the caller never touches it again (liveness) |
| Keeps the value | Aether copies it (live), so the count is at least 2: the callee takes the shared path, copies the fields and drops its reference; the caller's value is unchanged |
| Value shared elsewhere | counted (after F3-1/F3-2), so the count is at least 2: shared path |
| Passes a view | the view is copied first (rule 3), so it is never moved out of its owner |
| Shares across threads | the count is negative, so `is_unique` is false: shared path, atomic counting |
| Worker mode | no reference counting at all |

The VM consumes the same Aether: `Drop` becomes `AetherDrop`,
`DropSpecialized` becomes an `IsUnique` branch, and `Reuse` becomes
`DropReuse` (`src/core/to_ir/fn_ctx.rs`). The new shapes only drop variables
that are not used afterwards, which Rust's `Rc` handles as it does today. No
VM change is expected.

## Steps
[steps]: #steps

Each step is its own commit, checked before the next one starts. Until F3-4,
every step can only add references, so at worst it leaks.

| Step | Change | Can it crash? |
|---|---|---|
| F3-0 | **Oracles.** Allocation and free counts on the debug heap (`FLUX_HEAP_STATS=1` prints them at exit; every block goes through the debug allocator, so the counts are exact). Fixtures: a moved capture called twice (V2), a multi-shot handler moving a list after `perform` (V4), values from an array and a map passed on (V6), elements shared by `push` and `concat` (V7), a shared parent (V1), linear FBIP. No V5 fixture: as written, V5 does not occur natively (see the corrections); F3-2c adds one for the cached closure. | no change |
| F3-1 | **The C runtime counts what it returns and shares** (V6, V7): copy the value in `array_get`/`array_at`, `hamt_get`, `wrap_some`'s callers, `unwrap`/`unwrap_or` and `ho_find`, and the elements `set`/`push`/`concat`/`slice` share. | leaks only |
| F3-2 | **Views are copied before an owned use** (V1–V5): rule 3 in `plan_var`; captures and handler values planned as lent; a copy for out-of-scope binders; continuations copy their captures on entry. | leaks only |
| F3-3 | **Arms claim their value:** the claim in `insert.rs`; the shared path keeps its drop; the LIR rewrites a claimed cell in place unconditionally (`claimed_tokens`); the verifier's single-release check counts the shared side's drop, and a new `ViewConsumed` check is added; fbip statistics are taken from the unique body. | leaks only (the pin still holds) |
| F3-4 | **The match-time pin goes:** `emit_match_ctor` no longer copies the fields an arm binds, now that the planner copies every field it uses (F3-2). A LIR safety net copies an owned argument that is a field, member or global read. (Removing `dup_owned_call_args` was done early, in 8e7000c9.) | this is the step the oracles exist for |
| F3-5 | **The remaining leaks:** free the cell on a unique path that does not reuse it (a new `flux_free_shell`, debug-heap aware); drop unused fields at the unique path's entry; `push`/`concat` may consume their source instead of copying its elements (optional since the primop pin is gone). | no |

### Checks at every step

- **Counts:** `FLUX_HEAP_STATS=1` makes the debug heap print
  `heap: allocs=N frees=N live=N` at exit. Until F3-4, a step leaves `allocs`
  the same, and `frees` may only go down.
- **Debug heap vs VM:** the debug heap, plain native and the VM print the same
  over:
  - `tests/native_llvm/debug_heap/*`;
  - `examples/guide` (59 programs);
  - `examples/aether`;
  - the F3-0 fixtures.
- **Verifier:** `FLUX_AETHER_VERIFY=1` throughout.
- **Step-specific checks:**
  - **Reuse is back (F3-4):** a linear `bump` over 5000 cells allocates
    about 5000 blocks, where today it allocates about twice that.
  - **The pin is gone (F3-4):** `lldb_counts_references` reads 2 where it
    reads 3, and a new stop inside an owned callee of a linear caller
    reads `1 reference (unique: can be reused in place)`.
  - **Nothing leaks (F3-5):** dropping a whole list frees every block it
    allocated.
- **`cargo xtask verify <tag>` per step:** no new parity mismatch.

### Expected churn

- All `tests/snapshots/aether/*__dump_aether.snap`, which include the stdlib.
- The `*__dump_core_debug.snap` files that show Aether shapes:
  - `drop_spec_branchy`, `drop_spec_recursive`;
  - `reuse_specialization`;
  - `tree_updates`, `queue_workload`;
  - `polymorphic_core_types`;
  - `verify_aether`.
- The `verify_aether` trace snapshots.
- Unit tests in `insert.rs`, `drop_spec.rs`, `reuse_spec.rs` and `verify.rs`.
- `tests/aether/core_regressions.rs` copy and drop counts.
- `lldb_counts_references`.

## Drawbacks
[drawbacks]: #drawbacks

- **A wide change.** It touches the C runtime, the Aether planner, drop
  specialization, the verifier and the LIR. The staging limits the risk, but
  each step needs a full verify.
- **More copies in shared paths.** A shared claimed arm copies its fields
  before dropping the cell, which costs more than today's pinned path in that
  case. Linear code gains far more than shared code loses.
- **Snapshot churn.** Every Aether dump changes at least twice (F3-2 and
  F3-3).

## Rationale and alternatives
[alternatives]: #alternatives

- **Stop after F2.** Native stays sound, but loses in-place reuse and leaks
  one reference per owned call. The leak grows with call count, and Aether's
  main promise, FBIP, holds on the VM only.
- **A per-function "consumes this parameter" summary** used at call sites to
  decide whether to copy. It still needs:
  - a uniqueness check before fields are consumed;
  - release that agrees across arms;
  - the summary carried in module interfaces.

  That is the owned convention under another name, plus interface plumbing.
- **Remove the pin first and fix crashes as they appear.** The debug heap
  would find them, but each one would be a use after free in a user's program
  until fixed. The staged order never produces one.

## Prior art
[prior-art]: #prior-art

- **Koka (Perceus):** owned parameters are moved; matching dups the used
  fields and drops the cell, and drop-reuse specialization turns a unique
  cell into its rebuild. This proposal follows that shape.
- **Lean 4:** borrowed parameters are inferred and never released; owned
  ones are consumed. That is the contract F2 introduced.

## Unresolved questions
[unresolved]: #unresolved

- **Whether the VM needs any change.** It shouldn't, since the new shapes only
  drop dead variables, but F3-3 checks VM output on every fixture.
- **Whether drop specialization should handle arms with a `perform` or a
  lambda,** which it declines today. That would reach more reuse, but isn't
  needed for soundness.
- **The exact name and placement of the `ViewConsumed` check.** It starts
  behind `FLUX_AETHER_VERIFY` and becomes fatal once the stdlib is clean.

## Future possibilities
[future]: #future

- Reuse across different constructors of the same size (Koka's reuse of a
  `Cons` cell for a `Some`), once claims are in place.
- Borrowed parameters passed to owned positions without a copy when the
  callee is known not to escape them.
