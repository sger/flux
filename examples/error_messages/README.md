# Error Message Examples

This directory contains examples demonstrating enhanced error messages in Flux. Each file intentionally triggers a specific compiler error to showcase the improved contextual hints and suggestions.

## Enhanced Errors

### 1. Immutability Errors

**File:** `immutable_binding.flx`
**Error:** E002 - IMMUTABLE BINDING
**What it shows:** Enhanced explanation of Flux's immutability, with suggestions for rebinding via `let`.

```bash
cargo run -- examples/error_messages/immutable_binding.flx
```

### 2. Closure Scope Errors

**File:** `outer_assignment.flx`
**Error:** E003 - OUTER ASSIGNMENT
**What it shows:** Detailed explanation of closure variable capture with three solution approaches.

```bash
cargo run -- examples/error_messages/outer_assignment.flx
```

### 3. Unknown Keywords

**File:** `unknown_keyword.flx`
**Error:** E030 - UNKNOWN KEYWORD
**What it shows:** Lists all Flux keywords and highlights common mistakes from other languages (e.g., `function` vs `fun`).

```bash
cargo run -- examples/error_messages/unknown_keyword.flx
```

### 4. Syntax Errors

**File:** `unexpected_token.flx`
**Error:** E034 - UNEXPECTED TOKEN
**What it shows:** Context-aware hints about common syntax errors (missing semicolons, unclosed brackets, etc.).

```bash
cargo run -- examples/error_messages/unexpected_token.flx
```

### 5. Module Import Errors

**File:** `module_not_imported.flx`
**Error:** E013 - MODULE NOT IMPORTED
**What it shows:** Clear guidance on import statement placement and syntax, including alias usage.

```bash
cargo run -- examples/error_messages/module_not_imported.flx
```

**File:** `import_not_found.flx`
**Error:** E018 - IMPORT NOT FOUND
**What it shows:** Comprehensive explanation of module resolution, file paths, and the `--root` flag.

```bash
cargo run -- examples/error_messages/import_not_found.flx
```

## Purpose

These examples serve two purposes:

1. **Testing** - Verify that enhanced error messages display correctly
2. **Documentation** - Show developers what improved error messages look like in practice

## Key Improvements

All enhanced errors now include:

- **Context** - Why the error occurred
- **Multiple solutions** - Different ways to fix the issue
- **Common patterns** - Typical mistakes that lead to this error
- **Examples** - Concrete code showing the fix

## Before vs After

**Before (E002 - IMMUTABLE_BINDING):**
```
hint: Variables are immutable by default. Use `let x = ...` to rebind instead.
```

**After:**
```
hint: In Flux, bindings are immutable. Compute a new value with a new
      binding name (for example: `let next_x = ...`).
```

The enhanced messages provide more context, explain the reasoning, and offer multiple solutions.
