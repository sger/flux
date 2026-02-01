# Flux v0.0.3 Implementation Plan

## Overview

Create a comprehensive plan for Flux v0.0.3 focusing on the most important language features and compiler improvements based on documentation analysis and current compiler capabilities.

---

## Current State (v0.0.2 - Complete)

**Implemented Features:**
- Core operators: `<=`, `>=`, `%`, `&&`, `||`, `|>`
- Either type: `Left`/`Right` with pattern matching
- Lambda shorthand: `\x -> expr`
- 35 builtin functions (array, string, hash, math, type checking)
- Module system with imports, aliases, and forward references
- Pattern matching: literals, wildcards, `None`/`Some`, `Left`/`Right`
- Module-level constants with compile-time evaluation
- String interpolation
- Immutability enforcement

**Architecture:**
- Bytecode VM with 44 opcodes
- Closure support with free variable capture
- Short-circuit evaluation
- Comprehensive error codes (E001-E045+)
- Debug info tracking

---

## Version Goals for v0.0.3

**Primary Objectives:**
1. **Foundation:** Strengthen compiler architecture (error registry, AST spans)
2. **Usability:** Enhance pattern matching and add Tuple type
3. **Ergonomics:** Provide utility libraries for Option/Either
4. **Iteration:** Enable for-loop syntax for common patterns

**Success Criteria:**
- Zero breaking changes to v0.0.2
- Test coverage ≥ 85% for new features
- Performance regression < 5%
- All features documented with examples

---

## Proposed Milestones

### M1: Complete Error Code Registry
**Priority:** CRITICAL | **Effort:** Small (2-3 days) | **Risk:** Low
**Proposal:** [002_error_code_registry.md](../proposals/002_error_code_registry.md) (Enum-Based approach)

**Goal:** Complete the centralized error code registry to prevent duplicates and improve maintainability.

**Current State:**
- ✅ `src/frontend/error_codes.rs` exists with 31 error codes (E001-E106)
- ✅ `ErrorCode` struct with code + title
- ✅ `ERROR_CODES` array and helper functions
- ⚠️ Missing 26 v0.0.2 error codes (E120-E522 documented but not implemented)
- ⚠️ Error messages/hints hardcoded in parser.rs and compiler.rs (29 instances)

**Chosen Approach:**
Using the **Enum-Based Catalog** approach from proposal 002 (Alternative section):
- ✅ No external dependencies (toml/serde)
- ✅ Compile-time type safety
- ✅ Extends existing error_codes.rs structure
- ✅ Simpler implementation

**Implementation:**
1. **Extend ErrorCode struct** - Add `message` template and optional `hint` fields
   ```rust
   pub struct ErrorCode {
       pub code: &'static str,
       pub title: &'static str,
       pub message: &'static str,      // NEW: template with {} placeholders
       pub hint: Option<&'static str>, // NEW: optional hint
   }
   ```

2. **Add missing v0.0.2 error codes** - Implement 26 documented codes:
   - E120-E122 (lambda syntax errors)
   - E130-E131 (pipe errors)
   - E140-E141 (Either constructor errors)
   - E220 (short-circuit errors)
   - E230-E235, E240 (module constant errors)
   - E320-E321 (division/modulo by zero)
   - E330-E331 (Either unwrap errors)
   - E420-E421, E430 (type errors)
   - E520-E522 (module constant errors)

3. **Refactor usage sites** - Replace 29 hardcoded error strings with error_codes constants

4. **Add duplicate detection test** - Ensure no code collisions in ERROR_CODES array

5. **Generate ERROR_CATALOG_v0.0.3.md** - Document all error codes with examples

6. **Standardize error format** - Ensure consistent formatting across all error types

**Standardized Error Format:**

All errors use the same structure with only the **header** varying by type:

```
Compiler errors:
  error[E007]: UNDEFINED VARIABLE
    --> examples/file.flx:10:15
     |
  10 |     let x = uppr("hello");
     |             ^^^^
     |
     = error: I can't find a value named `uppr`.
     = hint: Define it first: let uppr = ...;

Runtime errors:
  runtime error: WRONG NUMBER OF ARGUMENTS
    --> examples/file.flx:25:7
     |
  25 | print(substring("HELLO", 0));
     |        ^^^^^^^^^^^^^^^^^^^^^
     |
     = error: function substring/3 expects 3 arguments, got 2
     = hint: substring(s, start, end)

  Stack trace:
    at <main> (examples/file.flx:25:7)
```

**Common elements:**
1. Header (differs by error type)
2. Source location: `--> file:line:col`
3. Code snippet with line numbers
4. Caret showing exact position (requires M2)
5. Error message: `= error: ...`
6. Hint: `= hint: ...`
7. Stack trace (runtime only)

**Benefits:**
- ✅ Builds on existing foundation (less work)
- ✅ Single source of truth for all error information
- ✅ Consistent error messages and hints
- ✅ Type-safe, compile-time checking
- ✅ No external dependencies (toml/serde)
- ✅ Easy documentation generation

**Critical Files:**
- `src/frontend/error_codes.rs` (extend struct, add 26 codes)
- `src/frontend/diagnostic.rs` (update to use message/hint)
- `src/frontend/parser.rs` (refactor 15+ error sites)
- `src/bytecode/compiler.rs` (refactor 10+ error sites)
- `tests/error_codes_tests.rs` (new: duplicate detection)
- `docs/reference/ERROR_CATALOG_v0.0.3.md` (new: generated catalog)

---

### M2: AST Spans for All Nodes
**Priority:** CRITICAL | **Effort:** Medium (4-5 days) | **Risk:** Medium

**Goal:** Add source position tracking to all AST nodes for precise diagnostics.

**Enables:** Precise `line:col` locations and caret positioning for the standardized error format (see M1)

**Implementation:**
1. Add `Span` field to all Expression and Statement variants
2. Update parser to track and attach spans
3. Enhance diagnostic messages with precise spans
4. Store span info in debug symbols
5. **Update diagnostic formatter** - Use spans for caret positioning

**Benefits:**
- **Enables standardized error format** - Precise `line:col` and caret positioning
- Better error messages with exact locations
- Foundation for IDE/LSP support
- Multi-line span highlighting
- Improved debugging

**Example Output:**
```
error[E007]: UNDEFINED VARIABLE
  --> examples/file.flx:10:15
   |
10 |     let x = uppr("hello");
   |             ^^^^
   |
   = error: I can't find a value named `uppr`.
   = hint: Did you mean `upper`?
```

**Critical Files:**
- `src/frontend/expression.rs` (add spans)
- `src/frontend/statement.rs` (add spans)
- `src/frontend/parser.rs` (track positions)
- `src/frontend/position.rs` (span utilities)
- `src/frontend/diagnostic.rs` (use spans)

---

### M3: Pattern Matching Guards
**Priority:** HIGH | **Effort:** Medium (3-5 days) | **Risk:** Low

**Goal:** Enable conditional logic within pattern matching.

**Syntax:**
```flux
match value {
    pattern if condition -> expr;
    pattern if other_condition -> expr;
    _ -> default;
}
```

**Implementation:**
1. Parse `if` keyword after pattern in match arms
2. Extend `MatchArm` AST with optional guard expression
3. Compile guard evaluation with conditional jump
4. Test with various pattern types

**Benefits:**
- More expressive pattern matching
- Reduces nested if statements
- Common feature in functional languages

**Example:**
```flux
match age {
    x if x >= 18 -> "Adult";
    x if x >= 13 -> "Teen";
    _ -> "Child";
}
```

**Critical Files:**
- `src/frontend/parser.rs` (parse guards)
- `src/frontend/expression.rs` (extend MatchArm)
- `src/bytecode/compiler.rs` (compile guards)
- `tests/vm_tests.rs` (test guards)

---

### M4: Tuple Type
**Priority:** HIGH | **Effort:** Medium (4-6 days) | **Risk:** Medium

**Goal:** Add tuple type for multi-value returns and fixed-size collections.

**Syntax:**
```flux
let pair = (1, "hello");       // Tuple literal
let x = pair.0;                 // Access by index
let (a, b) = pair;              // Pattern matching (M6)
```

**Implementation:**
1. Add `Object::Tuple(Vec<Object>)` variant
2. Add `OpTuple(size)` opcode
3. Parse tuple literals: `(a, b, c)`
4. Parse tuple access: `tuple.0`
5. Distinguish `(expr)` from `(expr,)` (single-element tuple)
6. Add `is_tuple(x)` builtin

**Benefits:**
- Multi-value returns without arrays
- Fixed-size heterogeneous collections
- Foundation for tuple patterns

**Example:**
```flux
fun min_max(arr) {
    (min(arr), max(arr))
}

let stats = min_max([3, 1, 4, 1, 5]);
print(stats.0);  // 1
print(stats.1);  // 5
```

**Critical Files:**
- `src/runtime/object.rs` (add Tuple variant)
- `src/bytecode/op_code.rs` (add OpTuple)
- `src/frontend/parser.rs` (parse tuples)
- `src/bytecode/compiler.rs` (compile tuples)
- `src/runtime/vm.rs` (execute OpTuple)

---

### M5: Option & Either Utility Modules
**Priority:** HIGH | **Effort:** Small (2-3 days) | **Risk:** Low

**Goal:** Provide ergonomic utilities as Flux library modules.

**Flow.Option Module:**
- `map(opt, f)` - transform inner value
- `unwrap_or(opt, default)` - get value or default
- `and_then(opt, f)` - chain operations
- `is_some(opt)`, `is_none(opt)` - type checks
- `filter(opt, pred)` - conditional unwrap

**Flow.Either Module:**
- `map(either, f)` - transform Right
- `map_left(either, f)` - transform Left
- `unwrap_or(either, default)` - get Right or default
- `and_then(either, f)` - chain operations
- `is_left(either)`, `is_right(either)` - type checks
- `to_option(either)` - convert to Option

**Implementation:**
1. Create `examples/flow/Option.flx`
2. Create `examples/flow/Either.flx`
3. Add comprehensive examples
4. Write integration tests

**Benefits:**
- Reduce boilerplate in common patterns
- Encourage functional error handling
- Pure Flux code (no compiler changes)

**Example:**
```flux
import Flow.Option as Opt
import Flow.Either as E

fun process(data) {
    validate(data)
        |> E.and_then(\valid -> transform(valid))
        |> E.map(\result -> result * 2)
        |> E.unwrap_or(0)
}
```

**Critical Files:**
- `examples/flow/Option.flx` (new)
- `examples/flow/Either.flx` (new)
- `tests/flow_library_tests.rs` (new)

---

### M6: For-Loop Syntax
**Priority:** MEDIUM | **Effort:** Medium (4-5 days) | **Risk:** Medium

**Goal:** Provide familiar iteration syntax as syntactic sugar.

**Syntax:**
```flux
for item in collection {
    print(item);
}
```

**Implementation:**
1. Add `for` keyword to lexer
2. Parse `for IDENT in EXPR BLOCK`
3. Desugar to recursive function:
   ```flux
   // User writes:
   for x in arr { print(x); }

   // Compiler generates:
   {
       let __iter = arr;
       fun __loop(__i) {
           if __i >= len(__iter) {
               None;
           } else {
               let x = __iter[__i];
               print(x);
               __loop(__i + 1);
           }
       }
       __loop(0);
   }
   ```

**Limitations:**
- Without TCO, large iterations may stack overflow
- Document recommended alternatives for large data
- Mark for future TCO optimization

**Benefits:**
- Familiar syntax for simple iterations
- No new opcodes needed
- Desugars to existing constructs

**Critical Files:**
- `src/frontend/token_type.rs` (add for keyword)
- `src/frontend/parser.rs` (parse for)
- `src/frontend/statement.rs` (For statement AST)
- `src/bytecode/compiler.rs` (desugar to recursion)

---

### M7: Array/Tuple Pattern Matching
**Priority:** MEDIUM | **Effort:** Large (5-7 days) | **Risk:** High

**Goal:** Enable destructuring arrays and tuples in match expressions.

**Syntax:**
```flux
match list {
    [] -> "empty";
    [x] -> "single";
    [x, y] -> "pair";
    [head, ...tail] -> "list";
}

match tuple {
    (x, y) -> x + y;
}
```

**Implementation:**
1. Parse array literal patterns: `[a, b, c]`
2. Parse rest patterns: `[head, ...tail]`
3. Parse tuple patterns: `(a, b)`
4. Compile length checks
5. Compile element extraction
6. Handle nested patterns

**Benefits:**
- Essential for data manipulation
- Completes pattern matching system
- Enables functional data processing

**Complexity:**
- Many edge cases (empty, single, rest, nested)
- Pattern compilation complexity
- Exhaustiveness checking

**Critical Files:**
- `src/frontend/parser.rs` (parse patterns)
- `src/bytecode/compiler.rs` (compile patterns)
- `src/frontend/expression.rs` (pattern AST)
- `tests/vm_tests.rs` (pattern tests)

---

## Implementation Phases

### Phase 1: Foundation (Weeks 1-2)
**Focus:** Architectural improvements

1. **Week 1:**
   - M1: Central Error Code Registry (2-3 days)
   - Start M2: AST Spans infrastructure

2. **Week 2:**
   - Complete M2: AST Spans (4-5 days)
   - Testing and integration

**Deliverable:** Improved error reporting and architectural foundation

---

### Phase 2: Language Features (Weeks 3-4)
**Focus:** Pattern matching and tuples

1. **Week 3:**
   - M3: Pattern Matching Guards (3-5 days)
   - Start M4: Tuple Type

2. **Week 4:**
   - Complete M4: Tuple Type (4-6 days)
   - Integration testing

**Deliverable:** Guards and tuples functional

---

### Phase 3: Ergonomics (Weeks 5-6)
**Focus:** Usability and iteration

1. **Week 5:**
   - M5: Option & Either Utilities (2-3 days)
   - Start M6: For-Loop Syntax

2. **Week 6:**
   - Complete M6: For-Loop Syntax (4-5 days)
   - Documentation and examples

**Deliverable:** Utility modules and for-loops

---

### Phase 4: Advanced Patterns (Weeks 7-8)
**Focus:** Complete pattern matching system

1. **Week 7-8:**
   - M7: Array/Tuple Pattern Matching (5-7 days)
   - Comprehensive testing
   - Performance validation

**Deliverable:** Full pattern matching capability

---

## Total Timeline: 8 weeks (2 months)

---

## Feature Dependencies

```
Error Registry (M1) ──┐
                      ├──> AST Spans (M2)
Pattern Guards (M3) ──┘

Tuple Type (M4) ────────> Array/Tuple Patterns (M7)

Option/Either Utils (M5)  [Independent]

For-Loop Syntax (M6)     [Independent]
```

**Critical Path:** M1 → M2 → M3 → M4 → M7

**Parallel Track:** M5, M6 can be developed alongside

---

## OUT OF SCOPE for v0.0.3

**Language Features (Deferred):**
- Structs/Records with named fields → v0.0.4
- Enums/ADTs beyond Either → v0.0.4
- Type annotations and inference → v1.0
- Default/named parameters → v0.0.4
- Block comments `/* */` → v0.0.4
- If-let expressions → v0.0.4
- Range syntax `1..10` → v0.0.4
- List comprehensions → v0.0.4
- Full Tail Call Optimization → v0.0.4 (exploration only in v0.0.3)

**Advanced Features (v2.0+):**
- Effect system
- Reactive streams
- Actor model
- Generics
- Full type inference

**Standard Library (v0.0.4+):**
- Flow.List module (needs TCO)
- Flow.String module
- Flow.Math module
- Flow.Func module

---

## Risk Assessment

### High Risk / High Value
- **Array/Tuple Pattern Matching:** Complex but essential
  - *Mitigation:* Incremental implementation, extensive testing
  - *Fallback:* Ship with limited pattern support

### Medium Risk / High Value
- **Tuple Type:** New object type affects multiple layers
  - *Mitigation:* Thorough layer-by-layer testing
  - *Fallback:* Launch without patterns initially

- **For-Loop Syntax:** Stack overflow without TCO
  - *Mitigation:* Clear documentation, recommend alternatives
  - *Fallback:* Keep as sugar with warnings

- **AST Spans:** Pervasive parser changes
  - *Mitigation:* Compiler checks for missing spans
  - *Fallback:* Partial spans better than none

### Low Risk / High Value
- **Pattern Matching Guards:** Well-understood feature
- **Option/Either Utilities:** Pure library code
- **Error Code Registry:** Refactoring existing code

---

## Success Metrics

### Functional
- [ ] All 7 milestones pass integration tests
- [ ] Zero breaking changes to v0.0.2
- [ ] Pattern matching covers 90% of common use cases
- [ ] Tuple type works in all contexts

### Quality
- [ ] Test coverage ≥ 85% for new code
- [ ] All error messages use error registry
- [ ] All AST nodes have spans
- [ ] Documentation complete for all features

### Performance
- [ ] Regression < 5% on v0.0.2 benchmarks
- [ ] Pattern matching compilation O(n) in pattern size
- [ ] Tuple operations same speed as array indexing

### Developer Experience
- [ ] Error messages show exact source locations
- [ ] Option/Either utilities reduce boilerplate
- [ ] For-loop syntax natural for simple iterations
- [ ] Guards eliminate nested match statements

---

## What's Next (v0.0.4)?

**High Priority:**
1. Tail Call Optimization - Enable stdlib development
2. Range Type - `1..10` syntax
3. List Comprehensions - Concise data transformations
4. If-Let Expressions - Ergonomic optional handling
5. Structs/Records - Named field types

**Foundation for v0.1.0 (Stdlib):**
- Flow.List module (map, filter, reduce)
- Flow.String module (extended utilities)
- Flow.Math module (advanced operations)
- Flow.Func module (composition, etc.)

---

## Verification Plan

### Unit Tests
- Each milestone has dedicated test suite
- Test coverage for edge cases
- Error handling tests

### Integration Tests
- Cross-feature interaction tests
- Realistic workflow examples
- Performance regression tests

### Manual Testing
- Example programs for each feature
- Documentation code samples
- Real-world use cases

### Continuous Validation
```bash
cargo test                    # All tests
cargo test --test integration # Integration tests only
cargo bench                   # Performance benchmarks
flux fmt examples/**/*.flx    # Formatter validation
```

---

## Summary

This plan for v0.0.3 balances:
- **Quick wins:** Option/Either utilities, guards (weeks 3-5)
- **Foundational work:** Error registry, AST spans (weeks 1-2)
- **High-value features:** Tuples, for-loops (weeks 4-6)
- **Advanced capabilities:** Array/tuple patterns (weeks 7-8)

The 8-week timeline is realistic with clear milestones, dependencies, and risk mitigation strategies. Each phase delivers independently testable value.
