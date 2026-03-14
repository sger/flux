- Feature Name: Cranelift JIT Backend for Flux
- Start Date: 2026-02-26
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0031: Cranelift JIT Backend for Flux

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Cranelift JIT Backend for Flux in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **AST → Cranelift IR** (not bytecode → IR) | Preserves structured control flow, avoids decompiling bytecode back to blocks |
| **All Values as i64 pointers** | Simple, uniform representation in Cranelift. One type for everything. |
| **Arena allocation** | Avoids per-value Box allocation. Arena reset between top-level expressions. |
| **Runtime helpers for everything** | v1 prioritizes correctness over speed. Inline integer fast-paths later. |
| **Feature-gated** | `--features jit` keeps binary size small by default. Cranelift adds ~5MB. |

### Key Design Decisions

### Key Design Decisions

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Context:** Flux currently compiles AST to custom bytecode and interprets it in a stack-based VM. Adding a Cranelift JIT backend will eliminate the dispatch loop overhead, pr...
- **Context:** Flux currently compiles AST to custom bytecode and interprets it in a stack-based VM. Adding a Cranelift JIT backend will eliminate the dispatch loop overhead, producing native...
- **Architecture:** The JIT compiles AST directly to Cranelift IR. Each Flux function becomes a native function. Values flow as `*mut Value` pointers (i64 in Cranelift). All type-checked operations...
- **Value Passing Convention:** Runtime helper signature: extern "C" fn(ctx: *mut JitContext, ...) -> *mut Value JIT function signature: extern "C" fn(ctx: *mut JitContext, args: *const *mut Value, nargs: i64)...
- **Module Structure:** ``` src/jit/ ├── mod.rs # Public API: JitEngine, feature gate ├── context.rs # JitContext: arena, globals, GC heap, error state ├── compiler.rs # AST → Cranelift IR (expressions...
- **New Files:** | File | Purpose | |------|---------| | `src/jit/mod.rs` | JitEngine public API | | `src/jit/context.rs` | JitContext struct (arena + globals + gc_heap + error) | | `src/jit/com...

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

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **AST → Cranelift IR** (not bytecode → IR) | Preserves structured control flow, avoids decompiling bytecode back to blocks |
| **All Values as i64 pointers** | Simple, uniform representation in Cranelift. One type for everything. |
| **Arena allocation** | Avoids per-value Box allocation. Arena reset between top-level expressions. |
| **Runtime helpers for everything** | v1 prioritizes correctness over speed. Inline integer fast-paths later. |
| **Feature-gated** | `--features jit` keeps binary size small by default. Cranelift adds ~5MB. |

### Key Design Decisions

### Key Design Decisions

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

### Future Optimizations (post-v1)

- **Inline integer arithmetic** — check tag, operate directly, skip helper call
- **NaN boxing** — encode primitives in 64 bits without allocation
- **Type specialization** — monomorphize hot functions for known argument types
- **Direct calls** — bypass `call_value` dispatch when callee is known at compile time

### Future Optimizations (post-v1)
