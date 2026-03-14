- Feature Name: Phase 2 - Advanced Module Split Plan
- Start Date: 2026-02-04
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0012: Phase 2 - Advanced Module Split Plan

## Summary
[summary]: #summary

Building on Phase 1's successful module organization, Phase 2 focuses on **advanced architectural patterns** and splitting remaining large files. This phase introduces more sophisticated patterns like builder patterns, visitor-based diagnostics, and command-driven CLI architecture.

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

- **Consolidated technical points:** - **Achievements from Phase 1 ✅:** Phase 1 successfully split the three largest files: - ✅ `compiler.rs` (1,671 lines) → 5 modules (~300 lines each) - ✅ `parser.rs` (1,144 lines...
- **Achievements from Phase 1 ✅:** Phase 1 successfully split the three largest files: - ✅ `compiler.rs` (1,671 lines) → 5 modules (~300 lines each) - ✅ `parser.rs` (1,144 lines) → 4 modules (~250 lines each) - ✅...
- **Remaining Issues (Phase 2 Targets):** **Files Still Too Large:** 1. **diagnostic.rs** - **1,412 lines** (CRITICAL - largest file in codebase!) 2. **compiler/expression.rs** - 789 lines (should be <500) 3. **main.rs*...
- **In Scope (Phase 2):** **Priority 1 (CRITICAL) - Diagnostics System:** 1. ✅ Split `diagnostic.rs` (1,412 lines) into focused modules 2. ✅ Split `compiler_errors.rs` (602 lines) by error category 3. ✅...
- **Out of Scope:** - ❌ New features or functionality changes - ❌ Performance optimizations (covered in Proposal 0011) - ❌ Breaking API changes
- **Current State:** ``` src/syntax/diagnostics/ ├── diagnostic.rs # 1,412 lines (TOO BIG!) ├── compiler_errors.rs # 602 lines (TOO BIG!) ├── runtime_errors.rs # 177 lines ├── aggregator.rs # 579 li...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Risk 1: Import Churn

**Likelihood:** High
**Impact:** Low
**Mitigation:**
- Keep old files as re-export wrappers
- Gradual migration, not big bang
- Use IDE refactoring tools

### Risk 2: Breaking Changes

**Likelihood:** Low
**Impact:** High
**Mitigation:**
- All old imports still work (re-exports)
- Deprecation warnings in v0.1.x
- Remove old structure in v0.2.0

### Risk 3: Over-Splitting

**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Follow Single Responsibility Principle
- Group related functionality
- Don't split below 50 lines

### Risk 1: Import Churn

### Risk 2: Breaking Changes

### Risk 3: Over-Splitting

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Phase 1 Module Split](0006_phase1_module_split_plan.md)
- [Visitor Pattern Proposal](0007_visitor_pattern.md)
- [Compiler Architecture](../architecture/compiler_architecture.md)
- Rust API Guidelines: [Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Potential Future Splits

- **AST types** - Split expression.rs, statement.rs into semantic groups
- **Bytecode generation** - Further split compiler/builder.rs
- **VM instructions** - Split op_code.rs by instruction category

### Potential Future Splits
