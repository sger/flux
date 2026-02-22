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
5. **Essential base functions** - Array and string operations

---

## Milestone Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  M1: Core Operators          ████████████████████████████████  │ 100% (3/3) ✅
│  M2: Pipe Operator           ████████████████████████████████  │ 100% ✅
│  M3: Either Type             ████████████████████████████████  │ 100% ✅
│  M4: Lambda Shorthand        ████████████████████████████████  │ 100% ✅
│  M5: Essential Base Functions      ████████████████████████████████  │ 100% (5/5) ✅
│  M6: Polish & Release        ████████████████████████████████  │ 100% ✅
└─────────────────────────────────────────────────────────────────┘
```

---

## Milestone 1: Core Operators

**Priority:** Critical
**Status:** ✅ COMPLETE (3/3)

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

### 1.3 Logical Operators: `&&` and `||` ✅

**Status:** COMPLETED

| Task | File(s) | Status |
|------|---------|--------|
| Add `And`, `Or` tokens | `token_type.rs` | ✅ Done |
| Lexer: recognize `&&` and `\|\|` | `lexer.rs` | ✅ Done |
| Add `LogicalAnd`, `LogicalOr` precedence levels | `precedence.rs` | ✅ Done |
| Parser: parse with short-circuit semantics | `parser.rs` | ✅ Done |
| Compiler: emit jump instructions | `compiler.rs` | ✅ Done |
| VM: execute with short-circuit evaluation | `vm.rs` | ✅ Done |
| Tests (including short-circuit verification) | `examples/basics/comparison.flx` | ✅ Done |

**Implementation Note:** These use conditional jumps for short-circuit evaluation.

**Compilation Strategy:**
```
// a && b compiles to:
evaluate a
OpJumpNotTruthy end  // if falsy, jump (leave a on stack)
// pop a (only if truthy)
evaluate b
end:

// a || b compiles to:
evaluate a
OpJumpTruthy end     // if truthy, jump (leave a on stack)
// pop a (only if falsy)
evaluate b
end:
```

**Acceptance Criteria:** ✅ ALL PASSING
```flux
print(true && true);    // true  ✅
print(true && false);   // false ✅
print(false && true);   // false (right side not evaluated) ✅

print(true || false);   // true (right side not evaluated) ✅
print(false || true);   // true  ✅
print(false || false);  // false ✅
```

**Implementation Notes:**
- Added `OpJumpTruthy` opcode (38) for `||` short-circuit
- Modified `OpJumpNotTruthy` and `OpJumpTruthy` to peek (not pop) for short-circuit semantics
- `&&` has higher precedence than `||` (matches C/JavaScript)
- Example file updated with 6 test cases

### 1.4 Milestone 1 Deliverables

- [x] ✅ Comparison operators: `<=`, `>=` (DONE)
- [x] ✅ Modulo operator: `%` (DONE)
- [x] ✅ Logical operators: `&&`, `||` (DONE)
- [x] ✅ Proper precedence: `&&` binds tighter than `||` (DONE)
- [x] ✅ Short-circuit evaluation for `&&` and `||` (DONE)
- [x] ✅ Unit tests for all operators (DONE)
- [x] ✅ Integration tests with complex expressions (DONE)
- [x] ✅ Updated error messages (DONE)

**🎉 Milestone 1 Complete!**

---

## Milestone 2: Pipe Operator

**Priority:** Critical
**Status:** ✅ COMPLETE
**Dependencies:** None (can be parallel with M1)

### 2.1 Implementation

| Task | File(s) | Status |
|------|---------|--------|
| Add `Pipe` token for `\|>` | `token_type.rs` | ✅ Done |
| Lexer: recognize `\|>` | `lexer.rs` | ✅ Done |
| Add `Pipe` precedence (lowest) | `precedence.rs` | ✅ Done |
| Parser: parse as left-associative infix | `parser.rs` | ✅ Done |
| Parser: handle MemberAccess for module functions | `parser.rs` | ✅ Done |
| Compiler: transform to function call | `compiler.rs` | ✅ Done (at parse time) |
| Unit tests | `vm.rs` | ✅ Done |
| Example files | `examples/` | ✅ Done |

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
    |> filter(fn(x) { x > 2 })
    |> map(fn(x) { x * 2 })
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

- [x] ✅ `|>` operator parsing and compilation (DONE)
- [x] ✅ Left-associativity (DONE)
- [x] ✅ Works with any function call (DONE)
- [x] ✅ Works with module functions via MemberAccess (DONE)
- [x] ✅ Unit tests - 10 comprehensive tests (DONE)
- [x] ✅ Example file: `examples/basics/pipe_operator.flx` (DONE)
- [x] ✅ Module example: `examples/Modules/pipe_with_modules.flx` (DONE)

**🎉 Milestone 2 Complete!**

**Implementation Notes:**
- Pipe operator has lowest precedence (below LogicalOr)
- Transformation happens at parse time, not compile time
- `a |> f` → `f(a)`
- `a |> f(b, c)` → `f(a, b, c)`
- `a |> Module.func` → `Module.func(a)` (MemberAccess support)

---

## Milestone 3: Either Type

**Priority:** High
**Status:** ✅ COMPLETE
**Dependencies:** M1 (for practical usage)

### 3.1 Runtime Support

| Task | File(s) | Status |
|------|---------|--------|
| Add `Object::Left(Box<Object>)` | `object.rs` | ✅ Done |
| Add `Object::Right(Box<Object>)` | `object.rs` | ✅ Done |
| Update `type_name()` | `object.rs` | ✅ Done |
| Update `Display` impl | `object.rs` | ✅ Done |
| Add equality comparison for Left/Right | `vm.rs` | ✅ Done |

### 3.2 Bytecode Support

| Task | File(s) | Status |
|------|---------|--------|
| Add `OpLeft`, `OpRight` opcodes | `op_code.rs` | ✅ Done |
| Add `OpIsLeft`, `OpIsRight` opcodes | `op_code.rs` | ✅ Done |
| Add `OpUnwrapLeft`, `OpUnwrapRight` opcodes | `op_code.rs` | ✅ Done |
| VM: implement all Either opcodes | `vm.rs` | ✅ Done |

### 3.3 Parser Support

| Task | File(s) | Status |
|------|---------|--------|
| Add `Left`, `Right` keywords | `token_type.rs` | ✅ Done |
| Parser: `Left(expr)` expression | `parser.rs` | ✅ Done |
| Parser: `Right(expr)` expression | `parser.rs` | ✅ Done |
| Parser: `Left(pat)` pattern | `parser.rs` | ✅ Done |
| Parser: `Right(pat)` pattern | `parser.rs` | ✅ Done |

### 3.4 Compiler Support

| Task | File(s) | Status |
|------|---------|--------|
| Compile `Left(expr)` | `compiler.rs` | ✅ Done |
| Compile `Right(expr)` | `compiler.rs` | ✅ Done |
| Pattern matching for Either | `compiler.rs` | ✅ Done |
| Linter support for Either | `linter.rs` | ✅ Done |

### 3.5 Acceptance Criteria ✅ ALL PASSING

```flux
// Construction
let success = Right(42);
let failure = Left("error message");

// Pattern matching
fn handle(result) {
    match result {
        Right(value) -> print("Success: " + to_string(value));
        Left(err) -> print("Error: " + err);
        _ -> print("unknown");
    }
}

// Practical usage
fn divide(a, b) {
    if b == 0 {
        Left("division by zero")
    } else {
        Right(a / b)
    }
}

let result = divide(10, 2);
match result {
    Right(v) -> print(v);  // 5
    Left(e) -> print(e);
    _ -> print("unknown");
}

let result = divide(10, 0);
match result {
    Right(v) -> print(v);
    Left(e) -> print(e);  // "division by zero"
    _ -> print("unknown");
}
```

### 3.6 Milestone 3 Deliverables

- [x] ✅ `Left` and `Right` object types (DONE)
- [x] ✅ Construction syntax: `Left(x)`, `Right(x)` (DONE)
- [x] ✅ Pattern matching: `Left(e) -> ...`, `Right(v) -> ...` (DONE)
- [x] ✅ Display formatting (DONE)
- [x] ✅ Equality comparison: `Left(1) == Left(1)` (DONE)
- [x] ✅ Unit tests - 7 comprehensive tests (DONE)
- [x] ✅ Example file: `examples/basics/either_type.flx` (DONE)
- [x] ✅ Example file: `examples/basics/either_and_option.flx` (DONE)
- [x] ✅ Example file: `examples/patterns/either_match.flx` (DONE)

**🎉 Milestone 3 Complete!**

**Implementation Notes:**
- Uses same pattern as Option (Some/None) for consistency
- 6 new opcodes: OpLeft (39), OpRight (40), OpIsLeft (41), OpIsRight (42), OpUnwrapLeft (43), OpUnwrapRight (44)
- Pattern matching supports binding (`Left(e)`) and wildcard (`Left(_)`)
- Either values can be nested: `Left(Right(42))`
- Works with arrays, hashes, and Option types
- Follows Haskell convention: no `Either` keyword, just `Left` and `Right`

---

## Milestone 4: Lambda Shorthand

**Priority:** Medium
**Status:** ✅ COMPLETE
**Dependencies:** None

### 4.1 Syntax

**Implemented syntax:** `\params -> expr`

```flux
// Single parameter (no parens required)
\x -> x * 2

// Multiple parameters (parens required)
\(x, y) -> x + y

// Zero parameters
\() -> 42

// With block body
\x -> {
    let doubled = x * 2;
    doubled + 1
}
```

### 4.2 Implementation

| Task | File(s) | Status |
|------|---------|--------|
| Add `Backslash` token | `token_type.rs` | ✅ Done |
| Lexer: recognize `\` | `lexer.rs` | ✅ Done |
| Parser: parse lambda expression | `parser.rs` | ✅ Done |
| AST: Reuse `Expression::Function` | `expression.rs` | ✅ Done |
| Compiler: compile lambda (same as Function) | `compiler.rs` | ✅ Done |
| Tests | `tests/` | ✅ Done |

### 4.3 Grammar

```ebnf
lambda = "\" (identifier | "(" [parameters] ")") "->" (expression | block)
parameters = identifier ("," identifier)*
```

### 4.4 Examples

```flux
// Basic lambda
let double = \x -> x * 2;
print(double(5));  // 10

// Multiple parameters
let add = \(a, b) -> a + b;
print(add(3, 4));  // 7

// Zero parameters
let constant = \() -> 42;
print(constant());  // 42

// Block body
let complex = \x -> {
    let doubled = x * 2;
    doubled + 1
};
print(complex(5));  // 11

// Lambda as argument
fn applyTwice(f, x) {
    f(f(x))
}
print(applyTwice(\x -> x * 2, 3));  // 12
```

### 4.5 Milestone 4 Deliverables

- [x] Lambda syntax parsing
- [x] Single parameter without parens
- [x] Multiple parameters with parens
- [x] Zero parameters with parens
- [x] Expression and block bodies
- [x] Works with higher-order functions
- [x] 8 unit tests (6 parser + 2 lexer)
- [ ] Example file: `examples/lambda.flx`

---

## Milestone 5: Essential Base Functions

**Priority:** High
**Status:** In Progress (5.1 Complete)
**Dependencies:** M1 (for `%` in some implementations)

### 5.1 Array Base Functions ✅ COMPLETE

| Base | Signature | Status |
|---------|-----------|--------|
| `concat(a, b)` | `Array, Array -> Array` | ✅ Done |
| `reverse(arr)` | `Array -> Array` | ✅ Done |
| `contains(arr, elem)` | `Array, Any -> Bool` | ✅ Done |
| `slice(arr, start, end)` | `Array, Int, Int -> Array` | ✅ Done |
| `sort(arr)` or `sort(arr, "asc"/"desc")` | `Array -> Array` | ✅ Done |

**Implementation Notes:**
- All array base functions registered in compiler (indices 7-11)
- `sort` supports optional second parameter: `"asc"` (default) or `"desc"`
- Smart comparison in sort: avoids f64 conversion when comparing same types
- Unit tests: 17 tests for array base functions
- Example file: `examples/basics/array_base_functions.flx`

### 5.2 String Base Functions ✅ COMPLETE

| Base | Signature | Priority |
|---------|-----------|----------|
| `split(s, delim)` | `String, String -> Array` | ✅ Done |
| `join(arr, delim)` | `Array, String -> String` | ✅ Done |
| `trim(s)` | `String -> String` | ✅ Done |
| `upper(s)` | `String -> String` | ✅ Done |
| `lower(s)` | `String -> String` | ✅ Done |
| `chars(s)` | `String -> Array` | ✅ Done |
| `substring(s, start, end)` | `String, Int, Int -> String` | ✅ Done |

### 5.3 Hash Base Functions ✅ COMPLETE

| Base | Signature | Status |
|---------|-----------|--------|
| `keys(h)` | `Hash -> Array` | ✅ Done |
| `values(h)` | `Hash -> Array` | ✅ Done |
| `has_key(h, k)` | `Hash, Any -> Bool` | ✅ Done |
| `merge(h1, h2)` | `Hash, Hash -> Hash` | ✅ Done |

```flux
// keys - get all keys from a hash
let person = {"name": "Alice", "age": 30, "city": "NYC" };
print(keys(person));  // ["name", "age", "city"]

// values - get all values from a hash
print(values(person));  // ["Alice", 30, "NYC"]

// has_key - check if a key exists
print(has_key(person, "name"));   // true
print(has_key(person, "email"));  // false

// merge - combine two hashes (second hash overwrites conflicts)
let defaults = {"theme": "dark", "lang": "en" };
let settings = {"lang": "fr", "notifications": true };
print(merge(defaults, settings));  // {"theme": "dark", "lang": "fr", "notifications": true }
```

### 5.4 Math Base Functions ✅ COMPLETE

| Base | Signature | Status |
|---------|-----------|--------|
| `abs(n)` | `Number -> Number` | ✅ Done |
| `min(a, b)` | `Number, Number -> Number` | ✅ Done |
| `max(a, b)` | `Number, Number -> Number` | ✅ Done |

```flux
// abs - absolute value
print(abs(-5));     // 5
print(abs(5));      // 5
print(abs(-3.14));  // 3.14

// min - smaller of two numbers
print(min(3, 7));       // 3
print(min(10, 2));      // 2
print(min(3.5, 2.1));   // 2.1

// max - larger of two numbers
print(max(3, 7));       // 7
print(max(10, 2));      // 10
print(max(3.5, 2.1));   // 3.5

// Practical example: Clamp value to range
fn clamp(value, minVal, maxVal) {
    min(max(value, minVal), maxVal);
}
print(clamp(15, 0, 10));   // 10
print(clamp(-5, 0, 10));   // 0
print(clamp(5, 0, 10));    // 5
```

### 5.5 Type Checking Base Functions ✅

| Base | Signature | Status |
|---------|-----------|--------|
| `type_of(x)` | `Any -> String` | ✅ |
| `is_int(x)` | `Any -> Bool` | ✅ |
| `is_float(x)` | `Any -> Bool` | ✅ |
| `is_string(x)` | `Any -> Bool` | ✅ |
| `is_bool(x)` | `Any -> Bool` | ✅ |
| `is_array(x)` | `Any -> Bool` | ✅ |
| `is_hash(x)` | `Any -> Bool` | ✅ |
| `is_none(x)` | `Any -> Bool` | ✅ |
| `is_some(x)` | `Any -> Bool` | ✅ |

```flux
// type_of - get type name as string
print(type_of(42));           // "Int"
print(type_of(3.14));         // "Float"
print(type_of("hello"));      // "String"
print(type_of(true));         // "Bool"
print(type_of([1, 2, 3]));    // "Array"
print(type_of({"a": 1}));     // "Hash"
print(type_of(None));         // "None"
print(type_of(Some(42)));     // "Some"

// is_* - type checking predicates
print(is_int(42));            // true
print(is_int(3.14));          // false
print(is_float(3.14));        // true
print(is_string("hello"));    // true
print(is_bool(true));         // true
print(is_array([1, 2]));      // true
print(is_hash({"a": 1}));     // true
print(is_none(None));         // true
print(is_some(Some(42)));     // true

// Practical example: Safe type conversion
fn safeAdd(a, b) {
    if is_int(a) && is_int(b) {
        a + b
    } else {
        None
    }
}
print(safeAdd(1, 2));         // 3
print(safeAdd(1, "hello"));   // None
```

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

- [x] 10+ new base functions (must have list) - 35 total base functions implemented
  Base Functions: `print`, `len`, `first`, `last`, `rest`, `push`, `to_string`, `concat`, `reverse`,
  `contains`, `slice`, `sort`, `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`,
  `keys`, `values`, `has_key`, `merge`, `abs`, `min`, `max`, `type_of`, `is_int`, `is_float`,
  `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`
- [x] Unit tests for each base - 89 unit tests
- [ ] Documentation (planned)
- [x] Example files demonstrating usage

---

## Milestone 6: Polish & Release

**Priority:** Required
**Dependencies:** M1-M5
**Status:** ✅ COMPLETE

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
| `examples/builtins_demo.flx` | New base functions |

### 6.3 Testing

| Task | Effort |
|------|--------|
| Integration tests for all features | Medium |
| Edge case tests | Medium |
| Performance benchmarks | Small |
| Run all existing tests (no regressions) | Small |

### 6.4 Release Checklist

- [x] All milestones complete
- [x] All tests passing
- [x] No compiler warnings
- [x] Documentation updated
- [x] Examples working
- [x] CHANGELOG updated
- [x] Version bumped to 0.0.2
- [x] Git tag created

---

## Version Comparison

| Feature | v0.0.1 | v0.0.2 |
|---------|--------|--------|
| Operators | `+ - * / == != < >` | `+ - * / == != < > <= >= % && \|\|` |
| Pipe | No | Yes (`\|>`) |
| Either Type | No | Yes (`Left`/`Right`) |
| Lambda | `fn(x) { x * 2 }` | `\x -> x * 2` |
| Array Base Functions | 5 | 10+ |
| String Base Functions | 2 | 8+ |
| Hash Base Functions | 0 | 4+ |
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
- **Effect System:** `fn f() with IO { ... }`
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
| M5: Essential Base Functions | 95% | High | Base pattern established |
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
- Base function infrastructure

**Required Changes:**
- Add `OpJumpTruthy` instruction
- Add `OpLessEqual`, `OpGreaterEqual`, `OpMod`
- Add `OpLeft`, `OpRight`, `OpIsLeft`, `OpIsRight`
- Add new base functions

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
| M5: Essential Base Functions | 6-8 hours | Low |
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

Week 3: M4 (Lambda) + M5 (Base Functions)
        └─ Lambda is independent
        └─ Base Functions enhance usability

Week 4: M6 (Polish & Release)
        └─ Integration testing
        └─ Documentation
```

### Conclusion

The v0.0.2 roadmap is **highly compatible** with the current compiler architecture. The existing infrastructure for:

- Jump instructions (enables short-circuit operators)
- `Some`/`None` types (template for `Left`/`Right`)
- Infix operator parsing (supports new operators)
- Base functions (easy to extend)

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
New Base Functions:   ~15 new functions
```

### Impact

With v0.0.2, Flux becomes practical for:

1. **Real conditional logic** - `if a > 0 && b <= 10 { ... }`
2. **Functional pipelines** - `data |> transform |> filter |> result`
3. **Error handling** - `Right(value)` / `Left(error)` pattern
4. **Concise lambdas** - `map(arr, \x -> x * 2)`
5. **Data manipulation** - `split`, `join`, `keys`, `values`, etc.

This establishes Flux as a viable functional programming language.
