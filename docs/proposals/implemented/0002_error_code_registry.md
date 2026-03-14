- Feature Name: Error Code Registry
- Start Date: 2026-02-26
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0002: Error Code Registry

## Summary

**Rationale:**

- ✅ Zero runtime overhead (compiled into binary)
- ✅ No external dependencies (toml/serde)
- ✅ Compile-time type safety
- ✅ Simpler implementation
- ✅ Single binary distribution

## Motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation

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

### Phase 3: Refactor Usage Sites (2-3 hours)

- Update parser.rs error sites (15+ instances)
- Update compiler.rs error sites (10+ instances)
- Update other modules as needed

### Design

### Usage API

### Phase 3: Refactor Usage Sites (2-3 hours)

## Reference-level explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Multiple vs Single Error Reporting:** **Compiler Errors: Multiple (Collect & Report All)** - **Goal:** Centralize error code definitions (title, message, hint) to: - Prevent...
- **Multiple vs Single Error Reporting:** The compiler continues after errors to find ALL issues: ``` -- COMPILER ERROR: UNDEFINED VARIABLE -- test.flx -- [E007]
- **Goal:** Centralize error code definitions (title, message, hint) to: - Prevent duplicate error codes - Ensure consistent error formatting - Make error messages easy to update - Provide...
- **✅ What Exists:** `src/syntax/error_codes.rs` (31 error codes): ```rust pub struct ErrorCode { pub code: &'static str, pub title: &'static str, }
- **⚠️ Problems:** 1. **Missing error codes:** 26 documented codes not implemented (E120-E522) 2. **Hardcoded messages:** 29+ instances in parser.rs and compiler.rs 3. **No message templates:** Me...
- **Message Formatting:** /// Format error message by replacing {} placeholders with values pub fn format_message(template: &str, values: &[&str]) -> String { let mut result = template.to_string(); for v...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives

### Decision Summary

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

### Unified Error Format Decision

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

### Alternative A: TOML-Based Catalog (Rejected)

**Structure:**

```toml

### Alternative B: Enum-Based Catalog (CHOSEN ✅)

**Why Chosen:**
- ✅ **Performance:** Zero overhead, instant access
- ✅ **Simplicity:** Just Rust structs and arrays
- ✅ **Distribution:** Single binary, no external files
- ✅ **Type safety:** Compile-time verification

### Decision Summary

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

### Unified Error Format Decision

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

## Prior art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [v0.0.3 Roadmap](../versions/roadmap_v0.0.3.md) - M1 milestone
- Current implementation: `src/syntax/error_codes.rs`
- Error formatting: `src/syntax/diagnostic.rs`

### References

## Unresolved questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
