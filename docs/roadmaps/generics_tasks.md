# Generics (0.0.7), type classes (0.0.8)

Working list for [0185](../proposals/0185_generalize_by_arity.md),
[0186](../proposals/0186_generics_foundations.md) and
[0187](../proposals/0187_specialisation.md).

## The split

**0.0.7 is generics. 0.0.8 is type classes.** Generics is *which definitions get
quantified, over what, and in what order* — binding groups, the monomorphism
restriction, specialisation. Type classes is *what a call site is handed* —
evidence, dictionaries, instance selection.

### Generalize-by-arity moved to 0.0.8 — measured, not assumed

It was the 0.0.7 headline. It is not reachable there, and the reason was
measured on 2026-09-08 by reapplying the two-line change and reading the
failures:

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

The alternative was patching forwarding where it surfaces, without C6's
deletion. That is what was done for KI-052, KI-061, KI-082, KI-083 and one
unfiled case — five local fixes to one bug — and it is the pattern 0186 exists
to stop.

### 0.0.7 exit criteria

1. **No known miscompilation** — Track R. **Done**: R1–R7 are all closed.
2. **The gate can detect a generics regression** — R5 and KI-062. **Done**: a
   fixture declaring `expect: success` must now actually run.
3. **The remaining generics-side bugs and the release mechanics** — Tracks G, E
   and F below. This is what is left.

### 0.0.8 exit criteria

1. **One evidence-passing translation** — Track C. The six resolution sites are
   deleted, not merely agreeing.
2. **`fn identity(x) { x }` works at two types** — B1 then B2, on top of C.
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

- [ ] **R1. Binding groups are emitted in source-anchor order, not dependency
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
- [ ] **R2. `bound_name` sees only `Let` and `Function`.** A `LetDestructure`
      between two members binds names invisibly to the hoist check. It should
      return *every* binder, walking the pattern —
      `FreeVarCollector::define_pattern_bindings` already does this walk and can
      be reused.
- [ ] **R3. The hoist check is one-sided.** Nothing asks whether an intervening
      statement *uses* a member, so a group can be emitted after code that calls
      it. Needs free variables of non-function statements:
      `collect_free_vars_in_statement` does not exist and must be added beside
      `collect_free_vars_in_function_body` — `FreeVarCollector` already has the
      `visit_stmt` arms.
- [ ] **R4. Drop `plan_block`'s `free_vars` callback.** All three call sites
      pass a byte-identical closure delegating to
      `crate::ast::free_vars::collect_free_vars_in_function_body`. The callback
      was justified as keeping the module off lowering internals, but it is
      `crate::ast`, which `binding_groups` already depends on. Removing it
      deletes three copies and is a prerequisite for R3 supplying statement
      free vars in one place.
- [ ] **R5. Close the test gap that let R1 through.** Ordered — the first is a
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
- [ ] **G4. [KI-094](../known_issues.md#ki-094)** — *Medium, found behind G3.*
      Its `E308` had been masking two further defects.

      **Fixed half:** *no* transparent type alias resolved —
      `alias IntPair = (Int, Int)` then `-> IntPair` was `E423`, because
      `is_known_annotation_type` did not consult `transparent_type_aliases` and
      that check runs before the Phase 1d expansion. New fixture
      `type_alias_transparent_basic.flx` covers it.

      **Open half:** an alias whose expansion carries an effect row
      (`() -> Option<a> with Async`) fails at runtime, and an alias taking an
      effect-row *parameter* (`AsyncFn<Int, Int, e>`) has no way to declare `e`
      at the use site. The second is a design question. Only
      `type_alias_transparent.flx` exercised any of this and it had never run,
      so the feature was effectively untested.

---

## Track E — 0185's remaining stages

[0185](../proposals/0185_generalize_by_arity.md) stages 0–2 shipped; stage 3
became 0186 stage 6 and is now Track B. Four stages remain, and 0186 changed
what two of them cost.

- [ ] **E1. Stage 5 — tuple projection as a constraint.** *Independent, and the
      most actionable thing left in 0185.*
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

- [ ] **F1. `CACHE_EPOCH`.** Currently 46. R1's fix changes lowering output, and
      B2 changes inferred schemes — each needs a bump with a one-line reason, or
      users get silently stale artifacts (D9 is what that looks like). One bump
      per landing, not one at the end.
- [ ] **F2. Amend KI-087.** Marked FIXED 2026-09-06; its fix introduced R1. The
      entry needs the regression recorded and a "verified when" note that
      covers the forward-reference shape, not just the mutual-recursion one.
- [ ] **F3. Mark the fixed issues.** *0.0.8.* KI-052, KI-061, KI-082, KI-083 are
      already marked FIXED individually; 0186 claims to have removed the *cause*
      they share. That claim is only true once C6 lands, so it is a 0.0.8 note —
      do not write it in the 0.0.7 release.
- [ ] **F4. Move the proposals.** 0185 and 0187 can move at 0.0.7 once their
      tables are all `done` or `withdrawn`. **0186 cannot** — its stage 5 is
      Track C, which is 0.0.8. Its stage 0e is already marked withdrawn.
- [ ] **F5. Update `roadmap_to_1_0_0.md`.** Its 0.0.7 entry lists tests, linter
      and language identity with no generics work at all, and nothing there
      matches 0.0.8 = type classes. Both entries need rewriting, and whatever
      0.0.7 displaces has to land somewhere.
- [ ] **F6. `CLAUDE.md` is untracked** and names paths this branch moved
      (`src/generics_frontend/`, the extracted crates). Its architecture section
      and its workspace description are both stale. Decide whether it is
      tracked, then fix it.
- [ ] **F8. Amend [0187](../proposals/0187_specialisation.md).** Its "Why this is
      a separate proposal" section says the cost of landing generalize-by-arity
      is "16 tests that assert an optimisation still fires". Measured: 4 do. The
      other 9 are missing or miscounted dictionaries, which specialisation does
      not touch. Its staging table needs Track C ahead of stage 1, and its open
      question "where specialisation runs" is answerable now — see B1.
- [ ] **F7. Write the PR description.** `CHANGELOG.md` is assembled from merged
      PRs at release time, so the PR description *is* the changelog entry.

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

- [ ] **B1. Specialisation pass over `core/`** — clone a constrained function at
      each concrete instantiation and rewrite that call site to the clone.

      *Where it goes, answered:* `elaborate_dictionaries` runs whole-program at
      Stage 0.5 in `run_core_passes_with_class_env`, before the per-def
      simplification loop and before `promote_builtins`. A specialisation pass
      slots in at Stage 0.6 — clone on a known-global dictionary argument,
      rewrite the call — and `promote_builtins` then sees a monomorphic body and
      can emit `IAdd` again. It must be **whole-program**: the simplification
      loop is `for def in &mut program.defs`, and specialisation rewrites call
      sites in *other* defs. It must also satisfy `verify_aether_contract_stage`
      and `core_lint_stage`.

      This answers 0187's open question in favour of "after `dict_elaborate`"
      rather than "on the solver's evidence". Note `src/core/passes/specialize.rs`
      already exists and is unrelated — it inlines single-use wrappers — so the
      new pass needs a different name.

      *Exit:* the 4 `DropSpecialized` tests pass with the current
      generalization rule unchanged.
      Exit: a constrained helper called only at `Int` lowers to `IAdd` again,
      **with the current generalization rule unchanged**, and the 16
      optimisation tests pass untouched.

      *Scope it narrowly first:* specialise only a function whose call sites all
      use **one** instance. That is the common case in `lib/Flow/`, it is a
      fraction of general monomorphisation, and it is very likely enough to
      satisfy all 16 tests. Widen only if it is not.
- [ ] **B2. Land generalize-by-arity** — `monomorphism_restriction` by arity in
      `finalize_and_bind_function_scheme`. Two lines; written and reverted in
      `60b3fa39`, so the diff already exists. Depends on B1.
      Exit: the same 16 tests still pass **without being weakened**.
- [ ] **B3. Bump `CACHE_EPOCH`.** Depends on B2.
- [ ] **B4. 0186 stage 4's remainder — one quantification decision per
      *group*.** Depends on B2, and only on B2. No failing case exists today:
      mutually recursive polymorphic functions, constrained ones included,
      already work. It becomes necessary once unannotated helpers are
      constrained, because one member's `forall` must not mention a variable
      another member left free.

Why this order: B2 alone despecialises every unannotated helper — `IAdd`
becomes a dictionary call, and `my_filter` goes from `FBIP: fip, FreshAllocs: 0`
to `fbip(1), FreshAllocs: 1`. Landing it before B1 means dismantling 16 tests
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


- [ ] **D7. [KI-069](../known_issues.md#ki-069)** — a contextual instance cannot
      compare a field of its own head type. *Dictionary elaboration; a C
      candidate.*


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
