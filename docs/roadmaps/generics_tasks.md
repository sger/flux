# Generics (0.0.7), type classes (0.0.8)

Working list for [0185](../proposals/0185_generalize_by_arity.md),
[0186](../proposals/0186_generics_foundations.md) and
[0187](../proposals/0187_specialisation.md).

## The split

**0.0.7 is generics. 0.0.8 is type classes.** Generics is *which definitions get
quantified, over what, and in what order* — binding groups, the monomorphism
restriction, specialisation. Type classes is *what a call site is handed* —
evidence, dictionaries, instance selection.

### Generalize-by-arity splits along the constraint boundary

**The unconstrained half is 0.0.7; the constrained half is 0.0.8.** That is the
same line the release split already draws — generics is which definitions get
quantified, type classes is what a call site is handed — and it turns out to cut
generalize-by-arity cleanly in two.

`fn identity(x) { x }` raises no class constraint. There is no dictionary to
plumb and no specialised arithmetic to lose, so **neither** reason to withhold
generalization reaches it. `fn double(x) { x + x }` raises `Num`, and both
reasons apply. Generalizing by arity *and an empty constraint set*
(`finalize_and_bind_function_scheme`) therefore lands in 0.0.7 on its own:

```flux
fn identity(x) { x }
fn main() with IO { print(identity(1)) print(identity("hi")) }   // works
```

Measured 2026-09-09: 445 tests across twelve suites, no change. The rule cannot
despecialise anything, because a definition with no constraints had no
dictionary call to specialise away.

What stays in 0.0.8 is generalizing a *constrained* definition — the case whose
cost was measured on 2026-09-08 by reapplying the unrestricted two-line change
and reading the failures:

| what fails | count | fixed by |
|---|---|---|
| `E004: can't find a value named __dict_m8_..._Num_Int` / `_Ord_Int` | 5 | Track C |
| `E1000: wrong number of arguments: want=2, got=1` | 2 | Track C |
| `expected function constant` | 1 | Track C |
| `typed pattern binders (IntRep) should eliminate DropSpecialized` | 4 | B1 |

**Only 4 of 13 are the lost-optimisation problem [0187](../proposals/0187_specialisation.md)
is written to solve.** The other 9 are dictionary plumbing, and the missing
symbol is `__dict_m8_466C6F772E4E756D_Num_Int` — character for character the one
in 0186's opening motivation as the unfiled forwarding bug that reproduces on
shipped `main`.

So 0187's premise — that specialisation is what unblocks generalize-by-arity —
is only a quarter true. **B1 → B2 becomes C → B1 → B2**, and all three are
0.0.8. 0187 needs amending to say so (F8).

Every one of those 13 failures is a *constrained* definition. None of them is
reachable by the unconstrained rule above, which is why that half can ship now
rather than waiting behind the whole of Track C.

The alternative was patching forwarding where it surfaces, without C6's
deletion. That is what was done for KI-052, KI-061, KI-082, KI-083 and one
unfiled case — five local fixes to one bug — and it is the pattern 0186 exists
to stop.

### 0.0.7 exit criteria

1. **No known miscompilation** — Track R. **Done**: R1–R7 are all closed.
2. **The gate can detect a generics regression** — R5 and KI-062. **Done**: a
   fixture declaring `expect: success` must now actually run.
3. **An unannotated definition with no class constraints is generic** — the
   unconstrained half of generalize-by-arity. **Done** (G5).
4. **The remaining generics-side bugs and the release mechanics** — Tracks G, E
   and F below. This is what is left.

### 0.0.8 exit criteria

1. **One evidence-passing translation** — Track C. The six resolution sites are
   deleted, not merely agreeing.
2. **`fn double(x) { x + x }` works at two types** — B1 then B2, on top of C.
   The *unconstrained* case (`fn identity(x) { x }`) shipped in 0.0.7; what
   remains here is the constrained one, which is the half that needs both the
   evidence translation and specialisation.
3. **No open High-severity type-class bug** — Track D: KI-071, KI-073, KI-076,
   KI-086, KI-090. (KI-091, KI-092 and KI-093 are name resolution, effect rows
   and map member access — they stay in 0.0.7's Track G.)

---

# 0.0.7 — generics

Tracks R and the KI-062 half of G are done. What remains is E1, two bugs, and
the release mechanics.

---

## Track R — regressions this branch introduced

All in stage 2's [`plan_block`](../../src/binding_groups.rs). R1 is confirmed by
running the program; R2, R3 and R5–R6 are reasoned from the code and not yet
reproduced.

- [x] **R1. Binding groups are emitted in source-anchor order, not dependency
      order.** *Confirmed, cold cache:*

      ```flux
      fn main() with IO {
          fn a() -> Int { b() + 1 }
          fn b() -> Int { 41 }
          print(a())
      }
      ```
      ```
      error[E1001]: Not A Function — Cannot call non-function value (got Uninit).
      ```

      `main` prints 42. A nested helper calling a *later* sibling is ordinary
      code — far more common than the KI-087 shape (mutual recursion split by a
      `let`) that stage 2 was written to fix. **KI-087's entry is marked FIXED
      and needs amending**: the fix introduced a worse bug than it closed.

      `strongly_connected_components` already returns dependencies first, which
      is the order `prepend_stmts` needs — it folds `plan.iter().rev()`, so the
      *first* plan item becomes the outermost binding. `plan_block` discards
      that order and anchors each group at its first member, so `a` becomes the
      outer `LetRec` and captures `b`'s uninitialised slot.

      *Investigated, and it narrows the fix:* of the three consumers, only
      lowering honours plan order. Inference (`type_infer/statement.rs`, three
      loops) and the AST-bytecode path (`compiler/statement.rs`) walk the
      statement slice **by index** via `group_index`, so they ignore plan order
      entirely and are unaffected. The main VM path reaches the VM through
      `core/` lowering, so fixing the plan order fixes both backends.

      The fix is a topological sort over plan items — not a plain sort, since
      `Other` statements keep source order and a group may not hoist above a
      `let` it reads. Four edge kinds: consecutive `Other`s; `Other → Group`
      where a member reads what it binds; `Group → Other` where the statement
      uses a member (R3); `Group → Group` for dependencies. Tie-break by source
      index so ordinary programs lay out exactly as they do today. A cycle
      (`let x = f(); fn f() { x }`) is a real error — fall back to source order
      deterministically.
- [x] **R2. `bound_name` sees only `Let` and `Function`.** A `LetDestructure`
      between two members binds names invisibly to the hoist check. It should
      return *every* binder, walking the pattern —
      `FreeVarCollector::define_pattern_bindings` already does this walk and can
      be reused.
- [x] **R3. The hoist check is one-sided.** Nothing asks whether an intervening
      statement *uses* a member, so a group can be emitted after code that calls
      it. Needs free variables of non-function statements:
      `collect_free_vars_in_statement` does not exist and must be added beside
      `collect_free_vars_in_function_body` — `FreeVarCollector` already has the
      `visit_stmt` arms.
- [x] **R4. Drop `plan_block`'s `free_vars` callback.** All three call sites
      pass a byte-identical closure delegating to
      `crate::ast::free_vars::collect_free_vars_in_function_body`. The callback
      was justified as keeping the module off lowering internals, but it is
      `crate::ast`, which `binding_groups` already depends on. Removing it
      deletes three copies and is a prerequisite for R3 supplying statement
      free vars in one place.
- [x] **R5. Close the test gap that let R1 through.** Ordered — the first is a
      prerequisite for the second.
      - **Fix [KI-062](../known_issues.md#ki-062) first.** *A parity fixture
        would not have caught R1.* The harness compares the two backends'
        outputs; when both fail identically, the outputs match and
        `expect: success` is never checked against the compile result. R1 fails
        the same way on both. Until `expect:` is enforced, a parity fixture for
        this proves nothing;
      - then a `tests/parity/` fixture for the plain nested forward reference;
      - a lowering test asserting `LetRec` **nesting**. The surviving ones only
        count nodes, which is why they stayed green;
      - restore the ordering coverage lost when
        `tarjan_scc_reverse_topological_order` was deleted along with
        `lower_fn_run_with_scc`. It was the only thing guarding this order and
        went out with the function it tested.

### Unverified — same review, verification pass never reported

- [x] **R6. `close_definition_scope` re-derives `quantified`** — *confirmed, and
      fixed.* It reached the same base set as `decide_quantification` but skipped
      the field-predicate pinning, so the implication would claim exactly the
      receiver variables stage 4 withheld. `Quantified::forall` is documented as
      "every other answer here is derived from this set", so the re-derivation
      contradicted its own contract.

      **No behavioural change.** Both derivations were instrumented and compared
      across the inference suites, `examples/guide` and `tests/parity` — ~190
      programs including the whole stdlib — and never diverged. Landed because a
      second derivation is what 0186 exists to remove, not because a program
      miscompiled. Stage 4's "one quantification decision" is now true.
- [x] **R7. `harvest_evidence` indexed only `Disposition::Solved` predicates** —
      *confirmed, and fixed.* `EvidenceSite.index` is the argument position, so
      `[Stuck, Solved]` put the solved evidence in the stuck predicate's slot.

      Worse, it defeated a guard: `EvidenceMap::args_for` returns `None` on a
      missing index precisely so a caller cannot build a partial argument list
      and emit "a call with the wrong arity that type-checks". Compacting made
      the list dense, so `args_for` returned `Some(short_list)` and the guard
      never fired. The counter now advances for every predicate and only solved
      ones are inserted, so a hole stays a hole. Three tests; two fail on the
      old code. **C5 would have been built on this.**

---

## Track G — the instrument, and what it found

- [~] **D8. [KI-088](../known_issues.md#ki-088)** — *typing half fixed; codegen half open.* — a nested `fn` shadowing a
      top-level name. Inference half fixed (`4654bad1`); a second lookup in the
      compiler's own resolution still reaches past the nested definition. The
      reproduction is rejected by a *compiler boundary* check, so a case in
      `tests/type_inference/` passes whether or not the bug is present —
      pinning it needs an end-to-end test.
- [x] **D6. [KI-062](../known_issues.md#ki-062)** — the parity harness accepted a
      fixture that fails to compile on both backends. **Fixed**: `expect: success`
      now requires every way to exit `Success`, and a support module with no
      entry point is no longer swept as a fixture.

      It found **7 of 133 fixtures had never run**. Four were stale and are
      repaired; three were reproducing real bugs while reporting as passing, and
      are filed below and marked `skip:`.

      Still open in that entry: the dead `++` code — `infer_semigroup_operator`,
      the `"++" => "append"` desugar arms, `CorePrimOp::Concat`. Removing a
      `CorePrimOp` variant changes lowering and owes an epoch bump, so it is its
      own branch.
- [x] **G1. [KI-091](../known_issues.md#ki-091)** — *High, fixed.* A user-defined
      top-level function is shadowed by a prelude function of the same name:
      `fn sum(a, b) { a + b }` then `sum(3, 4)` is `E300 expected List<Int>`.
      Same for `product`, `min`, `max`, `reverse`, `length`. Same family as
      KI-088 — a definition losing to something further away.

      Fixed: the unit's own function names are collected once and never treated
      as imported. `tests/parity/user_fn_name_no_collision.flx` is unskipped.
- [x] **G2. [KI-093](../known_issues.md#ki-093)** — *High, fixed.* A local
      binding named like an imported module is shadowed by the module:
      `let Math = { .. }` then `Math.square(5)` is `E012`, while the same code
      with the binding named `Widget` prints `25`. Same family as G1 — a
      definition losing to an import — via the module qualifier rather than a
      function contract. Fixed by checking `SymbolTable::is_bound` first.
      `tests/parity/import_member_access.flx` is unskipped.

      **The issue was originally filed with the wrong diagnosis** ("map member
      access silently yields `None`"). That was a mistake in the reproduction:
      a trailing `;` made the lambda return unit, which Flux spells `None`.
      Map member access on a function value has always worked.

- [x] **G3. [KI-092](../known_issues.md#ki-092)** — *Medium, fixed.* A type
      parameter used only in an effect row was rejected as phantom, so
      `alias Handler<a, e> = (a) -> a with <Async | e>` could not be written.
      `collect_type_expr_named_symbols` matched
      `TypeExpr::Function { params, ret, .. }` and the `..` discarded `effects`.
- [x] **G4. [KI-094](../known_issues.md#ki-094) — DONE 2026-09-09.** *Found
      behind G3, whose `E308` had been masking three further defects.*

      **Alias resolution:** *no* transparent type alias resolved —
      `alias IntPair = (Int, Int)` then `-> IntPair` was `E423`, because
      `is_known_annotation_type` did not consult `transparent_type_aliases` and
      that check runs before the Phase 1d expansion. Fixture
      `type_alias_transparent_basic.flx`.

      **The effect row was never about aliases.** An alias-free program has the
      same failure: `fn consume(s: (Int) -> Int with Async)` called with a
      matching `with Async` function is `E422 missing required effects: Async`.
      `Async` is an effect *alias*, and two sites left it undecomposed while
      every other row had been rewritten — `collect_contracts_from_statement`
      stored parameter and return annotations verbatim, and the pipeline ran
      effect-row expansion *before* transparent type alias expansion, so a row
      living in an alias body did not yet exist when its turn came. The phases
      now run in the other order. The `$1` in the original runtime `E1004` was
      that undecomposed row reaching contract lowering.

      **The design question was a fixture bug.** An alias's effect-row parameter
      is declared like any other type parameter, in the function's own `<...>`
      list: `fn apply_async<e>(f: AsyncFn<Int, Int, e>, ...)`. That works
      *because of G3* — a type parameter used only in an effect row is no longer
      rejected as phantom. Nothing had exercised it.

      `type_alias_transparent.flx` is unskipped, and because a parity fixture
      that fails on both backends reads as passing (KI-062 — which is how this
      stayed broken), `tests/integration/type_alias_effect_row_tests.rs` runs it
      and asserts the output.

- [x] **G5. Generalize an unannotated definition that raises no class
      constraints — DONE 2026-09-09.** The unconstrained half of
      generalize-by-arity, and the reason it is 0.0.7 work rather than 0.0.8 is
      in *The split* above: the two things that block the constrained case —
      dictionary plumbing and lost specialisation — have nothing to act on when
      there are no constraints.

      In `finalize_and_bind_function_scheme`: generalize when the definition
      declared type parameters **or** it takes parameters, raised no constraint
      of its own, and shares no type variable with a still-undischarged field or
      tuple predicate.

      **The second condition was not in the first version, and the suites did
      not catch its absence.** Raising no constraint is not enough: a definition
      that merely *forwards* a value to a constrained one has none of its own,
      and generalizing it quantifies the variable the callee's pinned receiver
      is waiting on. `jump_step(axis, ..)` passes `axis` to `jump_step_up`,
      which projects `axis.1` — four `examples/aoc/2024/day06*` programs broke
      with `E491` while all 445 tests stayed green. It is the disconnect of
      [KI-095](../known_issues.md#ki-095) and [KI-096](../known_issues.md#ki-096)
      by a third route: not a match arm, not a recursive sibling, but a plain
      forwarding call. Pinning covers the definition that owns a predicate; this
      covers everyone the receiver passes through on the way there.

      *Measured, differentially:* every `.flx` under `examples/`, `tests/` and
      `lib/` compiled with `--no-cache` before and after, comparing per-file
      error codes — **221 failures before, 221 after, the two lists identical**.
      An absolute count says nothing here, since the corpus contains hundreds of
      intentional-error fixtures; only the diff does. Plus 426 tests across ten
      suites. `fn identity(x) { x }` types at `Int` and `String`; annotated
      generics, constrained generics and two-level dictionary forwarding are
      unchanged.

      *What it deliberately does not cover:* `fn double(x) { x + x }` (raises
      `Num`) and `fn fst(p) { p.0 }` (raises a field predicate, which is also a
      constraint and is *pinned* — a receiver nothing determines must not be
      quantified). Both wait on B2.

---

## Track E — 0185's remaining stages

[0185](../proposals/0185_generalize_by_arity.md) stages 0–2 shipped; stage 3
became 0186 stage 6 and is now Track B. Four stages remain, and 0186 changed
what two of them cost.

- [x] **E1. Stage 5 — tuple projection as a constraint — DONE 2026-09-09.**
      `infer_tuple_field_access_expression` (`expression/access.rs:206`) still
      types an unresolved receiver with a hole, delaying the failure until a
      call site pins it. Convert it on 0184's template — a solver-internal
      predicate in the reserved module, discharged after inference, reported if
      the receiver is never determined.

      0186 makes this cheaper than when it was written: the field-access
      predicate it copies now also **pins** its receiver in
      `decide_quantification`, so the tuple predicate gets that behaviour for
      free rather than having to rediscover it. The proposal's note that this
      retires `generalize_constrained_vars` is stale — that function no longer
      exists.

      Landed on the third attempt. The first two were blocked by pre-existing
      holes in the machinery it copies rather than by anything in the
      conversion, and both are now fixed:

      1. [KI-095](../known_issues.md#ki-095) — a receiver bound by a match arm
         was never determined. **Fixed**; that took the stdlib failures from six
         to one.
      2. [KI-096](../known_issues.md#ki-096) — a recursive group's predeclared
         monotype is never unified with what the member infers, so a sibling's
         projection has no receiver. **Fixed**, in two parts: the missing
         unification, and reading the environment through the substitution
         before a `let` generalizes.

      Both reproduce for *record fields* on shipped `main`, with no part of E1
      applied. The pattern is worth stating: field predicates shipped in 0184
      with two shapes that cannot be determined, and no stdlib or test code hit
      either. Tuples hit both immediately, because tuple projection is common
      where named-field access is not. E1 is the instrument that found them.
      The conversion itself is small and works — predicate, pinning, discharge,
      `E491` for a receiver never determined, `E492` for an index past the end
      of the tuple. What it inherits is a hole in what it copies: a predicate
      whose receiver is bound by a **match arm** is never determined by its call
      site, on shipped `main`, for record fields too. No stdlib code accesses a
      named field that way, so 0184 shipped over it — but `Flow.Array`'s
      `update_many_go` and `accum_go` match a list of pairs and project `p.0`,
      so the tuple version breaks the standard library in six places on its
      first run — one after KI-095, and that one was KI-096.

      **Three more fixes were needed after those two**, each found by measuring
      rather than by reading:

      - *The effect row.* KI-096's unification compared whole function types.
        The placeholder's row is the one the siblings' calls accumulated; the
        inferred `fn_ty`'s row is the *declared* one, empty and closed for an
        unannotated function. Unifying the two fails on the row and leaves the
        result type — the thing being connected — unbound. At the top level the
        rows agreed, so a minimal repro passed while the real program failed.
        Parameters and result only, never the row.
      - *The pin.* `decide_quantification` pinned the receiver whether or not it
        was still unknown. Once it has resolved to a structure, pinning strips
        *that structure's* variables — which is how `Flow.Array.sort_by<a, b:
        Ord>` came to report its own declared `Ord<b>` as an ambiguity. It now
        applies only while the receiver is an unresolved variable.
      - *The cascade.* `mystery.0` on an undefined name reported `E004` and then
        a redundant `E491`. The field predicate already refuses an unbound
        receiver; the tuple path now uses the same guard.

      *Measured:* 1313 files under `examples/`, `tests/` and `lib/` compiled with
      `--no-cache`, **0** with `E491`/`E492`.
      `examples/aoc/2025/aoc_day11_haskell_style.flx` runs. `CACHE_EPOCH` 50.

      The work is on `wip/e1-tuple-projection-predicate` and is otherwise
      complete: predicate, pinning, discharge, `E491` for a receiver never
      determined, `E492` for an index past the end of the tuple. One refinement
      came out of the second attempt and is worth keeping whichever way E1
      goes — the pin in `decide_quantification` now applies only while the
      receiver is still an unresolved variable. Pinning a receiver that has
      already resolved to a structure strips *that structure's* variables, which
      is how `Flow.Array.sort_by<a, b: Ord>` came to report its own declared
      `Ord<b>` as an ambiguity.
- [ ] **E2. Stage 4 — report inferred ambiguity.** 0183's R6b. `Disposition`
      loses `Stuck`; a predicate reaching whole-program scope over an
      unresolved variable is reported with its origin. **Blocked on B2**: the
      stage's premise is that with generalize-by-arity landed, the residue is
      ambiguity rather than stranded obligations. Until then the residue is
      still the old kind and the report would be wrong. **B2 moved to 0.0.8, so
      this moves with it** — it is listed here only because it belongs to
      0185.
- [x] **E3. Stage 6 — size the instance-resolution unification.** Answered by
      0186 stage 5; close the stage rather than run the spike. Its three
      questions:
      - *What identifies a call site in Core such that the solver's `Evidence`
        can be found?* `ExprId` plus the predicate's index at that site
        (`EvidenceSite`). A span cannot serve — it cannot tell apart two
        predicates raised at one site for one class. `ExprId` already reached
        both re-resolvers before this work.
      - *Which derivation becomes canonical?* `class_solver.rs` —
        `solve_instance_evidence`, `entailed_by_givens`,
        `structural_builtin_evidence`.
      - *Can the AST bytecode fallback be retired?* Not answered; still open,
        and still worth answering before C6.
- [ ] **E4. Stage 7 — close 0183.** *0.0.8, with E2.* Documentation only. Mark R1–R5 shipped,
      record R6 as delivered by E2 + B2, close its open questions as decided,
      and move it to `docs/proposals/implemented/`. Do this **after** E2, or the
      record is written before the thing it records.

---

---

## Track F — release mechanics

Runs alongside the others; none of it is optional for a release.

- [x] **F1. `CACHE_EPOCH` — at 53.** Bumped per landing rather than once at the
      end, and every bump after 47 is for the same reason: the change alters
      inferred types, and so what a cached interface records. 47 for R1's
      dependency-order emission, 48 for KI-095's match propagation, 49 for
      KI-096's recursive-group predeclaration, 50 for 0185 stage 5's projection
      predicate, 51 for G5, 52 for KI-094's alias decomposition, **53** for the
      projection predicate no longer being retained on a scheme — an epoch-52
      `.flxi` records `__tuple` in `Flow.Array.update_many`'s context, and a
      caller reading it is an `E490` this compiler accepts.

      D9's fix (KI-079) deliberately took no bump — adding the compiler build to
      the key changes every hash, so stale entries stop being *found* rather
      than being read and mistaken for current; the reasoning is in the KI. That
      is specific to the landing whose own mechanism invalidated everything: 48
      through 53 all followed it and all bumped. B2 will need its own bump in
      0.0.8.
- [x] **F2. Amend KI-087.** Done 2026-09-09. The regression was already
      recorded in the entry; what it lacked was a *verified when* note, now a
      table naming what pins each shape — the parity fixture for the
      mutual-recursion case, three `test_forward_reference_*` cases that run and
      assert output for the forward-reference one, and the ordering unit tests
      for emission order — with the warning that the parity fixture alone is
      insufficient evidence, since it was green throughout the six sweeps the
      regression survived. Also corrected a stale
      `flux_generics::strongly_connected_components` path left by the crate
      fold-back.
- [ ] **F3. Mark the fixed issues.** *0.0.8.* KI-052, KI-061, KI-082, KI-083 are
      already marked FIXED individually; 0186 claims to have removed the *cause*
      they share. That claim is only true once C6 lands, so it is a 0.0.8 note —
      do not write it in the 0.0.7 release.
- [x] **F4. Move the proposals — none of them move at 0.0.7.** Checked
      2026-09-09; the premise was wrong. **0185** has four stages still open,
      not zero: stage 3 became Track B, stage 4 is blocked on B2, stage 5 is
      blocked on [KI-095](../known_issues.md#ki-095) and stage 7 is documentation
      that must follow stage 4. **0187** has not started — it is 0.0.8 in its
      entirety. **0186** cannot move for the reason already recorded: its stage 5
      is Track C. So all three stay in `docs/proposals/`, and the move is a
      0.0.8 task for whichever of them the type-class work finishes.
- [x] **F5. Update `roadmap_to_1_0_0.md`.** Done 2026-09-09. Both the release
      table rows and both prose sections rewritten: 0.0.7 is generics
      foundations, 0.0.8 is type classes. The displaced work is rehomed rather
      than dropped — the architecture theme (`0044`, `0085`, `0086`) and the
      tests/linter/identity theme (`0035`, `0010`, `0025`, `0043`) both move to
      0.0.9, where the architecture work sits next to the Aether work it was
      always meant to precede. Each entry says what it displaced.
- [x] **F6. `CLAUDE.md` — decided: stays untracked.** It was tracked briefly on
      this branch and that commit was removed on 2026-09-09 at the maintainer's
      instruction, so the decision the item asked for is made. Its content was
      corrected for the current tree while it was tracked (the crate fold-back,
      the two false claims about the docs guard and a `deny` attribute that does
      not exist), and that corrected copy is what remains on disk. Nothing about
      the compiler depends on it, so an untracked file is a defensible answer —
      but note the corollary: it is not reviewed, and it will drift again with
      no gate to catch it.
- [x] **F9. `examples/generics/` — the capability map.** Done 2026-09-11. 41
      programs organised by what the compiler does with them: `working/accepts`
      (compiles, runs, right answer), `working/rejects` (correctly rejected —
      the diagnostic is the feature), `failing/compile` and `failing/runtime`.
      Every file is pinned — the compile-only `examples_generics` snapshot, plus
      a second VM-only run snapshot for the runtime bucket, because a gap that
      compiles cleanly records as `ok` and pins nothing. Behaviour is asserted
      separately in `tests/flux/generics.flx` (19 cases, VM and native), which
      deliberately mirrors nothing from `failing/` so the native leg is safe by
      construction rather than by an exclusion list.

      **Every error code in it was measured, not copied from this file**, and
      three things came back different from what was written down:

      1. **KI-011, KI-069 and KI-070 no longer reproduce** against their own
         filed repros. Each now has a file in `working/accepts/` so a
         regression is a snapshot diff. They are candidates for F3's marking
         pass. KI-070's repro is additionally unrunnable as filed —
         `\y: a -> y` is a parse error, since lambda parameter annotations
         need parentheses — which may be why it read as unfixed.
      2. **A `let` bound to a lambda is generalized**, nested and top-level.
         Assuming the monomorphism restriction covered it was wrong; it applies
         to nullary bindings.
      3. The guide's "Only `fn f<T>(x: T)` syntax triggers let-polymorphism"
         (09_type_system_basics.md:181) was false after G5 and is corrected.

      **A specialisation gate ships with it.** KI-098 is invisible to every
      behavioural harness — a despecialised program compiles, runs and returns
      the right answer, so parity and every snapshot pass it. `failing/degraded/`
      holds a contrast pair, two programs differing only in one extra call site,
      and `snapshot_ki_098_*_core_def` in `tests/aether/cli_snapshots.rs` pins
      one extracted Core definition from each:

      ```
      baseline (one call site):   ::(h#N:Int, t#N:Box) →
      two call sites:             ::(h#N,     t#N:Box) →
      ```

      The extraction matters: a full `--dump-core=debug` is ~2900 lines of
      mostly stdlib, which would bury the signal and repaint on every unrelated
      `lib/Flow` change. Binder ids and temporaries are normalized to `N` for
      the same reason. **This is the baseline B2 will be measured against, and
      it cannot be reconstructed after B2 lands** — which is why it is pinned
      now rather than when that work starts.

      Six gaps are mapped in the README but have no file yet: KI-071, KI-073,
      KI-076, KI-086, KI-088 and E2. The first two are VM/native divergences
      that no VM-only harness can show.
- [x] **F8. Amend [0187](../proposals/0187_specialisation.md).** Done
      2026-09-09. The 16 became 4, with the other 9 named as dictionary
      plumbing and the conclusion drawn — specialisation is necessary but not
      sufficient. Track C is now stage 0 of its table, and "where specialisation
      runs" is answered in favour of after `dict_elaborate`, with the Stage 0.6
      placement and the narrow first scope recorded there rather than only here.
- [ ] **F7. Rewrite the PR description — the 2026-09-09 text is stale.**
      `CHANGELOG.md` is assembled from merged PRs at release time, so the PR
      description *is* the changelog entry, and it is deliberately not a tracked
      file — the PR body is where the release assembles it from. What was
      written on 2026-09-09 (G5 and 0185 stage 5 as the features, the eight
      fixes, the epoch 47 → 52 run, and a section on what the validation does
      *not* establish — KI-062, and that a sweep sees outcomes rather than
      precision) still holds as far as it goes, but it predates four things and
      closes on a claim that is now false.

      **Missing from it.** G4 ([KI-094](../known_issues.md#ki-094)) — no
      transparent type alias resolved at all, and an effect alias left
      undecomposed at two sites. B1 (`4f45fb70`) — specialisation at a single
      concrete instantiation, and with it the finding that rewrote the entry:
      G5 introduced a *second* kind of despecialisation this list had not
      anticipated, one with no dictionary to clone on. The Aether baseline
      update (`cd91f954`), which is the evidence for both.
      [KI-097](../known_issues.md#ki-097) and
      [KI-098](../known_issues.md#ki-098), filed rather than fixed. And the
      2026-09-10 gate run's own findings: a determined field or tuple
      projection predicate is no longer retained on a scheme
      (`src/types/quantify.rs`) — retaining it made every caller of
      `Flow.Array.update_many` an `E490`, because a `SchemeConstraint` cannot
      carry `TupleProjection`'s index, and it takes the epoch to **53** — plus
      `Flow.List.first` reached with an `Array` in
      `examples/functions/immutability_valid.flx`, four baselines and two
      fixture expectations that G5 and KI-094 had left stale, and a
      Windows-only `-D warnings` failure in the native driver. The "eight
      fixes" count no longer matches, and the epoch run is now 47 → 53.

      **The claim to drop.** "0.0.7 has no open items" is false.
      [KI-098](../known_issues.md#ki-098) is open and is *caused by this
      branch's own* G5 and B1 — a definition used at two types is still left
      despecialised, at seven sites recorded in the accepted baseline.
      [KI-097](../known_issues.md#ki-097) is open. F3 is deferred to 0.0.8.
      `ba239f68`'s subject line repeats the claim, and that commit is in the
      history a reviewer reads, so the PR body has to state what is open
      plainly rather than leave that subject standing as the summary.

---

## Verification

Per the standing arrangement, **the full gate is run by the user, not by me**;
I use `cargo build` and `cargo test --no-run` for compile correctness only.

- Gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --all --all-features`
  (nextest ~5 min with `--test-threads 6`; the plain runner ~55 min).
- Parity: `cargo run -- parity-check tests/parity --ways vm,llvm` and
  `cargo run -- parity-check examples/guide --compile --ways
  vm,llvm,vm_cached,vm_strict,llvm_strict`. `vm_cached` is what catches a missed
  epoch bump.
- **Neither instrument caught R1.** Every commit on this branch passed both
  while a three-line program miscompiled. Parity could not catch it for the
  reason in KI-062, and no unit test asserted `LetRec` nesting. Treat a green
  gate on this branch as necessary, not sufficient, until R5 lands.
- **`aether_cli_snapshots` is the only thing that shows *precision*.** A
  despecialised program compiles, runs, and gives the right answer, so neither
  parity nor a compile-and-run corpus sweep can see it — a sweep reports
  *outcomes*, and precision is not one. This suite captures `--dump-core` and
  `--dump-aether`, where a lost `:Int` on a binder is visible as a lost `:Int`.
  Run it after **any** change to inference, generalization or lowering.

  It was not run during the 0.0.7 generics work, and three separate regressions
  hid behind that: KI-095's snapshots were never updated (7 tests, red for
  fourteen commits), G5 despecialised nine more, and the corpus sweep reported
  a confident *zero regressions* for both because it is blind to this by
  construction. Choosing test binaries by topic is what failed — a suite named
  for Aether does not sound related to type inference, and it is the one that
  matters most.
- Diagnostics changes need `--no-cache` to verify: a warm cache masks them.

---

# 0.0.8 — type classes

Order: **C → B1 → B2 → B3 → B4 → D → A2**. C first: it is what the measurement
above says B2 is actually waiting on, and three of Track D's High-severity bugs
are the same class of defect it deletes at the root.

---

## Track C — 0186 stage 5: one evidence-passing translation

**The 0.0.8 headline.** Recording is done and gated — it landed during the
0.0.7 work because the quantification decision had to record *something*; what
remains is the emitter, and it is the only track that deletes anything.

One fact decides the shape of all of it: **`CoreExpr` carries no `ExprId`** —
zero references in `src/core/mod.rs`. Evidence is keyed by `ExprId`, so no Core
pass can consume it. `dict_elaborate` cannot be rewired in place; dictionary
arguments have to be emitted during AST→Core lowering, where the id is still in
hand, and the Core pass retired behind that.

- [x] **C1. `EvidenceMap` / `EvidenceSite`**, populated from the whole-program
      solve. `2426909e`
- [x] **C2. `translate.rs`** — evidence → `DictArg`, resolving nothing.
      `e04593a7`
- [x] **C3. `InstanceKey.dict_type_key`** — name a chosen instance without
      re-searching the class environment. `6e8d259f`
- [x] **C4. Attribute a checked sub-expression's predicates to itself.**
      `912e9bba`
- [ ] **C5. Emit dictionary arguments for ordinary constrained *calls* at
      lowering.** The safe first target: `insert_dict_args_at_call_sites`
      already handles these, so the new emission has a reference output to diff
      against — a divergence is a bug in the new path, and parity catches it
      rather than inspection. No synthesized `Lam` is needed for a call, so none
      of C6's difficulty applies here.
- [ ] **C6. Delete the six resolution sites.** Depends on C5.
      - `class_call_type_args` — both copies (`lower_ast/mod.rs:594`,
        `compiler/expression.rs:5368`) and the "must stay in lockstep" comment
      - `resolve_dict_arg`, `build_caller_dict_map`, `choose_candidate` and its
        first-candidate recovery, `build_contextual_dictionary_expr`,
        `superclass_evidence_expr` (`dict_elaborate.rs`)
      - `predeclare_instance_dictionary_globals` (`predeclaration.rs`)
      - the `±dictionaries` band in `check_known_call_arity`

      Answer first whether the AST bytecode fallback can be retired (E3's open
      question), so this has two consumers to satisfy rather than three.
- [ ] **C7. KI-090 — a constrained function referenced as a *value*.** The last
      case of the same emission, and the hardest: it is the only one needing a
      synthesized `CoreExpr::Lam`, which is where three attempts failed. Core is
      provably correct (`λ%t569. dbl(__dict_..._Num_Int, %t569)`) and **both
      backends still reject it** — VM `E1000`, native SIGSEGV. The defect is
      below Core, in how a synthesized `Lam` must be built for closure
      conversion; prime suspect is `param_types: vec![None; n]` /
      `result_ty: None`, which drive `FluxRep` selection. See
      [the known issue](../known_issues.md#ki-090) for the four obstacles
      already solved.

      *This is closure-conversion work, not generics work — and it does not
      block C6.*

---

---

## Track B — 0187: specialisation, then generalize-by-arity

`fn identity(x) { x }` usable at two types. **Moved here from 0.0.7**, because
the measurement in *The split* shows 9 of the 13 tests it breaks are dictionary
plumbing that only Track C fixes — B1 addresses 4. **C must land first**, which
is the opposite of what 0187 assumes.

- [x] **B1. Specialise a generalized definition at its single concrete
      instantiation — DONE 2026-09-10.** `4f45fb70`.

      **This entry previously described a different pass, and the difference is
      the finding.** It said: a `core/` pass at Stage 0.6, cloning on a
      known-global *dictionary argument*. That cannot fix what needed fixing.

      G5 introduced a **second** kind of despecialisation this list did not
      anticipate. The four optimisation tests it broke are class-free —
      `fn copy_head(xs) { match xs { [h | t] -> [h | [h | t]], _ -> [] } }` has
      no dictionary to clone on. Generalizing it replaces a concrete parameter
      type with a variable, and a variable has no `FluxRep`, so `IntRep` is lost
      and Aether must emit a `DropSpecialized` it had proved unnecessary. The
      constrained kind (`IAdd` → dictionary call) is real too, but it is B2's,
      and F8's "16 became 4" measured both at once because full
      generalize-by-arity triggers both.

      **And it cannot live at Stage 0.6.** A `core/` pass cannot see a call
      site's instantiation: `CoreExpr` carries no `ExprId` and a `CoreBinder`
      carries only a `FluxRep`. The type is in `hm_expr_types`, which is live
      during AST → Core lowering and gone afterwards — the same constraint that
      puts C5's emitter at lowering rather than inside `dict_elaborate`.

      What landed: during lowering, a definition whose call sites all agree on
      one fully concrete instantiation has its body lowered under that
      substitution. **In place — no clone, no call-site rewriting, no arity
      change**, so none of the `verify_aether_contract_stage` /
      `core_lint_stage` risk a cloning pass carries.

      Three things that had to be right:

      - **Self-calls are not instantiations.** A recursive occurrence is at the
        definition's own type by construction; counting it excludes every
        recursive function, which is half the failing tests.
      - **No `TypeEnv` dependency.** The first version read the function's
        scheme, which works in the compiler and does nothing under
        `lower_program_ast` (`type_env: None`) — the optimisation would have
        silently not happened on any path without a scheme table. The generic
        side is now recovered from parameter occurrences in the body.
      - **Constrained functions are excluded.** Specialising one rewrites the
        body types that several sites read to decide which instance a method
        call means, while its dictionaries stay positional and fixed by its
        signature. Verified failure: `result_directed_two_dictionaries.flx`
        prints `7` for a `String`. That is a *sixth* instance of the
        re-derivation family in `CLAUDE.md`, produced while fixing the fifth.

      *Exit:* the 4 optimisation tests pass untouched, the generalization rule
      unchanged. **Remainder: [KI-098](../known_issues.md#ki-098)** — a
      definition used at *two* types is still despecialised. Cloning is owed to
      B2, where it is the common case rather than seven sites.

- [ ] **B2. Land generalize-by-arity** — `monomorphism_restriction` by arity in
      `finalize_and_bind_function_scheme`. Two lines; written and reverted in
      `60b3fa39`, so the diff already exists. Depends on B1.
      Exit: the same 4 still pass **without being weakened**.
- [ ] **B3. Bump `CACHE_EPOCH`.** Depends on B2.
- [ ] **B4. 0186 stage 4's remainder — one quantification decision per
      *group*.** Depends on B2, and only on B2. No failing case exists today:
      mutually recursive polymorphic functions, constrained ones included,
      already work. It becomes necessary once unannotated helpers are
      constrained, because one member's `forall` must not mention a variable
      another member left free.

Why this order: B2 alone despecialises every unannotated helper — `IAdd`
becomes a dictionary call, and `my_filter` goes from `FBIP: fip, FreshAllocs: 0`
to `fbip(1), FreshAllocs: 1`. Landing it before B1 means dismantling the 4 tests
that assert superinstruction fusion, `DropSpecialized` elimination and tail
calls still fire.

---

---

## Track D — open type-class bugs

Ranked by severity; complete as of this writing.

### High — blockers for calling type classes finished

- [ ] **D1. [KI-071](../known_issues.md#ki-071)** — an instance method captures
      unqualified calls to a module-level function of the same name.
      *Name resolution.*
- [ ] **D2. [KI-073](../known_issues.md#ki-073)** — result-directed selection is
      lost on native when routed through a constrained function.
      *Native backend; a VM/native divergence.*
- [ ] **D3. [KI-076](../known_issues.md#ki-076)** — an operator on a
      class-constrained type parameter does not dispatch inside a `module`
      block. *Dictionary passing.*
- [ ] **D4. [KI-086](../known_issues.md#ki-086)** — a class declared inside a
      `module` loses its default method bodies. Two symptoms: a runtime
      `E1001 panic: No instance of Greet.greet` cross-module, and a default body
      cannot call a sibling method (`E004`). *Module interfaces.*
- [ ] **D5. [KI-090](../known_issues.md#ki-090)** — a constrained function
      passed as a value loses its dictionary. Tracked as **C7**; listed here so
      the High count is honest. Closure-conversion work, not generics work.


### Medium


- [x] **D7. [KI-069](../known_issues.md#ki-069)** — a contextual instance cannot
      compare a field of its own head type. **No longer reproduces, verified
      2026-09-11** against the entry's own repro, which now compiles and
      dispatches at `Tree<Int>`. The fixing change was not identified — this
      came out of building the generics corpus (F9), not a deliberate fix — so
      the entry is marked fixed-by-verification and pinned by
      `examples/generics/working/accepts/data_contextual_instance_recursive_head.flx`.
      Its "no workaround" line was the misleading part and is retracted.


- [ ] **D10. [KI-053](../known_issues.md#ki-053)** — the whole-program dumps
      report types from the wrong module. Not generics; it is the instrument
      used to *read* generics dumps, which is why it is on this list.

### Low

- [ ] **D11. [KI-054](../known_issues.md#ki-054)** — an imported contextual
      instance method is declared one parameter short natively.
- [ ] **D12. [KI-055](../known_issues.md#ki-055)** — native builds emit an
      unreferenced forwarding copy of every module-owned class method.
- [ ] **D13. [KI-068](../known_issues.md#ki-068)** — a bare `Compiler` cannot
      supply the standard classes' instance bodies.
- [ ] **D14. [KI-084](../known_issues.md#ki-084)** — a bare `Compiler` cannot
      build a dictionary for a contextual prelude instance. Same root as D13;
      fix together.
- [ ] **D15. [KI-074](../known_issues.md#ki-074)** — a lowercase class name is
      declarable but unusable in a `where` clause. *Parser.*
- [ ] **D16. [KI-075](../known_issues.md#ki-075)** — `<a: C>` on a
      multi-parameter class suggests an instance that cannot be written.
      *Diagnostics.*
- [ ] **D17. [KI-049](../known_issues.md#ki-049)** — one-version-per-package
      remains a linker limitation. Listed for completeness; not generics and
      not 0.0.7.

---

## Track A — the boundary, as a guard test

0e is withdrawn. The crate split it called for is not being built, and the three
crates that had been extracted are folded back: `src/source/`, `src/diagnostics/`
and `src/shared/scc.rs`. The workspace is the root crate plus `crates/flux-lsp`.
The reasoning is recorded in
[0186's *Where it lives*](../proposals/0186_generics_foundations.md).

- [x] **A0. Fold the crates back.** `flux-source` → `src/source/`,
      `flux-diagnostics` → `src/diagnostics/`, `flux-generics` →
      `src/shared/scc.rs`, `generics_frontend` → `src/binding_groups.rs`.
      ~35 line edits: the re-export aliases already matched the destination
      module names, so no import path outside the moved files changed.
- [x] **A1. `InstanceDef.type_key`** — hold the string every `__dict_*` and
      `__tc_*` symbol is derived from, so changing the representation cannot
      silently rename every dictionary. `6eb113c9`
- [ ] **A2. `InstanceDef.type_args: Vec<TypeExpr>` → `Vec<InferType>`**
      — 56 compile errors, 41 in `class_env.rs`. **Keep this one**: it stands on
      its own merits, independent of any boundary. It deletes
      `match_instance_type_expr` (4 call sites, the last `TypeExpr`-pattern
      matcher) in favour of `match_infer`, which already exists with one caller.
      Two decisions first:
      - instance head type parameters need a numbering convention — reuse stage
        0d's, class parameter `i` is `TypeVarId(i)`;
      - `PublicInstanceEntry.type_args` (`module_interface.rs:113`) is `.flxi`
        format. Convert at the boundary and the format is unchanged, so no
        further epoch bump is owed.
- [ ] **A3. The guard test.** One file, walking module imports, asserting all
      four CLAUDE.md architecture rules. It covers what a crate graph cannot:
      two of the four are about *contents* (`bytecode/` must not gain execution
      logic; `shared_ir/` is ID plumbing, not a pipeline stage). The two
      directional rules are one edge each — `llvm → bytecode` is `hash_bytes`,
      `syntax → core` is `CorePrimOp` — so the test should pin those two known
      edges and fail on a third.

      Rank it below R5. A shape test would not have caught R1, and R5 would
      have.
- [ ] ~~**A4. Move what remains into `flux-generics`**~~ — withdrawn with 0e.

---

## Deliberately not doing

- **0186 stage 0d's remainder** — threading `&ClassSurface` into eight more
  consumers. Superseded by A2, which addresses the same dependency at its root.
- **0186 stage 0e** — the `flux-generics` crate and the `src/types/` move.
  Withdrawn; replaced by A3, the guard test. The three extracted crates were
  folded back in `33eb1cfa`.
