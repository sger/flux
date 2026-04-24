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

### Track 2: Handler coverage ergonomics

Today a handler for `Console` must cover every declared operation, including
both `print` and `println`, even if the handled expression only performs one
operation. This is strict and simple, but it is not always ergonomic.

Investigate one of:

- keep full coverage and improve diagnostics/examples
- allow partial handlers when unhandled operations provably do not occur
- allow explicit passthrough/default arms if the language wants partial
  interception with clear runtime behavior

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
