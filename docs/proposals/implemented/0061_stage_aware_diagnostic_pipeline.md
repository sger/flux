- Feature Name: Stage-Aware Diagnostic Pipeline
- Start Date: 2026-02-28
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0061: Stage-Aware Diagnostic Pipeline

## Summary
[summary]: #summary

Introduce a stage-aware diagnostic pipeline that filters, deduplicates, and presents errors according to the compilation phase that produced them. Instead of dumping all diagnostics from all phases at once, the pipeline follows a strict **Parse -> Type -> Effect** cascade: parser errors suppress type errors, type errors suppress effect errors. This produces fewer, higher-quality diagnostics that point to root causes rather than downstream consequences.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Tag every diagnostic with the compilation phase that produced it.
2. Implement strict stage-aware filtering in the aggregator: Parse errors suppress Type/Effect errors; Type errors suppress Effect errors.
3. Collapse cascading parser errors to the earliest root cause with a recovery note.
4. When downstream errors are suppressed, emit a summary note: `"Note: N type/effect errors were suppressed because parsing failed. Fix the parse errors first."`.
5. Preserve the error-accumulating model internally (all phases still run for IDE use cases), but filter at the output layer.
6. Add a `--all-errors` CLI flag to disable stage filtering for debugging.

### 4. Non-Goals

1. No changes to the compilation phases themselves (all phases still run to completion).
2. No changes to the HM inference algorithm.
3. No changes to error code numbering or diagnostic message text.
4. No changes to runtime error handling.
5. No "error guarantee" type system (Rust's `ErrorGuaranteed` approach) — this would require major refactoring.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2.1 Current Pipeline (Error-Accumulating Model):** Today, Flux accumulates all errors from all phases into a single `all_diagnostics` pool: - **2.2 The Problem:** When `let...
- **2.1 Current Pipeline (Error-Accumulating Model):** Today, Flux accumulates all errors from all phases into a single `all_diagnostics` pool: ``` Parser -> Module Graph -> PASS 0 -> PASS 1 -> HM Inference -> PASS 2 ↘ ↘ ↘ ↘ ↘ ↘ ───...
- **2.2 The Problem:** When `let label: String = "The answer` (missing closing quote) appears on line 2, the parser recovers and continues, but the recovery may produce a malformed AST. If `fn add(a:...
- **2.3 The Elm/Rust Standard:** Both Elm and Rust's compilers implement stage-aware filtering: - **Elm**: Parse errors suppress all type checking. Type errors are shown only when parsing succeeds. - **Rust**:...
- **2.4 Architectural Gap:** The `Diagnostic` struct has no `phase` field. The `DiagnosticsAggregator` has no stage-aware filtering. Error codes implicitly encode phase membership (E001-E086 = parser/compil...
- **5.1 DiagnosticPhase Enum:** ```rust // src/diagnostics/types/diagnostic_phase.rs #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub enum DiagnosticPhase { /// Lexer and parser errors (E001–E086, E071,...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. No changes to the compilation phases themselves (all phases still run to completion).
2. No changes to the HM inference algorithm.
3. No changes to error code numbering or diagnostic message text.
4. No changes to runtime error handling.
5. No "error guarantee" type system (Rust's `ErrorGuaranteed` approach) — this would require major refactoring.

### 4. Non-Goals

### 11. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Stage filtering hides a real type error that isn't a cascade | `--all-errors` flag as escape hatch; phase tagging is conservative (untagged diagnostics are never filtered) |
| Parser cascade collapsing is too aggressive, hides independent parse errors | Heuristic only collapses generic E034 within 3 lines of a root error; specific error codes (E071, E076) are never collapsed |
| Effect error is independent of a type error but gets suppressed | Effect errors from PASS 0 validation (e.g., `fn main() with IO` missing) are tagged as `Validation`, not `Effect`, so they survive type-error filtering |
| Some effect-related diagnostics still appear while suppression note mentions effect suppression | This is intentional: structural checks such as `E400` are tagged `TypeCheck` and remain visible; suppression note reports only diagnostics actually removed by stage filtering |
| Performance overhead of phase tagging | Phase is a single byte enum stored in an `Option`; negligible overhead |
| IDE integrations need all errors | `--all-errors` or `disable_stage_filtering: true` when called from tooling |

### 4. Non-Goals

### 11. Risks and Mitigations

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
