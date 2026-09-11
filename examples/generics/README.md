# Generics Examples

What Flux generics supports today, and what it does not. Every file here is
pinned by a snapshot, so a gap that gets fixed — or a feature that regresses —
shows up as a diff rather than as prose nobody re-checked.

The release split this corpus is measured against
(`docs/roadmaps/generics_tasks.md:9`):

> **0.0.7 is generics. 0.0.8 is type classes.** Generics is *which definitions
> get quantified, over what, and in what order*. Type classes is *what a call
> site is handed*.

## How to read the directories

| Directory | Means | Pinned by |
|---|---|---|
| `working/accepts/` | compiles, runs, right answer | `examples_generics` snapshot + `tests/flux/generics.flx` on VM and native |
| `working/rejects/` | correctly **rejected** — the diagnostic is the feature | `examples_generics` snapshot |
| `failing/compile/` | a gap that surfaces as a compile error | `examples_generics` snapshot |
| `failing/runtime/` | compiles clean, then misbehaves at run time | `generics_runtime_fixtures_snapshot` (VM only) |

The split between the last two is not taxonomy for its own sake. The
`examples/` snapshot harness is compile-only, so a gap whose symptom is at run
time records as `ok` with no diagnostics and pins nothing — compare
`generics__failing__runtime__ki_090_constrained_fn_as_value.snap`, which says
`ok`, with the real symptom in `tests/snapshots/generics_runtime/`.

## Supported

### Quantification — what 0.0.7 changed

| File | Construct |
|---|---|
| `infer_identity_two_types.flx` | **G5:** an unannotated definition with parameters and no constraints is generalized |
| `infer_arity2_take_first.flx` | the same rule at arity 2 |
| `infer_higher_order_apply.flx` | unannotated higher-order over an unconstrained callee |
| `infer_let_lambda_polymorphic.flx` | a `let` bound to a lambda is generalized, nested and top-level |
| `infer_binding_group_order.flx` | **R1:** groups emitted in dependency order, not source order |
| `recursion_mutual_generic.flx` | **KI-096:** a mutually recursive group unifies with its predeclaration |
| `annot_polymorphic_recursion.flx` | recursion at a different instantiation, legal with a signature |

### Annotated generics and constraints

| File | Construct |
|---|---|
| `annot_identity_signature.flx` | `fn identity<a>(x: a) -> a` |
| `annot_wrap_option.flx` | a parameter flowing into `Option<T>` |
| `annot_map_with_effect_row.flx` | type parameters and `with |e` in one signature |
| `annot_lambda_names_rigid_param.flx` | a lambda annotation naming the enclosing rigid parameter |
| `constraint_inline_single.flx` | `<a: Show>` |
| `constraint_inline_multiple.flx` | `<a: Eq + Show>` |
| `constraint_where_single.flx` | `where Show<a>` |
| `constraint_where_multiple.flx` | `where Eq<a>, Show<a>` |

The four constraint files keep near-identical bodies so the diff between them
is only the syntax.

### Data, aliases, projection, effects, classes

| File | Construct |
|---|---|
| `data_tree_recursive.flx` | recursive parameterized ADT + a generic function over it |
| `data_named_fields_deriving_eq.flx` | named-field variant, `deriving (Eq)` over a parameter |
| `data_two_type_params.flx` | two parameters instantiated independently |
| `data_contextual_instance_recursive_head.flx` | hand-written contextual instance over a recursive head |
| `data_result_err_rewrap.flx` | `Err(e)` passed into a `Result` with a different success type |
| `alias_concrete_tuple.flx` | **G4:** a transparent alias resolves at all |
| `alias_generic_function.flx` | a parameterized function-type alias |
| `alias_effect_row_parameter.flx` | **G3/G4:** an alias taking an effect-row parameter |
| `project_tuple_known_shape.flx` | **0185 stage 5:** projection as a solver predicate |
| `project_field_from_match_arm.flx` | **KI-095:** a field receiver bound by a match arm |
| `effect_row_polymorphic_callback.flx` | a row variable threaded from callback to caller |
| `class_instance_two_types.flx` | one class, two instances, dispatch at both |
| `class_superclass_chain.flx` | a superclass method reached through a subclass constraint |
| `class_contextual_instance.flx` | `instance Eq<a> => MyEq<List<a>>` |

### Correctly rejected

| File | Code | Rule |
|---|---|---|
| `project_index_out_of_range.flx` | `E492` | `p.2` on a pair is checked, not silently widened |
| `project_undetermined_receiver.flx` | `E491` | a receiver nothing determines is reported at the access |
| `constrained_call_wrong_arity.flx` | `E056` | arity is counted on the source signature, not the lowered one |
| `annot_instantiation_mismatch.flx` | `E300` | a scheme is instantiated, not coerced |

## Not supported

Each file names its owner — a roadmap item, or a `docs/known_issues.md` entry.
Codes were measured on 2026-09-11, not copied from the roadmap.

| File | Code | Gap | Owner |
|---|---|---|---|
| `failing/compile/b2_unannotated_constrained_double.flx` | `E300` | `fn double(x) { x + x }` at two types — the constrained half of generalize-by-arity | **B2**, 0.0.8 |
| `failing/compile/b2_projection_reused_two_shapes.flx` | `E300` | `fn fst(p) { p.0 }` at two tuple shapes — the receiver is pinned, not quantified | 0.0.8 |
| `failing/compile/b2_forwarding_over_constrained_callee.flx` | `E300` | a helper that only *forwards* to a constrained callee is withheld too | **B2**, 0.0.8 |
| `failing/compile/ki_032_unannotated_row_poly_wrapper.flx` | `E419` | the effect-row analogue: an unannotated wrapper over a row-polymorphic function | KI-032 |
| `failing/compile/ki_074_lowercase_class_in_where.flx` | `E034` | a lowercase class name is declarable but unusable in `where` | KI-074 |
| `failing/compile/ki_075_inline_bound_multiparam_class.flx` | `E489`, `E444` | `<a: C>` on a multi-parameter class, with a hint that cannot be followed | KI-075 |
| `failing/compile/grammar_0182_multiple_superclasses.flx` | `E034` | more than one superclass | proposal 0182 |
| `failing/runtime/ki_090_constrained_fn_as_value.flx` | `E1000` | a constrained function passed as a **value** loses its dictionary | KI-090, High, 0.0.8 |

### The distance to "full generic support"

One sentence: **generics works; what does not work is generics meeting
evidence.** Every gap above is a constrained definition, or a constrained value,
or a class surface — not a quantification failure. That is the same line the
roadmap draws, and this corpus is the evidence for it.

`docs/roadmaps/generics_tasks.md:80` puts it as one of 0.0.8's exit criteria:
"`fn double(x) { x + x }` works at two types — B1 then B2, on top of C."

## Filed, but no longer reproducing

Three entries were re-tested on 2026-09-11 against their own filed repros and
did not reproduce. Each now has a file in `working/accepts/` so that a
regression is a snapshot diff:

| Entry | Filed symptom | Now | File |
|---|---|---|---|
| KI-011 | `E430` re-wrapping `Err(e)` into a different success type | works | `data_result_err_rewrap.flx` |
| KI-069 | `E004 __dict_Eq_Tree<a>` for a contextual instance over a recursive head | works | `data_contextual_instance_recursive_head.flx` |
| KI-070 | a lambda annotation naming an enclosing rigid parameter | works | `annot_lambda_names_rigid_param.flx` |

KI-070's filed repro is written `\y: a -> y`, which is a parse error for an
unrelated reason — lambda parameter annotations require parentheses,
`\(y: a) -> y`. That may be why it read as unfixed.

## Known gaps not yet given a file

Listed so the map is complete even where the corpus is not. The first two
cannot be shown by any VM-only harness; the rest are simply not written yet.

| Gap | Symptom | Owner |
|---|---|---|
| KI-071 | instance method captures an unqualified same-named module fn — wrong answer on VM, **SIGSEGV** natively | 0.0.8 (D1) |
| KI-073 | result-directed selection through `where Convert<a, b>` — VM prints `42`, native prints `<value>` | 0.0.8 (D2) |
| KI-076 | an operator on a class-constrained parameter does not dispatch inside a `module` | 0.0.8 (D3) |
| KI-086 | a class declared inside a `module` loses its default method bodies | 0.0.8 (D4) |
| KI-088 | nested `fn` shadowing an outer `fn` — the codegen half, `E1000` at run time | 0.0.7 (D8) |
| KI-098 | a G5-generalized helper used at two types is despecialised — compiles, runs, **right answer**, worse code | 0.0.8 (B2) |
| E2 | inferred ambiguity panics at run time instead of reporting a compile error | 0.0.8 |

KI-098 is worth singling out: it is invisible to every harness here *and* to a
parity sweep, because a despecialised program still gives the right answer. Only
`--dump-aether` and `--dump-core` show it, which is why its seven sites live in
`tests/snapshots/aether/`.

## See also

- `examples/type_classes/` — the class/instance surface in full, including
  associated types and default bodies
- `examples/effects/README.md` — effect-row syntax, which generic signatures
  carry
- `docs/guide/09_type_system_basics.md` — the guide chapter
