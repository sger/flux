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

**Status (2026-04-25):** delivered for `E400`, `E403`, and `E404`. The
0165 routing pass records the `ExprId` of each `Perform` node synthesized
from a direct user call in `Compiler::routed_call_perform_ids`;
`compile_perform` consults the set when emitting effect diagnostics:

- `E400` (missing ambient effect): "Call to `foo`" + label "effectful
  call occurs here" for call-origin Performs; "Performing `foo`" /
  "effectful perform occurs here" preserved for user-written
  `perform` expressions.
- `E403` (unknown effect) and `E404` (unknown effect operation): label
  toggles between "unknown effect/operation in call" and
  "unknown effect/operation in perform" with the same origin signal.
  The detail message is unchanged in both shapes.

Audit of the remaining effect diagnostics:

- `E402` (incomplete handler) is emitted from `handle` blocks, not
  from routed calls — out of scope.
- `E419` (unresolved effect variable) and `E420`/`E421`/`E422` are
  already emitted on the function-call path and already use
  call-shaped wording ("this call leaves an effect variable
  unconstrained", etc.) — no rewording needed.

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

**Status (2026-04-25):** delivered. The decision is to keep
`Flow.Primops` **visible-but-discouraged** rather than hide or
relocate the module. The boundary is enforced by the compiler in
two places — `__primop_*` names cannot appear in user source, and
`Flow.Primops` cannot be imported from user code — pinned by
`examples/effects/failing/03_reserved_internal_primop.flx` and
`failing/04_reserved_primops_module_import.flx` respectively.

The `examples/effects/README.md` gained a new section,
*`Flow.Primops` is the intrinsic layer*, that:

- explains the module's role as the delegation target for
  synthesized entrypoint default handlers,
- lists, in order, what users *should* learn instead (prelude
  calls → `Flow.Effects` labels → `perform`/`handle`),
- references the two failing fixtures that make the rejection
  visible.

Why visible-but-discouraged was chosen:

- The compiler already enforces the *hard* rule: reserved-name and
  reserved-module rejections gate any actual misuse. What was
  missing was a *teaching* boundary, not a *safety* one.
- Hiding the module would either require relocating/renaming it
  (large churn for a cosmetic win) or gating its visibility behind
  a compiler flag (more machinery for the same teaching outcome).
  Either way `Flow.Primops` itself stays needed, since synthesized
  default handlers reference it.
- A README note plus the existing failing fixtures composes
  cleanly: discoverability without breaking the pipeline.

This resolves the *Unresolved questions* item on `Flow.Primops`
visibility.

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

#### Status (2026-04-25): documented invariant + targeted regression tests

The audit (proposal 0171, Track 4) found that the five sites above are
**not** reading the same logic written five times — they are three
genuinely distinct representations, each tied to a specific pass:

1. **HM inference** — row algebra over `InferEffectRow`, the source
   of truth during type checking. Lives in
   `src/ast/type_infer/effects.rs`.
2. **CFG pre-validator** — `CompilerPhase::is_effect_in_declared` in
   `src/compiler/statement.rs`, declared-only static check that
   intentionally ignores synthesized handlers. Pre-pass safety gate.
3. **Lowering / bytecode emission** —
   `Compiler::is_effect_available` in `src/compiler/mod.rs`, the
   runtime view that walks `current_function_effects()` *and*
   `handled_effects`. The four call sites in `expression.rs`,
   `cfg_bytecode.rs`, and `statement.rs` all funnel through it.

Forcing a single helper across these would either lose information
(rows collapsed to symbols) or push complexity into every caller.
The 0165 bug was a *coordination* failure — the pre-validator did
not look at `Expression::Perform` shapes that the routing pass had
just synthesized, so a routed `println` slipped past it and was
caught only at lowering. The fix routes `Expression::Perform`
through the same pre-validator entry direct builtin calls use
(`expr_has_effect_row_error` in `statement.rs`), and that contract
is now made explicit in code.

What shipped on this track:

- **Single source of truth for the runtime predicate.**
  `Compiler::is_effect_available_name(&str)` no longer reimplements
  the symbol predicate; it does an `Interner::lookup` (read-only
  name → Symbol) and delegates to
  `Compiler::is_effect_available(Symbol)`. If the name has never
  been interned, no declared effect or handler can match it, so the
  predicate returns `false` directly. One implementation, two
  signatures.

- **Doc-block invariants on both sides.** Both
  `Compiler::is_effect_available` (lowering view, `mod.rs`) and
  `CompilerPhase::is_effect_in_declared` (pre-validator view,
  `statement.rs`) now carry parallel doc comments stating the
  three-way invariant explicitly:
  - **forward direction:** if HM unification accepts a fixture and
    the pre-validator accepts it, the lowering predicate must accept
    the same effect — pre-validator passing implies lowering
    passing.
  - **inverse direction does not hold:** lowering may legitimately
    accept effects the pre-validator rejects, because installed
    handlers (synthesized or user-written) become visible only at
    lowering. The pre-validator is conservative on declared-only
    state by design.
  - **regression anchor:** the comment names the 0165 bug and the
    `expr_has_effect_row_error` entry that closed it, so the next
    contributor reading either function sees the same story.

- **Three-way regression tests** in
  `src/compiler/compiler_test.rs`:
  - `helper_routed_prelude_is_caught_by_pre_validator_not_just_lowering`
    — pins the forward contract by asserting a 0165-style routed
    prelude in a helper without `with Console` produces *exactly
    one* E400. Two would indicate the suppression contract between
    pre-validator and lowering has drifted.
  - `helper_with_explicit_effect_is_accepted_by_all_three_passes` —
    positive control: same fixture with `with Console` declared
    must compile cleanly through all three passes.
  - `handled_effect_inside_helper_is_accepted_at_lowering_only` —
    pins the inverse-direction case: a routed `perform` inside a
    user `handle Console { ... }` is rejected by the pre-validator
    in spirit (handlers invisible) but accepted at lowering via
    `handled_effects`. A regression where lowering stopped
    consulting handlers would break this test.

This satisfies the proposal's acceptance criterion *"Effect
availability checks share a central helper or documented invariant
with regression tests for HM/strict/CFG parity"* via the
**documented-invariant + regression-tests** branch of that "or",
which the audit found to be the correct framing for these five
sites.

### Track 5: Entry default coverage

Add negative tests and examples around:

- `main`/`test_*` default handlers accepting operational effects
- ordinary helper functions still requiring explicit effects
- module-qualified effectful calls from entrypoints
- user handlers intercepting before defaults
- nested handler/default interactions

**Status (2026-04-25):** the example-fixture corpus for these scenarios
has been filled in. New positive fixtures (VM-only):

- `examples/effects/14_test_function_default_handlers.flx` — `test_*`
  entry exercises the same default Console + Clock handlers as `main`;
  symmetric with the long-standing `01_default_entry_handlers.flx`.
- `examples/effects/15_user_handler_shadows_default.flx` — `main`
  contains a user `handle Console { ... }` that intercepts inner
  `println`s while a Console call outside the user handler still
  falls through to the synthesized default.
- `examples/effects/16_nested_user_default.flx` — a helper-scoped user
  handler captures Console for its body while sibling `println` calls
  in `main` are discharged by the entry default, documenting the
  lexical-scoping rule for handler/default interaction.

New negative fixture:

- `examples/effects/failing/05_test_helper_no_default.flx` — a helper
  called from a `test_*` entry without `with Console` is rejected with
  E400, mirroring `failing/01_missing_effect_in_helper.flx` for the
  `main` case. Together they pin the rule that defaults wrap only the
  entry's body, never propagating into helpers.

Module-qualified calls from entrypoints are already covered by the
existing `04_modules_and_rows.flx` fixture.

Compiler-level unit tests in `src/compiler/compiler_test.rs`:

- `main_println_without_annotation_compiles_via_default_handler`
  (pre-existing) — confirms `main` defaults work.
- `test_function_println_without_annotation_compiles_via_default_handler`
  — same proof for `test_*` entries.
- `helper_called_from_main_does_not_inherit_default_handler` — pins
  the `main`-side helper rule with a direct E400 assertion.
- `helper_called_from_test_entry_does_not_inherit_default_handler` —
  pins the `test_*`-side helper rule symmetrically.

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

#### Status (2026-04-25): item-by-item disposition

| # | Item | Status |
|---|---|---|
| 1 | Evidence vector RC ownership | ✅ closed |
| 2 | Parameterized state replacement drops prior state | ✅ closed |
| 3 | Yield payload globals: RC roots or documented invariant | ✅ documented invariant |
| 4 | Process-global state vs. concurrency | ⚠ documented limitation, deferred |
| 5 | Windows/MSVC closure-entry symbol export | ⚠ deferred (Windows native blocker) |
| 6 | VM/native multi-shot resume divergence | ⚠ documented as deliberate, not a bug |

**Item 1 (closed by commit `7a342513`, 2026-04-24).** `EvvArray` entries
hold five tagged words per entry: htag, marker, handler, parent_evv,
state. Insertion (`flux_evv_insert`, `runtime/c/effects.c:130`) `flux_dup`s
all three owned fields (handler, parent_evv, state) for both copied
entries (line 142, via `evv_dup_owned_fields`) and the newly appended
entry (line 153). Teardown is the custom `flux_drop_evidence` scanner in
`runtime/c/rc.c:221-239`, which `flux_drop`s the same three fields per
entry before freeing the array. The `scan_fsize=0` choice on
`flux_gc_alloc_header` (line 99 of effects.c) is deliberate: evidence
arrays use the custom scanner because the payload mixes a 32-bit count
header with packed entries that interleave tagged ints (htag, marker)
and owned heap references.

**Item 2 (closed by commit `7a342513`).** State replacement now goes
through `flux_update_state_for_marker` (`runtime/c/effects.c`, around
line 290) with the dup-then-drop-then-assign ordering. The function now
carries a doc-block calling out why the ordering is safety-critical: the
operation must be safe when `next_state` aliases the current slot
(`resume(value, current_state)`); reordering to drop-then-dup would risk
freeing the new value before the dup. Both call sites
(`flux_resume_mark_called_closure_entry` at line ~340 and
`flux_compose_trampoline_closure_entry` at ~510) now use this helper.

**Item 3 (documented invariant).** The yield payload globals
(`flux_yield_clause`, `flux_yield_op_arg`, `flux_yield_op_state`,
`flux_yield_evv`, `flux_yield_conts[8]`) remain *borrowed* references —
they are not RC-rooted. Correctness depends on the LIR continuation
lowering keeping the source values alive across the unwind window. The
top-of-file comment block in `runtime/c/effects.c:27-55` now states this
as an explicit producer/consumer invariant with three named obligations:
(a) the LIR generator writes borrowed tagged values just before
unwinding to the prompt; (b) `flux_yield_prompt` reads each global into
a local and clears the slot before invoking handler clauses, breaking
the alias; (c) the LIR generator must not insert a `flux_drop` between
the yield write and the prompt consume. Switching to explicit RC roots
would require a `flux_dup` on every yield write and a `flux_drop` on
the prompt consume — deferred until concurrency lands, since the cost
is per-yield and the current invariant is sound on a single thread.

**Item 4 (concurrency blocker, deferred).** The full inventory of
process-global mutable state in the effect runtime: `flux_yield_*`
globals (yield path), `current_evv`, `marker_counter` (EVV state),
`flux_resume_called`, `flux_direct_resume_marker` (legacy direct-perform
counters). All would need to become thread/fiber-local before Flux can
support concurrency. The runtime header now states this explicitly
("This runtime is not concurrency-ready") and references this proposal
as the tracking location. Migration deferred — it is a systematic
change touching every yield and evidence path, justified only when an
actor/thread/fiber model is on the roadmap.

**Item 5 (Windows/MSVC, deferred).** Closure-entry symbols
(`flux_resume_mark_called.closure_entry`,
`flux_compose_trampoline.closure_entry`) use the GNU/Clang `__asm__`
label spelling because C identifiers cannot contain `.` and the LLVM
backend emits the dotted name. The current macro
(`runtime/c/effects.c:322-326`) handles Mach-O underscore prefixing and
falls through to the bare label on ELF; both are GNU/Clang-only.
Windows/MSVC requires a different mechanism (DEF file,
`__pragma(comment(linker, "/EXPORT:..."))`, or renaming the symbols to
MSVC-friendly forms and updating the LIR generator to match). This is a
real Windows-native blocker and is best handled as part of a dedicated
Windows porting workstream rather than retrofitted onto this polish
track.

**Item 6 (multi-shot divergence, deliberate).** The divergence is still
present and is by design, not a bug:

- VM: enforces one-shot via continuation guard slots (E1009 on second
  resume).
- Native default (yield-based): supports multi-shot natively because the
  prompt loop re-runs continuations; matches the proposal-0162 slice
  5-tr-fix path.
- Native legacy (`FLUX_YIELD_CHECKS=0`, direct-perform fast path):
  reports E1200/E1201 via the `flux_resume_called` counter for non-TR
  and multi-shot shapes respectively.

`tests/native_llvm/effect_multi_shot_tests.rs` exercises all three
configurations. The cross-backend parity suite
(`effect_runtime_parity_tests.rs`) deliberately excludes multi-shot
because the divergence is documented and intentional. Convergence (one
unified semantics across VM and native default) would be a policy
choice, not a bug fix; it is not tackled by this proposal.

#### What did *not* ship on this track

This track deliberately does not:

- Migrate process-global state to thread-local storage (item 4 deferral).
- Add MSVC export plumbing (item 5 deferral).
- Force VM/native convergence on multi-shot semantics (item 6 is a
  design choice, not a hardening item).
- Switch yield payload globals to RC roots (item 3 — the borrowed
  invariant is sound and the cost would be per-yield).

These remain known, documented limitations rather than blockers.

### Track 6: Default-handler policy

#### Current policy (recorded 2026-04-25)

Default handlers are **always-on for entrypoints** — `main` and any
`test_*` function — and only for the four built-in **operational**
effects:

- `Console` (`print`, `println`)
- `FileSystem` (`read_file`, `write_file`, `delete_file`, …)
- `Stdin` (`read_line`)
- `Clock` (`now_ms`)

`Flow.Debug` participates only on the synthesis side: the entry's
default handler set still includes a `Debug` arm so a routed
`perform Debug.trace(...)` from `debug` / `debug_labeled` /
`debug_with` is discharged inside an entrypoint, but `Debug` is
**not** part of call-site routing — users never write `trace(x)`
at the value level, and the routing pass deliberately excludes
`Debug` from `routed_call` so a user-defined `fn trace(...)` is
never silently rewritten to a `perform`. See the comment block at
`src/ast/route_effectful_primops.rs:518` for the rationale.

The synthesis is purely lexical: the routing pass wraps the
entrypoint's body in `handle E { ... }` blocks for each required
effect that the body actually uses. Helpers, closures, lambdas, and
non-entry module functions never receive defaults — they must
declare `with E` (or be called from a context that already has it).

User effects (anything declared with `effect E { ... }`) **never**
get defaults. Only the four built-in operational effects above.

#### Decision: keep always-on for now

This proposal **records the current always-on policy as the
deliberate baseline** rather than treating it as a placeholder
pending a future capability system.

Why this is the right baseline today:

- **Script-style ergonomics.** A one-file Flux program that prints
  a value should compile without effect ceremony. Requiring `with
  Console` on `main` for `println("hello")` is the kind of
  paper-cut that keeps newcomers out.
- **Test ergonomics.** Test bodies frequently mix `assert_eq` with
  trace prints, clock reads, and file I/O during debugging. Asking
  every `test_*` to enumerate its effects is friction that does
  not buy correctness.
- **Helpers stay strict.** Because defaults are scoped to the
  entrypoint body and never propagate into helpers, the *interesting*
  effect-tracking surface — function signatures across module
  boundaries — is unaffected. The convenience is limited to the
  outermost frame, where there is no caller whose signature could
  be polluted.
- **Reversibility.** Always-on is the *most* permissive policy. Any
  future tightening (opt-out flag, capability grant, edition gate)
  can land without breaking helper code, because helpers were
  already strict. Only entrypoint top-level code has to migrate, and
  the migration is mechanical (`fn main() with Console, Clock,
  FileSystem, Stdin`).

Why not the alternatives — yet:

- **Per-function/module opt-out** (e.g. `fn main() #[no_default_handlers]`)
  is plausible but has no current motivating use case. The kind of
  program that wants to *deny* `Console` to its own `main` typically
  also wants to deny it to its imported libraries, which opt-out
  attributes do not solve.
- **Explicit capability imports/config in a future edition** is the
  long-term direction if Flux ever pursues capability-secure
  execution. It is a much larger design surface (effect-as-capability,
  granted-by-runtime, possibly per-thread) and should ride with that
  larger story rather than being grafted onto this polish track.

#### When to revisit

Revisit the always-on policy when *any* of these hold:

1. A use case appears where a Flux program embeds another Flux
   program and needs to deny operational effects to the inner
   `main`/`test_*` (e.g. sandboxed eval, plugin host).
2. Capability-style permissions become a project goal — at which
   point default handlers should be unified with whatever grant
   mechanism is chosen, not retained as a parallel system.
3. A real-world program is observed where the entrypoint default
   handler hides a bug that an explicit `with` clause would have
   surfaced. (None has been reported as of 2026-04-25.)
4. Someone wants to add a fifth operational effect to the default
   set; that is the moment to ask whether *all* of them should
   still be implicit.

Until one of those triggers, the always-on policy is documented as
deliberate, and changes to it require a follow-up proposal that
addresses migration (`with`-clause inference for entrypoint bodies)
and tooling (the linter should emit a guided-fix when the policy
flips).

#### Documentation surface

The user-facing teaching of this rule lives in
`examples/effects/README.md` ("Entrypoint default handlers") and in
the example pair `01_default_entry_handlers.flx` /
`failing/01_missing_effect_in_helper.flx`. The 2026-04-25 Track 5
fixtures (`14_test_function_default_handlers.flx`,
`failing/05_test_helper_no_default.flx`) extend that pair to the
`test_*` entry shape so the rule is visibly symmetric across both
entrypoint kinds.

The README points at this section as the rationale-of-record;
keep that link intact when editing either document.

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

- **Resolved (2026-04-25):** Should partial handlers be allowed, or should
  full operation coverage remain the language invariant? — Full operation
  coverage remains the invariant. See Track 2's *Recommendation for a future
  decision*: Option A is the baseline, and any future relaxation should be a
  narrow Option B proposal, not the flow-sensitive Option C.
- If default handlers ever become configurable, should the switch be syntax,
  module metadata, CLI policy, or edition policy? — Track 6 records the
  current always-on policy as deliberate and lists the triggers that would
  motivate revisiting it; the *form* of any future switch is left to that
  follow-up proposal.
- **Resolved (2026-04-25):** Should `Flow.Primops` stay public but
  discouraged, or become hidden once all compiler entry paths reliably
  preload its declarations? — Visible-but-discouraged. The compiler
  already enforces the hard rule (reserved-name and reserved-module
  rejections); Track 3 adds the teaching boundary in
  `examples/effects/README.md` and records the rationale. Revisit
  only if relocation/hiding gains a concrete motivating use case.

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
