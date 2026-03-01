- Feature Name: Base Functions Module Architecture
- Start Date: 2026-02-02
- Proposal PR: 
- Flux Issue: 

# Proposal 0008: Base Functions Module Architecture

## Summary
[summary]: #summary

This proposal outlines a modular architecture for Flux's built-in functions to improve maintainability and make updates easier. The goal is to split the monolithic `base functions.rs` (757 lines, 35 functions) into focused, category-based modules with reduced boilerplate.

## Motivation
[motivation]: #motivation

The current `base functions.rs` has several issues: The current `base functions.rs` has several issues:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Primary Goals

1. **Easy updates** - Add/modify built-ins without touching other modules
2. **Clear organization** - Group functions by category (array, string, hash, etc.)
3. **Reduced boilerplate** - Declarative function definitions with automatic validation
4. **Better testing** - Test categories independently
5. **Self-documenting** - Function signatures and docs in one place

### Non-Goals

- Changing the VM interface (`OpGetBase`)
- Modifying function behavior
- Adding new built-in functions (that's a separate task)

### Primary Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **1. Module Structure:** ``` src/runtime/ ├── base functions/ │ ├── mod.rs # Public exports, registry, lookup functions │ ├── macros.rs # Declarative built-in definition macro...
- **1. Module Structure:** ``` src/runtime/ ├── base functions/ │ ├── mod.rs # Public exports, registry, lookup functions │ ├── macros.rs # Declarative built-in definition macros │ ├── helpers.rs # Valida...
- **2. Declarative Built-in Definition:** **Goal:** Reduce boilerplate using a macro-based approach
- **Current (verbose)::** // Later in file... BuiltinFunction { name: "first", func: base_first, }, ```
- **Proposed (declarative)::** **The macro expands to:** ```rust fn base_first(args: Vec<Object>) -> Result<Object, String> { // Auto-generated arity check if args.len() != 1 { return Err(arity_error("first",...
- **3a. `base functions/mod.rs` (150 lines):** pub mod macros; pub mod helpers; pub mod array_ops; pub mod string_ops; pub mod hash_ops; pub mod type_ops; pub mod numeric_ops; pub mod util_ops;

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

- Changing the VM interface (`OpGetBase`)
- Modifying function behavior
- Adding new built-in functions (that's a separate task)

### Non-Goals

### Risk 1: Breaking VM Compatibility

**Likelihood:** Low
**Impact:** High
**Mitigation:**
- Keep `BASE_FUNCTIONS` array order identical
- Test `get_base_by_index` thoroughly
- Run full integration tests

### Risk 2: Performance Regression

**Likelihood:** Very Low
**Impact:** Medium
**Mitigation:**
- Module structure doesn't affect runtime
- All functions still in static array
- Benchmark before/after

### Risk 3: Increased Complexity (More Files)

**Likelihood:** Low
**Impact:** Low
**Mitigation:**
- Clear module naming (array_ops, string_ops)
- Good documentation in mod.rs
- Benefits outweigh cost

### Non-Goals

### Risk 1: Breaking VM Compatibility

### Risk 2: Performance Regression

### Risk 3: Increased Complexity (More Files)

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Phase 1 Module Split Plan](0006_phase1_module_split_plan.md)
- [Rust API Guidelines: Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)
- Current implementation: `src/runtime/base functions.rs`

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### 3. Plugin System (Future)

Allow users to register custom built-ins:

```rust
register_base! {
    fn my_custom_function(args: Vec<Object>) -> Result<Object, String> {
        // custom logic
    }
}
```

### 3. Plugin System (Future)

Allow users to register custom built-ins:
