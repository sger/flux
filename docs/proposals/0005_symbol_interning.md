- Feature Name: Symbol Interning
- Start Date: 2026-02-01
- Proposal PR: 
- Flux Issue: 

# Proposal 0005: Symbol Interning

## Summary
[summary]: #summary

Replace string-based identifiers with interned symbols (u32 IDs) throughout the compiler to reduce memory usage by 70-80% and improve performance by 2-3x for identifier operations.

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

- **Consolidated technical points:** - **Current Problem:** Identifiers are stored as `String` throughout the codebase: - **1. Symbol Interner:** /// Compact symbol identifier #[derive(Debug, Clone, Copy, PartialEq...
- **Current Problem:** Identifiers are stored as `String` throughout the codebase: ```rust // AST nodes Expression::Identifier { name: String, ... }
- **1. Symbol Interner:** /// Compact symbol identifier #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct SymbolId(u32);
- **2. Update AST:** // Before pub enum Expression { Identifier { name: String, span: Span, }, // ... }
- **3. Update Parser:** pub struct Parser { lexer: Lexer, interner: SymbolInterner, // NEW // ... existing fields }
- **4. Update Compiler:** pub struct Compiler { interner: &'ctx SymbolInterner, module_constants: HashMap<SymbolId, Object>, symbol_table: SymbolTable<SymbolId>, // ... existing fields }

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

### Decision

**Rationale:** Current performance is acceptable. Focus on maintainability (Phase 1) before optimization (Phase 3).

**Next steps:**
1. Complete Phase 1 (module split)
2. Add performance benchmarks
3. Profile compiler on large files (>5000 identifiers)
4. Revisit this proposal in v0.1.0

### Decision

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Option A: Rc<str> (Shared String References)

```rust
HashMap<Rc<str>, Object>
```

**Pros:** Cheap clones, easier than lifetimes
**Cons:** Reference counting overhead, not as fast as u32

### References

1. **Papers:**
   - ["String Interning" - Wikipedia](https://en.wikipedia.org/wiki/String_interning)

2. **Implementations:**
   - [lasso crate](https://crates.io/crates/lasso) - Production-ready Rust interner
   - [rustc source](https://github.com/rust-lang/rust/tree/master/compiler/rustc_span) - Symbol interning in Rust compiler

3. **Related:**
   - [Proposal 0001: Module Constants](./0001_module_constants.md)
   - [COMPILER_ARCHITECTURE.md](../COMPILER_ARCHITECTURE.md) - Phase 3 overview

### Option A: Rc<str> (Shared String References)

```rust
HashMap<Rc<str>, Object>
```

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Future (If Needed)

- Implement Phase 2 (parser-level interning)
- Evaluate Phase 3 (arena allocation)

### Future (If Needed)
