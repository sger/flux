# Flux v0.0.2 Roadmap

**Target Release:** TBD
**Theme:** Core Language Completion & FP Essentials

This roadmap focuses on completing the core language features needed for practical functional programming in Flux.

---

## Release Goals

1. **Complete operator set** - All essential operators for real programs
2. **Pipe operator** - Idiomatic functional data transformation
3. **Either type** - Proper error handling with `Left`/`Right`
4. **Lambda shorthand** - Concise anonymous functions
5. **Essential builtins** - Array and string operations

---

## Milestone Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  M1: Core Operators          ████████████████████░░░░░░░░░░░░  │ 67% (2/3)
│  M2: Pipe Operator           ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
│  M3: Either Type             ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
│  M4: Lambda Shorthand        ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
│  M5: Essential Builtins      ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
│  M6: Polish & Release        ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Milestone 1: Core Operators

**Priority:** Critical
**Status:** In Progress (2/3 complete)

### 1.1 Comparison Operators: `<=` and `>=` ✅

**Status:** COMPLETED

| Task | File(s) | Status |
|------|---------|--------|
| Add `Lte`, `Gte` tokens | `token_type.rs` | ✅ Done |
| Lexer: recognize `<=` and `>=` | `lexer.rs` | ✅ Done |
| Parser: parse as infix operators | `parser.rs` | ✅ Done |
| Add `OpLessThanOrEqual`, `OpGreaterThanOrEqual` opcodes | `op_code.rs` | ✅ Done |
| Compiler: emit new opcodes | `compiler.rs` | ✅ Done |
| VM: execute comparisons | `vm.rs` | ✅ Done |
| Tests | `tests/` | ✅ Done |
| Update examples | `examples/basics/comparison.flx` | ✅ Done |

**Acceptance Criteria:** ✅ ALL PASSING
```flux
print(5 <= 5);   // true  ✅
print(5 <= 4);   // false ✅
print(3 >= 3);   // true  ✅
print(3 >= 4);   // false ✅
```

**Implementation Notes:**
- Opcodes are sequential (OpLessThanOrEqual = 8, OpGreaterThanOrEqual = 9)
- Works with integers, floats, mixed numeric types, and strings
- 18 comprehensive unit tests added
- Example file updated with 6 test cases

### 1.2 Modulo Operator: `%` ✅

**Status:** COMPLETED

| Task | File(s) | Status |
|------|---------|--------|
| Add `Percent` token | `token_type.rs` | ✅ Done |
| Lexer: recognize `%` | `lexer.rs` | ✅ Done |
| Parser: parse with `Product` precedence | `parser.rs` | ✅ Done |
| Add `OpMod` opcode | `op_code.rs` | ✅ Done |
| VM: execute modulo (int and float) | `vm.rs` | ✅ Done |
| Tests | `tests/` | ✅ Done |
| Create example file | `examples/basics/modulo.flx` | ✅ Done |

**Acceptance Criteria:** ✅ ALL PASSING
```flux
print(10 % 3);    // 1      ✅
print(7 % 2);     // 1      ✅ (odd check)
print(8 % 2);     // 0      ✅ (even check)
print(10.5 % 3);  // 1.5    ✅ (float modulo)
```

**Implementation Notes:**
- Opcode `OpMod = 5` (sequential numbering)
- Works with integers, floats, and mixed numeric types
- 18 comprehensive unit tests added
- Example file created with 9 test cases
- Follows same precedence as `*` and `/` (Product level)

### 1.3 Logical Operators: `&&` and `||`

| Task | File(s) | Effort |
|------|---------|--------|
| Add `And`, `Or` tokens | `token_type.rs` | Small |
| Lexer: recognize `&&` and `\|\|` | `lexer.rs` | Small |
| Add `LogicalAnd`, `LogicalOr` precedence levels | `precedence.rs` | Small |
| Parser: parse with short-circuit semantics | `parser.rs` | Medium |
| Compiler: emit jump instructions | `compiler.rs` | Medium |
| Tests (including short-circuit verification) | `tests/` | Medium |

**Implementation Note:** These cannot be simple opcodes - they require conditional jumps for short-circuit evaluation.

**Compilation Strategy:**
```
// a && b compiles to:
evaluate a
OpJumpNotTruthy end
OpPop
evaluate b
end:

// a || b compiles to:
evaluate a
OpJumpTruthy end
OpPop
evaluate b
end:
```

**Acceptance Criteria:**
```flux
print(true && true);    // true
print(true && false);   // false
print(false && true);   // false (right side not evaluated)

print(true || false);   // true (right side not evaluated)
print(false || true);   // true
print(false || false);  // false

// Short-circuit verification
let called = false;
fun side_effect() { called = true; false }
false && side_effect();
print(called);  // false (side_effect not called)
```

### 1.4 Milestone 1 Deliverables

- [x] ✅ Comparison operators: `<=`, `>=` (DONE)
- [x] ✅ Modulo operator: `%` (DONE)
- [ ] Logical operators: `&&`, `||`
- [ ] Proper precedence: `&&` binds tighter than `||`
- [ ] Short-circuit evaluation for `&&` and `||`
- [ ] Unit tests for all operators
- [ ] Integration tests with complex expressions
- [ ] Updated error messages

---

## Milestone 2: Pipe Operator

**Priority:** Critical
**Status:** Not Started
**Dependencies:** None (can be parallel with M1)

### 2.1 Implementation

| Task | File(s) | Effort |
|------|---------|--------|
| Add `Pipe` token for `\|>` | `token_type.rs` | Small |
| Lexer: recognize `\|>` | `lexer.rs` | Small |
| Add `Pipe` precedence (lowest) | `precedence.rs` | Small |
| Parser: parse as left-associative infix | `parser.rs` | Small |
| Compiler: transform to function call | `compiler.rs` | Medium |
| Tests | `tests/` | Medium |

**Semantics:**
```flux
// a |> f(b, c) transforms to f(a, b, c)
// Left side becomes FIRST argument

x |> f           // f(x)
x |> f(y)        // f(x, y)
x |> f(y, z)     // f(x, y, z)
x |> f |> g      // g(f(x))
```

**Precedence:** Lower than all other operators
```flux
a + b |> f       // f(a + b)
a |> f + b       // (f(a)) + b  -- probably not intended, document this
```

### 2.2 Acceptance Criteria

```flux
// Basic usage
let result = 5 |> double;  // double(5) = 10

// Chaining
let result = [1, 2, 3, 4, 5]
    |> filter(fun(x) { x > 2 })
    |> map(fun(x) { x * 2 })
    |> first;
print(result);  // 6

// With multiple arguments
let result = "hello"
    |> split(",")
    |> first;

// Complex pipeline
data
    |> validate
    |> transform
    |> save
    |> notify;
```

### 2.3 Milestone 2 Deliverables

- [ ] `|>` operator parsing and compilation
- [ ] Left-associativity
- [ ] Works with any function call
- [ ] Unit tests
- [ ] Documentation with examples
- [ ] Example file: `examples/pipe_operator.flx`

---

## Milestone 3: Either Type

**Priority:** High
**Status:** Not Started
**Dependencies:** M1 (for practical usage)

### 3.1 Runtime Support

| Task | File(s) | Effort |
|------|---------|--------|
| Add `Object::Left(Box<Object>)` | `object.rs` | Small |
| Add `Object::Right(Box<Object>)` | `object.rs` | Small |
| Add `HashKey` support if needed | `hash_key.rs` | Small |
| Update `type_name()` | `object.rs` | Small |
| Update `Display` impl | `object.rs` | Small |

### 3.2 Bytecode Support

| Task | File(s) | Effort |
|------|---------|--------|
| Add `OpLeft`, `OpRight` opcodes | `op_code.rs` | Small |
| Add `OpIsLeft`, `OpIsRight` opcodes | `op_code.rs` | Small |
| Add `OpUnwrapLeft`, `OpUnwrapRight` opcodes | `op_code.rs` | Small |
| VM: implement all Either opcodes | `vm.rs` | Medium |

### 3.3 Parser Support

| Task | File(s) | Effort |
|------|---------|--------|
| Add `Left`, `Right` keywords | `token_type.rs` | Small |
| Parser: `Left(expr)` expression | `parser.rs` | Small |
| Parser: `Right(expr)` expression | `parser.rs` | Small |
| Parser: `Left(pat)` pattern | `parser.rs` | Small |
| Parser: `Right(pat)` pattern | `parser.rs` | Small |

### 3.4 Compiler Support

| Task | File(s) | Effort |
|------|---------|--------|
| Compile `Left(expr)` | `compiler.rs` | Small |
| Compile `Right(expr)` | `compiler.rs` | Small |
| Pattern matching for Either | `compiler.rs` | Medium |

### 3.5 Acceptance Criteria

```flux
// Construction
let success = Right(42);
let failure = Left("error message");

// Pattern matching
fun handle(result) {
    match result {
        Right(value) -> print("Success: #{value}");
        Left(err) -> print("Error: #{err}");
    }
}

// Practical usage
fun divide(a, b) {
    if b == 0 {
        Left("division by zero");
    } else {
        Right(a / b);
    }
}

let result = divide(10, 2);
match result {
    Right(v) -> print(v);  // 5
    Left(e) -> print(e);
}

let result = divide(10, 0);
match result {
    Right(v) -> print(v);
    Left(e) -> print(e);  // "division by zero"
}
```

### 3.6 Milestone 3 Deliverables

- [ ] `Left` and `Right` object types
- [ ] Construction syntax: `Left(x)`, `Right(x)`
- [ ] Pattern matching: `Left(e) -> ...`, `Right(v) -> ...`
- [ ] Display formatting
- [ ] Unit tests
- [ ] Example file: `examples/either_type.flx`

---

## Milestone 4: Lambda Shorthand

**Priority:** Medium
**Status:** Not Started
**Dependencies:** None

### 4.1 Syntax Choice

**Chosen syntax:** `\params -> expr`

```flux
// Single parameter
\x -> x * 2

// Multiple parameters
\x, y -> x + y

// With block body
\x -> {
    let doubled = x * 2;
    doubled + 1
}
```

### 4.2 Implementation

| Task | File(s) | Effort |
|------|---------|--------|
| Add `Backslash` token | `token_type.rs` | Small |
| Lexer: recognize `\` | `lexer.rs` | Small |
| Parser: parse lambda expression | `parser.rs` | Medium |
| AST: Lambda expression type (or reuse Function) | `expression.rs` | Small |
| Compiler: compile lambda (same as Function) | `compiler.rs` | Small |
| Tests | `tests/` | Medium |

### 4.3 Grammar

```ebnf
lambda = "\" parameters "->" (expression | block)
parameters = identifier ("," identifier)*
```

### 4.4 Acceptance Criteria

```flux
// Basic lambda
let double = \x -> x * 2;
print(double(5));  // 10

// With map
let numbers = [1, 2, 3, 4, 5];
let doubled = map(numbers, \x -> x * 2);
print(doubled);  // [2, 4, 6, 8, 10]

// Multiple parameters
let add = \a, b -> a + b;
print(add(3, 4));  // 7

// With filter
let evens = filter(numbers, \x -> x % 2 == 0);
print(evens);  // [2, 4]

// Combining with pipe
numbers
    |> filter(\x -> x > 2)
    |> map(\x -> x * 2)
    |> print;  // [6, 8, 10]
```

### 4.5 Milestone 4 Deliverables

- [ ] Lambda syntax parsing
- [ ] Single and multiple parameters
- [ ] Expression and block bodies
- [ ] Works with all HOF patterns
- [ ] Unit tests
- [ ] Example file: `examples/lambda.flx`

---

## Milestone 5: Essential Builtins

**Priority:** High
**Status:** Not Started
**Dependencies:** M1 (for `%` in some implementations)

### 5.1 Array Builtins

| Builtin | Signature | Priority |
|---------|-----------|----------|
| `concat(a, b)` | `Array, Array -> Array` | Critical |
| `reverse(arr)` | `Array -> Array` | High |
| `contains(arr, elem)` | `Array, Any -> Bool` | High |
| `slice(arr, start, end)` | `Array, Int, Int -> Array` | Medium |
| `sort(arr)` | `Array -> Array` | Medium |

### 5.2 String Builtins

| Builtin | Signature | Priority |
|---------|-----------|----------|
| `split(s, delim)` | `String, String -> Array` | Critical |
| `join(arr, delim)` | `Array, String -> String` | Critical |
| `trim(s)` | `String -> String` | High |
| `upper(s)` | `String -> String` | Medium |
| `lower(s)` | `String -> String` | Medium |
| `chars(s)` | `String -> Array` | Medium |
| `substring(s, start, end)` | `String, Int, Int -> String` | Medium |

### 5.3 Hash Builtins

| Builtin | Signature | Priority |
|---------|-----------|----------|
| `keys(h)` | `Hash -> Array` | Critical |
| `values(h)` | `Hash -> Array` | Critical |
| `has_key(h, k)` | `Hash, Any -> Bool` | High |
| `merge(h1, h2)` | `Hash, Hash -> Hash` | Medium |

### 5.4 Math Builtins

| Builtin | Signature | Priority |
|---------|-----------|----------|
| `abs(n)` | `Number -> Number` | High |
| `min(a, b)` | `Number, Number -> Number` | High |
| `max(a, b)` | `Number, Number -> Number` | High |

### 5.5 Type Checking Builtins

| Builtin | Signature | Priority |
|---------|-----------|----------|
| `type_of(x)` | `Any -> String` | High |
| `is_int(x)` | `Any -> Bool` | Medium |
| `is_string(x)` | `Any -> Bool` | Medium |
| `is_array(x)` | `Any -> Bool` | Medium |

### 5.6 Implementation Priority for v0.0.2

**Must Have:**
- `concat`, `reverse`, `contains`
- `split`, `join`, `trim`
- `keys`, `values`
- `abs`, `min`, `max`
- `type_of`

**Nice to Have:**
- `slice`, `sort`
- `upper`, `lower`, `chars`, `substring`
- `has_key`, `merge`
- `is_int`, `is_string`, `is_array`

### 5.7 Milestone 5 Deliverables

- [ ] 10+ new builtins (must have list)
- [ ] Unit tests for each builtin
- [ ] Documentation
- [ ] Example files demonstrating usage

---

## Milestone 6: Polish & Release

**Priority:** Required
**Dependencies:** M1-M5

### 6.1 Documentation

| Task | Effort |
|------|--------|
| Update README with new features | Medium |
| Update CHANGELOG.md | Small |
| Add migration notes from 0.0.1 | Small |
| Create "What's New in 0.0.2" doc | Medium |

### 6.2 Examples

| Example | Demonstrates |
|---------|--------------|
| `examples/operators.flx` | All new operators |
| `examples/pipe_operator.flx` | Pipe usage patterns |
| `examples/either_type.flx` | Error handling with Either |
| `examples/lambda.flx` | Lambda shorthand |
| `examples/builtins_demo.flx` | New builtins |

### 6.3 Testing

| Task | Effort |
|------|--------|
| Integration tests for all features | Medium |
| Edge case tests | Medium |
| Performance benchmarks | Small |
| Run all existing tests (no regressions) | Small |

### 6.4 Release Checklist

- [ ] All milestones complete
- [ ] All tests passing
- [ ] No compiler warnings
- [ ] Documentation updated
- [ ] Examples working
- [ ] CHANGELOG updated
- [ ] Version bumped to 0.0.2
- [ ] Git tag created

---

## Version Comparison

| Feature | v0.0.1 | v0.0.2 |
|---------|--------|--------|
| Operators | `+ - * / == != < >` | `+ - * / == != < > <= >= % && \|\|` |
| Pipe | No | Yes (`\|>`) |
| Either Type | No | Yes (`Left`/`Right`) |
| Lambda | `fun(x) { x * 2 }` | `\x -> x * 2` |
| Array Builtins | 5 | 10+ |
| String Builtins | 2 | 8+ |
| Hash Builtins | 0 | 4+ |
| Forward References | Yes | Yes |
| Modules | Yes | Yes |

---

## Future (v0.0.3+)

Features deferred to future versions:

- **Pattern Guards:** `x if x > 0 -> ...`
- **Array Patterns:** `[head, ...tail] -> ...`
- **Block Comments:** `/* ... */`
- **Tuple Type:** `(a, b, c)`
- **Type Declarations:** `type Shape { Circle(r), Rectangle(w, h) }`
- **For Loops:** `for x in arr { ... }`
- **Effect System:** `fun f() with IO { ... }`
- **Streams:** Built-in reactive primitives
- **Actors:** Lightweight concurrent processes
- **TCO:** Tail call optimization
- **List Comprehensions:** `[x * 2 for x in arr if x > 0]`

---

## Compatibility Assessment

This section analyzes the compatibility of v0.0.2 proposals with the current compiler architecture.

### Overall Compatibility Score: 88/100

The current Flux compiler is well-architected for these additions. The existing patterns for `Some`/`None` provide direct templates for `Left`/`Right`, and the jump instruction infrastructure supports short-circuit operators.

### Milestone Compatibility Matrix

| Milestone | Compatibility | Confidence | Key Dependencies |
|-----------|---------------|------------|------------------|
| M1: Core Operators | 90% | High | Jump instructions exist |
| M2: Pipe Operator | 95% | High | Pure syntactic sugar |
| M3: Either Type | 92% | High | Some/None template exists |
| M4: Lambda Shorthand | 75% | Medium | New token needed |
| M5: Essential Builtins | 95% | High | Builtin pattern established |
| M6: Polish & Release | 100% | High | No compiler changes |

### Architecture Analysis

#### Lexer (`lexer.rs`)

**Current State:**
- Two-character token support exists (`==`, `!=`, `#{`)
- Clean token emission pattern

**Required Changes:**
- Add `<=`, `>=`, `&&`, `||`, `|>` recognition
- Add `\` (backslash) for lambda

**Compatibility:** 90% - Straightforward extension of existing patterns

#### Parser (`parser.rs`)

**Current State:**
- Infix expression parsing with precedence
- `Some(expr)` and `None` parsing implemented
- Match expression with pattern support

**Required Changes:**
- Add precedence levels for `&&`, `||`, `|>`
- Add `Left(expr)`, `Right(expr)` parsing (mirrors `Some`)
- Add lambda expression parsing
- Transform pipe into function calls

**Compatibility:** 85% - Lambda needs careful integration with expression parsing

#### Precedence (`precedence.rs`)

**Current State:**
```rust
pub enum Precedence {
    Lowest,
    Equals,      // ==, !=
    LessGreater, // <, >
    Sum,         // +, -
    Product,     // *, /
    Prefix,      // -x, !x
    Call,        // fn(x)
    Index,       // array[index]
}
```

**Required Changes:**
```rust
pub enum Precedence {
    Lowest,
    Pipe,        // |>           (NEW)
    LogicalOr,   // ||           (NEW)
    LogicalAnd,  // &&           (NEW)
    Equals,      // ==, !=
    LessGreater, // <, >, <=, >= (EXTENDED)
    Sum,         // +, -
    Product,     // *, /, %      (EXTENDED)
    Prefix,      // -x, !x
    Call,        // fn(x)
    Index,       // array[index]
}
```

**Compatibility:** 95% - Simple enum extension

#### Compiler (`compiler.rs`)

**Current State:**
- `OpJumpNotTruthy` instruction exists
- `Some`/`None` compilation pattern established
- Infix operator compilation working

**Required Changes:**
- Add short-circuit compilation for `&&`/`||`
- Add `Left`/`Right` opcodes
- Transform pipe to function call
- Compile lambda expressions

**Compatibility:** 88% - Short-circuit logic needs careful jump offset handling

**Short-Circuit Pattern (already supported):**
```
// a && b
evaluate a
OpJumpNotTruthy end    // ← This instruction EXISTS
OpPop
evaluate b
end:

// a || b (needs OpJumpTruthy)
evaluate a
OpJumpTruthy end       // ← Need to add this
OpPop
evaluate b
end:
```

#### VM (`vm.rs`)

**Current State:**
- Jump instructions implemented
- `Some`/`None` handling exists
- Builtin function infrastructure

**Required Changes:**
- Add `OpJumpTruthy` instruction
- Add `OpLessEqual`, `OpGreaterEqual`, `OpMod`
- Add `OpLeft`, `OpRight`, `OpIsLeft`, `OpIsRight`
- Add new builtins

**Compatibility:** 92% - All changes follow existing patterns

#### Objects (`object.rs`)

**Current State:**
```rust
pub enum Object {
    // ...
    Some(Box<Object>),
    None,
    // ...
}
```

**Required Changes:**
```rust
pub enum Object {
    // ...
    Some(Box<Object>),
    None,
    Left(Box<Object>),   // NEW - mirrors Some
    Right(Box<Object>),  // NEW - mirrors Some
    // ...
}
```

**Compatibility:** 95% - Direct pattern replication

#### Pattern Matching

**Current State:**
```rust
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Identifier(Identifier),
    None,
    Some(Box<Pattern>),
}
```

**Required Changes:**
```rust
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Identifier(Identifier),
    None,
    Some(Box<Pattern>),
    Left(Box<Pattern>),   // NEW
    Right(Box<Pattern>),  // NEW
}
```

**Compatibility:** 95% - Existing `Some` pattern provides template

### Effort Estimates

| Milestone | Estimated Hours | Risk Level |
|-----------|-----------------|------------|
| M1: Core Operators | 6-8 hours | Low |
| M2: Pipe Operator | 3-4 hours | Low |
| M3: Either Type | 4-6 hours | Low |
| M4: Lambda Shorthand | 4-6 hours | Medium |
| M5: Essential Builtins | 6-8 hours | Low |
| M6: Polish & Release | 3-4 hours | Low |
| **Total** | **26-36 hours** | **Low-Medium** |

### Critical Success Factors

1. **Jump Offset Handling**
   - Short-circuit operators require accurate jump offset calculation
   - Must handle nested expressions correctly
   - Test thoroughly with complex boolean expressions

2. **Precedence Ordering**
   - `||` must have lower precedence than `&&`
   - `|>` must have lowest precedence for intuitive chaining
   - Test mixed operator expressions

3. **Lambda Parsing Ambiguity**
   - `\` token must not conflict with escape sequences in strings
   - Parameter list parsing must handle trailing comma
   - Arrow `->` already used in match arms (need context awareness)

4. **Pipe Transformation**
   - Correctly identify partial application `x |> f(y)` → `f(x, y)`
   - Handle edge case of `x |> f` → `f(x)`
   - Maintain proper evaluation order

### Risk Mitigation

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Jump offset bugs | Medium | High | Extensive test coverage |
| Lambda parsing conflicts | Low | Medium | Use distinct token `\` |
| Precedence errors | Low | Medium | Copy established languages |
| Breaking existing code | Low | High | Full regression test suite |

### Recommended Implementation Order

Based on dependencies and risk:

```
Week 1: M1 (Operators) + M2 (Pipe) in parallel
        └─ Independent, can be worked on simultaneously

Week 2: M3 (Either Type)
        └─ Follows Some/None pattern closely

Week 3: M4 (Lambda) + M5 (Builtins)
        └─ Lambda is independent
        └─ Builtins enhance usability

Week 4: M6 (Polish & Release)
        └─ Integration testing
        └─ Documentation
```

### Conclusion

The v0.0.2 roadmap is **highly compatible** with the current compiler architecture. The existing infrastructure for:

- Jump instructions (enables short-circuit operators)
- `Some`/`None` types (template for `Left`/`Right`)
- Infix operator parsing (supports new operators)
- Builtin functions (easy to extend)

...provides a solid foundation. The main areas requiring careful attention are:

1. Jump offset calculations for `&&`/`||`
2. Lambda expression parsing without ambiguity
3. Pipe operator precedence and transformation

With proper testing, all v0.0.2 features can be implemented without architectural changes to the compiler.

---

## Summary

### v0.0.2 at a Glance

```
New Operators:  <= >= % && ||
New Syntax:     |> (pipe), \ -> (lambda)
New Types:      Left/Right (Either)
New Builtins:   ~15 new functions
```

### Impact

With v0.0.2, Flux becomes practical for:

1. **Real conditional logic** - `if a > 0 && b <= 10 { ... }`
2. **Functional pipelines** - `data |> transform |> filter |> result`
3. **Error handling** - `Right(value)` / `Left(error)` pattern
4. **Concise lambdas** - `map(arr, \x -> x * 2)`
5. **Data manipulation** - `split`, `join`, `keys`, `values`, etc.

This establishes Flux as a viable functional programming language.
