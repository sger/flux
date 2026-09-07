# Generics: what is left

Working list for proposals [0186](../proposals/0186_generics_foundations.md) and
[0187](../proposals/0187_specialisation.md), on `feat/0186-generics-foundations`
(30 commits, green at every gate).

Ordered within each track. Tracks are independent of each other unless a
dependency is stated.

---

## Track A — 0e: the crate boundary

The boundary 0186 is named for. Measured, not estimated: `src/types/` holds 149
production references to `TypeExpr`, two-thirds of them in code that should not
move at all.

- [x] **A1. `InstanceDef.type_key`** — hold the string every `__dict_*` and
      `__tc_*` symbol is derived from, so changing the representation cannot
      silently rename every dictionary. `6eb113c9`
- [ ] **A2. `InstanceDef.type_args: Vec<TypeExpr>` → `Vec<InferType>`**
      — 56 compile errors, 41 in `class_env.rs`. Two decisions to make first:
      - instance head type parameters need a numbering convention — reuse stage
        0d's, class parameter `i` is `TypeVarId(i)`;
      - `PublicInstanceEntry.type_args` (`module_interface.rs:113`) is `.flxi`
        format. Convert at the boundary and the format is unchanged, so no
        further epoch bump is owed.

      Payoff: deletes `match_instance_type_expr` (4 call sites, the last
      `TypeExpr`-pattern matcher) in favour of `match_infer`, which already
      exists with one caller.
- [ ] **A3. Move the frontend halves to the root crate** — `class_dispatch.rs`
      (2,191 lines; it synthesises `Statement::Function`) and `class_env`'s
      `Statement`-reading collection functions (~900 lines). Mechanical.
      Depends on A2 only for tidiness, not correctness.
- [ ] **A4. Move what remains into `flux-generics`**, re-export as
      `crate::types` so existing imports are untouched.

---

## Track B — 0187: specialisation, then generalize-by-arity

The user-visible win: `fn identity(x) { x }` usable at two types. Independent of
Track A.

- [ ] **B1. Specialisation pass over `core/`** — clone a constrained function at
      each concrete instantiation and rewrite that call site to the clone.
      Exit: a constrained helper called only at `Int` lowers to `IAdd` again,
      **with the current generalization rule unchanged**, and the 16
      optimisation tests pass untouched.
- [ ] **B2. Land generalize-by-arity** — `monomorphism_restriction` by arity in
      `finalize_and_bind_function_scheme`. Two lines; written and reverted in
      `60b3fa39`, so the diff already exists. Depends on B1.
      Exit: the same 16 tests still pass **without being weakened**.
- [ ] **B3. Bump `CACHE_EPOCH`.** Depends on B2.

Why this order: B2 alone despecialises every unannotated helper — `IAdd`
becomes a dictionary call, and `my_filter` goes from `FBIP: fip, FreshAllocs: 0`
to `fbip(1), FreshAllocs: 1`. Landing it before B1 means dismantling 16 tests
that assert superinstruction fusion, `DropSpecialized` elimination and tail
calls still fire.

---

## Track C — 0186 stage 5: one evidence-passing translation

Recording is done and gated. Everything left needs a working consumer.

- [x] **C1. `EvidenceMap` / `EvidenceSite`**, populated from the whole-program
      solve. `2426909e`
- [x] **C2. `translate.rs`** — evidence → `DictArg`, resolving nothing.
      `e04593a7`
- [x] **C3. `InstanceKey.dict_type_key`** — name a chosen instance without
      re-searching the class environment. `6e8d259f`
- [x] **C4. Attribute a checked sub-expression's predicates to itself.**
      `912e9bba`
- [ ] **C5. KI-090** — a constrained function referenced as a value. Three
      attempts; see [the known issue](../known_issues.md#ki-090) for the four
      obstacles already solved. Core is provably correct
      (`λ%t569. dbl(__dict_..._Num_Int, %t569)`) and **both backends still
      reject it** — VM `E1000`, native SIGSEGV. The remaining defect is below
      Core, in how a synthesized `CoreExpr::Lam` must be built for closure
      conversion. Prime suspect: `param_types: vec![None; n]` and
      `result_ty: None`, which drive `FluxRep` selection.
      *This is closure-conversion work, not generics work.*
- [ ] **C6. Delete the six resolution sites.** Depends on C5 or an equivalent
      consumer.
      - `class_call_type_args` — both copies (`lower_ast/mod.rs:594`,
        `compiler/expression.rs:5368`) and the "must stay in lockstep" comment
      - `resolve_dict_arg`, `build_caller_dict_map`, `choose_candidate` and its
        first-candidate recovery, `build_contextual_dictionary_expr`,
        `superclass_evidence_expr` (`dict_elaborate.rs`)
      - `predeclare_instance_dictionary_globals` (`predeclaration.rs`)
      - the `±dictionaries` band in `check_known_call_arity`

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

- **0186 stage 4's remainder** — one quantification decision per *group* rather
  than per member. No failing case could be constructed: mutually recursive
  polymorphic functions, constrained ones included, already work. It only pays
  off once B2 makes unannotated helpers constrained, so it is speculative until
  then.
- **0186 stage 0d's remainder** — threading `&ClassSurface` into eight more
  consumers. Superseded by A2, which addresses the same dependency at its root.
