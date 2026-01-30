# Flux Flow Library Proposal

This document outlines the plan for implementing a comprehensive standard library for Flux called **Flow**, including prerequisites, new language features, and module designs.

The name "Flow" complements "Flux" (meaning continuous change/movement) - data flows through transformations in a functional style.

## Table of Contents

1. [Current State](#current-state)
2. [Phase 0: New Object Types](#phase-0-new-object-types)
3. [Phase 1: Compiler Prerequisites](#phase-1-compiler-prerequisites)
4. [Phase 2: New Builtins](#phase-2-new-builtins)
5. [Phase 3: Flow Modules](#phase-3-flow-modules)
6. [Implementation Roadmap](#implementation-roadmap)

---

## Current State

### Object Types (8 total)

| Type | Rust Representation | Hashable | Description |
|------|---------------------|----------|-------------|
| `Integer` | `i64` | Yes | 64-bit signed integer |
| `Float` | `f64` | No | 64-bit floating point |
| `Boolean` | `bool` | Yes | `true` or `false` |
| `String` | `String` | Yes | UTF-8 string |
| `None` | unit | - | Absence of value |
| `Some` | `Box<Object>` | - | Optional wrapper |
| `Array` | `Vec<Object>` | No | Dynamic array |
| `Hash` | `HashMap<HashKey, Object>` | No | Key-value map |

Internal types: `Function`, `Closure`, `Builtin`, `ReturnValue`

### Builtin Functions (7 total)

| Function | Signature | Description |
|----------|-----------|-------------|
| `print` | `(...args) -> None` | Print to stdout |
| `len` | `String \| Array -> Int` | Get length |
| `first` | `Array -> Object` | First element |
| `last` | `Array -> Object` | Last element |
| `rest` | `Array -> Array` | All except first |
| `push` | `Array, Object -> Array` | Append element |
| `to_string` | `Object -> String` | Convert to string |

### Operators (7 total)

| Operator | Token | Opcode | Notes |
|----------|-------|--------|-------|
| `+` | `Plus` | `OpAdd` | Addition, string concat |
| `-` | `Minus` | `OpSub` | Subtraction |
| `*` | `Asterisk` | `OpMul` | Multiplication |
| `/` | `Slash` | `OpDiv` | Division |
| `==` | `Eq` | `OpEqual` | Equality |
| `!=` | `NotEq` | `OpNotEqual` | Inequality |
| `>` | `Gt` | `OpGreaterThan` | Greater than |
| `<` | `Lt` | `OpGreaterThan` | Less than (swapped operands) |

### Missing Operators

| Operator | Status | Impact |
|----------|--------|--------|
| `<=` | **Missing** | Cannot write `n <= 0` |
| `>=` | **Missing** | Cannot write `n >= 0` |
| `&&` | **Missing** | No short-circuit AND |
| `\|\|` | **Missing** | No short-circuit OR |
| `%` | **Missing** | No modulo operation |

---

## Phase 0: New Object Types

### Priority: High

#### `Either` Type (Error Handling / Sum Type)

Following Haskell's functional programming convention, we use `Either` with `Left` (typically for errors/failures) and `Right` (typically for success values). The mnemonic: "Right is right" (correct/success).

```rust
// In object.rs
Object::Left(Box<Object>)   // Error/failure case
Object::Right(Box<Object>)  // Success/value case
```

**Usage in Flux:**
```flux
fun divide(a, b) {
    if b == 0 {
        Left("division by zero");
    } else {
        Right(a / b);
    }
}

match divide(10, 2) {
    Right(value) -> print(value);
    Left(msg) -> print("Error: " + msg);
}
```

**Required Changes:**
- Add `Left` and `Right` to `Object` enum
- Add `OpLeft`, `OpRight`, `OpIsLeft`, `OpIsRight`, `OpUnwrapLeft`, `OpUnwrapRight` opcodes
- Add parser support for `Left(...)` and `Right(...)` expressions
- Add pattern matching support for `Left(e)` and `Right(x)`

### Priority: Medium

#### `Tuple` Type (Fixed-size Collections)

```rust
// In object.rs
Object::Tuple(Vec<Object>)
```

**Usage in Flux:**
```flux
let point = (10, 20);
let (x, y) = point;

fun min_max(arr) {
    (List.min(arr), List.max(arr));
}
```

**Required Changes:**
- Add `Tuple` to `Object` enum
- Add tuple literal syntax `(a, b, c)` to parser
- Add destructuring pattern `let (a, b) = tuple;`
- Add `OpTuple` opcode

#### `Range` Type (Iteration)

```rust
// In object.rs
Object::Range { start: i64, end: i64, step: i64 }
```

**Usage in Flux:**
```flux
let r = 1..10;        // Range from 1 to 9
let r = 1..=10;       // Range from 1 to 10 (inclusive)
let r = 0..10..2;     // Range with step: 0, 2, 4, 6, 8
```

### Priority: Low

#### `Set` Type (Unique Collections)

```rust
// In object.rs
Object::Set(HashSet<HashKey>)
```

#### `Char` Type (Single Characters)

```rust
// In object.rs
Object::Char(char)
```

---

## Phase 1: Compiler Prerequisites

These features must be implemented before building the stdlib.

### 1.1 Comparison Operators: `<=` and `>=`

**Priority:** Critical

**Files to modify:**
1. `src/frontend/token_type.rs` - Add `LessEqual`, `GreaterEqual` tokens
2. `src/frontend/lexer.rs` - Recognize `<=` and `>=`
3. `src/frontend/parser.rs` - Parse as infix operators
4. `src/bytecode/op_code.rs` - Add `OpLessEqual`, `OpGreaterEqual`
5. `src/bytecode/compiler.rs` - Emit new opcodes
6. `src/runtime/vm.rs` - Execute comparisons

**Implementation approach:**
```rust
// Option A: New opcodes
OpLessEqual    // a <= b
OpGreaterEqual // a >= b

// Option B: Combine existing ops (less efficient)
// a <= b  =>  !(a > b)
// a >= b  =>  !(a < b)
```

### 1.2 Logical Operators: `&&` and `||`

**Priority:** Critical

**Key requirement:** Short-circuit evaluation

```flux
// && should NOT evaluate right side if left is false
false && expensive_function()  // expensive_function not called

// || should NOT evaluate right side if left is true
true || expensive_function()   // expensive_function not called
```

**Implementation:**
- Cannot be simple opcodes (need conditional jumps)
- Compile to jump instructions like `if` expressions

```
// a && b compiles to:
evaluate a
OpJumpNotTruthy end
evaluate b
end:

// a || b compiles to:
evaluate a
OpJumpTruthy end
evaluate b
end:
```

### 1.3 Modulo Operator: `%`

**Priority:** High

**Files to modify:**
1. `src/frontend/token_type.rs` - Add `Percent` token
2. `src/frontend/lexer.rs` - Recognize `%`
3. `src/bytecode/op_code.rs` - Add `OpMod`
4. `src/runtime/vm.rs` - Execute modulo

**Usage:**
```flux
10 % 3    // 1
is_even(n) = n % 2 == 0
```

### 1.4 Tail Call Optimization (TCO)

**Priority:** Medium (but important for stdlib)

**Problem:** Without TCO, recursive functions stack overflow on large inputs.

```flux
// This will overflow on large arrays
fun reduce(arr, acc, f) {
    if len(arr) == 0 {
        acc;
    } else {
        reduce(rest(arr), f(acc, first(arr)), f);  // tail call
    }
}
```

**Implementation approaches:**
1. **Trampoline:** Convert tail calls to loop iterations
2. **Stack reuse:** Detect tail position and reuse current frame
3. **CPS transformation:** Convert to continuation-passing style

### 1.5 Pattern Matching Enhancements

**Priority:** Low (nice to have)

**Current support:**
- Literals: `1 -> ...`
- `None` / `Some(x)`
- Wildcard: `_ -> ...`
- Identifier binding: `x -> ...`

**Desired additions:**

```flux
// Array patterns
match arr {
    [] -> "empty";
    [x] -> "single: " + x;
    [x, y] -> "pair";
    [head, ...tail] -> "list";  // rest pattern
}

// Tuple patterns
match point {
    (0, 0) -> "origin";
    (x, 0) -> "on x-axis";
    (0, y) -> "on y-axis";
    (x, y) -> "point";
}

// Guard clauses
match n {
    x if x > 0 -> "positive";
    x if x < 0 -> "negative";
    _ -> "zero";
}
```

---

## Phase 2: New Builtins

### 2.1 Array Builtins

| Builtin | Signature | Why Builtin? |
|---------|-----------|--------------|
| `concat(a, b)` | `Array, Array -> Array` | O(n) vs O(n²) in pure Flux |
| `reverse(arr)` | `Array -> Array` | O(n) vs O(n²) in pure Flux |
| `slice(arr, start, end)` | `Array, Int, Int -> Array` | Can't do efficiently in Flux |
| `contains(arr, elem)` | `Array, Any -> Bool` | Early exit optimization |
| `index_of(arr, elem)` | `Array, Any -> Int \| None` | Early exit optimization |
| `sort(arr)` | `Array -> Array` | Requires efficient algorithm |
| `sort_by(arr, f)` | `Array, Fn -> Array` | Custom comparator |

### 2.2 String Builtins

| Builtin | Signature | Description |
|---------|-----------|-------------|
| `char_at(s, i)` | `String, Int -> String` | Get character at index |
| `split(s, delim)` | `String, String -> Array` | Split by delimiter |
| `join(arr, delim)` | `Array, String -> String` | Join with delimiter |
| `trim(s)` | `String -> String` | Remove whitespace |
| `trim_start(s)` | `String -> String` | Remove leading whitespace |
| `trim_end(s)` | `String -> String` | Remove trailing whitespace |
| `upper(s)` | `String -> String` | Convert to uppercase |
| `lower(s)` | `String -> String` | Convert to lowercase |
| `starts_with(s, prefix)` | `String, String -> Bool` | Check prefix |
| `ends_with(s, suffix)` | `String, String -> Bool` | Check suffix |
| `contains(s, sub)` | `String, String -> Bool` | Check substring |
| `replace(s, from, to)` | `String, String, String -> String` | Replace occurrences |
| `substring(s, start, end)` | `String, Int, Int -> String` | Extract substring |
| `index_of(s, sub)` | `String, String -> Int \| None` | Find substring |
| `chars(s)` | `String -> Array` | Split into characters |

### 2.3 Math Builtins

| Builtin | Signature | Description |
|---------|-----------|-------------|
| `abs(n)` | `Number -> Number` | Absolute value |
| `min(a, b)` | `Number, Number -> Number` | Minimum |
| `max(a, b)` | `Number, Number -> Number` | Maximum |
| `floor(n)` | `Float -> Int` | Round down |
| `ceil(n)` | `Float -> Int` | Round up |
| `round(n)` | `Float -> Int` | Round to nearest |
| `sqrt(n)` | `Number -> Float` | Square root |
| `pow(base, exp)` | `Number, Number -> Number` | Exponentiation |
| `log(n)` | `Number -> Float` | Natural logarithm |
| `log10(n)` | `Number -> Float` | Base-10 logarithm |
| `sin(n)` | `Number -> Float` | Sine |
| `cos(n)` | `Number -> Float` | Cosine |
| `tan(n)` | `Number -> Float` | Tangent |
| `random()` | `-> Float` | Random number [0, 1) |
| `random_int(min, max)` | `Int, Int -> Int` | Random integer |

### 2.4 Type Checking Builtins

| Builtin | Signature | Description |
|---------|-----------|-------------|
| `type_of(x)` | `Any -> String` | Get type name |
| `is_int(x)` | `Any -> Bool` | Check if integer |
| `is_float(x)` | `Any -> Bool` | Check if float |
| `is_number(x)` | `Any -> Bool` | Check if int or float |
| `is_string(x)` | `Any -> Bool` | Check if string |
| `is_bool(x)` | `Any -> Bool` | Check if boolean |
| `is_array(x)` | `Any -> Bool` | Check if array |
| `is_hash(x)` | `Any -> Bool` | Check if hash |
| `is_none(x)` | `Any -> Bool` | Check if None |
| `is_some(x)` | `Any -> Bool` | Check if Some |
| `is_function(x)` | `Any -> Bool` | Check if callable |

### 2.5 Conversion Builtins

| Builtin | Signature | Description |
|---------|-----------|-------------|
| `to_int(x)` | `Any -> Int \| None` | Convert to integer |
| `to_float(x)` | `Any -> Float \| None` | Convert to float |
| `parse_int(s)` | `String -> Int \| None` | Parse integer from string |
| `parse_float(s)` | `String -> Float \| None` | Parse float from string |

### 2.6 Hash Builtins

| Builtin | Signature | Description |
|---------|-----------|-------------|
| `keys(h)` | `Hash -> Array` | Get all keys |
| `values(h)` | `Hash -> Array` | Get all values |
| `entries(h)` | `Hash -> Array` | Get key-value pairs |
| `has_key(h, k)` | `Hash, Any -> Bool` | Check if key exists |
| `merge(h1, h2)` | `Hash, Hash -> Hash` | Merge two hashes |
| `remove(h, k)` | `Hash, Any -> Hash` | Remove key |

---

## Phase 3: Flow Modules

### Module Structure

```
src/flow/             (or examples/flow/)
├── Flow/
│   ├── List.flx      // Array operations
│   ├── String.flx    // String utilities
│   ├── Math.flx      // Math functions
│   ├── Option.flx    // Some/None utilities
│   ├── Either.flx    // Left/Right utilities (error handling)
│   ├── Dict.flx      // Hash utilities
│   ├── Func.flx      // Function combinators
│   └── IO.flx        // Input/Output (future)
```

### 3.1 `Flow.List` Module

```flux
module Flow.List {
    //===================
    // Transformations
    //===================

    // Apply function to each element
    fun map(arr, f) {
        if len(arr) == 0 {
            [];
        } else {
            concat([f(first(arr))], map(rest(arr), f));
        }
    }

    // Keep elements matching predicate
    fun filter(arr, pred) {
        if len(arr) == 0 {
            [];
        } else {
            let head = first(arr);
            let tail = filter(rest(arr), pred);
            if pred(head) {
                concat([head], tail);
            } else {
                tail;
            }
        }
    }

    // Accumulate values left-to-right
    fun reduce(arr, init, f) {
        if len(arr) == 0 {
            init;
        } else {
            reduce(rest(arr), f(init, first(arr)), f);
        }
    }

    // Accumulate values right-to-left
    fun reduce_right(arr, init, f) {
        if len(arr) == 0 {
            init;
        } else {
            f(first(arr), reduce_right(rest(arr), init, f));
        }
    }

    // Map then flatten one level
    fun flat_map(arr, f) {
        flatten(map(arr, f));
    }

    //===================
    // Queries
    //===================

    // Find first element matching predicate
    fun find(arr, pred) {
        if len(arr) == 0 {
            None;
        } else {
            let head = first(arr);
            if pred(head) {
                Some(head);
            } else {
                find(rest(arr), pred);
            }
        }
    }

    // Check if any element matches
    fun any(arr, pred) {
        if len(arr) == 0 {
            false;
        } else {
            if pred(first(arr)) {
                true;
            } else {
                any(rest(arr), pred);
            }
        }
    }

    // Check if all elements match
    fun all(arr, pred) {
        if len(arr) == 0 {
            true;
        } else {
            if pred(first(arr)) {
                all(rest(arr), pred);
            } else {
                false;
            }
        }
    }

    // Count elements matching predicate
    fun count(arr, pred) {
        reduce(arr, 0, fun(acc, x) {
            if pred(x) { acc + 1; } else { acc; }
        });
    }

    //===================
    // Slicing
    //===================

    // Take first n elements
    fun take(arr, n) {
        if n == 0 {
            [];
        } else {
            if len(arr) == 0 {
                [];
            } else {
                concat([first(arr)], take(rest(arr), n - 1));
            }
        }
    }

    // Drop first n elements
    fun drop(arr, n) {
        if n == 0 {
            arr;
        } else {
            if len(arr) == 0 {
                [];
            } else {
                drop(rest(arr), n - 1);
            }
        }
    }

    // Take while predicate is true
    fun take_while(arr, pred) {
        if len(arr) == 0 {
            [];
        } else {
            let head = first(arr);
            if pred(head) {
                concat([head], take_while(rest(arr), pred));
            } else {
                [];
            }
        }
    }

    // Drop while predicate is true
    fun drop_while(arr, pred) {
        if len(arr) == 0 {
            [];
        } else {
            if pred(first(arr)) {
                drop_while(rest(arr), pred);
            } else {
                arr;
            }
        }
    }

    //===================
    // Combining
    //===================

    // Pair up elements from two arrays
    fun zip(arr1, arr2) {
        if len(arr1) == 0 {
            [];
        } else {
            if len(arr2) == 0 {
                [];
            } else {
                concat(
                    [[first(arr1), first(arr2)]],
                    zip(rest(arr1), rest(arr2))
                );
            }
        }
    }

    // Combine with function
    fun zip_with(arr1, arr2, f) {
        if len(arr1) == 0 {
            [];
        } else {
            if len(arr2) == 0 {
                [];
            } else {
                concat(
                    [f(first(arr1), first(arr2))],
                    zip_with(rest(arr1), rest(arr2), f)
                );
            }
        }
    }

    // Flatten one level of nesting
    fun flatten(arr) {
        reduce(arr, [], fun(acc, x) { concat(acc, x); });
    }

    //===================
    // Aggregation
    //===================

    // Sum of numbers
    fun sum(arr) {
        reduce(arr, 0, fun(acc, x) { acc + x; });
    }

    // Product of numbers
    fun product(arr) {
        reduce(arr, 1, fun(acc, x) { acc * x; });
    }

    // Note: min/max require >= operator
    // fun min(arr) { ... }
    // fun max(arr) { ... }

    //===================
    // Utilities
    //===================

    // Check if empty
    fun is_empty(arr) {
        len(arr) == 0;
    }

    // Get nth element (0-indexed)
    fun nth(arr, n) {
        if len(arr) == 0 {
            None;
        } else {
            if n == 0 {
                Some(first(arr));
            } else {
                nth(rest(arr), n - 1);
            }
        }
    }

    // Alias for first
    fun head(arr) {
        first(arr);
    }

    // Alias for rest
    fun tail(arr) {
        rest(arr);
    }

    // All elements except last
    fun init(arr) {
        if len(arr) == 0 {
            [];
        } else {
            if len(rest(arr)) == 0 {
                [];
            } else {
                concat([first(arr)], init(rest(arr)));
            }
        }
    }

    // Create array of n copies
    fun replicate(n, value) {
        if n == 0 {
            [];
        } else {
            concat([value], replicate(n - 1, value));
        }
    }

    // Intersperse element between items
    fun intersperse(arr, sep) {
        if len(arr) == 0 {
            [];
        } else {
            if len(rest(arr)) == 0 {
                arr;
            } else {
                concat([first(arr), sep], intersperse(rest(arr), sep));
            }
        }
    }
}
```

### 3.2 `Flow.Option` Module

```flux
module Flow.Option {
    // Transform the inner value
    fun map(opt, f) {
        match opt {
            Some(x) -> Some(f(x));
            _ -> None;
        }
    }

    // Chain optional operations
    fun flat_map(opt, f) {
        match opt {
            Some(x) -> f(x);
            _ -> None;
        }
    }

    // Alias for flat_map
    fun and_then(opt, f) {
        flat_map(opt, f);
    }

    // Get value or default
    fun unwrap_or(opt, default) {
        match opt {
            Some(x) -> x;
            _ -> default;
        }
    }

    // Get value or compute default
    fun unwrap_or_else(opt, f) {
        match opt {
            Some(x) -> x;
            _ -> f();
        }
    }

    // Check if Some
    fun is_some(opt) {
        match opt {
            Some(_) -> true;
            _ -> false;
        }
    }

    // Check if None
    fun is_none(opt) {
        match opt {
            None -> true;
            _ -> false;
        }
    }

    // Filter by predicate
    fun filter(opt, pred) {
        match opt {
            Some(x) -> if pred(x) { Some(x); } else { None; };
            _ -> None;
        }
    }

    // Get value or panic (use with caution)
    fun unwrap(opt) {
        match opt {
            Some(x) -> x;
            _ -> None;  // Should panic, but Flux lacks panic
        }
    }

    // Provide alternative if None
    fun or_else(opt, f) {
        match opt {
            Some(_) -> opt;
            _ -> f();
        }
    }

    // Zip two options
    fun zip(opt1, opt2) {
        match opt1 {
            Some(x) -> match opt2 {
                Some(y) -> Some([x, y]);
                _ -> None;
            };
            _ -> None;
        }
    }
}
```

### 3.3 `Flow.Either` Module

The `Either` type follows Haskell convention where `Left` typically represents failure/error and `Right` represents success. The mnemonic is "Right is right" (correct).

```flux
module Flow.Either {
    // Transform the Right value
    fun map(either, f) {
        match either {
            Right(x) -> Right(f(x));
            Left(e) -> Left(e);
        }
    }

    // Transform the Left value
    fun map_left(either, f) {
        match either {
            Right(x) -> Right(x);
            Left(e) -> Left(f(e));
        }
    }

    // Chain operations (bind/flatMap)
    fun flat_map(either, f) {
        match either {
            Right(x) -> f(x);
            Left(e) -> Left(e);
        }
    }

    // Alias for flat_map (Haskell style)
    fun bind(either, f) {
        flat_map(either, f);
    }

    // Alias for flat_map (Rust style)
    fun and_then(either, f) {
        flat_map(either, f);
    }

    // Get Right value or default
    fun unwrap_or(either, default) {
        match either {
            Right(x) -> x;
            Left(_) -> default;
        }
    }

    // Get Right value or compute default
    fun unwrap_or_else(either, f) {
        match either {
            Right(x) -> x;
            Left(e) -> f(e);
        }
    }

    // Check if Right
    fun is_right(either) {
        match either {
            Right(_) -> true;
            Left(_) -> false;
        }
    }

    // Check if Left
    fun is_left(either) {
        match either {
            Left(_) -> true;
            Right(_) -> false;
        }
    }

    // Convert to Option (discards Left value)
    fun to_option(either) {
        match either {
            Right(x) -> Some(x);
            Left(_) -> None;
        }
    }

    // Provide alternative Either if Left
    fun or_else(either, f) {
        match either {
            Right(_) -> either;
            Left(e) -> f(e);
        }
    }

    // Swap Left and Right
    fun swap(either) {
        match either {
            Right(x) -> Left(x);
            Left(e) -> Right(e);
        }
    }

    // Apply function from Either to value in Either
    fun ap(either_f, either_x) {
        match either_f {
            Right(f) -> map(either_x, f);
            Left(e) -> Left(e);
        }
    }

    // Combine two Either values
    fun zip(either1, either2) {
        match either1 {
            Right(x) -> match either2 {
                Right(y) -> Right([x, y]);
                Left(e) -> Left(e);
            };
            Left(e) -> Left(e);
        }
    }

    // Fold/catamorphism: handle both cases
    fun fold(either, on_left, on_right) {
        match either {
            Right(x) -> on_right(x);
            Left(e) -> on_left(e);
        }
    }

    // Bimap: transform both sides
    fun bimap(either, f_left, f_right) {
        match either {
            Right(x) -> Right(f_right(x));
            Left(e) -> Left(f_left(e));
        }
    }
}
```

### 3.4 `Flow.Math` Module

```flux
module Flow.Math {
    // Constants (when module-level values are supported)
    // let PI = 3.141592653589793;
    // let E = 2.718281828459045;

    // Sign of number: -1, 0, or 1
    fun sign(n) {
        if n > 0 {
            1;
        } else {
            if n < 0 {
                -1;
            } else {
                0;
            }
        }
    }

    // Check if positive
    fun is_positive(n) {
        n > 0;
    }

    // Check if negative
    fun is_negative(n) {
        n < 0;
    }

    // Check if zero
    fun is_zero(n) {
        n == 0;
    }

    // Note: These require % operator
    // fun is_even(n) { n % 2 == 0; }
    // fun is_odd(n) { n % 2 != 0; }

    // Clamp value to range (requires >= operator)
    // fun clamp(n, lo, hi) {
    //     if n < lo { lo; }
    //     else { if n > hi { hi; } else { n; } }
    // }

    // Greatest common divisor (requires % operator)
    // fun gcd(a, b) {
    //     if b == 0 { a; } else { gcd(b, a % b); }
    // }

    // Least common multiple (requires % and abs)
    // fun lcm(a, b) {
    //     abs(a * b) / gcd(a, b);
    // }

    // Factorial
    fun factorial(n) {
        if n < 2 {
            1;
        } else {
            n * factorial(n - 1);
        }
    }

    // Fibonacci
    fun fib(n) {
        if n < 2 {
            n;
        } else {
            fib(n - 1) + fib(n - 2);
        }
    }
}
```

### 3.5 `Flow.String` Module

```flux
module Flow.String {
    // Check if empty
    fun is_empty(s) {
        len(s) == 0;
    }

    // Check if blank (empty or whitespace only)
    // Requires trim builtin
    // fun is_blank(s) {
    //     len(trim(s)) == 0;
    // }

    // Repeat string n times
    fun repeat(s, n) {
        if n == 0 {
            "";
        } else {
            s + repeat(s, n - 1);
        }
    }

    // Reverse string (requires chars builtin)
    // fun reverse(s) {
    //     join(Flow.List.reverse(chars(s)), "");
    // }

    // Check if string contains only digits
    // Requires char operations
    // fun is_numeric(s) { ... }

    // Check if string contains only letters
    // Requires char operations
    // fun is_alpha(s) { ... }
}
```

### 3.6 `Flow.Dict` Module

```flux
module Flow.Dict {
    // Check if empty
    fun is_empty(h) {
        len(keys(h)) == 0;
    }

    // Get value with default
    fun get_or(h, key, default) {
        let value = h[key];
        match value {
            Some(v) -> v;
            _ -> default;
        }
    }

    // Map over values
    fun map_values(h, f) {
        let ks = keys(h);
        Flow.List.reduce(ks, {}, fun(acc, k) {
            // Need syntax for adding to hash
            acc;
        });
    }

    // Filter by predicate on values
    fun filter_values(h, pred) {
        // Implementation depends on hash construction syntax
        h;
    }
}
```

### 3.7 `Flow.Func` Module

```flux
module Flow.Func {
    // Identity function
    fun identity(x) {
        x;
    }

    // Constant function
    fun constant(x) {
        fun(_) { x; };
    }

    // Function composition: (f . g)(x) = f(g(x))
    fun compose(f, g) {
        fun(x) { f(g(x)); };
    }

    // Flip argument order
    fun flip(f) {
        fun(a, b) { f(b, a); };
    }

    // Apply function to value
    fun apply(f, x) {
        f(x);
    }

    // Pipe value through functions
    fun pipe(x, f) {
        f(x);
    }

    // Call function n times
    fun times(n, f) {
        if n == 0 {
            None;
        } else {
            f();
            times(n - 1, f);
        }
    }

    // Memoization (requires mutable state - not possible yet)
    // fun memoize(f) { ... }
}
```

---

## Implementation Roadmap

### Milestone 1: Core Operators (Week 1-2)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `<=` operator | Critical | Small | None |
| Add `>=` operator | Critical | Small | None |
| Add `%` operator | High | Small | None |
| Add `&&` operator (short-circuit) | Critical | Medium | None |
| Add `\|\|` operator (short-circuit) | Critical | Medium | None |

### Milestone 2: Essential Builtins (Week 3-4)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `concat(arr1, arr2)` | Critical | Small | None |
| Add `reverse(arr)` | High | Small | None |
| Add `slice(arr, start, end)` | High | Small | None |
| Add type checking builtins | High | Medium | None |
| Add `keys(h)`, `values(h)` | Medium | Small | None |

### Milestone 3: String Builtins (Week 5-6)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `split(s, delim)` | High | Small | None |
| Add `join(arr, delim)` | High | Small | None |
| Add `trim(s)` | Medium | Small | None |
| Add `upper(s)`, `lower(s)` | Medium | Small | None |
| Add `substring(s, start, end)` | Medium | Small | None |
| Add `chars(s)` | Medium | Small | None |

### Milestone 4: Math Builtins (Week 7)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `abs(n)` | High | Small | None |
| Add `min(a, b)`, `max(a, b)` | High | Small | None |
| Add `floor`, `ceil`, `round` | Medium | Small | None |
| Add `sqrt`, `pow` | Medium | Small | None |

### Milestone 5: Core Flow Modules (Week 8-10)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Implement `Flow.List` | Critical | Large | M1, M2 |
| Implement `Flow.Option` | High | Medium | None |
| Implement `Flow.Either` | High | Medium | Either type |
| Implement `Flow.Math` | Medium | Medium | M1, M4 |
| Implement `Flow.Func` | Medium | Small | None |
| Implement `Flow.String` | Medium | Medium | M3 |

### Milestone 6: Advanced Features (Week 11+)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `Either` type (Left/Right) | High | Large | None |
| Add `Tuple` type | Medium | Large | None |
| Implement TCO | Medium | Large | None |
| Pattern matching for arrays | Low | Large | None |
| Implement `Flow.Dict` | Medium | Medium | Either type |

---

## Testing Strategy

### Unit Tests

Each builtin should have tests in `tests/`:
```rust
#[test]
fn test_builtin_concat() {
    assert_eq!(run("[1, 2] |> concat([3, 4])"), "[1, 2, 3, 4]");
}
```

### Integration Tests

Each Flow module should have a test file:
```
tests/
├── flow_list_tests.rs
├── flow_option_tests.rs
├── flow_either_tests.rs
├── flow_math_tests.rs
└── ...
```

### Example Files

Each module should have example usage:
```
examples/flow/
├── list_examples.flx
├── option_examples.flx
├── either_examples.flx
└── ...
```

---

## Open Questions

1. **Module loading:** How should stdlib modules be automatically available?
   - Implicit import?
   - Prelude-style auto-import?
   - Explicit import required?

2. **Naming conventions:**
   - `Flow.List` vs `List` vs `flow.list`?
   - `is_empty` vs `isEmpty` vs `empty?`?

3. **Error handling:**
   - Add `Either` type now or later?
   - How to handle errors in builtins (return None vs Left vs panic)?

4. **Performance:**
   - When is TCO needed?
   - Which functions must be builtins vs pure Flux?

---

## References

- Haskell Prelude: https://hackage.haskell.org/package/base/docs/Prelude.html
- Elm Core: https://package.elm-lang.org/packages/elm/core/latest/
- Rust std: https://doc.rust-lang.org/std/
- F# Core: https://fsharp.github.io/fsharp-core-docs/
