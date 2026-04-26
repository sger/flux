# What's New in Flux v0.0.5

Flux v0.0.5 is a language hardening release.

This version focuses on making Flux's typed functional core more coherent: effect handlers are now much closer to the user-facing model, type classes are implemented end to end, the standard library/primop boundary is cleaner, and diagnostics are more stable across the VM and native paths.

## Highlights

- **Effect system polish** — effectful prelude operations now route through handlers, with better diagnostics and parity fixtures
- **Parameterized handlers** — handlers can carry state, fall through, and compose with default entry handlers
- **Type classes** — parser, AST, type inference, Core lowering, dictionary elaboration, and backend support
- **Strict typing mode** — `--strict-types` adds stronger static checks for public APIs and unresolved types
- **Typed compiler pipeline** — stable expression IDs, type-informed folding, typed primops, and stronger Core contracts
- **Flow standard library growth** — expanded modules and clearer intrinsic primop routing
- **Diagnostics hardening** — stricter expected-output parity for failing fixtures and cleaner import-cycle/module diagnostics
- **Release automation** — changelog fragments, release cutting, and local preflight scripts

## Effects

### Effectful prelude operations

Flux now treats common effectful operations such as console, clock, and filesystem operations as proper effect operations instead of ad hoc runtime shortcuts.

Important pieces include:

- effectful primops routed through synthesized handlers
- reserved `Flow.Primops` internals guarded from user imports
- clearer `E400` and `E402` diagnostics for missing effects and handler coverage
- default entry/test handlers for the supported prelude effects
- stronger examples under `examples/effects`

This makes effects more predictable: if a helper performs `Console`, its signature needs to say so; entrypoint defaults do not silently make ordinary helpers effectful.

### Parameterized handlers

This release adds broader support for handlers that carry data or delegate behavior:

- parameterized counter/state-style handlers
- reader-style handlers
- console capture handlers
- fallthrough behavior
- nested default/user-handler behavior

These examples are now covered in parity checks across the maintained backend matrix where supported.

### Effect rows and aliases

Effect rows continue to mature:

- effect-row aliases such as `IO` normalize during HM checks
- aliases are threaded through the type inference context
- effect availability checks expand aliases before comparing rows
- diagnostics stay deterministic for row-subtraction and subset failures

This is especially visible with imported type-class methods whose signatures use aliases like `IO`.

## Type Classes

Flux v0.0.5 includes the first complete type-class pipeline.

What landed:

- type class and instance syntax
- `public class` and `public instance` parsing
- module-scoped class and instance declarations
- class and instance validation
- compile-time instance lookup
- type-class constraint generation during HM inference
- dictionary elaboration into Core
- backend execution for instance method calls
- built-in classes such as `Eq`, `Ord`, `Num`, `Show`, and `Semigroup`
- deriving support for ADTs

Method effects are now part of the type-class model too. `E452` reports instance methods whose effects violate their class method floor.

## Typing and Core

### Strict types

The new `--strict-types` mode strengthens static checking for public and entry-facing code.

It catches cases that normal mode may leave gradual or unresolved:

- missing public function annotations
- unresolved `Any` in strict surfaces
- public APIs whose effect signatures are incomplete
- strict entrypoint and helper boundary issues

### Signature-directed checking

Annotated bindings now use bidirectional checking in more places. `if`, `match`, `do`, and lambda bodies can receive an expected type from their annotation or call site, so errors often point at the offending sub-expression rather than only the outer binding.

This release also adds rigid type variables for declared type parameters. If a declared generic parameter would be unified with a concrete type inside the function body, Flux reports `E305`.

### Typed compiler data

Several compiler internals now carry more stable type information:

- stable expression IDs on parsed expressions
- type-informed AST folding
- typed primop signatures and returns
- polymorphic operator inference
- static contract validation at the type-infer to Core boundary
- typed Core lambda parameter binders
- Aether RC elision for unboxed primitive values

These changes reduce fallback `Any` paths and make later lowering stages less guessy.

## Flow and Primops

Flux's standard library story continues to move toward a clear split:

- Flow modules are the public user-facing surface
- intrinsic primops are the compiler/runtime implementation layer
- `Flow.Primops` is reserved and not user-importable

The Flow surface now includes broader numeric, math, either, collection, and debug functionality. Numeric and bitwise primops were expanded, and polymorphic primop signatures remove several old `Any` escape hatches.

## Diagnostics and Fixtures

This release puts a lot of effort into keeping diagnostics stable.

Notable improvements:

- import-cycle diagnostics now point at the import that enters the cycle
- module-resolution failures no longer cascade as aggressively into later stages
- unresolved concrete-type diagnostics lead with the source-level issue
- failing fixtures can pin expected stdout/stderr and diagnostic codes
- parity checks now validate expected compile failures across VM and LLVM output
- effect and type-system negative examples have stronger metadata

This matters because many Flux examples are intentionally negative. The parity harness now distinguishes an expected compiler rejection from a real release regression.

## Tooling and Release Flow

The release process is now more explicit:

- changelog fragments live in `changes/*.md`
- `scripts/changelog/changelog_from_fragments.sh` rebuilds `[Unreleased]`
- `scripts/release/release_cut.sh` cuts a version section
- `scripts/release/release_check.sh` runs the local release preflight
- changelog fragment validation runs in CI

The preflight now includes the maintained parity suites plus a release-green compile sweep across example folders.

## Migration Notes

### `Flow.Primops` is internal

User code should not import `Flow.Primops` or call `__primop_*` names directly. Use the public effectful operations such as `print`, `println`, `read_file`, and `now_ms`.

### Helper functions need explicit effects

Default handlers apply at supported entry/test boundaries. Ordinary helper functions still need explicit `with ...` annotations for effects they perform.

### Strict typing can reject older gradual code

If you enable `--strict-types`, add explicit public API annotations and remove unresolved `Any` from strict surfaces.

### Effect aliases are more precise

Aliases such as `IO` are now normalized more consistently during effect checks. Code that relied on alias opacity may report clearer missing-effect diagnostics.

## Recommended first things to try

Run the effect examples:

```bash
cargo run -- parity-check examples/effects --compile
```

Try the guide type-system examples:

```bash
cargo run -- parity-check examples/guide_type_system --compile
```

Inspect strict typing:

```bash
cargo run -- examples/guide_type_system/08_strict_public_api_ok.flx --strict-types
```

Run the release preflight:

```bash
scripts/release/release_check.sh
```

## In short

Flux v0.0.5 makes the language feel more intentional:

- effects are routed through the language model
- type classes are real compiler features
- strict typing has a dedicated mode
- Flow is the public library surface
- diagnostics and negative fixtures are better pinned
- release automation is in place

It is a stability release for the typed/effectful Flux core.
