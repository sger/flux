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

### Where it lives: `crates/flux-generics/`

The four changes above are one subject, and today that subject is smeared across
`ast/type_infer/`, `types/`, `core/passes/` and `compiler/`. It gets its own
**workspace crate**, not a module.

A module would leave the boundary to review, which is what the rest of this
document argues has already failed: the six resolution sites are held in
agreement by hand-written comments saying they "must stay in lockstep", and
KI-085 is what happened when they did stay in lockstep with each other and not
with inference. A crate boundary is checked by the compiler on every build.

```
crates/flux-source/        Symbol, Interner, Span, Position          (no deps)
crates/flux-diagnostics/   Diagnostic, error codes, rendering        -> flux-source
crates/flux-generics/                                                -> both
├── lib.rs          — the public surface: what inference and lowering call
├── scc.rs          — generic iterative Tarjan, ordered successors
├── types/          — the type and class vocabulary, moved from src/types/
├── solver/         — class_solver, class_disposition, Evidence
├── quantify.rs     — the single quantification decision (GHC's decideQuantification)
├── evidence.rs     — the recorded solver decision: wanted constraint → evidence
└── translate.rs    — the dictionary-passing translation, the only emitter

flux (root crate)          -> flux-generics, flux-diagnostics, flux-source
└── src/generics_frontend/ — AST → binding plan; the renamer-equivalent
```

The two supporting crates exist because `flux-generics` must be able to name a
symbol and report an error without depending on the compiler. They are pure
extractions and carry no design of their own.

Dependency direction is one-way and now enforced mechanically: `flux-generics`
cannot reach into `compiler/`, `cfg/`, `bytecode/` or `llvm/` because it does
not depend on the crate they live in. A backend receives a program whose
evidence is already explicit, and has no say in it.

**Binding-group *graph construction* stays in the root crate**, in
`generics_frontend`, because it walks the AST; only the graph algorithm
(`scc.rs`) moves. This is GHC's split exactly — dependency analysis lives in the
renamer, and the solver never sees surface syntax.

`scc.rs` is deliberately first and deliberately standalone: it is a graph
algorithm over names, it needs nothing from the type checker, and it can be
tested on its own before anything else moves.

### Staging

Each stage is independently landable and independently testable. No stage
depends on a later one being designed correctly.

Stages 0a–0d are preparation: two pure crate extractions, then two design fixes
made **in place**, in the root crate, where the tree stays green. They come
first because `src/types/` cannot cross a crate boundary while the class
environment stores surface syntax and executable code — moving it in that state
would mean widening a dozen private helpers to `pub` and extracting nine
`Statement`-reading collection functions, churn the design fixes would then
undo. With them done, stage 0e is a move rather than surgery.

This table is the single source of truth for sequencing. An earlier draft
numbered these 1–8 with the crate move second; that ordering did not survive
contact, because `src/types/` cannot cross a crate boundary while the class
environment stores surface syntax and executable code. The preparation stages
below were introduced to fix that first, and everything after shifted.

| stage | status | change | exit |
|---|---|---|---|
| 0a | done | `crates/flux-source`: Symbol, Interner, Span, Position | suite green; pure move |
| 0b | done | `crates/flux-diagnostics`: the diagnostics module | suite green; pure move |
| 0c | done | Default method bodies out of `MethodSig` into a side table (`ClassBodies`, widened to `ClassSurface` in 0d) | suite + parity green; behaviour identical |
| 0d | partial | Class method signatures converted `TypeExpr` → `InferType` at collection. Done: `MethodSig.infer_type`, `match_type` deleted. Remaining: thread `&ClassSurface` into the eight surface consumers and drop `MethodSig`'s `TypeExpr` fields — tidiness, **not** a prerequisite for 0e, since `InstanceDef.type_args` keeps `TypeExpr` in the class environment regardless (131 structural reads). | suite + parity green |
| 0e | | `crates/flux-generics` + the `src/types/` move; `register_prelude_classes` stays behind | suite + parity green; pure move |
| 1 | done | `scc.rs`: iterative, ordered, generic; delete both existing Tarjans | determinism under permuted input; 10k-node chain does not overflow |
| 2 | done | `generics_frontend::plan`; wire Core lowering to consume binding groups | mutual recursion across an intervening `let`; suite green |
| 3 | done | Bump `CACHE_EPOCH` 44 → 45 **before** the red middle, not after | a stale artifact cannot survive stages 4–5 |
| 4 | next | `quantify.rs`: one decision returning quantified vars *and* retained context; MR becomes a parameter | the 0185 `E490` regression cannot recur by construction |
| 5 | | `evidence.rs` + `translate.rs`: solver records evidence; delete the other resolution sites | the forwarding reproduction compiles and runs |
| 6 | | Land 0185's generalize-by-arity rule on the new foundation | stdlib residue **0**; suite + parity green |

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
