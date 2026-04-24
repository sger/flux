- Feature Name: Effect System Polish and Hardening
- Start Date: 2026-04-24
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0161](implemented/0161_effect_system_decomposition_and_capabilities.md), [0162](implemented/0162_unified_effect_handler_runtime.md), [0165](implemented/0165_io_primop_migration_to_effect_handlers.md), [0170](implemented/0170_polymorphic_effect_operations.md)

# Proposal 0171: Effect System Polish and Hardening

## Summary
[summary]: #summary

Polish the post-0165 effect system without changing its core semantics.
0161, 0162, 0165, and 0170 made effects operational: prelude primops now
route through `perform`, entrypoints get default handlers, fine-grained
labels are available, and polymorphic effect operations work. This proposal
tracks the remaining user-experience and compiler-maintenance hardening.

## Motivation
[motivation]: #motivation

The effect system is now usable and architecturally sound, but several
rough edges remain:

- diagnostics expose lowered terminology for calls users wrote as ordinary
  function calls
- handler coverage is strict even when only a subset of operations is
  performed
- `Flow.Primops` is visible but should remain an implementation layer
- effect availability checks are duplicated across HM, strict audit, CFG
  pre-validation, routing, and lowering
- entrypoint default handlers are convenient but subtle
- the VM path and LLVM/native path still need explicit parity hardening for
  effectful helper calls that are discharged by synthesized entrypoint
  default handlers
- the language has not decided whether default handlers are permanently
  always-on or controlled by explicit capability policy

These issues are polish and hardening work, not blockers for 0165.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

From the user's point of view, the desired direction is:

- source calls like `println(x)` should diagnose as calls, not as lowered
  `perform` nodes
- examples and docs should teach `Flow.Effects` and prelude operations first
- `Flow.Primops` should be documented as an implementation/intrinsic layer
- entrypoint default handlers should be clearly explained: `main` and
  `test_*` get default capabilities, ordinary helpers do not
- handler ergonomics should be reconsidered where full operation coverage is
  surprising

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Track 1: Source-aware diagnostics

For routed prelude operations, diagnostics should preserve source intent.

Current example:

```text
Performing `println` requires effect `Console` in this function signature.
```

Target shape:

```text
Call to `println` requires effect `Console` in this function signature.
```

The compiler may still lower to `Perform`; diagnostics should carry enough
origin information to render the user-facing call shape.

**Status (2026-04-24):** delivered for `E400`. The 0165 routing pass now
records the `ExprId` of each `Perform` node synthesized from a direct user
call in `Compiler::routed_call_perform_ids`; `compile_perform` consults the
set when emitting `E400` and renders "Call to `foo`" + label "effectful
call occurs here" for call-origin Performs, preserving the existing
"Performing `foo`" / "effectful perform occurs here" phrasing for
user-written `perform` expressions. Other effect diagnostics
(`E402`/`E403`/`E404`/`E419`…) have not yet been audited for the same
treatment.

### Track 2: Handler coverage ergonomics

Today a handler for `Console` must cover every declared operation, including
both `print` and `println`, even if the handled expression only performs one
operation. This is strict and simple, but it is not always ergonomic.

**Status (2026-04-24):** the conservative half of this track has shipped.
`E402` (`INCOMPLETE EFFECT HANDLER`) now emits a copy-pasteable arm
skeleton for every missing operation, with correct shape for both
stateless and parameterized handlers (`op(resume, _argN) -> resume(())`
for `Unit`-returning ops; `op(resume, _argN, _state) -> resume(todo!(),
_state)` for parameterized handlers; typed placeholders for non-`Unit`
return types). The effects README's new *Handler coverage is total*
section documents the handle-and-discard idiom so the pattern is
recognizable when reading examples. Full coverage is still required; the
language-level question of whether to relax it is unresolved — see the
three alternatives below.

#### Option A — keep full coverage, invest in ergonomics (chosen baseline, delivered)

Status: **delivered on the `feature/effects` branch** as described above.

What: handlers remain total; the compiler helps the user satisfy coverage
instead of allowing partial handlers. E402 prints arm skeletons; docs
show the idiom; `Flow.Effects` stays the single source of truth for what
a handler must cover.

Pros:

- **No new semantics.** Handle continues to be a *total* interpretation
  of an effect, matching Koka, OCaml 5, and Eff.
- **Predictable.** Whether a handler compiles depends only on its
  syntactic arms, not on flow-sensitive effect analysis or catch-all
  resolution order.
- **Stable under effect decomposition.** Splitting an effect into
  fine-grained labels (proposal 0161) doesn't silently change which
  arms a handler needs; every new op is a visible compile error.
- **Cheap to deliver.** Already shipped.

Cons:

- The `handle Console { print(..) -> .., println(..) -> .. }` kind of
  "handle-and-discard" duplication is still syntactically present.
  Readers have to learn that identical-looking arms are *intentional*
  rather than copy-paste sloppiness.
- Adding an operation to an effect is a breaking change for every
  existing handler of that effect. The diagnostic is good, but the
  churn still happens.

#### Option B — explicit passthrough / default arm (deferred)

What: accept a single catch-all arm that matches any declared operation
not mentioned explicitly. A strawman shape:

```flux
do { ... } handle Console {
    println(resume, msg) -> { count := count + 1; resume(()) }
    _(resume, _args, _state?) -> resume(())   // applies to `print`
}
```

Design decisions that would need to be resolved before shipping:

- **Arm shape under heterogeneous signatures.** Different ops of one
  effect can have different arities and different return types. A
  catch-all arm has to either (a) be typed polymorphically and require
  `resume` to accept a value whose type the compiler proves matches the
  current op's return type at each call site, or (b) be restricted to
  effects whose ops uniformly return `Unit`, or (c) receive args as an
  untyped tuple / existentially-typed value. Each choice locks in a
  piece of semantics.
- **Name/arg exposure.** Does the arm see the operation's name (for
  logging / routing)? As a string? An enum of declared ops? If so,
  extending the effect adds a new enum variant — arguably the same
  churn Option A has today, just moved behind a discriminator.
- **Interaction with explicit arms.** Does the catch-all match only
  ops not mentioned, or does it shadow explicit arms when listed last?
  Koka requires explicit enumeration; OCaml 5 allows a default
  effect-handler clause but with very constrained typing.
- **Parameterized handlers.** Does the catch-all thread `state` the
  same way explicit arms do? If yes, the arm body is obligated to call
  `resume(value, new_state)` — but `value`'s type depends on which op
  was caught.

Pros:

- Handle-and-discard boilerplate disappears for the common case.
- Library authors can extend an effect without breaking every handler
  that doesn't care about the new op.

Cons:

- Every one of the design decisions above is semantic surface area.
  Once shipped, each becomes a compatibility constraint.
- Weakens the "handler is a total interpretation" guarantee: a handler
  author can silently forget an operation.
- Discovery cost: reading a handler no longer tells you which ops it
  actually treats specially vs. discards. Tooling would have to
  synthesize the catch-all's matched set for LSP / docs.

Open questions for a follow-up proposal:

1. Polymorphic `resume` typing vs. restricting catch-all to `Unit`-only
   effects.
2. Whether the catch-all sees operation identity and args, and in what
   shape.
3. Whether catch-all is allowed as the *only* arm (handle-any) or only
   alongside at least one explicit arm.
4. Whether the catch-all participates in Option A's E402 coverage
   check at all — i.e. does its presence satisfy coverage, or does the
   compiler still print a skeleton so the user can choose explicit
   arms?

#### Option C — statically-inferred partial handlers (deferred, lower priority)

What: the compiler runs effect-row analysis on the handled expression,
proves that some operations of the handled effect *cannot* occur, and
silently accepts a handler that omits those ops.

Pros:

- No new syntax at all; a handler that happens to be complete for the
  flow it actually handles just compiles.
- Most ergonomic in the immediate case.

Cons:

- **Fragile under row polymorphism.** The analysis depends on the
  handled expression's inferred effect row, which for code involving
  callbacks, HKT, or `with |e` parameters can widen based on how the
  function is called from elsewhere. A handler can compile in one call
  context and fail in another — a "spooky action at a distance"
  failure mode Flux has otherwise avoided.
- **Invisible coupling between handler and body.** Editing the body of
  a handled expression — e.g. adding a new `perform` behind a
  conditional — can turn a previously-accepted handler into a compile
  error far from the edit site.
- **Analysis is nontrivial to trust.** "Provably cannot occur" in the
  presence of effect-row subtraction, aliasing, and row variables is
  exactly the kind of check whose false negatives and false positives
  are both costly to explain to users.
- Koka, OCaml 5, and Eff all reject this direction for these reasons.

Open questions for a follow-up proposal:

1. Whether the analysis runs before or after effect-row alias
   expansion.
2. How row variables in the handled expression's row are treated —
   conservatively (require coverage) or optimistically (assume absent).
3. How the diagnostic reads when the analysis flips between
   "complete" and "incomplete" on a re-infer.

#### Recommendation for a future decision

Ship Option A's improvements (done), **keep full coverage as the default**,
and if and when relaxation lands it should be Option B with a narrow,
deliberate design — not Option C. The invisible-analysis failure mode in
C is the kind of thing that is very hard to walk back once users depend
on it.

### Track 3: `Flow.Primops` documentation boundary

Document `Flow.Primops` as the intrinsic implementation layer. Users should
normally learn:

- prelude calls such as `println`, `read_file`, `now_ms`
- `Flow.Effects` labels and operation signatures
- `perform`/`handle` for user-defined and intercepted effects

They should not learn `__primop_*` as an API. User calls and definitions of
reserved `__primop_*` names remain rejected.

### Track 4: Shared effect availability checker

Audit duplicate effect availability logic across:

- HM/effect inference
- strict ambient-effect audit
- CFG pre-validation
- AST routing/default-handler synthesis
- final expression lowering checks

The goal is a shared helper or shared data contract that prevents drift like
the 0165 bug where CFG pre-validation accepted a routed `perform` that should
have remained compile-time `E400`.

### Track 5: Entry default coverage

Add negative tests and examples around:

- `main`/`test_*` default handlers accepting operational effects
- ordinary helper functions still requiring explicit effects
- module-qualified effectful calls from entrypoints
- user handlers intercepting before defaults
- nested handler/default interactions

Also track the current implementation gap: effectful helper calls under
synthesized entrypoint default handlers are reliable on the VM path, but the
LLVM/native path has shown continuation/default-handler parity issues. Examples
may be refactored or marked VM-only while that backend gap is being fixed, but
the language semantics should remain `perform`/`handle` based rather than
introducing backend-specific behavior.

Current forced parity status for `examples/effects`:

- `cargo run --features llvm -- parity-check examples/effects --ways vm,llvm`
  currently reports 6 passing examples and 7 mismatches.
- The VM-only examples are intentional markers for the remaining native effect
  gaps, not language semantics: `01_default_entry_handlers`,
  `02_explicit_effect_rows_and_aliases`, `04_modules_and_rows`,
  `05_row_polymorphic_callbacks`, `06_sealing_effect_scope`,
  `08_filesystem_and_clock`, and `11_parameterized_console_capture`.
- Known mismatch classes include native/default-handler output ordering, native
  state threading for parameterized captures, VM runtime failures in some
  default-handler helper shapes, and unstable clock output normalization.
- Low-level runtime coverage now includes a dedicated `flux_yield_conts >= 8`
  overflow/composition fixture (`tests/parity/effect_yield_conts_overflow.flx`).
  Keep this fixture in both VM/native parity because it protects the native
  continuation compression path.

### Track 5b: Native effect runtime hardening

Track the C native effect runtime separately from source-level effect semantics:

- Evidence vectors must own and release their handler closures, parent EVVs,
  and parameterized state values through RC.
- Parameterized handler state replacement must drop the previous state.
- Yield payload globals (`flux_yield_clause`, `flux_yield_op_arg`,
  `flux_yield_op_state`, `flux_yield_evv`) must either become explicit RC
  roots or have a documented/proven lowering invariant that keeps borrowed
  values alive until `flux_yield_prompt` consumes them.
- `flux_resume_called`, `flux_direct_resume_marker`, evidence state, and yield
  state are process-global today. This is a blocker for future threads/fibers;
  any concurrency work must make this state thread/fiber-local or move it into
  an explicit scheduler context.
- Closure-entry symbol names that rely on GNU/Clang `asm` labels need a
  Windows/MSVC-compatible export strategy before Windows native is considered
  supported.
- VM and native still enforce unsupported/multi-shot resume shapes through
  different mechanisms: the VM uses one-shot continuation guards, while native
  default yield handling supports multi-shot and the legacy
  `FLUX_YIELD_CHECKS=0` direct path reports E1200/E1201. This divergence is
  documented in `tests/native_llvm/effect_multi_shot_tests.rs` and should be
  revisited before declaring one cross-backend handler semantics story.

### Track 6: Default-handler policy

Decide whether default handlers remain always-on for entrypoints or become
controlled by explicit capability policy in a later edition/syntax.

Options:

- keep always-on defaults permanently
- add a per-function/module opt-out
- require explicit capability imports/config in a future edition

This proposal does not choose the policy; it requires documenting the current
policy and recording a deliberate decision before any behavior change.

### Track 7: Effect-row delimiter rationalization (pre-1.0 question, deferred)

Flux currently writes effect-row collections with three distinct delimiters
depending on context:

| Context | Shape | Separator |
|---|---|---|
| `with` clause | bare list | `,` |
| Effect expression | algebraic, no delimiter | `+` / `-` / `\| e` |
| Alias body | `< ... >` | `\|` |
| Sealing allow-set | `{ ... }` | `\|` |
| Sealing algebraic | `( ... )` | `ambient - E` |

The `<>` / `{}` split between alias bodies and sealing allow-sets is
syntactically meaningful — `alias IO = <Console | FileSystem>` declares a
new name whereas `f() sealing { Console }` restricts an expression — and
the enclosing punctuation is what disambiguates them at a glance. But for
a pre-1.0 language it is worth asking whether one collapsed form (e.g.
angle brackets for both, or braces for both) would simplify teaching
without losing precision.

**Status (2026-04-24):** deferred. Two small latent inconsistencies have
been closed on the `feature/effects` branch without touching the larger
delimiter question:

- `parse_sealing_brace_row` used to silently accept both `|` and `,` as
  separators; it now rejects `,` with a targeted hint pointing at the
  `|` form, so examples stay consistent and the canonical form is
  unambiguous.
- The effects README gained an *Effect-row syntax at a glance* section
  that documents all four contexts in one table, making the surface
  explicit teaching material.

The `<>` vs `{}` collapse itself is recorded here as a pre-1.0 design
consideration rather than a polish item. Any move on it should be a
deliberate proposal covering:

1. Which delimiter survives (`<>` has historical ML/Haskell resonance
   for row types; `{}` reads as "set of labels" more naturally and
   matches some ML-family effect-handler papers).
2. The migration cost — every alias body or sealing site has to be
   rewritten, and every doc page, proposal, and example file with it.
3. Whether the change is tied to a broader row-syntax cleanup (for
   example, unifying `with A, B` and `with A + B` on `,` in all
   positions, at which point `+` becomes exclusively an algebraic
   operator used with `-`).

Recommendation: **do not change** the delimiter surface on this polish
track. Revisit only if a broader pre-1.0 syntax pass is planned, and
pair it with the `with`-clause separator question so users learn one
coherent story rather than two piecemeal changes.

## Drawbacks
[drawbacks]: #drawbacks

- Source-aware diagnostics may require carrying origin metadata through
  routing/lowering.
- Partial handlers can complicate static validation and runtime dispatch.
- A shared checker may require refactoring compiler passes that currently
  use local, context-specific representations.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

Keeping this as a separate proposal avoids reopening 0165 after its semantic
goal is complete. The effect model should now stabilize through small,
testable hardening steps.

The alternative is to fold all polish into 0165, but that would blur the
line between the implemented semantic migration and the next usability pass.

## Prior art
[prior-art]: #prior-art

- Koka and Effekt distinguish user-facing effect operations from runtime
  implementation details.
- Capability-oriented systems commonly separate entrypoint ambient authority
  from ordinary helper-function effects.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should partial handlers be allowed, or should full operation coverage remain
  the language invariant?
- If default handlers ever become configurable, should the switch be syntax,
  module metadata, CLI policy, or edition policy?
- Should `Flow.Primops` stay public but discouraged, or become hidden once all
  compiler entry paths reliably preload its declarations?

## Acceptance criteria
[acceptance-criteria]: #acceptance-criteria

- Routed prelude diagnostics render source-aware call messages.
- `Flow.Primops` / `Flow.Effects` docs clearly separate user and intrinsic
  layers.
- New tests cover entry defaults versus ordinary helper requirements.
- Native parity coverage exists for effectful helper calls discharged through
  synthesized entrypoint default handlers, or the remaining backend gap is
  documented with focused failing/VM-only fixtures.
- Effect availability checks share a central helper or documented invariant
  with regression tests for HM/strict/CFG parity.
- Handler coverage policy is either retained with better docs/diagnostics or
  changed with focused tests.
- Default-handler policy is documented explicitly.
