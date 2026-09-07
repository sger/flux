- Feature Name: `generics_foundations`
- Start Date: 2026-09-06
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Rebuild the foundation Flux's generics rest on. Today a definition is
generalized **where it is written**, in source order, and the evidence its call
sites need is assembled afterwards by several passes that each re-derive it.
That order is backwards, and it is the direct cause of a run of defects —
KI-052, KI-061, KI-082, KI-083 and the dictionary-forwarding bug recorded below
— that look unrelated but are one bug wearing different clothes.

This proposal replaces that with the arrangement Haskell has used since 1990:
**dependency analysis into binding groups, generalization one group at a time,
and a single evidence-passing translation** that is the only thing in the
compiler permitted to decide what a call site passes.

It supersedes [0185](0185_generalize_by_arity.md) Stage 3. 0185's shipped
stages (0, 1, 2) stand, and its measurements are reused.

## Motivation
[motivation]: #motivation

### What the code does now

Top-level inference is two passes over the statement list
(`src/ast/type_infer/statement.rs`):

- **Phase A** binds every top-level function name. A function with a complete
  explicit signature gets its declared polymorphic scheme. A function without
  one gets `Scheme::mono(fresh_var)` — a placeholder that carries **no class
  constraints**.
- **Phase B** infers each statement in **source order**, and
  `finalize_and_bind_function_scheme` generalizes each function at its own
  definition site.

Evidence is then attached by later passes: `lower_ast` emits dictionary
references, `dict_elaborate` inserts dictionary parameters and rewrites call
sites, and the bytecode compiler resolves instances again. Proposal 0183
counted **four** places that decide which instance a call uses, kept in
agreement by comments saying they "must stay in lockstep".

### Why that ordering cannot work

A reference to a function whose scheme is decided *later* — a forward
reference, self-recursion, or mutual recursion — is resolved against the Phase A
placeholder, which has no constraints and therefore no dictionary parameters.
When that function is subsequently generalized *with* dictionary parameters,
every reference already resolved against the placeholder disagrees about the
callee's arity. Nothing detects the disagreement, because the two decisions are
made in different passes with no shared record.

That is not a hypothesis. It reproduces on the shipped compiler, with explicit
bounds and no generalization changes at all:

```flux
fn step<a: Num>(p: a, q: a) -> a { p - q }
fn go<a: Num>(x: a) -> a { step(x, x) }
fn main() with IO { print(go(10)) }
```

```
error[E004]: Undefined Variable
I can't find a value named `__dict_m8_466C6F772E4E756D_Num_Int`.
```

`go` must pass its own dictionary to `step`. The evidence is never declared,
because `phase_predeclaration` reads the demanded classes off signatures
*before inference has run* — and even an explicitly annotated `go` has no
scheme in `type_env` at that point.

### The same bug, five times

| | symptom | what was actually missing |
|---|---|---|
| KI-052 | call receives `None` | dictionary named before the parameter holding it existed |
| KI-061 | evidence missing across a module boundary | `__dict_*` not declared in the importing unit |
| KI-082 | arity error escapes to run time | evidence count guessed from a `±dictionaries` band |
| KI-083 | `E1001` on a top-level call | evidence stored after the code that reads it |
| *(this proposal)* | `E004` / `E1001` on a forwarded call | evidence never declared for an inferred constraint |

Every one is "the evidence a call needs was not available, in the right form,
at the point the call was compiled". They have been fixed one at a time, each
fix local to the pass that happened to surface it. The supply of them is not
exhausted, because the thing generating them — several passes independently
re-deriving evidence that no single pass owns — is still there.

### Why now

0185 Stage 3 (generalize by arity) is the correct rule and its benefit is
measured: stdlib terminal residue **9 → 0**. But an attempt to land it against
the current architecture produced, in one sitting: a lost `E490` diagnostic, a
runtime miscompilation (`E1009`, field access lowered to an index into
uninitialised memory), a native-backend HTTP 500, and the forwarding bug above.
Generalizing by arity does not create these. It makes nearly every helper
constrained, so it converts *latent* evidence bugs into *universal* ones.

The rule cannot land until the foundation under it can carry evidence
correctly. That foundation is what this proposal builds.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Nothing in the surface language changes. A Flux programmer writes the same
code; the difference is that the compiler decides *once*, in one place, what
each generic call passes, instead of four passes agreeing by convention.

Two user-visible consequences follow, both improvements:

- A function that uses a class operator without annotating its parameters
  becomes properly polymorphic instead of accidentally monomorphic, so it can
  be reused at more than one type.
- Mutually recursive functions are inferred as a group, so they can share
  inferred constraints instead of one of them silently pinning the other.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### How Haskell does it

GHC's arrangement, in the order it runs (`GHC/Tc/Gen/Bind.hs`,
`GHC/Tc/Solver.hs`, `GHC/Core/Opt/Specialise.hs`):

1. **Dependency analysis.** The renamer builds a call graph over a `let`'s
   bindings and computes its strongly-connected components. `tcBindGroups`
   receives the SCCs in topological order. This is the step Flux has no
   equivalent of.

2. **One group at a time, monomorphic inside.** Within a group, every member is
   bound at a *monotype* while the group's bodies are checked, so a recursive
   or mutual reference uses the same type the definition is being given —
   `Note [Polymorphic recursion]`. Only after the whole group is checked is it
   generalized. A group whose members are all annotated skips the monomorphic
   stage, which is how polymorphic recursion is expressible at all: with a
   signature, and only with one.

3. **Quantification is decided once**, by `decideQuantification`, which returns
   the quantified variables *and* the retained context together. The
   monomorphism restriction (`checkMonomorphismRestriction`) is one input to
   that decision, not a separate rule applied elsewhere: a binding with
   arguments is generalized, a nullary one is restricted.

4. **Evidence is a term, produced by the solver.** The constraint solver emits
   `EvBind`s — actual bindings of evidence variables to dictionary expressions —
   and `TcEvidence` records, for every wanted constraint, the evidence that
   discharged it. The desugarer (`GHC/HsToCore/Binds.hs`) turns quantified
   constraints into lambda-bound dictionary parameters and wanted constraints
   into applications of the evidence the solver already chose. **The desugarer
   does not re-solve anything.** There is exactly one instance-resolution
   decision per constraint, made by the solver, recorded, and later consumed.

5. **Specialisation is a separate, later, optional pass.** `SPECIALISE` and
   `-fspecialise` clone a generic function at a concrete dictionary and drop
   the parameter. Correctness never depends on it; it exists purely to recover
   the cost of the dictionary that step 4 introduced.

The load-bearing idea is step 4: **the solver decides, and everything
downstream reads its decision**. Flux currently has the opposite — the solver
produces `Evidence` that (per 0183) *nothing reads*, and three later passes
each re-derive what it already worked out.

### The proposed architecture

Four changes, in dependency order. Each is independently testable.

#### A. Binding groups

Add dependency analysis over top-level and `let`-bound functions: build the
reference graph, compute SCCs, order them topologically, and infer group by
group. Replace Phase A's `Scheme::mono(fresh_var)` placeholder with the real
rule — every member of the group being checked is bound at a monotype for the
duration of that group, and generalized when the group closes.

Flux already has SCC and topological-sort machinery for two other graphs
(`syntax/module_graph`, `compiler/module_constants/dependency.rs`); this is a
third consumer, not new algorithmic work.

This alone fixes the class of bug where a reference is resolved against a
scheme that later changes, because within a group no scheme changes after a
reference is resolved, and across groups the callee is already generalized.

#### B. One quantification decision

Fold the quantification decision into a single function that returns the
quantified variables and the retained constraints together, as
`decideQuantification` does. Today `finalize_binding_class_constraints`
computes the quantified set internally for `split`, and the caller separately
recomputes a set to build the scheme's `forall`. Those two can disagree, and
when they do a predicate is classified "generalized" by one and dropped by the
other, so nothing reports it.

The monomorphism restriction becomes an input to that one function — which is
where 0185's "generalize by arity" rule lands, as a *parameter* rather than a
separate site.

#### C. Evidence as a recorded decision

Make the solver's evidence the single source of truth, and give it somewhere to
live: a map from wanted constraint to the evidence that discharged it, keyed so
that Core lowering can consume it. Then:

- `lower_ast` stops emitting dictionary references by name before the
  parameters exist (the KI-052 shape).
- `dict_elaborate` stops re-deriving instances and becomes a mechanical
  translation: quantified constraint → dictionary parameter, wanted constraint →
  the recorded evidence.
- The bytecode compiler's third copy of instance resolution is deleted.

This is the change that retires the "must stay in lockstep" comments, and with
them the bug family in the table above.

#### D. Evidence availability is a property of the translation

Once evidence is a recorded decision rather than a name resolved late, the
questions "is `__dict_C_T` declared here", "is it stored before the code that
reads it", and "is it complete" stop being separate hazards handled in
`predeclaration.rs` and `codegen.rs`. A dictionary is either the evidence the
solver recorded — in which case the translation emits it — or it is not needed.

### What this deletes

- `predeclare_instance_dictionary_globals`'s guess at which classes are
  "demanded", and its dependence on signatures visible before inference.
- The `±dictionaries` band in `check_known_call_arity` (0185 Stage 2 narrowed
  it; this removes the need for it, because arity is known from the
  translation).
- The AST twin of `current_context_dictionary`.
- Two of the four instance-resolution sites.

### Where it lives: `src/` — the crate boundary, reconsidered

**This section records a decision that was reversed.** The original design put
this subject in a workspace crate, on the argument that a module leaves the
boundary to review and review is what already failed — the six resolution sites
are held in agreement by hand-written comments saying they "must stay in
lockstep", and KI-085 is what happened when they stayed in lockstep with each
other and not with inference.

Stages 0a and 0b executed that plan: `crates/flux-source` and
`crates/flux-diagnostics`, both pure moves, both green. Stage 0e — the crate the
proposal is named for, plus the `src/types/` move — was abandoned, and 0a/0b
were then folded back. `src/types/` now holds `quantify.rs`, `evidence.rs` and
`translate.rs`; the vocabulary lives in `src/source/`, `src/diagnostics/` and
`src/shared/scc.rs`; and the workspace is the root crate plus `crates/flux-lsp`.

Three findings overturned the argument:

1. **`src/types/` cannot be moved cheaply.** Measured, not estimated: 149
   production references into `syntax`, two-thirds in code that should not move
   at all. Not a pure move — surgery, on the axis this work is most likely to
   break.

2. **A crate boundary enforces dependency *direction*, and nothing else.** Two
   of the architecture rules this proposal wanted mechanised are about
   *contents* — "`bytecode/` must not gain compile-time or execution logic",
   "`shared_ir/` is ID plumbing, not a pipeline stage" — and no crate graph can
   express either. The two directional rules were then measured and found to be
   one edge each: `llvm → bytecode` is `hash_bytes`, `syntax → core` is
   `CorePrimOp`. Two edges is a guard test, not a workspace split.

3. **Each extracted crate had exactly one consumer.** `flux-lsp` depends on
   `flux`, not on any of them, so the boundary constrained a single client — and
   a guard test constrains every module in `src/`, not just three.

What settled it was stage 2's own regression. Every commit on this branch passed
a full suite plus a parity sweep while a nested forward reference miscompiled to
`E1001 ... (got Uninit)`. No boundary would have caught it: the defect is an
ordering inside one function, not a dependency anyone crossed. The scarce thing
is tests that assert behaviour, not tests that assert shape.

`flux-generics` is the cautionary case in miniature. It shipped as 342 lines
holding one Tarjan function — whose second consumer turned out to be the borrow
checker — under a `lib.rs` advertising "binding groups, quantification, evidence
and the dictionary translation". The name described the plan; the contents were
a graph algorithm. It is now `src/shared/scc.rs`, and the naming rule that came
out of it is applied to its neighbour: the AST-walking half is
`src/binding_groups.rs`, named for what is in the file rather than for what may
one day join it.

**What survives unchanged.** The layering the crate was meant to enforce is
still the design, and still correct:

```
src/source/          Symbol, Interner, Span, Position          (depends on nothing)
src/diagnostics/     Diagnostic, error codes, rendering        -> source
src/shared/scc.rs    generic iterative Tarjan, ordered successors
src/types/           type + class vocabulary, the solver, quantify / evidence / translate
src/binding_groups.rs  AST -> binding plan; the renamer-equivalent
```

Dependency direction is one-way, the solver still never sees surface syntax, and
a backend still receives a program whose evidence is already explicit and has no
say in it. **Binding-group graph construction stays out of the solver** because
it walks the AST — GHC's split, where dependency analysis is the renamer's job.
The difference is that this is now asserted by a guard test over module imports
rather than by the crate graph, which costs one file instead of 149 call sites
and covers the two contents rules a crate graph could never reach.

### Staging

Each stage is independently landable and independently testable. No stage
depends on a later one being designed correctly.

Stages 0a–0d are preparation. 0a and 0b were pure crate extractions and have
since been folded back into `src/` (see above); they are left in the table
because 0c and 0d were sequenced behind them. 0c and 0d are design fixes made
**in place**, in the root crate, where the tree stays green.

This table is the single source of truth for sequencing. Two earlier orderings
did not survive contact. The first numbered these 1–8 with the crate move
second; that failed because `src/types/` cannot cross a crate boundary while the
class environment stores surface syntax and executable code, so the preparation
stages were introduced to fix that first. The second kept 0e — the crate move
itself — and that is now **withdrawn**: the boundary is a guard test instead.

| stage | status | change | exit |
|---|---|---|---|
| 0a | reverted | `crates/flux-source` — extracted, then folded back to `src/source/` | suite green; pure move, both ways |
| 0b | reverted | `crates/flux-diagnostics` — extracted, then folded back to `src/diagnostics/` | suite green; pure move, both ways |
| 0c | done | Default method bodies out of `MethodSig` into a side table (`ClassBodies`, widened to `ClassSurface` in 0d) | suite + parity green; behaviour identical |
| 0d | done (descoped) | Class method signatures converted `TypeExpr` → `InferType` at collection. Done: `MethodSig.infer_type`, `match_type` deleted. Remaining work **descoped**: threading `&ClassSurface` into the eight surface consumers buys nothing this proposal needs, because `InstanceDef.type_args` keeps `TypeExpr` in the class environment regardless (131 structural reads). The goal was to get executable code and surface syntax out of `MethodSig`, and that is done. | suite + parity green |
| 0e | withdrawn | `crates/flux-generics` + the `src/types/` move. Not a pure move: `src/types/` holds 149 production references into `syntax`. Replaced by a guard test over module imports — see *Where it lives*. | — |
| 1 | done | `scc.rs`: iterative, ordered, generic; delete both existing Tarjans | determinism under permuted input; 10k-node chain does not overflow |
| 2 | done | `binding_groups`; wire Core lowering to consume binding groups | mutual recursion across an intervening `let`; suite green |
| 3 | done | Bump `CACHE_EPOCH` 44 → 45 **before** the red middle, not after | a stale artifact cannot survive stages 4–5 |
| 4 | partial | `quantify.rs`: one decision returning quantified vars *and* retained context; MR becomes a parameter. Done: `decide_quantification` computes the quantified set once (it was derived at four sites) and `Quantified::into_scheme` consumes that set rather than re-deriving it; `GeneralizationMode` becomes `MonoRestriction` with `monomorphism_restriction(arity, has_signature)` as the rule. Done: `infer_binding_group`, so all four statement passes walk the same plan. Remaining: one quantification decision *per group* rather than per member — this is what retires `finalize_and_bind_function_scheme` and `refine_unannotated_self_recursive_return`.

**`refine_unannotated_self_recursive_return` cannot be deleted on its own.** It was tried (and reverted): predeclaration already links a self-call to its definition, so the pass looked dead, and unannotated `fact` / `sum_to` / a function whose return type comes only from its recursive call all stayed correct without it. What it actually carries is *precision*, not correctness — it re-infers the body against fully substituted parameter types and keeps the more concrete of the two answers. Removing it dropped `:Box` binder annotations throughout the Aether Core dumps (`aether__drop_spec_recursive`, `drop_spec_branchy`, and six more), because recursive functions over lists lost the second pass's refinement. Tying the predeclared slot to the inferred type does not recover it, and leaks the declared effect row: `List.map(non_empty_lines(text), from_line)` in `lib/Flume/Schema/Index.flx` became `expected () -> Unit, found () -> Unit with Fail`. The precision has to come from the group's own quantification decision before the pass can go. | the 0185 `E490` regression cannot recur by construction |
| 5 | next | `evidence.rs` + `translate.rs`: solver records evidence; delete the other resolution sites. The forwarding reproduction already compiles and runs on this branch, so [KI-090](../known_issues.md#ki-090) is the exit criterion instead — a constrained function referenced *as a value* never has its dictionary applied. **Fixing it locally was attempted and is impossible**, which is the strongest available argument for the evidence map: eta-expanding the reference is the right shape, but `resolve_dict_arg` can only answer for a concrete predicate or one the caller already holds, and at a reference the scheme says `Num<a>` while the instantiation to `Num<Int>` lives at the use site. Core cannot supply it — `CoreVarRef` is a name and a binder id — so the evidence has to be recorded during inference and carried. | KI-090's reproduction runs; the six sites are gone |
| 6 | moved out | Generalize-by-arity now lives in [Proposal 0187](0187_specialisation.md), behind the `core/` specialisation pass it requires. It was implemented here and reverted: the rule is correct, but an unannotated helper becomes constrained and loses its specialised lowering, failing 16 tests that assert an optimisation still fires. A prerequisite outside this proposal should not hold a stage inside it. | — |

### Stage 0e, measured

Two-thirds of the `TypeExpr` use in `src/types/` is code that should not move
at all: `class_dispatch.rs` (34 refs) synthesises `Statement::Function`, and
`class_surface.rs` (9) holds surface syntax by definition. The blocker is the
residue.

1. **`InstanceDef.type_key`** — done (`6eb113c9`). Every `__dict_*` and `__tc_*`
   symbol is derived from the instance head, and four sites re-rendered it from
   `TypeExpr`. Holding the key means changing the representation cannot
   silently rename every dictionary in a program.
2. **`InstanceDef.type_args: Vec<TypeExpr>` → `Vec<InferType>`** — **56
   compile errors**, 41 of them in `class_env.rs`. Smaller than the 131
   `.type_args` reads suggested, because most of those are on
   `SchemeConstraint`, which is already `InferType`. Two decisions this forces:
   - instance head type parameters need a numbering convention, the same one
     stage 0d gave method signatures (class parameter `i` is `TypeVarId(i)`);
   - `PublicInstanceEntry.type_args` (`module_interface.rs:113`) is part of the
     `.flxi` format. Convert at the boundary and the format is unchanged, so no
     further epoch bump is owed.

   The payoff is concrete: `match_instance_type_expr` — four call sites, the
   last `TypeExpr`-pattern matcher — is replaced by `match_infer`, which stage
   0d already wrote and which currently has one caller.
3. **Move `class_dispatch.rs` and `class_env`'s collection half** into the root
   crate. Mechanical.
4. **Move what remains** into `flux-generics` and re-export.

Measurement note: from stage 2 on, validation must **run** programs, not only
compile them. The compile-only sweep used during the 0185 attempt reported
neutral while a runtime miscompilation was present. `cargo test` and
`parity-check` execute; that is why they are the gate.

## Drawbacks
[drawbacks]: #drawbacks

This is a structural change to the middle of the compiler, and it touches
inference, Core lowering and both backends' inputs. It is substantially more
work than 0185 Stage 3 as originally scoped, and it cannot be landed in one
commit.

It also introduces a real cost the current accidental monomorphism hides: an
unannotated helper that uses a class operator becomes constrained and takes a
dictionary parameter. Measured on the VM, arithmetic still fuses
(`OpSubLocals`), but the function gains an argument. GHC pays the same cost and
recovers it with specialisation, which Flux does not have and which this
proposal does not add.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Continue patching.** Rejected on evidence: five bugs of one shape have been
fixed individually, and attempting the sixth (0185 Stage 3) produced four more
in a single sitting, including a silent miscompilation. The defect rate is a
property of the architecture, not of the individual fixes.

**Adopt A and B but not C.** This would fix recursion and the quantification
disagreement while leaving evidence re-derived in three places. It is a
legitimate smaller step and is how the staging below is ordered — but it leaves
the bug family intact, so it is a stopping point, not a destination.

**Monomorphise instead of passing dictionaries.** Whole-program monomorphisation
(the Rust/C++ model) removes evidence entirely. It is a bigger change than this
one, is hostile to separate compilation, and conflicts with Flux's module
caching (`.flxi` interfaces, `CACHE_EPOCH`). Not pursued.

## Prior art
[prior-art]: #prior-art

- Peyton Jones et al., *Type classes: an exploration of the design space* — the
  dictionary-passing translation this proposal makes explicit.
- Jones, *Typing Haskell in Haskell* — `split`, which Flux already implements;
  this proposal supplies the binding-group analysis THIH assumes around it.
- GHC `Note [Polymorphic recursion]`, `Note [Choosing the best method binding]`,
  `decideQuantification`.
- Flux's own [GHC comparison](../internals/typeclass_vs_ghc.md), which
  identified generalization and instance resolution as the two divergent seams.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Does evidence need to survive into `.flxi` interfaces, or can a cross-module
  call re-solve from the callee's published scheme? This decides whether
  `CACHE_EPOCH` bumps once or per stage.
- How much of `dict_elaborate` survives as a translation once it stops
  resolving instances.
- Whether the compile-only sweep can be extended to *run* programs. Every
  Stage 3 measurement taken so far was compile-only, which is why a runtime
  miscompilation survived a 1,366-program sweep reported as neutral. This
  proposal should not be measured with the same instrument.

## Future possibilities
[future-possibilities]: #future-possibilities

- Specialisation, to recover the dictionary cost (GHC's `-fspecialise`).
- Polymorphic recursion with a signature, which falls out of B for free.
- Associated types and multi-parameter classes, which need a solver whose
  evidence is a first-class recorded term — i.e. C.
