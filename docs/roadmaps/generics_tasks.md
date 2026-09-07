# Generics: what is left

Working list for proposals [0186](../proposals/0186_generics_foundations.md) and
[0187](../proposals/0187_specialisation.md), on `feat/0186-generics-foundations`
(30 commits).

Ordered within each track, and the tracks are listed in the order to do them.
**Track R first** — the branch has a confirmed correctness regression, and it is
worse than the bug it was introduced to fix. Then C, the only track that deletes
anything; A exists to protect what C removes. B is independent of both and is
the only track a user would notice.

Every commit on this branch passed a full `cargo test --all --all-features` plus
a parity sweep, and R1 was present the whole time. Nothing on Track A would have
caught it — a dependency-shape guard test cannot see a runtime miscompilation.
That is the reason R ranks above A here.

---

## Track R — regressions this branch introduced

Found by the stage-5 code review, all in stage 2's `plan_block`
(`src/binding_groups.rs`). R1 is confirmed by running the program; R2–R4
are reasoned from the same code and not yet reproduced.

- [ ] **R1. Binding groups are emitted in source-anchor order, not dependency
      order.** *Confirmed, cold cache, on this branch:*

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
      `let`) that stage 2 was written to fix.

      `flux_generics::strongly_connected_components` already returns reverse
      topological order — dependencies first, which is exactly the order
      `prepend_stmts` needs, since it folds `plan.iter().rev()` so the *first*
      plan item becomes the outermost binding. `plan_block` discards that order
      and anchors each group at its first member instead, so `a` becomes the
      outer `LetRec` and its closure captures `b`'s uninitialised slot. The
      deleted `lower_fn_run_with_scc` kept the SCC order; nothing replaced it.

      Two consumers share the defect: `prepend_stmts`
      (`core/lower_ast/mod.rs:1589`) and the VM's statement loop
      (`compiler/statement.rs:3105`).

      The fix is not a plain sort — non-function statements must keep their
      source order, and a group may not hoist above a `let` whose binding it
      reads. What is needed is a topological order over *plan items*, with edges
      for: consecutive `Other`s (source order), `Other → Group` where a member
      reads what it binds, `Group → Other` where it reads a member (R3), and
      `Group → Group` for dependencies — tie-broken by source index so ordinary
      programs are laid out exactly as they are today.
- [ ] **R2. `bound_name` sees only `Let` and `Function`.** A `LetDestructure`
      between two members binds names invisibly to the hoist check, so a group
      can be placed above a destructured binding it reads. It should return
      *every* binder of a statement, walking the pattern.
- [ ] **R3. The hoist check is one-sided.** `reads_intervening_binding` asks
      whether a member reads an intervening binding, but nothing asks whether an
      intervening statement *uses* a member — so a group can be emitted after
      code that calls it. Needs free variables of non-function statements, which
      the `free_vars` callback does not currently supply (lowering returns an
      empty set for anything but a `Function`).
- [ ] **R4. Close the test gap that let R1 through.** Three parts, and the third
      is the one that matters:
      - a `tests/parity/` fixture for the plain nested forward reference.
        `expect: success` catches it — the program fails outright rather than
        diverging between backends, which is why VM-vs-native comparison alone
        would not have;
      - a lowering test that asserts `LetRec` **nesting**. The surviving ones
        only count nodes, which is why they stayed green;
      - restore the determinism/ordering coverage lost when
        `tarjan_scc_reverse_topological_order` was deleted along with
        `lower_fn_run_with_scc`. It was the only thing guarding this order, and
        it went out with the function it tested.

### Unverified — from the same review, verification never reported

The review's finder pass returned eight angles; the verification pass had not
reported when the session ended, so these two are plausible and unconfirmed.
Check them before acting.

- [ ] **R5. `close_definition_scope` re-derives `quantified`**, which stage 4
      made `decide_quantification`'s job. If the two derivations disagree, one
      of them is wrong and the whole point of stage 4 — *one* quantification
      decision — is not yet true.
- [ ] **R6. `harvest_evidence` indexes only `Disposition::Solved` predicates.**
      `EvidenceSite.index` is documented as the predicate's argument position;
      skipping unsolved predicates shifts every later index, so a site with a
      stuck predicate before a solved one would pass a dictionary in the wrong
      slot.

---

## Track C — 0186 stage 5: one evidence-passing translation

**Do this after Track R.** Recording is done and gated; what remains is the
emitter, and it is the only track that deletes anything.

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

      Rank it below R4. A shape test would not have caught R1, and R4 would
      have.
- [ ] ~~**A4. Move what remains into `flux-generics`**~~ — withdrawn with 0e.

---

## Track B — 0187: specialisation, then generalize-by-arity

The user-visible win: `fn identity(x) { x }` usable at two types. Independent of
Track A.

- [ ] **B1. Specialisation pass over `core/`** — clone a constrained function at
      each concrete instantiation and rewrite that call site to the clone.
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

## Track D — open bugs found along the way

- [ ] **D1. [KI-086](../known_issues.md#ki-086)** — a class declared inside a
      `module` loses its default method bodies. Two symptoms: a runtime
      `E1001 panic: No instance of Greet.greet` cross-module, and a default body
      cannot call a sibling method (`E004`). Predates this branch.
- [ ] **D2. [KI-088](../known_issues.md#ki-088)** — a nested `fn` shadowing a
      top-level name. Inference half fixed (`4654bad1`); a second lookup in the
      compiler's own resolution still reaches past the nested definition.
      Note the reproduction is rejected by a *compiler boundary* check, so a
      case in `tests/type_inference/` passes whether or not the bug is present
      — pinning it needs an end-to-end test.

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
      still the old kind and the report would be wrong.
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
- [ ] **E4. Stage 7 — close 0183.** Documentation only. Mark R1–R5 shipped,
      record R6 as delivered by E2 + B2, close its open questions as decided,
      and move it to `docs/proposals/implemented/`. Do this **after** E2, or the
      record is written before the thing it records.

---

## Deliberately not doing

- **0186 stage 0d's remainder** — threading `&ClassSurface` into eight more
  consumers. Superseded by A2, which addresses the same dependency at its root.
