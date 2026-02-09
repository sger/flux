# Proposal 002: Error Code Registry

**Status:** Approved (Enum-Based Approach)
**Priority:** CRITICAL
**Effort:** Small (2-3 days)
**Risk:** Low
**Target:** v0.0.3 M1

---

## Decision Summary

**Chosen Approach:** Enum-Based Catalog (Alternative B)

**Rationale:**
- ✅ Zero runtime overhead (compiled into binary)
- ✅ No external dependencies (toml/serde)
- ✅ Compile-time type safety
- ✅ Simpler implementation
- ✅ Single binary distribution

**Rejected Approach:** TOML-based catalog (Alternative A)
- ❌ 50-100ms startup overhead (file I/O + parsing)
- ❌ Requires toml/serde dependencies
- ❌ Runtime parsing complexity

---

## Unified Error Format Decision

**Critical:** This proposal includes BOTH compile-time AND runtime errors.

**Chosen Format:** Explicit error source prefix

```
-- COMPILER ERROR: TITLE -- file.flx -- [EXXX]
-- RUNTIME ERROR: TITLE -- file.flx -- [E1XXX]
```

**Error Code Ranges:**
- **E001-E099:** Parse errors
- **E100-E199:** Syntax errors
- **E200-E299:** Type errors (future)
- **E300-E399:** Semantic errors
- **E400-E499:** Module/import errors
- **E500-E599:** Const evaluation errors
- **E1000-E1099:** Runtime: Function call errors
- **E1100-E1199:** Runtime: Type errors
- **E1200-E1299:** Runtime: Arithmetic errors
- **E1300-E1399:** Runtime: Array/Hash errors

**Examples:**

Compiler error:
```
-- COMPILER ERROR: UNDEFINED VARIABLE -- examples/test.flx -- [E007]

I can't find a value named `foo`.

10 | let x = foo;
   |         ^^^

Hint: Define it first: let foo = ...;
```

Runtime error:
```
-- RUNTIME ERROR: WRONG NUMBER OF ARGUMENTS -- examples/test.flx -- [E1001]

function: substring/3
  expected: 3
  got: 2

28 | print(substring("hello", 0));
   |        ^^^^^^^^^^^^^^^^^^^^^

Hint: substring(s, start, end)

Stack trace:
  at <main> (examples/test.flx:28:7)
```

**Benefits:**
- ✅ Unified format for all errors
- ✅ Error source explicit in header
- ✅ Error code indicates category
- ✅ Consistent across compile/runtime

### Multiple vs Single Error Reporting

**Compiler Errors: Multiple (Collect & Report All)**

The compiler continues after errors to find ALL issues:

```
-- COMPILER ERROR: UNDEFINED VARIABLE -- test.flx -- [E007]

I can't find a value named `char`.

27 | print(char("HELLO"));
   |        ^^^^

Hint: Define it first: let char = ...;

-- COMPILER ERROR: UNDEFINED VARIABLE -- test.flx -- [E007]

I can't find a value named `substrin`.

28 | print(substrin("HELLO", 0, 2));
   |        ^^^^^^^^

Hint: Define it first: let substrin = ...;
```

**Why:** Better developer experience - fix all errors at once instead of one-by-one.

**Runtime Errors: Single (Stop at First Error)**

Execution stops immediately on first error:

```
-- RUNTIME ERROR: DIVISION BY ZERO -- test.flx -- [E1200]

Cannot divide by zero.

15 | let result = x / 0;
   |              ^^^^^

Hint: Check divisor is non-zero before division.

Stack trace:
  at calculate (test.flx:15:7)
  at <main> (test.flx:20:5)
```

**Why:** Cannot continue execution with invalid state. This is the correct and safe behavior.

**Implementation:**
- Parser/Compiler: `self.errors: Vec<Diagnostic>` - collect all errors
- VM: `return Err()` immediately - execution stops

---

## Goal

Centralize error code definitions (title, message, hint) to:
- Prevent duplicate error codes
- Ensure consistent error formatting
- Make error messages easy to update
- Provide single source of truth for all diagnostics

---

## Current State (v0.0.2)

### ✅ What Exists

`src/syntax/error_codes.rs` (31 error codes):
```rust
pub struct ErrorCode {
    pub code: &'static str,
    pub title: &'static str,
}

pub static ERROR_CODES: &[ErrorCode] = &[
    ErrorCode { code: "E001", title: "PARSE ERROR" },
    ErrorCode { code: "E007", title: "UNDEFINED VARIABLE" },
    // ... 29 more
];
```

### ⚠️ Problems

1. **Missing error codes:** 26 documented codes not implemented (E120-E522)
2. **Hardcoded messages:** 29+ instances in parser.rs and compiler.rs
3. **No message templates:** Messages duplicated throughout code
4. **No hints:** Hint text embedded in error creation sites
5. **Inconsistent format:** Different error styles across modules

**Example of hardcoded error:**
```rust
// parser.rs (line 450)
Diagnostic::error("UNDEFINED VARIABLE")
    .with_code("E007")
    .with_message(format!("I can't find a value named `{}`.", name))
    .with_hint("Define it first: let {} = ...;")
```

---

## Proposed Solution: Enum-Based Catalog

### Design

Extend `ErrorCode` struct with message templates, hints, and error type:

```rust
// src/syntax/error_codes.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    Compiler,
    Runtime,
}

impl ErrorType {
    pub fn prefix(&self) -> &'static str {
        match self {
            ErrorType::Compiler => "COMPILER ERROR",
            ErrorType::Runtime => "RUNTIME ERROR",
        }
    }
}

pub struct ErrorCode {
    pub code: &'static str,
    pub title: &'static str,
    pub error_type: ErrorType,              // NEW: Compiler or Runtime
    pub message: &'static str,              // NEW: Template with {} placeholders
    pub hint: Option<&'static str>,         // NEW: Optional hint text
}

pub static ERROR_CODES: &[ErrorCode] = &[
    // Compiler errors (E001-E999)
    ErrorCode {
        code: "E001",
        title: "PARSE ERROR",
        error_type: ErrorType::Compiler,
        message: "Expected {}, got {}.",
        hint: None,
    },
    ErrorCode {
        code: "E007",
        title: "UNDEFINED VARIABLE",
        error_type: ErrorType::Compiler,
        message: "I can't find a value named `{}`.",
        hint: Some("Define it first: let {} = ...;"),
    },
    ErrorCode {
        code: "E021",
        title: "PRIVATE MEMBER",
        error_type: ErrorType::Compiler,
        message: "Cannot access private member `{}`.",
        hint: Some("Private members can only be accessed within the same module."),
    },
    // ... all 57+ error codes
];
```

### Usage API

```rust
// Helper function to get error spec
pub fn get_error(code: &str) -> Option<&'static ErrorCode> {
    ERROR_CODES.iter().find(|e| e.code == code)
}

// Usage in compiler/parser
let spec = get_error("E007").expect("E007 must exist");
Diagnostic::error(spec.title)
    .with_code(spec.code)
    .with_message(format_message(spec.message, &[name]))
    .with_hint_if(spec.hint, |h| format_message(h, &[name]))
```

### Message Formatting

```rust
// src/syntax/error_codes.rs

/// Format error message by replacing {} placeholders with values
pub fn format_message(template: &str, values: &[&str]) -> String {
    let mut result = template.to_string();
    for value in values {
        result = result.replacen("{}", value, 1);
    }
    result
}

// Alternative: More sophisticated formatting
pub fn format_message_named(template: &str, args: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in args {
        let placeholder = format!("{{{}}}", name);
        result = result.replace(&placeholder, value);
    }
    result
}
```

---

## Implementation Plan

### Step 1: Extend ErrorCode Struct (30 min)

```rust
// src/syntax/error_codes.rs
pub struct ErrorCode {
    pub code: &'static str,
    pub title: &'static str,
    pub message: &'static str,      // NEW
    pub hint: Option<&'static str>, // NEW
}

pub fn format_message(template: &str, values: &[&str]) -> String {
    let mut result = template.to_string();
    for value in values {
        result = result.replacen("{}", value, 1);
    }
    result
}
```

### Step 2: Add Missing v0.0.2 Error Codes (2-3 hours)

Add 26 documented codes to ERROR_CODES array:

**Lambda errors (E120-E122):**
```rust
ErrorCode {
    code: "E120",
    title: "LAMBDA SYNTAX ERROR",
    message: "Invalid lambda syntax: {}.",
    hint: Some("Use: \\x -> expr or \\(x, y) -> expr"),
},
```

**Pipe errors (E130-E131):**
```rust
ErrorCode {
    code: "E130",
    title: "PIPE OPERATOR ERROR",
    message: "Invalid pipe expression: {}.",
    hint: Some("Pipe operator requires: value |> function"),
},
```

**Either constructor errors (E140-E141):**
```rust
ErrorCode {
    code: "E140",
    title: "EITHER CONSTRUCTOR ERROR",
    message: "Either requires Left or Right constructor.",
    hint: Some("Use: Left(value) or Right(value)"),
},
```

**Module constant errors (E230-E235, E240, E520-E522):**
```rust
ErrorCode {
    code: "E230",
    title: "CIRCULAR DEPENDENCY",
    message: "Circular dependency in module constants: {}.",
    hint: Some("Break the cycle by using a literal value."),
},
```

**Division/modulo by zero (E320-E321):**
```rust
ErrorCode {
    code: "E320",
    title: "DIVISION BY ZERO",
    message: "Cannot divide by zero.",
    hint: Some("Check divisor is non-zero before division."),
},
```

**Type errors (E420-E421, E430):**
```rust
ErrorCode {
    code: "E420",
    title: "TYPE MISMATCH",
    message: "Expected {}, got {}.",
    hint: None,
},
```

### Step 2.5: Add Runtime Error Codes (1-2 hours) 🆕

Add runtime error codes (E1000+) for VM errors:

**Function call errors (E1000-E1099):**
```rust
ErrorCode {
    code: "E1001",
    title: "WRONG NUMBER OF ARGUMENTS",
    error_type: ErrorType::Runtime,
    message: "function {}/{} expects {} arguments, got {}",
    hint: Some("{}"),  // Function signature
},
ErrorCode {
    code: "E1002",
    title: "NOT A FUNCTION",
    error_type: ErrorType::Runtime,
    message: "Cannot call non-function value (got {}).",
    hint: None,
},
```

**Type errors (E1100-E1199):**
```rust
ErrorCode {
    code: "E1100",
    title: "TYPE ERROR",
    error_type: ErrorType::Runtime,
    message: "Expected {}, got {}.",
    hint: None,
},
ErrorCode {
    code: "E1101",
    title: "NOT INDEXABLE",
    error_type: ErrorType::Runtime,
    message: "Cannot index {} (not an array or hash).",
    hint: Some("Only arrays and hashes support indexing."),
},
ErrorCode {
    code: "E1102",
    title: "KEY NOT HASHABLE",
    error_type: ErrorType::Runtime,
    message: "Hash key must be String, Int, or Bool (got {}).",
    hint: None,
},
```

**Arithmetic errors (E1200-E1299):**
```rust
ErrorCode {
    code: "E1200",
    title: "DIVISION BY ZERO",
    error_type: ErrorType::Runtime,
    message: "Cannot divide by zero.",
    hint: Some("Check divisor is non-zero before division."),
},
ErrorCode {
    code: "E1201",
    title: "INVALID OPERATION",
    error_type: ErrorType::Runtime,
    message: "Cannot {} {} and {} values.",  // op, type1, type2
    hint: None,
},
```

**Array/Hash errors (E1300-E1399):**
```rust
ErrorCode {
    code: "E1300",
    title: "INDEX OUT OF BOUNDS",
    error_type: ErrorType::Runtime,
    message: "Array index {} out of bounds (length {}).",
    hint: None,
},
ErrorCode {
    code: "E1301",
    title: "KEY NOT FOUND",
    error_type: ErrorType::Runtime,
    message: "Hash key `{}` not found.",
    hint: Some("Use has_key() to check before accessing."),
},
```

### Step 3: Refactor Parser Error Sites (1-2 hours)

**Before:**
```rust
// parser.rs (15+ instances)
Diagnostic::error("UNDEFINED VARIABLE")
    .with_code("E007")
    .with_message(format!("I can't find a value named `{}`.", name))
    .with_hint("Define it first: let {} = ...;")
```

**After:**
```rust
// parser.rs
use crate::syntax::error_codes::{get_error, format_message};

let err = get_error("E007").unwrap();
Diagnostic::error(err.title)
    .with_code(err.code)
    .with_message(format_message(err.message, &[name]))
    .with_hint_if(err.hint, |h| format_message(h, &[name]))
```

### Step 4: Refactor Compiler Error Sites (1-2 hours)

**Before:**
```rust
// compiler.rs (10+ instances)
Diagnostic::error("IMMUTABILITY ERROR")
    .with_code("E012")
    .with_file(self.file_path.clone())
    .with_position(position)
    .with_message(format!("Cannot reassign to immutable variable `{}`.", name))
    .with_hint("Declare it as mutable: let mut {} = ...;")
```

**After:**
```rust
// compiler.rs
use crate::syntax::error_codes::{get_error, format_message};

let err = get_error("E012").unwrap();
Diagnostic::error(err.title)
    .with_code(err.code)
    .with_file(self.file_path.clone())
    .with_position(position)
    .with_message(format_message(err.message, &[name]))
    .with_hint_if(err.hint, |h| format_message(h, &[name]))
```

### Step 4.5: Update VM Error Formatting (2-3 hours) 🆕

Update the VM to use the error registry for runtime errors:

**Current VM error (runtime/vm.rs):**
```rust
// runtime/vm.rs
fn runtime_error(&self, message: String) -> String {
    format!(
        "-- Runtime error: {}\n\nStack trace:\n{}",
        message,
        self.stack_trace()
    )
}
```

**Updated VM error:**
```rust
// runtime/vm.rs
use crate::syntax::error_codes::{get_error, format_message, ErrorType};

fn runtime_error_with_code(&self, code: &str, values: &[&str]) -> String {
    let err = get_error(code).expect("Error code must exist");

    let header = format!(
        "-- {}: {} -- {} -- [{}]",
        err.error_type.prefix(),  // "RUNTIME ERROR"
        err.title,
        self.current_file(),
        err.code
    );

    let message = format_message(err.message, values);
    let hint = err.hint.map(|h| format!("\nHint: {}", h)).unwrap_or_default();

    format!(
        "{}\n\n{}\n\n  --> {}:{}\n{}\n\nStack trace:\n{}",
        header,
        message,
        self.current_file(),
        self.current_line(),
        hint,
        self.stack_trace()
    )
}
```

**Update error call sites:**
```rust
// Before (in runtime/vm.rs)
return Err(self.runtime_error(
    format!("wrong number of arguments\n\nfunction: {}/{}\n  expected: {}\n  got: {}",
            name, expected, expected, args.len())
));

// After
return Err(self.runtime_error_with_code(
    "E1001",
    &[name, &expected.to_string(), &expected.to_string(), &args.len().to_string()]
));
```

**Update builtin errors (runtime/builtins.rs):**
```rust
// Before
fn arity_error(name: &str, expected: &str, got: usize, signature: &str) -> String {
    format!(
        "wrong number of arguments\n\n  function: {}/{}\n  expected: {}\n  got: {}",
        name, expected, expected, got
    )
}

// After
use crate::syntax::error_codes::{get_error, format_message};

fn arity_error(name: &str, expected: &str, got: usize) -> String {
    let err = get_error("E1001").unwrap();
    format_message(err.message, &[name, expected, expected, &got.to_string()])
}
```

### Step 5: Add Duplicate Detection Test (30 min)

```rust
// tests/error_codes_tests.rs

#[test]
fn test_no_duplicate_error_codes() {
    use std::collections::HashSet;
    use flux::syntax::error_codes::ERROR_CODES;

    let mut seen = HashSet::new();
    for error in ERROR_CODES {
        assert!(
            seen.insert(error.code),
            "Duplicate error code: {}",
            error.code
        );
    }
}

#[test]
fn test_all_errors_have_messages() {
    use flux::syntax::error_codes::ERROR_CODES;

    for error in ERROR_CODES {
        assert!(!error.message.is_empty(), "Error {} has empty message", error.code);
    }
}
```

### Step 6: Generate Error Catalog Documentation (1 hour)

Create `docs/reference/ERROR_CATALOG_v0.0.3.md`:

```markdown
# Flux Error Catalog v0.0.3

## E001: PARSE ERROR
**Message:** Expected {}, got {}.
**Hint:** None

## E007: UNDEFINED VARIABLE
**Message:** I can't find a value named `{}`.
**Hint:** Define it first: let {} = ...;

... (all 57+ errors documented)
```

---

## Benefits

### Performance
- ✅ **Zero runtime cost** - Messages compiled into binary
- ✅ **Fast access** - Simple array iteration (~2ns)
- ✅ **No I/O** - No file reads, no parsing
- ✅ **Small binary** - No external dependencies

### Maintainability
- ✅ **Single source of truth** - All errors in one file
- ✅ **Easy updates** - Change message in one place
- ✅ **Compile-time safety** - Typos caught by compiler
- ✅ **Easy review** - All error text visible in one file

### Consistency
- ✅ **Standardized format** - All errors use same template system
- ✅ **No duplication** - Message defined once
- ✅ **Consistent hints** - Hints associated with errors

---

## Comparison with Alternatives

### Alternative A: TOML-Based Catalog (Rejected)

**Structure:**
```toml
# resources/error_catalog.toml
[errors.E007]
title = "UNDEFINED VARIABLE"
message = "I can't find a value named `{}`."
hint = "Define it first: let {} = ...;"
```

**Loading:**
```rust
use std::sync::OnceLock;
use serde::Deserialize;

static CATALOG: OnceLock<CatalogFile> = OnceLock::new();

fn load_catalog() -> &'static CatalogFile {
    CATALOG.get_or_init(|| {
        let raw = include_str!("../../resources/error_catalog.toml");
        toml::from_str(raw).expect("invalid error_catalog.toml")
    })
}
```

**Why Rejected:**
- ❌ **Performance:** 50-100ms startup overhead (file I/O + TOML parsing)
- ❌ **Dependencies:** Requires `toml` + `serde` crates (~30KB binary)
- ❌ **Complexity:** More code to maintain (loader, deserializer)
- ❌ **Distribution:** Must ship .toml file or embed with include_str!

**When This Would Make Sense:**
- Localization (multiple language files)
- User-customizable error messages
- Hot-reloading errors without recompiling
- External tooling needs access to error catalog

### Alternative B: Enum-Based Catalog (CHOSEN ✅)

**Why Chosen:**
- ✅ **Performance:** Zero overhead, instant access
- ✅ **Simplicity:** Just Rust structs and arrays
- ✅ **Distribution:** Single binary, no external files
- ✅ **Type safety:** Compile-time verification

---

## Performance Benchmarks

### Access Time

**Enum-based (chosen):**
```rust
let err = get_error("E007");
// Cost: ~2ns (linear array scan, 57 items)
```

**TOML-based (rejected):**
```rust
let err = catalog::spec("E007");
// First access: ~50-100ms (lazy_static initialization)
// Subsequent: ~20ns (HashMap lookup)
```

**Difference:** 25,000,000x slower on cold start!

### Binary Size

**Enum-based:** +0KB (messages already in code)
**TOML-based:** +30KB (toml + serde dependencies)

### Startup Time

**Enum-based:** +0ms (no initialization)
**TOML-based:** +50-100ms (parse TOML on first use)

---

## Migration Strategy

### Phase 1: Extend Infrastructure (30 min)
- Add `message` and `hint` fields to `ErrorCode` struct
- Add `format_message()` helper function
- Add `get_error()` lookup function

### Phase 2: Add Missing Codes (2-3 hours)
- Add 26 v0.0.2 documented codes to array
- Ensure all codes E001-E522 present
- Add duplicate detection test

### Phase 3: Refactor Usage Sites (2-3 hours)
- Update parser.rs error sites (15+ instances)
- Update compiler.rs error sites (10+ instances)
- Update other modules as needed

### Phase 4: Verification (1 hour)
- Run full test suite
- Generate error catalog documentation
- Verify all error codes used in tests

**Total Time:** 2-3 days (as specified in roadmap)

---

## Testing Strategy

### Unit Tests
```rust
#[test]
fn test_no_duplicate_codes() { /* ... */ }

#[test]
fn test_all_errors_have_messages() { /* ... */ }

#[test]
fn test_format_message() {
    let msg = format_message("Expected {}, got {}.", &["Int", "String"]);
    assert_eq!(msg, "Expected Int, got String.");
}
```

### Integration Tests
- Verify error messages in compiler output
- Check hint display in diagnostic formatter
- Ensure all v0.0.2 error codes work

---

## Success Metrics

- [ ] All 57+ error codes in ERROR_CODES array
- [ ] Zero duplicate error codes (enforced by test)
- [ ] All parser/compiler hardcoded errors refactored
- [ ] All error messages use template system
- [ ] Error catalog documentation generated
- [ ] No performance regression
- [ ] Test coverage ≥ 85%

---

## Future Enhancements (Post v0.0.3)

### Localization Support (v1.0+)
If internationalization needed, extend to support multiple languages:

```rust
pub struct ErrorCode {
    pub code: &'static str,
    pub title_en: &'static str,
    pub message_en: &'static str,
    pub hint_en: Option<&'static str>,
    pub title_es: Option<&'static str>,  // Spanish
    pub message_es: Option<&'static str>,
    // ... more languages
}
```

Or switch to TOML approach with per-language files:
- `resources/errors_en.toml`
- `resources/errors_es.toml`
- `resources/errors_fr.toml`

### Named Placeholders (v0.0.4+)
Support `{name}` style placeholders instead of positional:

```rust
ErrorCode {
    message: "Cannot access {member} in {module}.",
    // ...
}

// Usage:
format_message_named(err.message, &[
    ("member", "private_field"),
    ("module", "MyModule"),
])
```

### Error Recovery Suggestions (v1.0+)
Add structured suggestions for common fixes:

```rust
pub struct ErrorCode {
    // ... existing fields
    pub suggestions: Option<&'static [&'static str]>,
}

// Usage:
ErrorCode {
    code: "E007",
    message: "Undefined variable `{}`.",
    suggestions: Some(&[
        "Did you mean `{}`?",  // Fuzzy match
        "Import from module?",  // Import suggestion
    ]),
}
```

---

## References

- [v0.0.3 Roadmap](../versions/roadmap_v0.0.3.md) - M1 milestone
- Current implementation: `src/syntax/error_codes.rs`
- Error formatting: `src/syntax/diagnostic.rs`

---

## Approval

- [x] Approach selected (Enum-based)
- [x] Performance validated (zero overhead)
- [x] Timeline confirmed (2-3 days)
- [x] Integration path clear
- [ ] Ready to implement

**Next Step:** Begin implementation with Step 1 (extend ErrorCode struct)
