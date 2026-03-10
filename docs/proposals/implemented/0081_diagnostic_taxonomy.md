- Feature Name: Diagnostic Taxonomy
- Start Date: 2026-03-08
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: 0002, 0061, 0080

# Proposal 0081: Diagnostic Taxonomy

## Summary

Add an explicit semantic taxonomy for diagnostics on top of the existing code, severity, phase, and runtime-vs-compiler metadata. The goal is to let Flux answer "what kind of problem is this?" in a structured way without changing diagnostic codes, while extending the machine-facing JSON contract with an additive `category` field.

## Motivation

Flux diagnostics now have stable error codes, severity, phase metadata, and human-facing display titles. That is enough to render good error messages, but it is not enough to classify diagnostics consistently across the compiler and runtime.

Today, semantic grouping is mostly implicit:

- the error code sometimes implies the class of problem
- the diagnostic title sometimes implies the class of problem
- the subsystem that built the diagnostic often implies the class of problem

That is workable for local reasoning, but it is brittle for tooling, tests, and long-term maintenance.

The main problem is that these signals are not the same thing:

- error codes are stable identifiers, but some codes intentionally cover several parser contexts
- display titles are editorial and contextual, but not a durable taxonomy
- subsystem boundaries are implementation details, not user-facing categories

`E034` is the clearest example. It now renders as contextual parser titles such as `Missing Function Body`, `Missing Match Arm Arrow`, or `Missing Else Body`, but all of those still share one error code. That is good for compatibility, but it means the code alone cannot answer whether a diagnostic is a declaration error, a delimiter error, or a missing separator.

This makes several things harder than they need to be:

- tests must assert on message strings when they really want to assert on diagnostic class
- suppression and aggregation logic cannot reason directly about semantic categories
- future CLI filtering or grouped summaries would have to infer categories from titles or codes
- contributors do not have one explicit place to classify a new diagnostic

Flux now has enough diagnostic structure that this missing layer is worth formalizing.

## Guide-level explanation

This proposal adds one new idea for compiler contributors: every diagnostic can carry a semantic category in addition to its existing code and severity.

For example:

| Problem | Code | Proposed Category |
|---|---|---|
| Missing function body | `E034` | `ParserDeclaration` |
| Missing closing `]` | `E076` | `ParserDelimiter` |
| Missing match arm arrow | `E034` | `ParserSeparator` |
| Module path mismatch | `E024` | `ModuleSystem` |
| Type mismatch | `E300` | `TypeInference` |
| Missing ambient effect | `E400` | `Effects` |
| Runtime type error | `E1004` | `RuntimeType` |

This does not change the user-visible code or the default diagnostic text. Instead, it gives the implementation a stable way to say what family of problem a diagnostic belongs to.

For contributors, this changes how diagnostics should be authored:

1. choose the stable error code as today
2. choose the human-facing message/title as today
3. assign the semantic category that best describes the problem

That makes diagnostics easier to reason about across phases.

For example, parser recovery tests could assert that a malformed file produced one `ParserDelimiter` error and one `ParserDeclaration` error, instead of depending on the exact English phrasing of both messages.

It also improves future UX work. A later CLI feature could group output like this:

- parser declaration errors
- parser delimiter errors
- type inference errors
- runtime type errors

without inventing a second inference layer outside the diagnostics model.

## Reference-level explanation

### New internal type

Add a new enum in `src/diagnostics/types`, for example:

```rust
pub enum DiagnosticCategory {
    ParserDeclaration,
    ParserDelimiter,
    ParserSeparator,
    ParserExpression,
    ParserPattern,
    ParserKeyword,
    NameResolution,
    ModuleSystem,
    Orchestration,
    TypeInference,
    Effects,
    RuntimeType,
    RuntimeExecution,
    Internal,
}
```

The exact variant names can be refined during implementation, but the initial set should stay intentionally small. The taxonomy should be broad enough to be stable and useful, not so granular that it becomes another version of the error code registry.

### Diagnostic model change

Extend `Diagnostic` with an optional category:

```rust
pub struct Diagnostic {
    ...
    pub category: Option<DiagnosticCategory>,
    ...
}
```

and add a builder:

```rust
pub fn with_category(mut self, category: DiagnosticCategory) -> Self
```

This should be additive only:

- existing diagnostics without a category remain valid
- JSON output gains one additive `category` field
- renderer behavior does not need to change for the first implementation

### Stable JSON serialization

The `category` field should serialize using stable strings derived directly from `DiagnosticCategory`.

The current serialized vocabulary is:

- `parser_declaration`
- `parser_delimiter`
- `parser_separator`
- `parser_expression`
- `parser_pattern`
- `parser_keyword`
- `name_resolution`
- `module_system`
- `orchestration`
- `type_inference`
- `effects`
- `runtime_type`
- `runtime_execution`
- `internal`

### Category assignment policy

There are two main assignment paths:

1. registry-level default categories for codes that always mean one kind of problem
2. contextual builder overrides for codes that span several semantic categories

The first path applies to diagnostics such as:

- `E004` / `E080` / similar unresolved-name diagnostics -> `NameResolution`
- `E024` -> `ModuleSystem`
- `E300` / `E301` -> `TypeInference`
- `E400`-family effect errors -> `Effects`
- `E1004` -> `RuntimeType`

The second path matters most for parser diagnostics, especially shared codes such as `E034`.

Examples:

- missing function/module/if/else/do body -> `ParserDeclaration`
- missing `)` / `]` / `}` -> `ParserDelimiter`
- missing match-arm arrow / lambda arrow / hash colon / effect-op colon -> `ParserSeparator`
- expected expression / malformed grouped expression -> `ParserExpression`
- malformed pattern positions -> `ParserPattern`
- bad keyword-driven forms like stray `else` or malformed `perform` keyword sites -> `ParserKeyword`

This means the code remains stable while the category captures the semantic class.

### Initial mapping

The first implementation should assign categories for these areas:

- parser diagnostics:
  - declaration-structure errors
  - delimiter errors
  - separator errors
  - expression-shape errors
  - pattern errors
  - keyword/form errors
- module/import diagnostics:
  - import path mismatch
  - module path mismatch
  - skipped module orchestration notes
- name-resolution diagnostics:
  - undefined variables
  - unknown base members
  - similar unresolved identifier/member/type paths
- type inference diagnostics:
  - `E300`
  - `E301`
  - closely related HM/type mismatch diagnostics
- effect diagnostics:
  - effect-row solver failures
  - missing ambient effects
  - unresolved effect requirements
- runtime diagnostics:
  - runtime type failures like `E1004`
  - runtime execution/dispatch failures such as invalid operation or runtime trap classes
- orchestration diagnostics:
  - `Module Skipped`
  - `Downstream Errors Suppressed`
- internal/compiler-failure diagnostics:
  - ICEs and implementation-failure paths

The intended rollout is broader than parser coverage alone. After the initial taxonomy field lands, Flux should continue contextualizing the remaining compiler-side dynamic diagnostics and attach categories everywhere the builder already knows the semantic problem.

That follow-up work should prioritize:

- contextual compiler display titles for dynamic `E300` sites that still fall back to generic registry titles
- contextual effect titles for `E400`/`E401`/`E402`/`E403`/`E404`/`E405`/`E406`/`E407` and effect-row diagnostics
- module/entrypoint orchestration diagnostics such as skipped modules and `main` validation failures

The goal is that no important compiler diagnostic depends on raw registry titles alone when the call site already knows whether it is reporting a type mismatch, effect requirement mismatch, missing ambient effect, entrypoint error, or module-system error.

### Interaction with existing metadata

The category does not replace:

- `code`: stable external identifier
- `severity`: error/warning/note/help
- `phase`: parse/type/runtime pipeline metadata
- `error_type`: compiler-vs-runtime domain metadata
- `display_title`: text-only human title

Instead, category sits alongside them:

- `code` answers "which stable diagnostic is this?"
- `display_title` answers "how should this be presented?"
- `category` answers "what family of problem is this?"

That separation matters because these concerns evolve at different rates.

### Aggregation and filtering

The first implementation does not need to change rendering or suppression behavior. The category should initially be available for:

- tests
- internal assertions
- future reporting/grouping features

Follow-up proposals or implementation work may use category for:

- smarter duplicate suppression
- grouped summaries
- CLI filters like `--diagnostic-category parser-delimiter`
- editor integration and machine-readable grouping

### Compatibility

This proposal is intentionally additive:

- no diagnostic codes change
- no current text diagnostics need to change
- JSON gains one additive `category` field
- existing builders can adopt categories incrementally

However, the target end state is complete category coverage for user-facing diagnostics. Incremental adoption is a rollout strategy, not the final design point.

That lets Flux land the taxonomy without forcing a whole-repo migration in one step.

## Drawbacks

Adding a taxonomy introduces one more field contributors need to maintain.

There is also a risk of overfitting. If the category set grows too large, it becomes hard to apply consistently and starts competing with the error-code registry instead of complementing it.

Another drawback is partial rollout. For some time, not every diagnostic will carry a category, so the system will be useful but incomplete.

## Rationale and alternatives

The main alternative is to keep inferring categories from codes, titles, or subsystem paths.

That is weaker for several reasons:

- codes are sometimes too broad
- titles are presentation-oriented and can change
- subsystem boundaries are implementation details

Another alternative is to use a much finer-grained taxonomy, close to one category per code family. That would likely become hard to maintain and would not offer much more value than the existing registry.

The proposed design chooses a middle ground:

- broad semantic categories
- additive to the current diagnostics model
- explicit overrides where shared codes like `E034` need context

That gives Flux a durable classification layer without destabilizing the existing diagnostics contract.

## Prior art

Rust, Clang, Elm, and GHC all have some combination of stable codes, severity levels, and subsystem grouping, but they vary in how explicit their semantic categorization is.

- Rust diagnostics rely heavily on error codes plus rich structured rendering. Internal passes and lints often carry category-like information even when the CLI surface emphasizes the code and message.
- Clang groups diagnostics strongly by warning/error classes and supports category-driven tooling and filtering.
- Elm emphasizes human-facing messages more than stable machine taxonomy, which makes presentation strong but leaves less formal structure for internal grouping.
- GHC uses stable message families and subsystem-aware reporting, but many categories are still implicit in the message source rather than represented as one explicit semantic field.

Flux can benefit from a small explicit category layer without copying any one of these systems wholesale.

## Unresolved questions

- Should registry-level default categories live on `ErrorCode`, in the registry, or in a dedicated helper layer?
- Should note/help diagnostics carry the same category as their parent diagnostic, or should categories be limited to primary diagnostics?
- Should future suppression logic use category directly, or should that remain a separate policy layer?
- Which remaining compiler-side dynamic diagnostics should still get explicit contextual `display_title` overrides instead of relying on registry titles during the completion phase of this proposal?

## Future possibilities

Once category coverage is broad enough, Flux could build on it in several ways:

- grouped diagnostic summaries by category
- CLI filtering by category
- snapshot tests that assert on category instead of English phrasing
- editor/IDE integration that groups parser/type/runtime failures more clearly
- lint-style contributor checks that reject new diagnostics without an explicit category when one is required
- completion tooling or contributor checks that flag compiler dynamic diagnostics still missing explicit contextual titles when the builder has enough information to provide one

This proposal deliberately stops before those UX changes. Its goal is to add the taxonomy layer first so those later improvements have a stable foundation.
