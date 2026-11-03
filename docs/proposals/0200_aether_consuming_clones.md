- Feature Name: `aether_consuming_clones`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0189](0189_aether_owned_moves.md) F3-5 (a unique cell frees its shell alone, KI-164)
- Relates to: [0193](0193_parameter_ownership_annotations.md) (`@own`, which makes a clone unneeded for that parameter), [0137](implemented/0137_modular_aether_and_module_interfaces.md) (owned-by-default, rejected), [0196](0196_checked_fip.md) (the unique path of `DropSpecialized`), [0130](0130_benchmark_framework.md) (benchmarks)

# Proposal 0200: A consuming copy of a function that takes its argument apart

## Summary
[summary]: #summary

A function that only takes a parameter apart is inferred to borrow it. A
caller that gives such a function the last reference to a value still has to
free that value afterwards, with a second walk over it. This proposal gives
such a function a private second copy, `name$own`, in which the parameter is
owned. A call site that hands over its last reference calls the copy, and the
caller's release goes away. The copy frees each cell as it reads it (0189
F3-5). Callers that keep the value call the original, unchanged.

Only Aether changes. Nothing in the language changes, and module interfaces
are unchanged.

## Motivation
[motivation]: #motivation

`benchmarks/flux/binarytrees.flx` builds a tree, walks it with `check_tree`,
and drops it:

```flux
fn check_tree(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> check_tree(left) + check_tree(right) + 1,
        Tip -> 0,
    }
}

fn sum_trees(iterations: Int, depth: Int, acc: Int) -> Int {
    if iterations <= 0 {
        acc
    } else {
        let tree = make_tree(depth)
        sum_trees(iterations - 1, depth, acc + check_tree(tree))
    }
}
```

`check_tree` only takes `tree` apart, so borrow inference makes it borrowed
(`src/aether/borrow_infer.rs`: a parameter is owned only if something
consumes it, and a match does not). The caller, `sum_trees`, then releases
the tree with `flux_drop`, which walks it again (`flux_drop_free_recx`,
`runtime/c/rc.c`).

On the native build at f35b7705 (2026-11-03, n=21, macOS arm64), the profile
is:

| Where | Share |
|---|---|
| `check_tree` | 37% |
| the drop walk (`flux_drop`) | 34% |
| `make_tree` | 26% |

The run takes 2,808 ms, against GHC's 1,997 ms. GHC's copying collector never
visits a dead tree, so its whole cost here is the walk and the allocation.

Making `check_tree`'s parameter owned removes the second walk, since each
call frees the cell it took apart. A throwaway build that forced it owned was
24% slower (2,576 → 3,197 ms), because a unique cell that builds nothing had
no way to be freed without its fields: each call copied both children and
released the cell (KI-164). 0189 F3-5 fixed that (07cdfc77). With it, the
same throwaway build takes **TBD ms** (P1-2 measurement, to fill in), which is
the most this proposal can gain on binarytrees.

Forcing the parameter owned everywhere is not the answer. A function that
only looks at a value the caller keeps, such as `rbtree`'s `lookup`, would
then copy its argument at every call and release it in the callee: the caller
keeps its reference, so the count is at least 2, and the callee takes the
shared path at every level, with a copy and a decrement per cell where the
borrowed walk does no reference counting at all. Owned-by-default was
rejected in 0137 for this reason. The choice has to be made per call site,
and the original function has to stay.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Nothing changes in how programs are written. `--dump-aether` shows the copy:

```text
── fn check_tree ──            (unchanged: borrows tree)
── fn check_tree$own ──        (owns tree)
lam(tree) ->
  case tree of
    Node(left, right) ->
      drop_specialized tree {
        unique -> free_shell tree in
                    check_tree$own(left) + check_tree$own(right) + 1
        shared -> dup left in dup right in drop tree in
                    check_tree$own(left) + check_tree$own(right) + 1
      }
    Tip -> drop tree in 0
── fn sum_trees ──
  ... aether_call[owned] check_tree$own(tree) ...   (no drop tree after it)
```

A backtrace shows `check_tree$own`. A breakpoint on a line in `check_tree`
stops in both copies; `b check_tree` by name stops only in the original.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Which parameters qualify

Parameter `i` of function `f` is a candidate when all of these hold:

- **E1.** `f` is a top-level named function (not `main`, not a dictionary,
  not anonymous), parameter `i` is inferred Borrowed, and its type is
  reference counted.
- **E2.** Every occurrence of the parameter is the scrutinee of a `Case`. It
  is never in a guard, a primop, a call (borrowed or not) or a lambda.
- **E3.** Some arm with a flat constructor pattern (each field a variable or
  a wildcard, which is what drop specialization accepts) passes a field, used
  once in that arm, as argument `j` of a call to `g`, where `(g, j)` is itself
  a candidate.
- **E4.** `f` performs no effect: no `Perform`, `Handle`, `Lam`, `LetRec` or
  `LetRecGroup`, and it calls only primops or same-module functions that also
  pass E4. A clone then never meets `cont_split` or a continuation.
- **E5.** `f`'s Core size is at most a limit, 200 to start with, set from
  measurements.

The candidates are the greatest fixpoint over the module: start from every
parameter that passes E1, E2, E4 and E5, and remove what fails E3 until
nothing changes. Mutually recursive walkers qualify together.

Each function gets at most one clone, in which every candidate parameter is
Owned. A call uses the clone only if every candidate position hands over its
last reference. One clone per parameter (2^k copies) is left open.

### How a clone is represented

- It is an `AetherDef` only, not a `CoreDef`, made inside
  `lower_core_to_aether_program` after borrow inference. Module interfaces
  are built from Core defs, so they never see it.
- It gets a fresh binder from `next_fresh_binder_id` and the original's body.
- Its signature is not inferred: it is the original's, with the candidate
  parameters Owned. It is registered by binder only, never by name.
- `AetherDef` gains `owned_clone_of: Option<CoreBinderId>`.
- Clones are made on native builds only. The VM's values are `Rc`s, so a
  clone gains nothing there; parity is checked on output.

**The name has to be interned.** `Interner::resolve` panics on a symbol it
did not create, and LIR lowering resolves every def's name to find `main`.
Reusing the original's name instead gives one name two binders in
`upsert_user_signature`'s by-name map and in `prefer_same_module_binder`
(`src/lir/lower.rs`), which can swap the clone's binder back to the
original's. So the compiler interns `"{name}$own"` for each function that
passes E1 and E2 before Aether runs, and passes the map in. In LIR, the
clone's qualified name is the original's plus `$own` (`Flow.List.length$own`),
which stays unique per module. `$` is valid in LLVM and Mach-O symbol names.

### How a call site picks the clone

In the planner's `App` case (`src/aether/insert.rs`), a call to `f` becomes a
call to `f$own` when every candidate argument `x` meets all of these:

- `x` is a resolved variable whose type is reference counted, and it is not
  live after the call;
- it occurs once among the arguments, and is not the function called;
- it is not a borrowed parameter, not lent, not under a borrowed parent, and
  no view of it is live after the call;
- and either the caller releases `x` after its last use (an owned parameter,
  or a `let` that made a fresh value; the planner records these in a new
  `scope.owners` set), or `x` is a field of a flat pattern whose matched value
  is such an owner and is dead after the call.

The second case is what makes a clone's body call clones: in `check_tree$own`,
`left` is a field of the owned `tree`. It also helps owned functions like
KI-164's `nodes`.

When the rule applies, the callee becomes the clone's binder and the
arguments take its modes. `plan_var` then moves `x`, and the caller's release
of it disappears, since the call already gives it away. A tail call stays a
jump.

### Planning order

1. Plan every candidate clone.
2. Remove each clone in which no arm on a consumed parameter became a
   `DropSpecialized` whose unique path frees or reuses the cell; a clone that
   cannot free as it reads only adds code. Re-plan, and repeat until the set
   stops shrinking.
3. Plan the original functions, with call sites choosing clones.
4. Keep only the clones some original function reaches.

Bisect names: `owned_clone` (making a clone) and `owned_clone_call` (calling
one).

### When the clone gets a shared value

A caller hands over its last reference, but the value can still be shared:
kept in a list, captured, or a static object. The clone then takes the shared
path at the root: it copies the fields, decrements the cell, and recurses with
counts of at least 2, so the whole shared part is walked with writes. That is
a uniqueness check, two copies and a decrement per cell, where the borrowed
walk does no counting at all: roughly 2–3× the borrowed walk, on the shared
part only. The total number of cells freed never changes.

If measurements show that matters, a fallback: when the clone's body is the
case at its root and its other parameters are borrowed or unboxed, its shared
path calls the original and then drops the cell (`let r = check_tree(tree) in
drop tree in r`). A shared value then costs the borrowed walk plus one
decrement. This needs `invalid_drop_specialized_uses_aether`
(`src/aether/verify.rs`) to allow borrowed reads on the shared side before the
drop.

### Debugging

- The DWARF name is `check_tree$own`, and backtraces show it.
- A breakpoint on a line resolves to both copies.
- `check_fbip` and the fbip statistics skip clones.
- `--metrics` counts `owned_clones`.

## Steps
[steps]: #steps

| Step | Change | Can it crash? |
|---|---|---|
| C0 | **Fixtures and baselines** in `examples/aether`, each with expected output: a binarytrees-shaped `sum_trees`; `length` of a dead list; a dead tree also kept in a list (count 2); even/odd walkers; a two-parameter `same(a, b)`; a kept-tree `rbtree` lookup; an effectful walk (excluded); `nodes` handing a field to `check_tree`. Record debug-heap counts and timings. | no change |
| C1 | **Candidates.** `src/aether/owned_clone.rs`, `candidates(&CoreProgram, &BorrowRegistry)` with E1–E5 and the fixpoint; the compiler interns the `$own` names. Nothing calls it yet. | no |
| C2 | **Clones and call sites:** making and planning the clones, the call-site rule, LIR names, the dump, off on the VM. | yes: a wrong last-reference decision is a use after free; the verifier and the debug heap catch it |
| C3 | **The shared fallback** and its verifier change, only if the C0 shared fixture is more than about 5% slower than the borrowed walk. | yes: a read after the drop; `check_reads_after_release` catches it |
| C4 | **Debugger:** `$own` in backtraces, a line breakpoint stopping in both copies, `flux refs` reading 1 on the unique path. | no |
| C5 | **Evidence:** commits, timings, and this proposal's status. | no |

### Checks at every step

- `FLUX_AETHER_VERIFY=1` throughout.
- Debug heap against the VM over `examples/aether`, `examples/guide` and the
  C0 fixtures; `allocs`, `frees` and `live` unchanged at every step.
- `cargo xtask verify <tag>`, `cargo xtask sanitize` and
  `cargo xtask sanitize --thread`.
- Timings at C2 and C3, five interleaved runs against the step before and
  GHC: binarytrees, cfold, deriv, rbtree, rbtree_ck, rbtree_del, nqueens,
  qsort. `rbtree` must not get slower.

## Drawbacks
[drawbacks]: #drawbacks

- **Code size and compile time.** Each clone is a second copy of a function.
  E5, pruning unreached clones, and the `--metrics` count bound it.
- **Shared input costs more** in the clone than in the borrowed original.
- **Two symbols for one function** in a debugger.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Infer the parameter owned when a caller hands over its last reference.**
  A parameter has one mode for all callers, so every caller that keeps the
  value would pay.
- **Owned by default** (0137): rejected there, for the same reason.
- **`@own`** (0193): lets a person choose the owned mode for every caller of a
  function. A clone chooses per call site, without an annotation.
- **A faster drop walk.** It still visits every dead cell; the clone removes
  the second visit.

## Prior art
[prior-art]: #prior-art

- **Koka** (Perceus): owned parameters by default, with drop specialization
  and reuse of a unique matched cell, which frees a cell as it reads it.
- **Lean 4**: borrow inference marks a parameter owned when the body consumes
  it; it does not copy functions per call site.
- **GHC**: a copying collector never visits dead cells, so it has no drop walk
  to remove.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- One clone per parameter instead of one per function.
- Exporting clones across modules (cross-module calls use the borrowed
  original for now).
- Relaxing E2 to allow reads before the claim, such as `if is_leaf(t) ...`.
- Allowing effects (E4), which needs clones to meet `cont_split`.

## Future possibilities
[future-possibilities]: #future-possibilities

- Feeding call-site ownership facts back into borrow inference, so a function
  whose callers all hand over their last reference becomes owned outright.
- Using the same machinery for `@own` (0193) as an explicit request.
