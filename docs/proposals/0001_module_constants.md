- Feature Name: Module Constants
- Start Date: 2026-01-30
- Proposal PR: 
- Flux Issue: 

# Proposal 0001: Module Constants

## Summary
[summary]: #summary

This proposal covers **two aspects** of module constants in Flux: This proposal covers **two aspects** of module constants in Flux:

## Motivation
[motivation]: #motivation

Flux modules originally only supported function declarations. Module-level constants enable: Flux modules originally only supported function declarations. Module-level constants enable:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Syntax

```flux
module Flow.Math {
    // Compile-time constants (order doesn't matter!)
    let TAU = PI * 2;  // Can reference PI even though it's defined below
    let PI = 3.141592653589793;
    let E = 2.718281828459045;

    // Functions can use module constants
    fn circle_area(r) {
        PI * r * r;
    }

    fn circle_circumference(r) {
        TAU * r;
    }
}

// Usage
print(Flow.Math.PI);              // 3.141592653589793
print(Flow.Math.circle_area(5));  // 78.53981633974483
```

### Syntax

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **What's Allowed (Constant Expressions):** // Arithmetic on numbers let TAU = PI * 2; let DOUBLE_E = E + E; let HALF_PI = PI / 2; - **What's NOT Allowed:** // ERROR: Variables...
- **What's Allowed (Constant Expressions):** // Arithmetic on numbers let TAU = PI * 2; let DOUBLE_E = E + E; let HALF_PI = PI / 2;
- **What's NOT Allowed:** // ERROR: Circular dependencies let A = B + 1; let B = A + 1; // A and B depend on each other
- **Order Independence (Automatic Dependency Resolution):** Constants can be defined in **any order**. The compiler automatically resolves dependencies: ```flux module Example { let C = B * 2; // OK: B will be resolved let B = A + 1; //...
- **Privacy:** Use `_` prefix for private constants (existing convention): ```flux module Math { let _INTERNAL = 42; // Private let PI = 3.14159; // Public
- **Chosen Approach: Compile-Time Constants:** | Aspect | Compile-Time | Load-Time | Lazy | |--------|--------------|-----------|------| | **Runtime cost** | Zero | Once per load | On access | | **Predictability** | High | M...

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

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### 4. Pattern for Future Features

Establishes template for adding new compiler features:
1. Create analysis function in dedicated module (pure logic)
2. Add orchestration call in compiler.rs (stateful operations)
3. Write unit tests for pure functions

**Future applications:**
- Pattern matching analysis (`pattern_eval/`)
- Type inference (`type_infer/`)
- Effect checking (`effect_check/`)

### 4. Pattern for Future Features
