- Feature Name: Lexer & Parser Code Review and Refactoring
- Start Date: 2026-02-04
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0014: Lexer & Parser Code Review and Refactoring

## Summary
[summary]: #summary

This document provides a comprehensive code review of the lexer and parser, identifying areas for improvement, potential bugs, and proposing a modular architecture for better maintainability. It now also records implementation status for completed vs pending items.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal should be read as a user-facing and contributor-facing guide for the feature.

- The feature goals, usage model, and expected behavior are preserved from the legacy text.
- Examples and migration expectations follow existing Flux conventions.
- Diagnostics and policy boundaries remain aligned with current proposal contracts.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Current State:** **Files:** [src/syntax/lexer/mod.rs](../src/syntax/lexer/mod.rs) + lexer submodules - **✅ Strengths:** 1. **Clear Responsibilities** - Clean separation betw...
- **Current State:** **Files:** [src/syntax/lexer/mod.rs](../src/syntax/lexer/mod.rs) + lexer submodules
- **✅ Strengths:** 1. **Clear Responsibilities** - Clean separation between character reading and token scanning - Good handling of string interpolation edge cases - Proper line/column tracking
- **1. **String Handling Could Be More Robust**:** **Issue:** Escape sequence handling is permissive ```rust // Line 323: Unknown escapes just return the character Some(c) => { // Unknown escape - just return the character as-is...
- **2. **Number Parsing Could Be More Strict**:** **Analysis:** The concern about parsing malformed numbers like `1.2.3` is not actually a lexer issue.
- **3. **Missing Edge Cases**:** All recommended number formats are now supported: - ✅ Scientific notation (`1e10`, `1.5e-3`, `2.5E+5`) - ✅ Hex literals (`0xFF`, `0x1A_BC`) - ✅ Binary literals (`0b1010`, `0b111...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Phase 2 Module Split](0012_phase2_module_split_plan.md)
- [Lexer Implementation](../src/syntax/lexer/mod.rs)
- [Parser Implementation](../src/syntax/parser/)
- **Crafting Interpreters:** Chapter on Lexing & Parsing

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
