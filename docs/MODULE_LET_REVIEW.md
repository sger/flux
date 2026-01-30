# Module Let Bindings Proposal Review

**Reviewer:** Design Review
**Date:** 2026-01-30
**Document:** MODULE_LET_PROPOSAL.md
**Status:** Ready for Implementation

---

## Executive Summary

The proposal for compile-time module constants is **well-designed and ready for implementation**. The chosen approach (compile-time evaluation like Elixir) is appropriate for Flux's current needs and aligns with functional programming principles.

**Verdict:** Approve with minor suggestions

---

## Strengths

### 1. Clear Rationale

The document clearly explains:
- Why compile-time over load-time or lazy evaluation
- How it compares to Haskell and Elixir
- What trade-offs were considered

### 2. Appropriate Scope

The feature is well-scoped:
- Solves a real need (Flow.Math constants)
- Doesn't over-engineer (no lazy evaluation complexity)
- Follows existing patterns (`_` prefix for private)

### 3. Good Language Comparison

The Haskell/Elixir comparison table is valuable for understanding design choices. The decision to follow Elixir's approach (compile-time, sequential) is well-justified.

### 4. Concrete Examples

The proposal includes practical examples:
- Flow.Math constants
- Configuration defaults
- Lookup tables
- Error codes

### 5. Implementation Details

The Rust code snippets for `eval_const_expr` and module compilation changes are clear and implementable.

---

## Concerns & Suggestions

### Concern 1: Hash Literal Support

**Issue:** The proposal mentions hash literals as allowed:
```flux
let CONFIG = {
    "timeout": 30000,
    "retries": 3
};
```

**Question:** Can hash keys be computed expressions, or only literals?

**Suggestion:** Clarify that hash keys must also be constant expressions:
```flux
// OK: literal keys
let CONFIG = { "key": 1 };

// ERROR: computed keys
let KEY = "key";
let CONFIG = { KEY: 1 };  // Should this work?
```

**Recommendation:** Allow identifier references as keys if they resolve to constants:
```flux
let KEY_NAME = "timeout";
let CONFIG = { KEY_NAME: 30000 };  // OK - KEY_NAME is a constant
```

### Concern 2: Array/Hash Size Limits

**Issue:** No mention of size limits for constant arrays/hashes.

**Suggestion:** Consider adding a compile-time limit to prevent:
```flux
let HUGE = [1, 2, 3, ..., 1000000];  // Memory issues?
```

**Recommendation:** Add a reasonable limit (e.g., 10KB of constant data per module) with clear error message.

### Concern 3: Negation and Prefix Operators

**Issue:** The `eval_const_expr` code shows binary operators but not prefix operators.

**Question:** Are these valid?
```flux
let NEGATIVE = -1;
let NOT_TRUE = !true;
```

**Recommendation:** Add explicit support for prefix operators:
```rust
Expression::Prefix { operator, right } => {
    let r = self.eval_const_expr(right, defined)?;
    match (operator.as_str(), &r) {
        ("-", Object::Integer(n)) => Ok(Object::Integer(-n)),
        ("-", Object::Float(f)) => Ok(Object::Float(-f)),
        ("!", Object::Boolean(b)) => Ok(Object::Boolean(!b)),
        _ => Err(...),
    }
}
```

### Concern 4: Cross-Module Constant References

**Issue:** The proposal shows:
```flux
module Math {
    import Constants;
    let TAU = Constants.PI * 2;
}
```

**Questions:**
1. How does the compiler know `Constants.PI` is a constant vs a function?
2. What's the compilation order if `Constants` is in another file?

**Recommendation:** Add a section on cross-module constant resolution:
- Constants from imported modules are resolved at compile time
- Import order determines compilation order
- Circular module dependencies with constants should error clearly

### Concern 5: Error Recovery

**Issue:** What happens when a constant fails to evaluate?

**Example:**
```flux
module M {
    let A = 1 / 0;  // Division by zero at compile time
    let B = A + 1;  // What error does B get?
}
```

**Recommendation:**
- Stop at first error (don't cascade)
- Clear error: "Evaluation of constant 'A' failed: division by zero"

---

## Minor Suggestions

### 1. Add More Operators

Consider supporting these at compile time:
- `<=`, `>=` (comparison)
- `%` (modulo)
- String comparison

### 2. Document Unsupported Expressions

Add explicit list of what's NOT allowed:
- Function calls
- If/else expressions
- Match expressions
- Array indexing
- Member access (except module constants)

### 3. Consider `const` Keyword

**Alternative syntax:**
```flux
module Math {
    const PI = 3.14159;  // Explicit compile-time
    let COMPUTED = PI * 2;  // Could be either?
}
```

**Recommendation:** Keep `let` for simplicity. The context (module level) makes it clear these are constants. A `const` keyword could be added later if needed.

### 4. Future: Type Annotations

Reserve syntax for future type annotations:
```flux
let PI: Float = 3.14159;
```

Ensure the parser can handle this even if types aren't checked yet.

---

## Implementation Checklist

Based on the proposal, here's a suggested implementation order:

### Phase 1: Parser Changes
- [ ] Allow `Statement::Let` in module body
- [ ] Update module validation to accept let statements
- [ ] Add tests for parsing module constants

### Phase 2: Constant Evaluator
- [ ] Implement `eval_const_expr` function
- [ ] Handle literals (int, float, string, bool)
- [ ] Handle arrays and hashes
- [ ] Handle binary operations (+, -, *, /, &&, ||)
- [ ] Handle prefix operations (-, !)
- [ ] Handle identifier references to earlier constants
- [ ] Add comprehensive error messages

### Phase 3: Compiler Integration
- [ ] Modify `compile_module_statement` to process let bindings
- [ ] Store evaluated constants in module_constants map
- [ ] Inline constants at use sites
- [ ] Handle qualified name resolution (Math.PI)

### Phase 4: Testing
- [ ] Unit tests for constant evaluator
- [ ] Integration tests for module constants
- [ ] Error case tests (forward ref, circular, non-constant)
- [ ] Cross-module constant tests

### Phase 5: Documentation
- [ ] Update language documentation
- [ ] Add examples to Flow.Math
- [ ] Update error catalog

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Constant evaluator bugs | Medium | Medium | Comprehensive tests |
| Cross-module ordering issues | Low | High | Clear dependency error messages |
| Backward compatibility | None | None | New feature, no existing code affected |
| Performance regression | None | None | Compile-time only |

---

## Comparison with Alternatives

### Alternative 1: Load-Time Evaluation

**Rejected because:**
- Adds runtime overhead
- Less predictable
- More complex to implement
- Not needed for Flux's use cases

### Alternative 2: Lazy Evaluation (Haskell-style)

**Rejected because:**
- Significantly more complex
- Requires thunk infrastructure
- Unpredictable evaluation timing
- Overkill for module constants

### Alternative 3: No Module Constants

**Rejected because:**
- Flow.Math needs PI, E, etc.
- Users expect this feature
- Other FP languages have it

---

## Final Verdict

**Status: APPROVED**

The proposal is well-thought-out and addresses a real need. The compile-time approach is the right choice for Flux at this stage.

**Recommended Actions:**
1. Clarify hash key handling (Concern 1)
2. Add prefix operator support (Concern 3)
3. Document cross-module resolution (Concern 4)
4. Proceed with implementation

---

## Appendix: Test Cases

### Happy Path Tests

```flux
// Basic constants
module Test1 {
    let X = 42;
    let Y = 3.14;
    let Z = "hello";
    let B = true;
}

// Computed constants
module Test2 {
    let A = 1;
    let B = A + 1;
    let C = B * 2;
    let D = A + B + C;
}

// Arrays and hashes
module Test3 {
    let ARR = [1, 2, 3];
    let HASH = { "a": 1, "b": 2 };
}

// String operations
module Test4 {
    let HELLO = "Hello, ";
    let WORLD = "World!";
    let GREETING = HELLO + WORLD;
}

// Boolean operations
module Test5 {
    let A = true;
    let B = false;
    let C = A && B;
    let D = A || B;
    let E = !A;
}
```

### Error Cases

```flux
// Forward reference
module Err1 {
    let A = B;  // E231
    let B = 1;
}

// Function call
module Err2 {
    let A = compute();  // E234
}

// Non-constant expression
module Err3 {
    let A = if true { 1 } else { 2 };  // E230
}

// Type mismatch
module Err4 {
    let A = "hello" + 42;  // E233
}

// Circular dependency
module Err5 {
    let A = B + 1;  // E232
    let B = A + 1;
}
```
