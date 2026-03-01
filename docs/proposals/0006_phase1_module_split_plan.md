- Feature Name: Phase 1 - Module Split Plan
- Start Date: 2026-02-01
- Proposal PR: 
- Flux Issue: 

# Proposal 0006: Phase 1 - Module Split Plan

## Summary
[summary]: #summary

This proposal outlines the module split strategy for Phase 1 of the Flux compiler architecture improvements. The goal is to improve code maintainability by breaking down large files (>500 lines) into focused, single-responsibility modules.

## Motivation
[motivation]: #motivation

Three critical files exceed 800 lines and violate the Single Responsibility Principle:
- `compiler.rs` (1,671 lines) - handles expression/statement compilation, symbols, errors
- `parser.rs` (1,144 lines) - handles expression/statement parsing, utilities
- `vm.rs` (824 lines) - handles instruction dispatch, operations, tracing

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

- **Consolidated technical points:** - **In Scope:** - Refactor 3 critical files into focused modules - Maintain 100% backward compatibility - Preserve all existing tests - Document new module structure - **Out of...
- **In Scope:** - Refactor 3 critical files into focused modules - Maintain 100% backward compatibility - Preserve all existing tests - Document new module structure
- **Out of Scope:** - API changes or new features - Performance optimizations - Symbol interning (Phase 3)
- **1. Compiler.rs Split (HIGH PRIORITY):** **Current:** 1,671 lines, single `impl Compiler` block
- **Module Breakdown:** **1a. `bytecode/compiler/expression.rs`** (280 lines) - `compile_expression()` - Main dispatcher - `compile_if_expression()` - `compile_match_expression()` - `compile_function_l...
- **File Structure:** ``` src/bytecode/ ├── compiler/ │ ├── mod.rs # Public exports, Compiler struct │ ├── expression.rs # Expression compilation │ ├── statement.rs # Statement compilation │ ├── buil...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Risk 1: Breaking Changes

**Likelihood:** Low
**Impact:** High
**Mitigation:**
- Keep original files as re-export wrappers during transition
- Extensive regression testing
- Gradual migration path

### Risk 2: Performance Regression

**Likelihood:** Low
**Impact:** Medium
**Mitigation:**
- Benchmark before/after each split
- Profile hot paths (especially VM)
- Inline critical functions if needed

### Risk 3: Increased Complexity

**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Clear module documentation
- Consistent naming conventions
- Module dependency diagrams

### Risk 4: Merge Conflicts

**Likelihood:** Medium (if concurrent work)
**Impact:** Medium
**Mitigation:**
- Communicate refactoring schedule
- Work on feature branches
- Merge frequently

### Risk 1: Breaking Changes

### Risk 2: Performance Regression

### Risk 3: Increased Complexity

### Risk 4: Merge Conflicts

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Compiler Architecture](../architecture/compiler_architecture.md)
- [Symbol Interning Proposal](0005_symbol_interning.md)
- Rust API Guidelines: [Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
