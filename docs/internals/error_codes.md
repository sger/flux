# Error Code Reference

> Source: `src/diagnostics/compiler_errors.rs`, `src/diagnostics/runtime_errors.rs`, `src/diagnostics/registry.rs`

Flux uses stable error codes for all diagnostics. Codes are prefixed `E` (error) or `W` (warning).

## Code Ranges

| Range | Category | Source |
|-------|----------|--------|
| `E001–E060` | Compiler — semantic checks | `compiler_errors.rs` |
| `E061–E070` | Internal compiler errors (ICE) | `compiler_errors.rs` |
| `E071–E077` | Lexer / parser errors | `compiler_errors.rs` |
| `E1000–E1021` | Runtime errors | `runtime_errors.rs` |
| `W2xx` | Warnings (linter) | `compiler_errors.rs` |

---

## Compiler Errors (E001–E077)

### Variable and Binding

| Code | Constant | Description |
|------|----------|-------------|
| `E001` | `DUPLICATE_NAME` | Name already declared in this scope |
| `E002` | `IMMUTABLE_BINDING` | Attempt to reassign an immutable `let` binding |
| `E003` | `OUTER_ASSIGNMENT` | Assignment to a variable in an outer scope |
| `E004` | `UNDEFINED_VARIABLE` | Variable used before declaration |
| `E007` | `DUPLICATE_PARAMETER` | Parameter name used more than once in a function |

### Operators

| Code | Constant | Description |
|------|----------|-------------|
| `E005` | `UNKNOWN_PREFIX_OPERATOR` | Unrecognized prefix operator |
| `E006` | `UNKNOWN_INFIX_OPERATOR` | Unrecognized infix operator |

### Module System

| Code | Constant | Description |
|------|----------|-------------|
| `E008` | `INVALID_MODULE_NAME` | Module name does not match file path or naming rules |
| `E009` | `MODULE_NAME_CLASH` | Two modules share the same name |
| `E010` | `INVALID_MODULE_CONTENT` | Illegal declaration inside a module body |
| `E011` | `PRIVATE_MEMBER` | Accessing an `_underscore` member from outside the module |
| `E012` | `UNKNOWN_MODULE_MEMBER` | Member does not exist on the module |
| `E013` | `MODULE_NOT_IMPORTED` | Qualified access to a module that was not imported |
| `E017` | `IMPORT_SCOPE` | `import` used inside a function or block (top-level only) |
| `E018` | `IMPORT_NOT_FOUND` | Imported module file could not be found |
| `E019` | `IMPORT_READ_FAILED` | Imported module file could not be read |
| `E021` | `IMPORT_CYCLE` | Import cycle detected |
| `E022` | `SCRIPT_NOT_IMPORTABLE` | Importing a script file (no `module` declaration) |
| `E023` | `MULTIPLE_MODULES` | File contains more than one module declaration |
| `E024` | `MODULE_PATH_MISMATCH` | Module name in source does not match the file path |
| `E025` | `MODULE_SCOPE` | Module declaration is not at top level |
| `E026` | `INVALID_MODULE_ALIAS` | Alias name in `import ... as` is invalid |
| `E027` | `DUPLICATE_MODULE` | Same module found in multiple roots |
| `E028` | `INVALID_MODULE_FILE` | Module file is malformed |
| `E029` | `IMPORT_NAME_COLLISION` | Two imports resolve to the same name |
| `E044` | `CIRCULAR_DEPENDENCY` | Circular dependency between constants or definitions |

### Pattern Matching

| Code | Constant | Description |
|------|----------|-------------|
| `E014` | `EMPTY_MATCH` | `match` expression has no arms |
| `E015` | `NON_EXHAUSTIVE_MATCH` | `match` does not cover all cases |
| `E016` | `CATCHALL_NOT_LAST` | Wildcard `_` arm is not the last arm |
| `E020` | `INVALID_PATTERN` | Pattern is not valid in this context |
| `E035` | `INVALID_PATTERN_LEGACY` | Legacy pattern syntax error |
| `E075` | `DUPLICATE_PATTERN_BINDING` | Same name bound twice in one pattern |

### Either / Option

| Code | Constant | Description |
|------|----------|-------------|
| `E041` | `EITHER_CONSTRUCTOR_ERROR` | Misuse of `Left` / `Right` constructor |
| `E042` | `EITHER_VALUE_ERROR` | Invalid value inside `Left` / `Right` |
| `E053` | `EITHER_UNWRAP_ERROR_LEFT` | Unwrapping `Left` as `Right` at compile time |
| `E054` | `EITHER_UNWRAP_ERROR_RIGHT` | Unwrapping `Right` as `Left` at compile time |

### Type Errors

| Code | Constant | Description |
|------|----------|-------------|
| `E055` | `TYPE_MISMATCH` | Types are incompatible |
| `E056` | `TYPE_ERROR` | Invalid type for this operation |
| `E057` | `INCOMPATIBLE_TYPES` | Two operand types cannot be combined |

### Constant Evaluation

| Code | Constant | Description |
|------|----------|-------------|
| `E045` | `CONST_EVAL_ERROR` | Error during compile-time constant evaluation |
| `E046` | `CONST_NOT_FOUND` | Constant reference not found |
| `E047` | `CONST_NOT_PUBLIC` | Constant is private (`_` prefix) |
| `E048` | `CONST_INVALID_EXPR` | Expression cannot be evaluated at compile time |
| `E049` | `CONST_TYPE_ERROR` | Type error in constant expression |
| `E050` | `CONST_SCOPE_ERROR` | Constant used outside of valid scope |
| `E051` | `DIVISION_BY_ZERO_COMPILE` | Division by zero in constant expression |
| `E052` | `MODULO_BY_ZERO_COMPILE` | Modulo by zero in constant expression |
| `E058` | `CONST_RUNTIME_ERROR` | Runtime error during constant evaluation |
| `E059` | `CONST_DIVISION_BY_ZERO` | Division by zero in constant fold |
| `E060` | `CONST_OVERFLOW` | Integer overflow in constant expression |

### Pipe and Short-Circuit

| Code | Constant | Description |
|------|----------|-------------|
| `E039` | `PIPE_OPERATOR_ERROR` | Invalid use of pipe operator `\|>` |
| `E040` | `PIPE_TARGET_ERROR` | Pipe target is not callable |
| `E043` | `SHORT_CIRCUIT_ERROR` | Invalid use of `&&` / `\|\|` |

### Internal Compiler Errors (ICE)

These indicate a bug in the compiler, not user code:

| Code | Constant |
|------|----------|
| `E061` | `ICE_SYMBOL_SCOPE_LET` |
| `E062` | `ICE_SYMBOL_SCOPE_ASSIGN` |
| `E063` | `ICE_TEMP_SYMBOL_MATCH` |
| `E064` | `ICE_TEMP_SYMBOL_SOME_PATTERN` |
| `E065` | `ICE_SYMBOL_SCOPE_PATTERN` |
| `E066` | `ICE_TEMP_SYMBOL_SOME_BINDING` |
| `E067` | `ICE_TEMP_SYMBOL_LEFT_PATTERN` |
| `E068` | `ICE_TEMP_SYMBOL_RIGHT_PATTERN` |
| `E069` | `ICE_TEMP_SYMBOL_LEFT_BINDING` |
| `E070` | `ICE_TEMP_SYMBOL_RIGHT_BINDING` |

### Lexer / Parser

| Code | Constant | Description |
|------|----------|-------------|
| `E030` | `UNKNOWN_KEYWORD` | Unrecognized keyword (e.g. `fun` instead of `fn`) |
| `E031` | `EXPECTED_EXPRESSION` | Expected an expression, found something else |
| `E032` | `INVALID_INTEGER` | Integer literal out of range or malformed |
| `E033` | `INVALID_FLOAT` | Float literal malformed |
| `E034` | `UNEXPECTED_TOKEN` | Token not valid in this position |
| `E036` | `LAMBDA_SYNTAX_ERROR` | Lambda `\` syntax error |
| `E037` | `LAMBDA_PARAMETER_ERROR` | Invalid parameter in lambda |
| `E038` | `LAMBDA_BODY_ERROR` | Invalid lambda body |
| `E071` | `UNTERMINATED_STRING` | String literal not closed |
| `E072` | `UNTERMINATED_INTERPOLATION` | `#{` not closed |
| `E073` | `MISSING_COMMA` | Missing comma in expression list |
| `E074` | `UNTERMINATED_BLOCK_COMMENT` | `/*` not closed |
| `E076` | `UNCLOSED_DELIMITER` | `(`, `[`, or `{` not closed |
| `E077` | `LEGACY_LIST_TAIL_NONE` | Old-style list tail syntax |

---

## Runtime Errors (E1000–E1021)

| Code | Constant | Description |
|------|----------|-------------|
| `E1000` | `WRONG_NUMBER_OF_ARGUMENTS` | Function called with wrong arity |
| `E1001` | `NOT_A_FUNCTION` | Calling a non-function value |
| `E1002` | `FUNCTION_NOT_FOUND` | Named function could not be resolved |
| `E1003` | `BUILTIN_ERROR` | A builtin function returned an error |
| `E1004` | `RUNTIME_TYPE_ERROR` | Wrong type for a runtime operation |
| `E1005` | `NOT_INDEXABLE` | Indexing a value that doesn't support `[]` |
| `E1006` | `KEY_NOT_HASHABLE` | Hash map key is not a hashable type |
| `E1007` | `NOT_ITERABLE` | Iterating over a non-iterable value |
| `E1008` | `DIVISION_BY_ZERO_RUNTIME` | Division by zero at runtime |
| `E1009` | `INVALID_OPERATION` | Operation not supported for this value |
| `E1010` | `INTEGER_OVERFLOW` | Integer arithmetic overflow |
| `E1011` | `MODULO_BY_ZERO_RUNTIME` | Modulo by zero at runtime |
| `E1012` | `INDEX_OUT_OF_BOUNDS` | Array or tuple index out of bounds |
| `E1013` | `KEY_NOT_FOUND` | Key missing from hash map |
| `E1014` | `NEGATIVE_INDEX` | Negative index used |
| `E1015` | `INVALID_SLICE` | `slice(arr, lo, hi)` bounds are invalid |
| `E1016` | `MATCH_ERROR` | No match arm matched the value |
| `E1017` | `OPTION_UNWRAP_ERROR` | Unwrapping `None` as `Some` |
| `E1018` | `EITHER_UNWRAP_ERROR` | Unwrapping the wrong `Left`/`Right` variant |
| `E1019` | `STRING_INDEX_ERROR` | String character index out of range |
| `E1020` | `STRING_ENCODING_ERROR` | Invalid UTF-8 in string operation |
| `E1021` | `INVALID_SUBSTRING` | `substring` bounds are invalid |

---

## Adding a New Error Code

1. Define the constant in `compiler_errors.rs` or `runtime_errors.rs`:
   ```rust
   pub const MY_ERROR: ErrorCode = ErrorCode {
       code: "E078",
       error_type: ErrorType::Compiler,
   };
   ```

2. Register it in `registry.rs` with a description and hint template.

3. Use `diag_enhanced(&MY_ERROR)` to create the diagnostic, then chain `with_*` builder methods:
   ```rust
   use crate::diagnostics::{Diagnostic, DiagnosticBuilder};
   use crate::diagnostics::compiler_errors::MY_ERROR;

   diag_enhanced(&MY_ERROR)
       .with_span(span)
       .with_message("what went wrong")
       .with_hint("how to fix it")
   ```
