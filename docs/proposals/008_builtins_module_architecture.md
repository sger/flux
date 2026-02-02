# Proposal 008: Builtins Module Architecture

**Status:** Planning
**Priority:** Medium (Developer Experience)
**Created:** 2026-02-02

## Overview

This proposal outlines a modular architecture for Flux's built-in functions to improve maintainability and make updates easier. The goal is to split the monolithic `builtins.rs` (757 lines, 35 functions) into focused, category-based modules with reduced boilerplate.

## Problem Statement

The current `builtins.rs` has several issues:

1. **Monolithic structure** - All 35 built-in functions in one file
2. **Repetitive boilerplate** - Every function manually handles:
   - Arity checking
   - Type validation
   - Error formatting
   - Signature hints
3. **Poor discoverability** - Hard to find functions by category
4. **Difficult to test** - All functions tightly coupled
5. **Manual registration** - Must add to both function definition and `BUILTINS` array
6. **Update friction** - Small changes require navigating large file

**Current structure:**
```
builtins.rs (757 lines)
├── Helper functions (120 lines)
│   ├── format_hint, arity_error, type_error
│   ├── check_arity, check_arity_range
│   └── arg_string, arg_array, arg_int, arg_hash, arg_number
├── 35 builtin functions (550 lines)
│   ├── Array: first, last, rest, push, reverse, slice, sort, concat
│   ├── String: chars, contains, join, lower, split, substring, trim, upper
│   ├── Hash: keys, values, has_key, merge
│   ├── Type: type_of, is_int, is_float, is_string, is_bool, is_array, is_hash, is_none, is_some
│   ├── Numeric: abs, min, max
│   └── Utility: print, len, to_string
└── Registration (87 lines)
    └── BUILTINS array + lookup functions
```

## Goals

### Primary Goals
1. **Easy updates** - Add/modify built-ins without touching other modules
2. **Clear organization** - Group functions by category (array, string, hash, etc.)
3. **Reduced boilerplate** - Declarative function definitions with automatic validation
4. **Better testing** - Test categories independently
5. **Self-documenting** - Function signatures and docs in one place

### Non-Goals
- Changing the VM interface (`OpGetBuiltin`)
- Modifying function behavior
- Adding new built-in functions (that's a separate task)

## Proposed Architecture

### 1. Module Structure

```
src/runtime/
├── builtins/
│   ├── mod.rs              # Public exports, registry, lookup functions
│   ├── macros.rs           # Declarative built-in definition macros
│   ├── helpers.rs          # Validation and error helpers
│   ├── array_ops.rs        # Array operations (8 functions)
│   ├── string_ops.rs       # String operations (8 functions)
│   ├── hash_ops.rs         # Hash operations (4 functions)
│   ├── type_ops.rs         # Type introspection (9 functions)
│   ├── numeric_ops.rs      # Numeric operations (3 functions)
│   └── util_ops.rs         # Utility functions (3 functions)
└── builtins.rs             # DEPRECATED - re-exports for compatibility
```

### 2. Declarative Built-in Definition

**Goal:** Reduce boilerplate using a macro-based approach

#### Current (verbose):
```rust
fn builtin_first(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "first", "first(arr)")?;
    let arr = arg_array(&args, 0, "first", "argument", "first(arr)")?;
    if arr.is_empty() {
        Ok(Object::None)
    } else {
        Ok(arr[0].clone())
    }
}

// Later in file...
BuiltinFunction {
    name: "first",
    func: builtin_first,
},
```

#### Proposed (declarative):
```rust
builtin! {
    /// Returns the first element of an array, or None if empty.
    fn first(arr: Array) -> Object {
        arr.first().cloned().unwrap_or(Object::None)
    }
}
```

**The macro expands to:**
```rust
fn builtin_first(args: Vec<Object>) -> Result<Object, String> {
    // Auto-generated arity check
    if args.len() != 1 {
        return Err(arity_error("first", "1", args.len(), "first(arr)"));
    }

    // Auto-generated type validation
    let arr = match &args[0] {
        Object::Array(a) => a,
        other => return Err(type_error("first", "argument", "Array", other.type_name(), "first(arr)")),
    };

    // User's logic
    Ok(arr.first().cloned().unwrap_or(Object::None))
}
```

### 3. Module Breakdown

#### 3a. `builtins/mod.rs` (150 lines)
```rust
//! Built-in function registry and lookup

pub mod macros;
pub mod helpers;
pub mod array_ops;
pub mod string_ops;
pub mod hash_ops;
pub mod type_ops;
pub mod numeric_ops;
pub mod util_ops;

use crate::runtime::builtin_function::BuiltinFunction;

/// All built-in functions in order (index matters for OpGetBuiltin)
pub static BUILTINS: &[BuiltinFunction] = &[
    // Utility
    util_ops::PRINT,
    util_ops::LEN,
    util_ops::TO_STRING,

    // Array operations
    array_ops::FIRST,
    array_ops::LAST,
    array_ops::REST,
    array_ops::PUSH,
    array_ops::CONCAT,
    array_ops::REVERSE,
    array_ops::SLICE,
    array_ops::SORT,

    // String operations
    string_ops::CHARS,
    string_ops::CONTAINS,
    string_ops::JOIN,
    string_ops::SPLIT,
    string_ops::SUBSTRING,
    string_ops::LOWER,
    string_ops::UPPER,
    string_ops::TRIM,

    // Hash operations
    hash_ops::KEYS,
    hash_ops::VALUES,
    hash_ops::HAS_KEY,
    hash_ops::MERGE,

    // Type introspection
    type_ops::TYPE_OF,
    type_ops::IS_INT,
    type_ops::IS_FLOAT,
    type_ops::IS_STRING,
    type_ops::IS_BOOL,
    type_ops::IS_ARRAY,
    type_ops::IS_HASH,
    type_ops::IS_NONE,
    type_ops::IS_SOME,

    // Numeric operations
    numeric_ops::ABS,
    numeric_ops::MIN,
    numeric_ops::MAX,
];

pub fn get_builtin(name: &str) -> Option<&'static BuiltinFunction> {
    BUILTINS.iter().find(|b| b.name == name)
}

pub fn get_builtin_by_index(index: usize) -> Option<&'static BuiltinFunction> {
    BUILTINS.get(index)
}
```

#### 3b. `builtins/macros.rs` (200 lines)
```rust
//! Declarative macros for defining built-in functions

/// Define a built-in function with automatic type checking and validation.
///
/// # Examples
///
/// ```
/// builtin! {
///     /// Get first element of array
///     fn first(arr: Array) -> Object {
///         arr.first().cloned().unwrap_or(Object::None)
///     }
/// }
/// ```
#[macro_export]
macro_rules! builtin {
    (
        $(#[$meta:meta])*
        fn $name:ident( $($arg:ident: $typ:ty),* ) -> $ret:ty {
            $($body:tt)*
        }
    ) => {
        $(#[$meta])*
        fn $name(args: Vec<Object>) -> Result<Object, String> {
            // Expand to validation + body
            builtin_impl!($name, args, [$(($arg, $typ)),*], { $($body)* })
        }

        pub static [<$name:upper>]: BuiltinFunction = BuiltinFunction {
            name: stringify!($name),
            func: $name,
        };
    };
}

// Helper macro for type validation
macro_rules! validate_arg {
    ($args:expr, $idx:expr, $name:expr, Array) => {
        match &$args[$idx] {
            Object::Array(a) => a,
            other => return Err(type_error(
                stringify!($name),
                &format!("argument {}", $idx + 1),
                "Array",
                other.type_name(),
                &signature_for!($name),
            )),
        }
    };
    ($args:expr, $idx:expr, $name:expr, String) => {
        match &$args[$idx] {
            Object::String(s) => s.as_str(),
            other => return Err(type_error(
                stringify!($name),
                &format!("argument {}", $idx + 1),
                "String",
                other.type_name(),
                &signature_for!($name),
            )),
        }
    };
    // ... similar for Integer, Hash, etc.
}
```

**Alternative (simpler):** Skip macros initially, just use helper functions with better abstractions.

#### 3c. `builtins/helpers.rs` (120 lines)
```rust
//! Validation and error formatting helpers

pub fn format_hint(signature: &str) -> String {
    format!("\n\nHint:\n  {}", signature)
}

pub fn arity_error(name: &str, expected: &str, got: usize, signature: &str) -> String {
    format!(
        "wrong number of arguments\n\n  function: {}/{}\n  expected: {}\n  got: {}{}",
        name, expected, expected, got, format_hint(signature)
    )
}

pub fn type_error(name: &str, label: &str, expected: &str, got: &str, signature: &str) -> String {
    format!(
        "{} expected {} to be {}, got {}{}",
        name, label, expected, got, format_hint(signature)
    )
}

// Arity checking
pub fn check_arity(
    args: &[Object],
    expected: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> { /* ... */ }

pub fn check_arity_range(
    args: &[Object],
    min: usize,
    max: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> { /* ... */ }

// Type extraction
pub fn arg_string<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a str, String> { /* ... */ }

pub fn arg_array<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a Vec<Object>, String> { /* ... */ }

pub fn arg_int(
    args: &[Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<i64, String> { /* ... */ }

pub fn arg_hash<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a HashMap<HashKey, Object>, String> { /* ... */ }

pub fn arg_number(
    args: &[Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<f64, String> { /* ... */ }
```

#### 3d. `builtins/array_ops.rs` (200 lines)
```rust
//! Array manipulation built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object};
use super::helpers::*;

/// first(arr) - Return first element of array, or None if empty
fn builtin_first(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "first", "first(arr)")?;
    let arr = arg_array(&args, 0, "first", "argument", "first(arr)")?;
    Ok(arr.first().cloned().unwrap_or(Object::None))
}

pub static FIRST: BuiltinFunction = BuiltinFunction {
    name: "first",
    func: builtin_first,
};

/// last(arr) - Return last element of array, or None if empty
fn builtin_last(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "last", "last(arr)")?;
    let arr = arg_array(&args, 0, "last", "argument", "last(arr)")?;
    Ok(arr.last().cloned().unwrap_or(Object::None))
}

pub static LAST: BuiltinFunction = BuiltinFunction {
    name: "last",
    func: builtin_last,
};

/// rest(arr) - Return array without first element
fn builtin_rest(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "rest", "rest(arr)")?;
    let arr = arg_array(&args, 0, "rest", "argument", "rest(arr)")?;
    if arr.is_empty() {
        Ok(Object::None)
    } else {
        Ok(Object::Array(arr[1..].to_vec()))
    }
}

pub static REST: BuiltinFunction = BuiltinFunction {
    name: "rest",
    func: builtin_rest,
};

// ... similar for push, concat, reverse, slice, sort
```

#### 3e. `builtins/string_ops.rs` (200 lines)
```rust
//! String manipulation built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object};
use super::helpers::*;

/// chars(s) - Convert string to array of single-character strings
fn builtin_chars(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "chars", "chars(s)")?;
    let s = arg_string(&args, 0, "chars", "argument", "chars(s)")?;
    let chars: Vec<Object> = s.chars().map(|c| Object::String(c.to_string())).collect();
    Ok(Object::Array(chars))
}

pub static CHARS: BuiltinFunction = BuiltinFunction {
    name: "chars",
    func: builtin_chars,
};

/// lower(s) - Convert string to lowercase
fn builtin_lower(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "lower", "lower(s)")?;
    let s = arg_string(&args, 0, "lower", "argument", "lower(s)")?;
    Ok(Object::String(s.to_lowercase()))
}

pub static LOWER: BuiltinFunction = BuiltinFunction {
    name: "lower",
    func: builtin_lower,
};

// ... similar for upper, trim, split, join, substring, contains
```

#### 3f. `builtins/hash_ops.rs` (120 lines)
```rust
//! Hash manipulation built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object, hash_key::HashKey};
use super::helpers::*;

/// Convert a HashKey back to an Object
fn hash_key_to_object(key: &HashKey) -> Object {
    match key {
        HashKey::Integer(v) => Object::Integer(*v),
        HashKey::Boolean(v) => Object::Boolean(*v),
        HashKey::String(v) => Object::String(v.clone()),
    }
}

/// keys(h) - Return array of all keys in hash
fn builtin_keys(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "keys", "keys(h)")?;
    let hash = arg_hash(&args, 0, "keys", "argument", "keys(h)")?;
    let keys: Vec<Object> = hash.keys().map(hash_key_to_object).collect();
    Ok(Object::Array(keys))
}

pub static KEYS: BuiltinFunction = BuiltinFunction {
    name: "keys",
    func: builtin_keys,
};

// ... similar for values, has_key, merge
```

#### 3g. `builtins/type_ops.rs` (150 lines)
```rust
//! Type introspection built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object};
use super::helpers::*;

/// type_of(x) - Return type name as string
fn builtin_type_of(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "type_of", "type_of(x)")?;
    Ok(Object::String(args[0].type_name().to_string()))
}

pub static TYPE_OF: BuiltinFunction = BuiltinFunction {
    name: "type_of",
    func: builtin_type_of,
};

/// is_int(x) - Check if value is an integer
fn builtin_is_int(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Integer(_))))
}

pub static IS_INT: BuiltinFunction = BuiltinFunction {
    name: "is_int",
    func: builtin_is_int,
};

// ... similar for is_float, is_string, is_bool, is_array, is_hash, is_none, is_some
```

#### 3h. `builtins/numeric_ops.rs` (80 lines)
```rust
//! Numeric operation built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object};
use super::helpers::*;

/// abs(n) - Return absolute value
fn builtin_abs(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "abs", "abs(n)")?;
    let n = arg_number(&args, 0, "abs", "argument", "abs(n)")?;
    Ok(if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 {
        Object::Integer(n.abs() as i64)
    } else {
        Object::Float(n.abs())
    })
}

pub static ABS: BuiltinFunction = BuiltinFunction {
    name: "abs",
    func: builtin_abs,
};

// ... similar for min, max
```

#### 3i. `builtins/util_ops.rs` (100 lines)
```rust
//! Utility built-in functions

use crate::runtime::{builtin_function::BuiltinFunction, object::Object};
use super::helpers::*;

/// print(...) - Print values to stdout
fn builtin_print(args: Vec<Object>) -> Result<Object, String> {
    for arg in &args {
        print!("{}", arg);
    }
    println!();
    Ok(Object::None)
}

pub static PRINT: BuiltinFunction = BuiltinFunction {
    name: "print",
    func: builtin_print,
};

/// len(x) - Return length of array/string/hash
fn builtin_len(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "len", "len(x)")?;
    match &args[0] {
        Object::String(s) => Ok(Object::Integer(s.chars().count() as i64)),
        Object::Array(arr) => Ok(Object::Integer(arr.len() as i64)),
        Object::Hash(h) => Ok(Object::Integer(h.len() as i64)),
        other => Err(format!(
            "len expected String, Array, or Hash, got {}{}",
            other.type_name(),
            format_hint("len(x)")
        )),
    }
}

pub static LEN: BuiltinFunction = BuiltinFunction {
    name: "len",
    func: builtin_len,
};

// ... similar for to_string
```

### 4. Migration Strategy

**Phase 1: Setup (30 minutes)**
1. Create `src/runtime/builtins/` directory
2. Create `mod.rs` with module declarations
3. Move helpers to `helpers.rs`

**Phase 2: Extract Categories (2 hours)**
4. Extract array operations to `array_ops.rs` (test after each)
5. Extract string operations to `string_ops.rs`
6. Extract hash operations to `hash_ops.rs`
7. Extract type operations to `type_ops.rs`
8. Extract numeric operations to `numeric_ops.rs`
9. Extract utility operations to `util_ops.rs`

**Phase 3: Update Registry (30 minutes)**
10. Update `mod.rs` to reference all module constants
11. Run full test suite
12. Update `builtins.rs` to re-export from `builtins/mod.rs`

**Phase 4: Verify (30 minutes)**
13. Run all tests
14. Run all examples
15. Verify no performance regression

**Total estimated time:** 4-5 hours

## Benefits

### Developer Experience
1. **Easy to add built-ins** - Just add to the relevant category file
2. **Easy to find** - Functions organized by purpose
3. **Easy to test** - Test categories independently
4. **Clear scope** - Each module has single responsibility

### Code Quality
1. **Reduced duplication** - Shared helpers extracted
2. **Better documentation** - Functions grouped logically
3. **Easier review** - Changes localized to relevant module
4. **Less merge conflicts** - Multiple developers can work on different categories

### Example: Adding a New Built-in

**Before (monolithic):**
```rust
// 1. Scroll through 757-line file to find right spot
// 2. Add function (lines 550-570 in builtins.rs)
fn builtin_capitalize(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "capitalize", "capitalize(s)")?;
    let s = arg_string(&args, 0, "capitalize", "argument", "capitalize(s)")?;
    // implementation
}

// 3. Scroll to BUILTINS array (line 608)
// 4. Add entry in correct position
BuiltinFunction {
    name: "capitalize",
    func: builtin_capitalize,
},
```

**After (modular):**
```rust
// 1. Open builtins/string_ops.rs
// 2. Add function and constant in one place
fn builtin_capitalize(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "capitalize", "capitalize(s)")?;
    let s = arg_string(&args, 0, "capitalize", "argument", "capitalize(s)")?;
    // implementation
}

pub static CAPITALIZE: BuiltinFunction = BuiltinFunction {
    name: "capitalize",
    func: builtin_capitalize,
};

// 3. Add to builtins/mod.rs registry
string_ops::CAPITALIZE,
```

**Improvement:** 3 steps, all in related files, no navigation through large file.

## Testing Strategy

### Per-Module Tests
Each module gets its own test suite:

```rust
// builtins/array_ops.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_with_empty_array() {
        let result = builtin_first(vec![Object::Array(vec![])]);
        assert_eq!(result, Ok(Object::None));
    }

    #[test]
    fn test_first_with_elements() {
        let result = builtin_first(vec![Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
        ])]);
        assert_eq!(result, Ok(Object::Integer(1)));
    }

    // ... more tests
}
```

### Integration Tests
Ensure registry works correctly:

```rust
#[test]
fn test_all_builtins_registered() {
    assert_eq!(BUILTINS.len(), 35);
    assert!(get_builtin("first").is_some());
    assert!(get_builtin("last").is_some());
    // ... test all names
}

#[test]
fn test_builtin_index_order() {
    assert_eq!(BUILTINS[0].name, "print");
    assert_eq!(BUILTINS[1].name, "len");
    // Critical: VM relies on this order
}
```

## Success Metrics

### Code Organization
- **File count:** 1 → 9 files (mod.rs + 7 category modules + helpers)
- **Largest file:** 757 lines → ~200 lines max
- **Average file size:** ~100 lines

### Developer Experience
- **Time to add built-in:** 15 min → 5 min
- **Lines to add built-in:** 10-15 lines → 8-10 lines
- **Time to find built-in:** 2 min (search) → 10 sec (go to category)

### Maintainability
- **Test isolation:** ✅ Each category independently testable
- **Documentation:** ✅ Functions grouped by purpose
- **Merge conflicts:** Reduced (work on different categories)

## Risks and Mitigation

### Risk 1: Breaking VM Compatibility
**Likelihood:** Low
**Impact:** High
**Mitigation:**
- Keep `BUILTINS` array order identical
- Test `get_builtin_by_index` thoroughly
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

## Future Enhancements

### 1. Macro-Based Definitions (Optional)
After initial split, consider adding macros:

```rust
builtin! {
    /// first(arr) - Return first element
    fn first(arr: Array) -> Object {
        arr.first().cloned().unwrap_or(Object::None)
    }
}
```

Reduces boilerplate by ~30%.

### 2. Auto-Generated Documentation
Generate docs from built-in definitions:

```bash
cargo run --bin generate-builtins-docs
# Outputs: docs/builtins.md
```

### 3. Plugin System (Future)
Allow users to register custom built-ins:

```rust
register_builtin! {
    fn my_custom_function(args: Vec<Object>) -> Result<Object, String> {
        // custom logic
    }
}
```

## Implementation Checklist

- [ ] Create `src/runtime/builtins/` directory
- [ ] Extract helpers to `helpers.rs`
- [ ] Split array operations to `array_ops.rs`
- [ ] Split string operations to `string_ops.rs`
- [ ] Split hash operations to `hash_ops.rs`
- [ ] Split type operations to `type_ops.rs`
- [ ] Split numeric operations to `numeric_ops.rs`
- [ ] Split utility operations to `util_ops.rs`
- [ ] Create `mod.rs` with registry
- [ ] Update `builtins.rs` to re-export
- [ ] Add per-module tests
- [ ] Run full test suite
- [ ] Run all examples
- [ ] Update documentation

## References

- [Phase 1 Module Split Plan](006_phase1_module_split_plan.md)
- [Rust API Guidelines: Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)
- Current implementation: `src/runtime/builtins.rs`

## Conclusion

This modular architecture makes built-in functions easier to maintain, update, and test. The split follows Rust best practices and aligns with Phase 1's module organization goals.

**Recommendation:** Implement after compiler/parser/VM splits are complete (Phase 1 weeks 4-5).
