# 0.0.8 — type classes: the phased plan

Supersedes the 0.0.8 half of [generics_tasks.md](generics_tasks.md). That file
remains the record of 0.0.7 and of the measurements this plan relies on. How
GHC solves each phase's problem, and what that changed here, is in
[typeclass_ghc_analysis.md](../internals/typeclass_ghc_analysis.md).

## Context

0.0.7 shipped generics: an unannotated definition with **no** class constraint
is generic (`fn identity(x) { x }`). 0.0.8 does the constrained half, and removes
the reason it has been blocked.

**The problem.** Six places in the compiler independently decide which dictionary
a call site gets. They are kept in agreement by "keep in lockstep" comments, and
that has produced the same bug five times (KI-052, KI-061, KI-082, KI-083, and
one unfiled case). The solver already records the right answer per call site in
an `EvidenceMap` (`src/types/evidence.rs`). A translator from that answer to a
dictionary argument already exists (`src/types/translate.rs`,
`evidence_to_arg`). **Neither has a caller in `src/`.**

**The outcome.**
- One translation from evidence to dictionary arguments.
- On top of it, `fn double(x) { x + x }` works at two types with no signature.
- No open High-severity type-class bug.

**What "syntax" means here.** 0.0.8 adds **no new surface syntax**. The
class-syntax proposal [0182](../proposals/0182_typeclass_syntax_completeness.md)
(multiple superclasses, multiple instance contexts, lowercase classes in
`where`) stays in 0.0.9. Each phase's *Syntax* section shows the Flux programs
that fail before the phase and work after it. Where the change is only visible
in Core, it shows the Core shape instead.

**Starting point.** `main` at `3f123c02`, the 0.0.7 release. Phase 0 had been
started on an earlier branch that no longer exists, so it is redone here.

---

## Rules — apply to every phase

1. **One branch per phase**, cut from `main`, and one PR per phase. Branch names
   say what the work is, never the version: `feat/evidence-prereqs` (0),
   `feat/evidence-translation` (1), `feat/constrained-generalization` (2),
   `fix/typeclass-high-bugs` (3), `docs/typeclass-closure` (4). The PR description is
   the changelog entry.
2. **One commit per numbered step.** A behavioural fix never shares a commit
   with a refactor, so a regression bisects to one step.
3. **Pin before fixing.** Every fix starts with a failing test, or a fixture
   under `examples/generics/failing/…`. When the fix lands, the fixture moves to
   `working/accepts/`. The fixture move is the progress record.
4. **No new re-derivation.** New code must not work out an instance from types.
   It reads `EvidenceMap`, or it fails loudly. "Pick the first candidate" and
   "default to `Int`" are rejected changes.
5. **Fail closed.** A missing dictionary is a compile-time internal error that
   names the site. It is never a short argument list, and never a silently
   wrong dictionary.
6. **Bump `CACHE_EPOCH`** (`src/shared/cache_paths.rs`, currently 53) in the
   same commit as any change to inferred types or to `.flxi` contents, and add
   the reason to the doc comment above it.
7. **Run the precision gate** after any change to inference, generalization or
   lowering: `aether_cli_snapshots` plus `tests/aether/core_regressions.rs`.
   Parity and corpus sweeps cannot see despecialisation.
8. **Parity fixtures must run.** `expect: success` is enforced (KI-062). A
   native-only failure is registered with `// skip: KI-xxx`, never dropped.
9. **The gate is split.** The implementer runs `cargo build` and
   `cargo test --no-run`. The maintainer runs the full gate:
   - fmt;
   - clippy with `-D warnings`;
   - nextest;
   - parity on `vm,llvm`, and `examples/guide` on
     `vm,llvm,vm_cached,vm_strict,llvm_strict`.
10. **Docs close each phase.** Tick this file's checkboxes, and update the
    affected [known_issues.md](../known_issues.md) entries and the
    `examples/generics/README.md` capability map.

---

## Phase 0 — prerequisites

*No proposal owns these.* **Goal:** make the evidence map safe to consume, and
remove one silent wrong-answer path, before anything reads either.

### Syntax

Correct programs do not change. Wrong ones do: a call site where no instance
matches used to run with an arbitrary dictionary, and now gets a diagnostic.

```flux
class Size<a> { fn size(x: a) -> Int }
instance Size<Int>    { fn size(x) { 1 } }
instance Size<String> { fn size(x) { len(x) } }
// If elaboration reaches NoMatch at a dispatch site, it used to answer with
// Size<Int> anyway. After 0d it declines, and the caller reports the site.
```

### Steps

- [ ] **0b. Re-measure B2's cost.**
  1. Temporarily make the `unconstrained` conjunct in `should_generalize_function`
     (`src/ast/type_infer/function.rs`) always `true`.
  2. Run nextest and `examples_generics`.
  3. Classify each failure: dictionary plumbing, precision, or other.
  4. Revert, and record the table here. The commit is docs only.

  Why re-measure: the old 13/9/4 split predates B1, KI-094/095/096 and the
  projection fix. Its rows also add up to 12, not 13.
- [ ] **0c. `EvidenceMap::args_for` returns a short list when the tail is
  missing.** It counts the answers that are present, not the predicates that
  were raised.
  - Add `raised: HashMap<ExprId, u16>` to `EvidenceMap`, filled from
    `harvest_evidence`'s per-site counter
    (`src/compiler/passes/type_inference.rs`).
  - `args_for` returns `None` unless every index in `0..raised` is present.
  - Test: `a_site_with_a_hole_at_the_tail_refuses_to_build_an_argument_list`.
    It must fail on the old code.
  - `FLUX_DBG_EVIDENCE` prints `raised=n` for each site, and sorts its output so
    the dump is deterministic.
- [ ] **0d. `choose_candidate`'s `Ambiguous | NoMatch => Some(first)`**
  (`src/core/passes/dict_elaborate.rs`) becomes `None`, and the caller reports
  the site.
  - First pin both arms with unit tests. Nothing pins them today.
  - Land this step alone, so any fallout is attributable to it.

### Exit

- The gate is green.
- A test fails on the old `args_for` and passes on the new one.
- No path in `choose_candidate` returns a candidate known not to match.

---

## Phase 1 — one evidence-passing translation

*[0186](../proposals/0186_generics_foundations.md) stage 5, Track C.* **Goal:**
lowering (AST → Core) produces dictionary **arguments** and **parameters** from
`EvidenceMap`, and the six resolution sites are deleted.

It has to be lowering. `CoreExpr` carries no `ExprId`, and the evidence is keyed
by `ExprId`, so no Core pass can read it.

### Syntax

These programs already work. They must keep working, now through a single path:

```flux
fn square<a: Num>(x: a) -> a { x * x }
let d = square(3)                                          // KI-083 shape

fn show_all<a: Enc>(xs: List<a>) -> String { enc(xs) }    // KI-052 shape

fn via<a, b>(x: a) -> b where Convert<a, b> { convert(x) } // multi-parameter class
```

The target Core shape (`--dump-core`):

```
square   = λ__dict_Num. λx. (__dict_Num.mul)(x, x)          -- parameter from the scheme's givens
d        = square(__dict_Num_Int, 3)                         -- DictArg::Global
show_all = λ__dict_Enc. λxs. enc(__dict_Enc_List(__dict_Enc), xs)
                                                             -- DictArg::Applied over DictArg::Param
```

### Steps

- [ ] **1a. Thread evidence into lowering.** No behaviour change.
  - Add `evidence: Option<&EvidenceMap>` to `AstLowerer`
    (`src/core/lower_ast/mod.rs`).
  - Pass it down from `Compiler::lower_core_from_program`.
  - The `src/cfg/` callers pass `None`.
- [ ] **1b. Define how a site is keyed, then write it down.** Two rules, both
  documented in `evidence.rs`:
  - A scheme's predicates are keyed at the **callee identifier's** `ExprId`, not
    at the call's.
  - Marker classes take an index but produce `DictArg::None`, so a dictionary's
    position is its index among the non-marker predicates.

  Then add a helper, `dict_args_at(callee_id, givens) -> Option<Vec<DictArg>>`.

  Verify two more things before 1c
  ([GHC analysis §2, §5](../internals/typeclass_ghc_analysis.md)):
  - A method call inside a constrained body is harvested as `FromGiven`, with
    its superclass path. If it isn't, 1d cannot delete `choose_candidate` until
    that recording gap is closed.
  - Operator predicates are keyed at the operator's own id.
- [ ] **1c. Emit dictionary arguments in shadow mode (C5).**
  - The emitter lives where an **identifier** is lowered, not in the `Call` arm.
    It produces the identifier applied to its dictionaries, and `Call` lowering
    flattens that into one call, `f(d…, x…)`.
  - One emitter then covers a call, a bare reference passed as a value
    (KI-090), and an operator (KI-076). This is GHC's model, where evidence
    wraps the occurrence ([§1](../internals/typeclass_ghc_analysis.md)).
  - Build it *alongside* the existing three `Call` paths.
  - Under `FLUX_DBG_EVIDENCE_DIFF`, report every place the two disagree.
  - Sweep `examples/`, `tests/` and `lib/`. Each divergence is a bug in one path
    or the other: record it and resolve it.
- [ ] **1d. Create dictionary parameters at lowering.**
  - For a `Statement::Function` whose scheme has dictionary constraints, prepend
    one `__dict_*` `Lam` parameter per constraint, taken from the scheme's
    givens.
  - `DictArg::Param { index, path }` then renders as that variable followed by
    `TupleField` projections.
  - Method calls in the body resolve through the same paths, not through
    `choose_candidate`.
  - This replaces the parameter-adding half of `rewrite_constrained_functions`.
- [ ] **1e. Switch over (C5 lands).** Evidence becomes the only source of
  dictionary arguments. A site with no evidence is an internal error that names
  the site (rule 5).
- [ ] **1f. Delete deletion group 1 (C6).**
  - `class_call_type_args`, both copies: `lower_ast/mod.rs` and
    `compiler/expression.rs`.
  - `resolve_dict_args_for_call`, `resolve_dict_args_for_scheme`, and
    `resolve_constraint_type_args` together with its **`Int` default**.
  - `insert_dict_args_at_call_sites`, `resolve_dict_arg`,
    `build_caller_dict_map`, `choose_candidate`.
  - Keep `build_instance_dictionaries` and `build_contextual_dictionary_expr`.
    They build dictionary *values*; they don't choose instances.
  - Leave deletion group 2, the ~920 lines on the AST path, alone. It waits on
    E3's open question: whether the AST bytecode fallback can be retired.
- [ ] **1g. One name per instance.** Every `__dict_*` name is derived from
  `InstanceDef.type_key`. Today these places re-render `type_args` instead:
  - `predeclaration.rs`
  - `codegen.rs`
  - `class_solver.rs`
  - `dict_elaborate.rs`
  - `lower_ast/mod.rs`

**Tracker correction.** C6's list included "the ±dictionaries band in
`check_known_call_arity`". That band was already removed with KI-082.

### Exit

- `rg 'choose_candidate|class_call_type_args|resolve_constraint_type_args' src/core`
  finds nothing.
- Parity passes on all five ways.
- `examples_generics` and `generics_runtime` are re-run and the snapshot diff is
  read. This tests the claim that Track C fixes KI-076, KI-071 and KI-073 at the
  root. Record which ones moved.

---

## Phase 2 — constrained generalize-by-arity

*[0187](../proposals/0187_specialisation.md) stages 2, 1-remainder and 3; 0186
stage 4's remainder.* **Goal:** the release headline.

### Syntax — programs that start working

```flux
fn double(x) { x + x }            // no signature

fn main() with IO {
    print(double(21))             // 42
    print(double(1.5))            // 3.0
}
```

```flux
fn largest(a, b) { if a < b { b } else { a } }                // raises Ord
fn sum_with(f, xs) { fold(xs, 0, \(acc, x) -> acc + f(x)) }  // raises Num

fn main() with IO {
    print(largest("pear", "apple"))
    print(largest(3, 9))
}
```

The inferred schemes appear in `--dump-types` and on hover:

```
double  : forall a. Num<a> => (a) -> a
largest : forall a. Ord<a> => (a, a) -> a
```

Two fixtures move to `working/accepts/`: `b2_unannotated_constrained_double.flx`
and `b2_forwarding_over_constrained_callee.flx`.

### Steps

- [ ] **2a. Clone per instantiation (KI-098, B1's remainder).**
  - Extend B1's `collect_specializations` (`lower_ast/mod.rs`) from one agreed
    instantiation to the *set* of ground instantiations.
  - Emit one clone per instantiation, `f$<type_key>`, and point each call site
    at its clone. A self-call follows the clone it sits in.
  - **Key** each clone on its dictionary arguments *and* on the type arguments
    that change `FluxRep`. `String` and `List<Int>` share a clone; `Int` and a
    boxed type do not. GHC keys on dictionaries only, which cannot fix KI-098
    ([§8](../internals/typeclass_ghc_analysis.md)).
  - **Rewrite call sites directly** through a `(function, key) → clone` map,
    self-calls included. GHC uses rewrite rules here; Flux has none.
  - Re-specialise each clone immediately. A recursive group gets two sweeps, not
    a fixpoint, and a stack of in-progress functions refuses re-entry.
  - **Cap: 3 clones per function** (SpecConstr's default). Past the cap,
    callers use the generic body, and `FLUX_DBG_SPEC` logs it as a missed
    specialisation.
  - Only known global dictionaries count. Dictionary parameters never do, and
    instance dictionary constructors are never cloned.
  - Once Phase 1 has made dictionaries positional, constrained functions can be
    cloned too. In a clone, method calls on its now-global dictionaries become
    direct `__tc_*` calls, and its dictionary parameters disappear.

  **Exit:** the `snapshot_ki_098_despecialised_core_def` snapshot converges on
  the baseline, `::(h#N:Int, t#N:Box)`.
- [ ] **2a′. Settle numeric defaulting before B2.**
  - `decide_quantification` defaults numeric type variables *during inference*
    (`build_numeric_default_subst`, `src/types/quantify.rs`). GHC defaults only
    at the top level ([§6](../internals/typeclass_ghc_analysis.md)).
  - With B2 applied locally, measure whether `fn double(x) { x + x }` comes out
    as `forall a. Num<a> => …` or is defaulted to `Int`.
  - If it is defaulted, restrict defaulting to variables that will not be
    quantified.
- [ ] **2b. B2 — land the generalization.**
  - In `should_generalize_function`, drop the `unconstrained` conjunct.
  - Keep the forwarder guard (`shares_var_with_pending_field_predicate`) and the
    field-receiver pinning.
  - `let` bindings stay `Restricted`.
- [ ] **2c. B4 — one quantification decision per binding group.**
  `infer_binding_group` currently quantifies each member on its own. Follow
  GHC's shape (`tcPolyInfer` + `simplifyInfer`):
  - collect every member's constraints;
  - decide once, over the union of the members' types;
  - trim each member's scheme to the variables and predicates its own type
    reaches.

  That way no member's `forall` names a variable another member left free. Test
  with the pair below, plus a pair where only one member uses the constrained
  variable:

  ```flux
  fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
  fn is_odd(n)  { if n == 0 { false } else { is_even(n - 1) } }
  ```
- [ ] **2d. B3 — `CACHE_EPOCH` → 54.**

### Exit

- `double` works at `Int` and `Float` on both vm and llvm.
- The four DropSpecialized tests in `core_regressions.rs` pass **unmodified**.
- The `my_filter` snapshot in `verify_aether` keeps `FBIP: fip, FreshAllocs: 0`.
- A differential `--no-cache` corpus sweep finds no file newly rejected.

---

## Phase 3 — the High-severity bugs

*Track D and C7.* **Goal:** no open High-severity type-class bug. Re-measure each
bug after Phase 1 first, and fix only what Phase 1 did not.

### Syntax — programs that start working

```flux
// KI-090 — a constrained fn passed as a value
fn twice<a>(f: (a) -> a, x: a) -> a { f(f(x)) }
fn dbl<a: Num>(x: a) -> a { x + x }
print(twice(dbl, 2))                                  // 8
```

```flux
// KI-076 — an operator on a constrained type parameter inside a module
module M {
    public fn bigger<a: Ord>(x: a, y: a) -> a { if x <= y { y } else { x } }
}
```

```flux
// KI-071 — a module fn is not taken over by an instance method of the same name
module M {
    public fn compare(a: Ver, b: Ver) -> Ordering { ... }
    public instance Eq<Ver> => Ord<Ver> { fn compare(x, y) { ... } }
}
```

```flux
// KI-086 — default method bodies in a class declared inside a module
module A {
    public class Greet<a> {
        fn name(x: a) -> String
        fn greet(x: a) -> String { "hello " + name(x) }
    }
}
```

```flux
// KI-073 — result-directed selection through `where`, on native
fn via<a, b>(x: a) -> b where Convert<a, b> { convert(x) }
let s: String = via(42)                               // native prints "42"
```

### Steps

- [ ] **3a. KI-090 (C7).** Eta-expand a constrained function referenced as a
  value into `λxs. f(dicts, xs)`. Build it in 1c's identifier emitter, as its
  not-in-call-position case. GHC needs no lambda here because it has partial
  application; Flux doesn't (0052).
  - Fill `param_types` and `result_ty` from `hm_expr_types` rather than leaving
    them `None`. They drive `FluxRep` selection in closure conversion, the prime
    suspect.
  - Verify on the VM first, then native.
- [ ] **3b. KI-076.** Expected to be fixed by Phase 1. If it isn't, make the
  operator path read evidence.
- [ ] **3c. KI-071.** Lowering must not rebind a name that inference resolved to
  the module function. Consume inference's resolution instead of re-resolving.
- [ ] **3d. KI-086 — default methods as ordinary functions** (GHC's `$dm`,
  [§3](../internals/typeclass_ghc_analysis.md)).
  - Compile each default body once, in the class's module, as an exported
    `__dm_<Class>_<method>` that takes the class dictionary as its first
    parameter. A sibling call inside it resolves from that dictionary, which
    fixes the `E004` half.
  - An instance that omits the method fills its dictionary slot with
    `__dm_<Class>_<method>(__dict_<Class>_<T>)`.
  - `.flxi` records a has-default flag per method, not the body. That fixes the
    cross-module half, and the interface change takes an epoch bump.
  - Pin the runtime half with a *cached* run. `--no-cache` hides it.
- [ ] **3e. KI-073.** Diff native against VM Core/LIR for `syntax_tour.flx` and
  fix the divergence, then un-skip the parity fixture.

### Exit

- All five KIs are marked FIXED, each with the test that pins it.
- Their fixtures are in `working/accepts/`.
- The KI-073 parity fixture is un-skipped.

---

## Phase 4 — closure

*0183 and 0185 stage 4, F3, the release.*

### Syntax — a diagnostic that starts appearing

```flux
class Zero<a> { fn zero() -> a }
instance Zero<Int>   { fn zero() { 0 } }
instance Zero<Float> { fn zero() { 0.0 } }
let x = zero()   // before: runtime E1009 — after: a compile-time ambiguity error with its origin
```

### Steps

- [ ] **4a. E2 / 0183 R6d.** `Disposition` loses `Stuck`. A predicate that
  reaches whole-program scope over an unresolved variable is reported as
  ambiguous, with its origin. `e2_ambiguous_instance_selection.flx` moves to
  `working/rejects/`.
  - Lead with "ambiguous" only when the variable is still unknown, some instance
    could match, and no given applies. Otherwise report "no instance".
  - The message names the variable and the use that raised it, the constraint,
    and a fix that lists the candidate instances
    ([§7](../internals/typeclass_ghc_analysis.md)).
- [ ] **4b. E4.** Close 0183 and move it to `docs/proposals/implemented/`.
- [ ] **4c. F3.** Mark KI-052, KI-061, KI-082 and KI-083 as closed at the root
  by Phase 1.
- [ ] **4d. Release.**
  - Move 0186 and 0187 to `docs/proposals/implemented/`, and 0185 too if every
    stage is done.
  - Update `roadmap_to_1_0_0.md`.
  - Write `whats_new_v0.0.8.md` and the CHANGELOG section.
  - Update guide chapters 09 and 21. Both still say a constrained definition
    needs a signature, and chapter 21's *Current limits* still lists KI-061.

---

## Out of scope for 0.0.8

- 0182's syntax and the Low defects D11–D16. These are 0.0.9.
- 0184 stage 2.
- A2 and A3.
- Retiring the AST bytecode fallback, and with it deletion group 2.
- D10 and D17.

## Verification

- **Every step:** `cargo build` and `cargo test --no-run`.
- **Every phase:** the full gate (rule 9), plus:
  - `cargo run -- parity-check tests/parity --ways vm,llvm`
  - `cargo run -- parity-check examples/guide --compile --ways vm,llvm,vm_cached,vm_strict,llvm_strict`
  - the `examples_generics` and `generics_runtime` snapshots
  - `aether_cli_snapshots` and `core_regressions`
- **Phases 1 and 2:** a differential corpus sweep with `--no-cache`. Compare
  per-file error codes before and after. The diff is the evidence, not the
  absolute count.
- **Phase 1c:** no unexplained `FLUX_DBG_EVIDENCE_DIFF` divergence before 1e
  switches over.
