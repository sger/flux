- Feature Name: Compatibility and Editions Policy
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0087: Compatibility and Editions Policy

## Summary
[summary]: #summary

Define Flux's compatibility policy for the path to `1.0.0` and for post-`1.0.0` evolution.

This proposal establishes:

1. what `1.0.0` compatibility means
2. which kinds of changes are source-breaking versus non-breaking
3. how `Base` and `Flow` API stability should be treated
4. how VM/JIT parity relates to compatibility
5. whether Flux should use **editions** for future breaking language changes

The core policy is:

> Before `1.0.0`, Flux may still evolve aggressively, but every breaking change should be
> deliberate, documented, and migration-guided. After `1.0.0`, the default expectation is
> source compatibility, with breaking language changes gated through explicit editions.

## Motivation
[motivation]: #motivation

Flux is moving quickly across several fronts at once:

- syntax
- type/effect semantics
- standard library structure
- runtime memory model
- actor concurrency
- dual VM/JIT backend support

Without an explicit compatibility policy, several problems appear:

1. users cannot tell which behavior is stable versus provisional
2. proposals can accidentally make incompatible decisions without release discipline
3. `1.0.0` risks becoming a label rather than a meaningful promise
4. backend parity expectations remain ambiguous
5. post-`1.0.0` language evolution becomes harder because there is no defined mechanism
   for breaking changes

Flux needs compatibility to become a first-class design constraint before the `1.0.0`
release line hardens.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What `1.0.0` should promise

After `1.0.0`, users should be able to expect:

1. **Source compatibility by default**
   - a valid Flux `1.x` program should continue to compile on later `1.x` versions unless
     it uses explicitly unstable features

2. **Stable core language behavior**
   - syntax, typing, effect semantics, module behavior, and actor MVP behavior should not
     change casually

3. **Stable standard library contracts**
   - `Base` and stable `Flow` APIs should not be renamed, removed, or meaningfully changed
     without a compatibility process

4. **Backend parity as part of the contract**
   - VM and JIT should remain observably equivalent within the supported semantics

5. **Documented deprecation path**
   - when something must change, users get warnings, migration guidance, and time

### What `1.0.0` does not need to promise

Flux does not need to promise that:

- every diagnostic wording stays byte-for-byte unchanged forever
- every optimization remains identical
- every experimental feature becomes stable
- every proposed feature lands before `1.0.0`

It should promise semantics and supported APIs, not accidental implementation details.

### Pre-`1.0.0` policy

Before `1.0.0`, Flux is still allowed to make breaking changes, but under discipline.

Required for a breaking pre-`1.0.0` change:

1. proposal-level documentation
2. changelog entry
3. migration note if user-visible
4. examples/docs updated in the same release window

This avoids "silent instability" where the language changes but the project behaves as if
nothing happened.

### Editions

Flux should adopt **editions** as the default mechanism for post-`1.0.0` breaking language
changes.

An edition is a named language mode such as:

```text
2026 edition
2028 edition
```

Editions are appropriate for:

- syntax changes
- keyword changes
- parser behavior changes
- meaningfully different type/effect rules
- language-level defaults that cannot remain backward-compatible

Editions are **not** required for:

- bug fixes that restore documented behavior
- performance improvements
- internal representation changes
- additive stdlib growth

### Stability levels

Flux should classify public surfaces as:

1. **Stable**
   - covered by compatibility guarantees in the current major line

2. **Provisional**
   - intended to stabilize, but not yet promised

3. **Experimental**
   - opt-in or clearly labeled; may change without normal compatibility guarantees

This classification should apply to:

- language features
- CLI commands/flags
- `Base` APIs
- `Flow` modules/APIs
- runtime/backend flags

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Compatibility domains

Flux compatibility should be defined across the following domains.

#### 1. Source compatibility

A program is source-compatible if:

- it parses successfully
- name resolution rules remain valid
- type/effect checking remains valid under the same stable semantics
- referenced stable `Base`/`Flow` APIs still exist with compatible signatures

Breaking examples:

- syntax removal
- keyword introduction that invalidates existing identifiers
- changing effect rules so formerly valid stable code is now rejected
- removing or renaming stable prelude/stdlib APIs

#### 2. Semantic compatibility

A program is semantically compatible if stable code preserves its documented observable
behavior.

This includes:

- evaluation results
- type/effect meaning
- actor send/recv/spawn behavior
- effect handling behavior
- runtime boundary checks where documented

This does not require identical internal memory layouts, optimization strategies, or IR.

#### 3. Standard library compatibility

`Base` and stable `Flow` APIs require explicit policy:

- additive functions/modules are compatible
- renaming or removing stable APIs is breaking
- changing a stable function's meaning or effect contract is breaking
- tightening types in a way that rejects previously valid stable programs is breaking

Experimental modules may evolve faster, but they must be labeled as such.

#### 4. Backend compatibility

VM/JIT parity is part of the compatibility contract for supported features.

This means:

- stable Flux programs must not observe different semantics across VM and JIT
- differences in performance are allowed
- differences in debug-only output are allowed if documented
- differences in stable language/runtime results are not allowed

If a feature is VM-only or JIT-experimental, it must be labeled that way explicitly.

#### 5. Diagnostic compatibility

Diagnostics should be treated as two layers:

- **stable classes and codes**: compatibility-sensitive
- **exact wording/layout**: best-effort, not a hard compatibility promise

Flux should aim to keep:

- error code identity
- broad diagnostic category
- major structured fields where tooling depends on them

but not freeze every sentence permanently.

### Editions policy

Recommended editions model:

1. A new edition may introduce breaking syntax or semantic defaults.
2. Old editions remain supported for a defined window or major line.
3. Tooling should help migrate code between editions where feasible.
4. The default edition for new projects may advance over time.

Edition scope should be limited to:

- language syntax/semantics
- parser/typechecker behavior with source-visible consequences

Editions should not exist merely to version:

- optimizer internals
- bytecode format
- runtime memory representation
- JIT internals

### Pre-`1.0.0` breaking change policy

Before `1.0.0`, breaking changes are allowed, but should follow this process:

1. proposal approved
2. release notes/changelog entry added
3. user-facing docs/examples updated
4. migration guidance added when the change affects normal user code

Recommended phrasing in release notes:

```text
Breaking change:
- match arms now use `=>` instead of `->`
- use `flux fmt` or the migration guide to update existing code
```

### Post-`1.0.0` breaking change policy

After `1.0.0`, breaking changes should generally require one of:

1. a new edition
2. a deprecation cycle followed by a major version boundary
3. explicit experimental-status escape hatch already declared in docs

The default assumption should be:

```text
Flux 1.x is source-compatible by default.
```

### Stability marking

Flux should explicitly mark public surfaces using one of:

- `Stable`
- `Provisional`
- `Experimental`

Suggested initial treatment:

- core syntax/type/effect semantics targeted for `1.0.0`: `Stable`
- actor concurrency before it proves out fully: `Provisional` until `1.0.0`, then `Stable`
- M:N scheduler: `Experimental`
- macro system: `Experimental`
- NaN-boxing runtime path: `Experimental`

### Documentation requirements

For `1.0.0`, compatibility information should be reflected in:

- `README.md`
- `CHANGELOG.md`
- `docs/versions/`
- language reference/manual
- release notes for any breaking pre-`1.0.0` change

### CLI and tooling policy

The Flux CLI should follow the same stability model:

- stable commands/flags should not change casually after `1.0.0`
- new commands/flags are additive
- renames/removals require deprecation or a major version boundary
- debug/internal commands may be marked experimental

### Bytecode/cache compatibility

Bytecode format compatibility is an implementation policy, not a source compatibility
guarantee.

Recommended rule:

- source compatibility is primary
- bytecode/cache formats may change as needed
- cache invalidation/version bumps are acceptable if documented and transparent

### Platform support interaction

Compatibility guarantees apply within the supported platform matrix for a release line.

If Flux narrows or expands support, that should be documented in release engineering policy,
not smuggled in as an implicit compatibility break.

## Drawbacks
[drawbacks]: #drawbacks

1. Compatibility policy adds process overhead to language evolution.
2. Editions can increase conceptual complexity if overused.
3. Early formalization can feel constraining while the language is still evolving quickly.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why define this before `1.0.0`?

Because waiting until `1.0.0` is too late. Compatibility needs to shape the release path,
not merely be announced at the end.

### Why editions instead of frequent major breaks?

Editions let Flux preserve user trust while still evolving language design where necessary.
They are better suited than casual major-version breaks for syntax/semantic evolution.

### Why not promise exact diagnostic wording?

Because that would freeze too much implementation detail and slow necessary diagnostics
improvement. Stability should focus on codes, categories, and semantics.

## Prior art
[prior-art]: #prior-art

- Rust editions model
- semver expectations for language/tooling ecosystems
- changelog/release-note driven compatibility discipline in language projects

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should edition names be calendar-year based or semantic labels?
2. How long should older editions remain supported after a new one ships?
3. Which `Flow` modules should start as `Stable` at `1.0.0` versus `Provisional`?
4. Should diagnostic JSON or machine-readable output get a stronger compatibility contract
   than human-readable text?
5. Should the formatter output be considered compatibility-sensitive after `1.0.0`?

## Future possibilities
[future-possibilities]: #future-possibilities

- edition migration tooling
- machine-readable stability metadata for stdlib APIs
- automated compatibility reports in CI
