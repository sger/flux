- Feature Name: `constraint_solver_terminal_states`
- Start Date: 2026-09-03
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Give every wanted class constraint a terminal outcome. Today the solver may
finish with a constraint in `Disposition::Stuck`, and nothing reads that state:
`SolveOutcome::stuck()` has no production caller. A stuck predicate at
whole-program scope is a silent acceptance — the program compiles with an
obligation nobody discharged, and the failure resurfaces at run time as an
`E1009` panic on the native backend. This proposal removes the terminal stuck
state by adopting GHC's structure: quantification *consumes* a predicate,
defaulting resolves what it can, and whatever survives is reported.

## Status
[status]: #status

Work is tracked as seven refactors (R1–R7) on `feat/0183-terminal-constraint-states`,
which is merged into `fix/phase1-promote-e442`. GHC citations are read from the
checkout at commit `2ca87972f6`.

| Item | What it does | Status | Commit | Measured effect |
|---|---|---|---|---|
| Stage 1 | quantification consumes the predicate it discharges | shipped | `e89c3a8c` | residue 13,737 → 4,460 |
| R1 | shape the wanted set as a tree of implications (GHC M1) | shipped | `9dbdf227` | none by design |
| R2 | emit a residual implication instead of deleting (M2) | shipped | `a6cfb322`, `e26c51a9` | none by design |
| R3 | solve each scope against the context it holds; budget exhaustion is its own error (M3) | shipped | `e26c51a9` | `E488` replaces a wrong `E444` |
| R4 | classify on the substituted type, not a flag frozen at emission (M4) | shipped | `728f1764` | residue 4,460 → 3,496; fixes Example B |
| R4a | superclass minimisation (`mkMinimalBySCs`) and given entailment | shipped | `31bf52e0` | `mconcat` loses a redundant dictionary |
| R4b | a body is held to the context its signature declares (`E489`) | shipped | `5a76501c` | residue → 3,106; fixes Example C |
| R4c | a deferred `String` operand discharges its addition obligation | shipped | `b6659cb3` | 3 programs; no spurious `Num<String>` |
| R5 | verified defaulting against the whole group (M5) | shipped, **inert** | `00f53cc6` | **none** — see below |
| R6a | `+` becomes `Flow.Add`, a class `String` instantiates | shipped | `f8d8f585` | no diagnostic change in 1,305 programs |
| R6b | a match arm keeps the scrutinee's type (KI-080) | shipped | `fd4b5338` | stdlib residue 15 → 11; no diagnostic change |
| R6c | a class-method call stops emitting an unbindable instance context (KI-081) | shipped | — | stdlib residue 11 → 9; no diagnostic change |
| — | [0184](0184_field_access_constraints.md) Stage 1: field access emits a predicate | shipped | — | new `E490`; no diagnostic change in 1,305 programs |
| R6 | generalize unannotated definitions, then report what survives (M6) | **2 blockers left** ([KI-082](../known_issues.md#ki-082)) | — | stdlib residue 9 → **0**; 2 of 1,305 programs break |
| R7 | clear the fallout across `lib/Flow`, `examples`, `tests` | not started | — | — |
| — | one `CACHE_EPOCH` bump covering R2 and R4 | not started | — | — |
| — | rewrite this proposal around the GHC study; file Examples A/B/C as `#KI-nnn` | not started | — | — |

### R5 is inert, and that is a finding, not a gap

R5 was written to GHC's `disambigGroup`, and it is correct: it groups unary
obligations per variable, blocks only on non-unary ones, tries `[Int, Float]`
in order, keeps a candidate only if it discharges *every* obligation in the
group, and commits the first survivor.

Traced over all 1,305 `.flx` programs in the repository it **never fires**. Not
because the class environment is missing — it is present, and groups do form —
but because every candidate group is blocked before the defaultable-class test.
The cause is upstream: Flux has no `Num`-polymorphic literal. `1` is `Int`, not
`Num<a> => a`, so the ambiguous numeric variable that defaulting exists to
resolve never arises, and the 3,106 remaining predicates (`Ord`, `Eq`,
`Sendable`) contain no `Num` obligation for defaulting to act on.

Two consequences for this proposal. `Float` in the candidate list is
*unreachable*, not a language change, so it needs no guide-level note. And
running defaulting at whole-program scope as well as binding scope — planned as
the second half of R5 — is provably inert for the same reason and was not
implemented.

### R6's blocker: an unannotated definition is never generalized

Investigating the residue found a single upstream cause, and it is not in the
solver. `finalize_and_bind_function_scheme` (`src/ast/type_infer/function.rs`)
generalizes only a function that declared type parameters:

```rust
let scheme = if !type_params.is_empty() {
    self.finalize_binding_scheme(...)   // generalizes; consumes its predicates
} else {
    Scheme::mono(fn_ty)                 // no generalization at all
};
```

So `fn pick(a, b) { if a > b { a } else { b } }` is bound monomorphically. Its
`Ord<v>` obligation is never generalized, never consumed, and `v` stays one
shared variable across every use. If some call site pins `v` to a concrete
type the predicate solves by accident; if every caller is itself generic, it
survives to whole-program scope and sits stuck forever.

That is the whole residue. Measured on the module fixture in
`/tmp/modtest`, against a stdlib baseline of 15:

| variant | residue |
|---|---:|
| stdlib only (`print(1)`) | 15 |
| + module-private unannotated recursive helper | 17 |
| + the same helper made `public` | 17 |
| + the same helper never called | 17 |
| + a non-recursive unannotated helper | 16 |
| + the helper annotated `fn pick<a: Ord>(..)` | 16 (moves to its unannotated caller) |
| the identical helper at file top level, called at `Int` | 15 (a call site pinned it) |

`lib/Flow/*` is entirely module-wrapped generic code, so it hits this on almost
every helper — which is why a bare `print(1)` already carries 15 stuck
predicates before the user's program is even read.

**The 3,106 figure is a multiple, not a population.** The trace runs per
compiled module, so re-compiling the stdlib for each of ~200 corpus programs
counts the same ~15 predicates again and again. Every count in this document
before this section is inflated the same way and should be read as a relative
measure only.

This blocks R6 as planned. Reporting what survives the fixpoint would reject
the standard library, because these predicates are not defects — they are
obligations that generalization should have consumed and did not. R6 therefore
depends on a decision recorded under Unresolved questions: whether an
unannotated definition should be generalized (full Hindley–Milner, with the
dictionary parameters that implies) or stay monomorphic.

### R6a — `+` is its own class

The first of R6's two prerequisites is done. `+` desugared to `Num`'s `add`,
which forced a choice between rejecting `"a" + "b"` and hard-coding `String`
into the solver — the latter is what shipped in `b6659cb3`, as a built-in rule
keyed on a dedicated constraint origin.

`Flow.Add` replaces that with the ordinary answer:

```flux
public class Add<a> { fn add(x: a, y: a) -> a }
public instance Add<Int> { ... }
public instance Add<Float> { ... }
public instance Add<String> { ... }
```

`Num` names `Add` as a superclass (`public class Add<a> => Num<a>`) and keeps
`sub`, `mul` and `div`, so a function constrained by `Num` alone still admits
`+` through the superclass evidence slot, while `String` gains `+` without
gaining the rest of arithmetic. The `InferredAddOperator` origin and the
solver's built-in string rule are both deleted — the instance does the work.

Verified behaviour:

| program | result |
|---|---|
| `"a" + "b"` | runs |
| `+` on operands still variable at emission | runs |
| `fn cat(a, b) { a + b }` used at `String` | runs |
| a deferred `-` at `String` | `E300` + `E444` |
| `fn twice<a: Num>(x: a) { x + x }` | runs — superclass projection |
| `+` on an ADT (Example B) | `E300` + `E444` |

Zero diagnostic changes across all 1,305 programs, and `test_runner_cli`
(129) and `typeclass_baseline_tests` (48) both pass. `CACHE_EPOCH` is bumped to
42: `Num` loses its `add` slot and gains a leading superclass slot, so every
`Num` dictionary changes layout.

The second prerequisite — record field access emitting a constraint rather than
a hole — is specified as [Proposal 0184](0184_field_access_constraints.md).
R6 is blocked on its Stage 1.

### Generalizing unannotated definitions: attempted, and what stopped it

The blocker above was addressed directly: make every definition generalize,
whether or not it declared type parameters. The patch is kept at
`scratchpad/r6-generalize-unannotated.patch` rather than committed, because it
is not green.

It works, as far as the solver is concerned. Measured on a `print(1)` program,
whose entire residue is stdlib:

| | residue |
|---|---:|
| before | 15 |
| generalize every definition | 6 |
| generalize only the variables a class constraint mentions | 6 |

The remaining 6 were traced to a cause outside the solver entirely. The
implication is built correctly and the instance context *is* in scope; the
body's type variable is simply not the one the instance declared
(`want=[Var(10827)]` against `givens=[MyEq<Var(10821)>]`, with `10827` absent
from the quantified set). The variable is lost in `match`:
`arm_pattern_scrutinee_ty` binds each arm against a *fresh fallback variable*
when the arms disagree on pattern family, discarding the scrutinee's type, so a
pattern-bound variable inside a generic function has no connection to the
function's type parameter. Filed as [KI-080](../known_issues.md#ki-080), with a
reproduction that needs no instances at all, and **fixed**: a pattern family is
decided by the scrutinee's head, so `List<a>` settles it as well as `List<Int>`
does. The stdlib residue falls from 15 to 11.

This is a third instance of the pattern this proposal keeps meeting: a
construct whose typing is completed by later unification rather than by a
constraint. `+` was one (fixed by `Flow.Add`), record field access is another
([0184](0184_field_access_constraints.md)), and match-arm isolation is the
third.

What is not tractable inside this proposal is the fallout. Of 1,305 programs, 5
broke, and 4 of them share one cause. Restricting quantification to the
variables a class constraint mentions — `generalize_constrained_vars`, so that
variables no predicate mentions stay free and keep their existing behaviour —
fixed one (`contact_book.flx`) and left four:

| program | failure |
|---|---|
| `mini_database.flx` | `E430` — `record.id` has no type |
| `function_arg_mismatch.flx` | `E430` — unresolved variable reaches the backend |
| `multi_file_lib.flx` | `E998` — Core lint: duplicate binder in a `Lam` |
| `inline_labels_demo.flx` | `E444` — `Num<String>` from `+` at a call site |

The cause is in `infer_member_access_expression`
(`src/ast/type_infer/expression/access.rs`):

```rust
let object_ty = self.infer_expression(object);
if let Some(field_ty) = self.resolve_named_field_access(&object_ty, member, expr.span()) {
    return field_ty;
}
self.alloc_fallback_var()
```

Field access on a receiver whose type is not yet a known named-field ADT emits
**no constraint** — it allocates a hole. Nothing later can discharge that hole
except unifying the enclosing definition's type with a call site, which only
happens while the definition is bound monomorphically. The adjacent tuple path
says as much in its own comment: it exists so that "later call-site unification
[can] discharge local helper projections". The `+` overload is the same shape:
`Num<a>` at a call site where `a` is `String` is only accepted today because
nothing generalized it.

So generalization is incompatible with every construct Flux types by deferred
unification rather than by a constraint. Making unannotated definitions
generalize therefore depends on giving those constructs real constraints —
row-polymorphic record access for `record.id`, and a class `String` instantiates
for `+`. Both are separate proposals, and both are prerequisites for R6 rather
than parts of it.

### The residue R6 inherits

3,106 terminal stuck predicates over `examples/type_classes` + `tests/parity`,
all but 6 of them `UnresolvedAfterGeneralization`:

| count | class | origin |
|---:|---|---|
| 1,146 | `Ord` | `InferredOperator` |
| 604 | `Eq` | `SchemeUse` |
| 576 | `Eq` | `InferredOperator` |
| 573 | `Eq` | `MethodCall` |
| 116 | `Semigroup` / `Functor` / `Applicative` | `ExplicitBound` |
| 42 | `Sendable` | `SchemeUse` |
| 49 | everything else | mixed |

## Motivation
[motivation]: #motivation

`Disposition::Stuck` was introduced by proposal 0179 Stage 3 to replace a bare
`continue` in the solver. That was the right first move: it made the undecided
set *countable* instead of invisible, and the doc comment on `StuckReason`
already states the intended end state —

> A stuck predicate is *not* an error at binding scope — it is handed back to a
> wider scope. One that is still stuck at whole-program scope is an error,
> which is what stops this variant from becoming a renamed silent drop.

That escalation was never implemented. Measuring the corpus
(`examples/type_classes` and `tests/parity`, 2026-09-03) explains why nobody
could: **13,737** terminal stuck constraints, and even
`fn main() { print(1) }` produced **68**. Escalating that wholesale would
reject every program in the repository. The count was too large to act on, so
the state stayed inert.

The measurement also shows the count was not what it appeared to be. The
dominant shape was:

```
UnresolvedAfterGeneralization   Ord  ExplicitBound  ["?9032"]
UnresolvedAfterGeneralization   Eq   ExplicitBound  ["?10106"]
```

`ExplicitBound` means a constraint the programmer *wrote*, in a signature like
`fn sort<a: Ord>(xs: List<a>) -> List<a>`. Those had already been generalized
into the binding's scheme at `SolveScope::Binding`, where `mark_generalized`
upgrades the disposition correctly. The whole-program pass then re-solved the
same raw wanted list with no memory of that decision, found a residual type
variable, and recorded it unresolved. The predicate was solved twice and judged
on the second pass.

Removing that double-count takes the corpus from **13,737 to 4,443** and the
trivial program from **68 to 22**, without changing what any program means. The
remainder is small enough to classify honestly, which is what makes the rest of
this proposal tractable.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

For a Flux programmer, the visible change is that an obligation the compiler
cannot discharge is reported at compile time rather than surfacing as a runtime
panic on the native backend, or as a silently-wrong dispatch on the VM.

Concretely, this program

```flux
fn main() with IO {
    let ignored = convert(42)
    print(1)
}
```

reports `E459` because nothing fixes `Convert`'s second parameter. It already
does — but for the wrong reason, and this proposal makes the reason principled
(see the monomorphism restriction below).

For a compiler contributor the mental model is:

- A predicate is **solved** when an instance discharges it.
- A predicate is **generalized** when a *definition* quantifies it. That
  discharges it here and moves the obligation to the call sites, which
  elaboration rewrites to pass a dictionary. The predicate is then **gone** from
  the wanted set.
- A predicate is **defaulted** when a rule picks a type for its variable.
- Anything else is **reported**.

There is no fifth outcome. `Disposition::Stuck` remains, but only as a
*binding-scope* state meaning "handed to a wider scope"; it never survives to
the end of the program.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Stage 1 — quantification consumes the predicate

`finalize_binding_scheme` records the source site of every predicate whose
disposition came back `Generalized`, and `resolve_class_constraints` drops
those before the whole-program solve sees them.

The key is `(span.start, span.end, class_id)` — the identity of the site that
emitted the wanted. `Span` is not `Hash`, so the positions are spelled out.

This is sound because a generalized predicate is re-emitted at every use site:
instantiating the scheme produces a fresh `SchemeUse` wanted, which is where the
real check happens. The original is redundant, not load-bearing.

**Consumption is gated on `GeneralizationMode::Definition`.** Only a
definition's obligation transfers, because only its call sites are rewritten to
pass a dictionary. `collect_scheme_constraints` already notes that a nested
binding cannot receive a dictionary parameter — yet it still generalizes
non-operator predicates there. A predicate a `let` "generalizes" is discharged
nowhere, so it must stay visible to the whole-program solve.

This is exactly what Haskell's **monomorphism restriction** exists to prevent,
and the `E459` fixture is the proof: `let ignored = convert(42)` is a pattern
binding with no signature, GHC's MR forbids quantifying it, and GHC reports
*"Ambiguous type variable ‘b0’ arising from a use of ‘convert’"*. Flux was
reaching the same verdict by accident — generalizing (wrong), then reporting via
the double-solve (right, for the wrong reason). Gating consumption on
`Definition` makes the verdict principled without changing it.

Measured effect, whole corpus:

| | before | after |
|---|---|---|
| terminal stuck | 13,737 | 4,443 |
| `fn main() { print(1) }` | 68 | 22 |
| `ExplicitBound` rows | thousands | ~120 |

### Stage 2 — the residue

After Stage 1 the terminal set is:

| rows | reason | origin | disposition |
|---:|---|---|---|
| 2,688 | `NonConcreteOperator` | `InferredOperator` | see below |
| 1,559 | `UnresolvedAfterGeneralization` | `MethodCall` / `SchemeUse` | default, else report |
| ~120 | `UnresolvedAfterGeneralization` | `ExplicitBound` | Stage 1 leak |
| 6 | `SyntheticOrigin` | — | give generated code a real origin |

**`NonConcreteOperator`** should not be a category. `classify_constraint`
returns it whenever `origin == InferredOperator && !originated_from_concrete_type`,
before looking at the type at all. The measurement contains

```
NonConcreteOperator   Num  InferredOperator  ["Int"]
```

— a concrete `Int`, with an obvious `Num Int` instance, stuck only because the
flag was false when the constraint was emitted. In GHC an operator's wanted is
an ordinary wanted with a `CtOrigin`; nothing about arising from `+` changes how
it is solved. The fix is to classify on the type as it stands after
substitution, not on how it was born.

**The ~120 residual `ExplicitBound` rows** are `Semigroup`, `Functor` and
`Applicative` — higher-kinded classes, where `mark_generalized`'s exact
`type_args` equality evidently fails to match the scheme constraint. That is a
matching bug in Stage 1, not a separate category.

**`SyntheticOrigin`** is decided by `constraint.span == Span::default()`, a
proxy for "compiler-generated". GHC attaches a real `CtOrigin` at constraint
creation instead of inferring one from a missing span. Giving Flux's generated
constraints a proper origin removes the row.

### Stage 3 — defaulting, then report

What survives Stage 2 is genuine ambiguity. GHC's `simplifyTop` ends with
`applyDefaultingRules`, then `reportUnsolved`. Flux already has
`build_numeric_default_subst` at binding scope; whole-program scope needs the
same treatment followed by a report. Only then does the escalation the 0179 doc
comment promised become possible, and by then the number it applies to is small
and understood.

## Drawbacks
[drawbacks]: #drawbacks

Stages 2 and 3 will reject programs that compile today. That is the intent —
those programs have obligations nobody discharges — but the fallout is real and
lands in the stdlib first, since `lib/Flow/*` is where the generic code lives.

Stage 1 is not subject to this: it changes no program's meaning, only how many
times a predicate is examined.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not escalate terminal `Stuck` to an error directly?** That was the plan
in 0179 and the measurement is the argument against it: 68 errors on
`print(1)`. Escalation is the *last* step, not the first, and it is only
affordable after the double-count and the mis-classification are gone.

**Why not keep `Stuck` as a permanent, documented state?** Because nothing
reads it. A state with no consumer is a silent drop with better paperwork —
which is precisely what the `StuckReason` doc comment says it must not become.

**Why gate consumption on `Definition` rather than adopting a full
monomorphism restriction?** The MR's costs are well known and it is one of
Haskell's most-disliked corners. Flux needs its *effect* in exactly one place:
don't treat a predicate as discharged when no call site will pass its
dictionary. Gating on the mode buys that without inflicting the MR's surprises
on ordinary `let` bindings.

## Prior art
[prior-art]: #prior-art

GHC has no terminal stuck category. `TcSimplify.simplifyTop` runs the solver to
a fixed point and every residual wanted takes one of four exits:

1. **Defaulting** — `applyDefaultingRules`, covering the `default` declaration,
   `ExtendedDefaultRules`, and `OverloadedStrings`.
2. **Quantification** — `decideQuantification` / `growThetaTyVars` decides which
   predicates join the signature, and **removes** them from the wanted set.
   This is THIH's `split`, and it is the step Flux was missing.
3. **Report** — `TcErrors.reportUnsolved`, producing `No instance for (C t)`,
   `Ambiguous type variable ‘a0’ arising from…`, or `Could not deduce (C a)
   from the context…`.
4. **Deliberate deferral** — `-fdefer-type-errors`, opt-in, and it materialises
   the error as a crashing runtime term rather than dropping it.

GHC does have internal stuck states (`Irred`, and stuck type families), but
they are solver states, never program outcomes; anything surviving to the top is
reported.

The second lesson is about *reasons*. GHC does not tag a constraint with why it
went undecided. It carries the constraint's **origin** (`CtOrigin`) and its
**provenance** (`CtLoc`, with implication nesting), and derives the explanation
at report time — which is how it produces "arising from a use of `fmap` at
Foo.hs:25:5" and "from the context: Monad f". Flux's `StuckReason` decides the
reason at solve time, which is why `NonConcreteOperator` can outrank the actual
type: the tag was chosen before the type was looked at.

THIH (Jones, *Typing Haskell in Haskell*) supplies the underlying frame:
`split` partitions deferred from retained predicates, and `toHnfs`/`byInst`
perform context reduction. Flux implements `split` and, since
`reduce_to_head_normal_form` landed, context reduction; the missing piece is
that `split`'s retained half must *leave* the wanted set.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Which defaulting rules does Flux want at whole-program scope? Numeric
  defaulting exists at binding scope; whether Flux wants an
  `ExtendedDefaultRules` equivalent for `Show`/`Eq`/`Ord` is a language
  decision, not a bug fix.
- Should there be a `-fdefer-type-errors` equivalent? It is the only principled
  way to keep today's permissiveness available, and it would make the
  escalation in Stage 3 much easier to land.
- Does `SchemeUse` at a polymorphic call site need its own treatment, or does
  Stage 1's consumption in the *caller's* binding scope already cover it? The
  measurement suggests the latter but does not prove it.

## Future possibilities
[future-possibilities]: #future-possibilities

Replacing `StuckReason` with a GHC-style `CtOrigin` plus provenance chain would
let diagnostics say *why* a predicate exists ("arising from a use of `fmap`"),
which is the single biggest quality gap in Flux's class errors today. That is
also the groundwork for the narrative type errors deferred out of 0173.

Once every predicate has a terminal outcome, the dictionary-passing path can
drop its remaining defensive fallbacks — the dispatch stub whose body is
`panic("No instance …")` exists only because a constraint can reach codegen
undischarged.
