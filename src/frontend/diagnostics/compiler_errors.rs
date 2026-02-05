use super::builders::DiagnosticBuilder;
use super::types::{ErrorCode, ErrorType};

pub const DUPLICATE_NAME: ErrorCode = ErrorCode {
    code: "E001",
    title: "DUPLICATE NAME",
    error_type: ErrorType::Compiler,
    message: "Duplicate binding: `{}` is already defined.",
    hint: Some("Use a different name or remove the previous definition."),
};

pub const IMMUTABLE_BINDING: ErrorCode = ErrorCode {
    code: "E002",
    title: "IMMUTABLE BINDING",
    error_type: ErrorType::Compiler,
    message: "Cannot reassign to immutable variable `{}`.",
    hint: Some(
        "In Flux, variables are immutable by default for safety. To allow reassignment, declare with `let mut {}` instead. Alternatively, create a new binding with `let` to shadow the previous value.",
    ),
};

pub const OUTER_ASSIGNMENT: ErrorCode = ErrorCode {
    code: "E003",
    title: "OUTER ASSIGNMENT",
    error_type: ErrorType::Compiler,
    message: "Cannot assign to variable `{}` from outer scope.",
    hint: Some(
        "Variables captured by closures cannot be reassigned. Options: 1) Create a new local binding with `let {}`, 2) Make the outer variable mutable with `let mut`, or 3) Return the new value from the function instead.",
    ),
};

pub const UNDEFINED_VARIABLE: ErrorCode = ErrorCode {
    code: "E004",
    title: "UNDEFINED VARIABLE",
    error_type: ErrorType::Compiler,
    message: "I can't find a value named `{}`.",
    hint: Some("Define it first: let {} = ...;"),
};

pub const UNKNOWN_PREFIX_OPERATOR: ErrorCode = ErrorCode {
    code: "E005",
    title: "UNKNOWN PREFIX OPERATOR",
    error_type: ErrorType::Compiler,
    message: "Unknown prefix operator: `{}`.",
    hint: Some("Valid prefix operators: !, -"),
};

pub const UNKNOWN_INFIX_OPERATOR: ErrorCode = ErrorCode {
    code: "E006",
    title: "UNKNOWN INFIX OPERATOR",
    error_type: ErrorType::Compiler,
    message: "Unknown infix operator: `{}`.",
    hint: Some("Valid infix operators: +, -, *, /, %, ==, !=, <, >, <=, >=, &&, ||, |>"),
};

pub const DUPLICATE_PARAMETER: ErrorCode = ErrorCode {
    code: "E007",
    title: "DUPLICATE PARAMETER",
    error_type: ErrorType::Compiler,
    message: "Duplicate parameter name: `{}`.",
    hint: Some("Each parameter must have a unique name."),
};

pub const INVALID_MODULE_NAME: ErrorCode = ErrorCode {
    code: "E008",
    title: "INVALID MODULE NAME",
    error_type: ErrorType::Compiler,
    message: "Invalid module name: `{}`.",
    hint: Some(
        "Module names must start with an uppercase letter and contain only alphanumeric characters and dots.",
    ),
};

pub const MODULE_NAME_CLASH: ErrorCode = ErrorCode {
    code: "E009",
    title: "MODULE NAME CLASH",
    error_type: ErrorType::Compiler,
    message: "Module name `{}` conflicts with existing binding.",
    hint: Some("Choose a different module name or rename the conflicting binding."),
};

pub const INVALID_MODULE_CONTENT: ErrorCode = ErrorCode {
    code: "E010",
    title: "INVALID MODULE CONTENT",
    error_type: ErrorType::Compiler,
    message: "Invalid content in module `{}`: {}.",
    hint: Some("Modules can only contain function definitions and constant declarations."),
};

pub const PRIVATE_MEMBER: ErrorCode = ErrorCode {
    code: "E011",
    title: "PRIVATE MEMBER",
    error_type: ErrorType::Compiler,
    message: "Cannot access private member `{}`.",
    hint: Some(
        "Private members can only be accessed within the same module. Use `pub` to make it public.",
    ),
};

pub const UNKNOWN_MODULE_MEMBER: ErrorCode = ErrorCode {
    code: "E012",
    title: "UNKNOWN MODULE MEMBER",
    error_type: ErrorType::Compiler,
    message: "Module `{}` has no member named `{}`.",
    hint: Some("Check the module's public members or import the correct module."),
};

pub const MODULE_NOT_IMPORTED: ErrorCode = ErrorCode {
    code: "E013",
    title: "MODULE NOT IMPORTED",
    error_type: ErrorType::Compiler,
    message: "Module `{}` is not imported.",
    hint: Some(
        "Add an import statement at the top of your file: `import {}`. You can also use an alias: `import {} as ShorterName`. Remember: imports must be at the top, before any other code.",
    ),
};

pub const EMPTY_MATCH: ErrorCode = ErrorCode {
    code: "E014",
    title: "EMPTY MATCH",
    error_type: ErrorType::Compiler,
    message: "Match expression must have at least one arm.",
    hint: Some("Add at least one pattern match arm: match value { pattern -> expr; }"),
};

pub const NON_EXHAUSTIVE_MATCH: ErrorCode = ErrorCode {
    code: "E015",
    title: "NON-EXHAUSTIVE MATCH",
    error_type: ErrorType::Compiler,
    message: "Match expressions must end with a `_` or identifier arm.",
    hint: Some("Add a catch-all pattern: _ -> default_value"),
};

pub const CATCHALL_NOT_LAST: ErrorCode = ErrorCode {
    code: "E016",
    title: "INVALID PATTERN",
    error_type: ErrorType::Compiler,
    message: "Catch-all patterns must be the final match arm.",
    hint: Some("Move `_` or the binding pattern to the last arm."),
};

pub const IMPORT_SCOPE: ErrorCode = ErrorCode {
    code: "E017",
    title: "IMPORT SCOPE",
    error_type: ErrorType::Compiler,
    message: "Import statements must be at the top of the file.",
    hint: Some("Move all import statements before any other declarations."),
};

pub const IMPORT_NOT_FOUND: ErrorCode = ErrorCode {
    code: "E018",
    title: "IMPORT NOT FOUND",
    error_type: ErrorType::Compiler,
    message: "Cannot find module `{}` to import.",
    hint: Some(
        "Check that: 1) The module file exists (e.g., `{}.flx`), 2) The file is in a module root directory (current dir or ./src by default), 3) The module path matches the file structure (e.g., `Foo.Bar` → `Foo/Bar.flx`). Use `--root` flag to add more search paths.",
    ),
};

pub const IMPORT_READ_FAILED: ErrorCode = ErrorCode {
    code: "E019",
    title: "IMPORT READ FAILED",
    error_type: ErrorType::Compiler,
    message: "Failed to read module file `{}`: {}.",
    hint: Some("Check file permissions and that the file is valid UTF-8."),
};

pub const INVALID_PATTERN: ErrorCode = ErrorCode {
    code: "E020",
    title: "INVALID PATTERN",
    error_type: ErrorType::Compiler,
    message: "Invalid pattern in match expression: {}.",
    hint: Some("Valid patterns: literals, identifiers, None, Some(x), Left(x), Right(x), _"),
};

pub const IMPORT_CYCLE: ErrorCode = ErrorCode {
    code: "E021",
    title: "IMPORT CYCLE",
    error_type: ErrorType::Compiler,
    message: "Circular import detected: {}.",
    hint: Some("Reorganize your modules to break the circular dependency."),
};

pub const SCRIPT_NOT_IMPORTABLE: ErrorCode = ErrorCode {
    code: "E022",
    title: "SCRIPT NOT IMPORTABLE",
    error_type: ErrorType::Compiler,
    message: "Cannot import from script file `{}` (scripts cannot be imported).",
    hint: Some("Only module files can be imported. Convert the script to a module."),
};

pub const MULTIPLE_MODULES: ErrorCode = ErrorCode {
    code: "E023",
    title: "MULTIPLE MODULES",
    error_type: ErrorType::Compiler,
    message: "File contains multiple module declarations.",
    hint: Some("Each file should contain only one module declaration."),
};

pub const MODULE_PATH_MISMATCH: ErrorCode = ErrorCode {
    code: "E024",
    title: "MODULE PATH MISMATCH",
    error_type: ErrorType::Compiler,
    message: "Module name `{}` doesn't match file path `{}`.",
    hint: Some("Rename the module or move the file to match."),
};

pub const MODULE_SCOPE: ErrorCode = ErrorCode {
    code: "E025",
    title: "MODULE SCOPE",
    error_type: ErrorType::Compiler,
    message: "Module declaration must be at the top of the file.",
    hint: Some("Move the module declaration before all other statements."),
};

pub const INVALID_MODULE_ALIAS: ErrorCode = ErrorCode {
    code: "E026",
    title: "INVALID MODULE ALIAS",
    error_type: ErrorType::Compiler,
    message: "Invalid module alias: `{}`.",
    hint: Some("Module aliases must start with an uppercase letter."),
};

pub const DUPLICATE_MODULE: ErrorCode = ErrorCode {
    code: "E027",
    title: "DUPLICATE MODULE",
    error_type: ErrorType::Compiler,
    message: "Duplicate module declaration: `{}`.",
    hint: Some("Remove one of the duplicate module declarations."),
};

pub const INVALID_MODULE_FILE: ErrorCode = ErrorCode {
    code: "E028",
    title: "INVALID MODULE FILE",
    error_type: ErrorType::Compiler,
    message: "Invalid module file: {}.",
    hint: Some("Module files must have .flx extension and valid Flux code."),
};

pub const IMPORT_NAME_COLLISION: ErrorCode = ErrorCode {
    code: "E029",
    title: "IMPORT NAME COLLISION",
    error_type: ErrorType::Compiler,
    message: "Import name `{}` collides with existing binding.",
    hint: Some("Use an alias: import Module as Alias"),
};

// Syntax Errors (E100-E199)
pub const UNKNOWN_KEYWORD: ErrorCode = ErrorCode {
    code: "E030",
    title: "UNKNOWN KEYWORD",
    error_type: ErrorType::Compiler,
    message: "Unknown keyword: `{}`.",
    hint: Some(
        "Flux keywords are: let, fun, if, else, match, import, module, return, true, false, None. Common mistakes: use `fun` (not `function` or `def`), use `let` (not `var` or `const`). Check for typos in your keyword.",
    ),
};

pub const EXPECTED_EXPRESSION: ErrorCode = ErrorCode {
    code: "E031",
    title: "EXPECTED EXPRESSION",
    error_type: ErrorType::Compiler,
    message: "Expected expression, found {}.",
    hint: None,
};

pub const INVALID_INTEGER: ErrorCode = ErrorCode {
    code: "E032",
    title: "INVALID INTEGER",
    error_type: ErrorType::Compiler,
    message: "Invalid integer literal: {}.",
    hint: Some("Integer literals must be valid numbers without leading zeros."),
};

pub const INVALID_FLOAT: ErrorCode = ErrorCode {
    code: "E033",
    title: "INVALID FLOAT",
    error_type: ErrorType::Compiler,
    message: "Invalid float literal: {}.",
    hint: Some("Float literals must have digits before and after the decimal point."),
};

pub const UNEXPECTED_TOKEN: ErrorCode = ErrorCode {
    code: "E034",
    title: "UNEXPECTED TOKEN",
    error_type: ErrorType::Compiler,
    message: "Unexpected token: {} (expected {}).",
    hint: Some(
        "Common causes: missing semicolon, unclosed parenthesis/bracket, or misplaced operator. Check the line above for syntax errors.",
    ),
};

pub const INVALID_PATTERN_LEGACY: ErrorCode = ErrorCode {
    code: "E035",
    title: "INVALID PATTERN",
    error_type: ErrorType::Compiler,
    message: "Invalid pattern: {}.",
    hint: None,
};

pub const LAMBDA_SYNTAX_ERROR: ErrorCode = ErrorCode {
    code: "E036",
    title: "LAMBDA SYNTAX ERROR",
    error_type: ErrorType::Compiler,
    message: "Invalid lambda syntax: {}.",
    hint: Some("Use: \\x -> expr or \\(x, y) -> expr"),
};

pub const LAMBDA_PARAMETER_ERROR: ErrorCode = ErrorCode {
    code: "E037",
    title: "LAMBDA PARAMETER ERROR",
    error_type: ErrorType::Compiler,
    message: "Invalid lambda parameter: {}.",
    hint: Some("Lambda parameters must be identifiers."),
};

pub const LAMBDA_BODY_ERROR: ErrorCode = ErrorCode {
    code: "E038",
    title: "LAMBDA BODY ERROR",
    error_type: ErrorType::Compiler,
    message: "Lambda must have an expression body.",
    hint: Some("Lambda syntax: \\params -> expression"),
};

pub const PIPE_OPERATOR_ERROR: ErrorCode = ErrorCode {
    code: "E039",
    title: "PIPE OPERATOR ERROR",
    error_type: ErrorType::Compiler,
    message: "Invalid pipe expression: {}.",
    hint: Some("Pipe operator requires: value |> function"),
};

pub const PIPE_TARGET_ERROR: ErrorCode = ErrorCode {
    code: "E040",
    title: "PIPE TARGET ERROR",
    error_type: ErrorType::Compiler,
    message: "Pipe target must be a function call.",
    hint: Some("Use: value |> func or value |> func(arg)"),
};

pub const EITHER_CONSTRUCTOR_ERROR: ErrorCode = ErrorCode {
    code: "E041",
    title: "EITHER CONSTRUCTOR ERROR",
    error_type: ErrorType::Compiler,
    message: "Either requires Left or Right constructor.",
    hint: Some("Use: Left(value) or Right(value)"),
};

pub const EITHER_VALUE_ERROR: ErrorCode = ErrorCode {
    code: "E042",
    title: "EITHER VALUE ERROR",
    error_type: ErrorType::Compiler,
    message: "Either constructor requires exactly one argument.",
    hint: Some("Use: Left(value) or Right(value), not Left() or Left(a, b)"),
};

pub const SHORT_CIRCUIT_ERROR: ErrorCode = ErrorCode {
    code: "E043",
    title: "SHORT-CIRCUIT EVALUATION ERROR",
    error_type: ErrorType::Compiler,
    message: "Invalid short-circuit expression: {}.",
    hint: Some("Use && for logical AND, || for logical OR"),
};

pub const CIRCULAR_DEPENDENCY: ErrorCode = ErrorCode {
    code: "E044",
    title: "CIRCULAR DEPENDENCY",
    error_type: ErrorType::Compiler,
    message: "Circular dependency in module constants: {}.",
    hint: Some(
        "Break the cycle by using a literal value or moving one constant to a different module.",
    ),
};

pub const CONST_EVAL_ERROR: ErrorCode = ErrorCode {
    code: "E045",
    title: "CONSTANT EVALUATION ERROR",
    error_type: ErrorType::Compiler,
    message: "Cannot evaluate constant at compile time: {}.",
    hint: Some(
        "Constants must be evaluable at compile time using only literals and other constants.",
    ),
};

pub const CONST_NOT_FOUND: ErrorCode = ErrorCode {
    code: "E046",
    title: "CONSTANT NOT FOUND",
    error_type: ErrorType::Compiler,
    message: "Constant `{}` not found in module `{}`.",
    hint: Some("Check that the constant is defined and public."),
};

pub const CONST_NOT_PUBLIC: ErrorCode = ErrorCode {
    code: "E047",
    title: "CONSTANT NOT PUBLIC",
    error_type: ErrorType::Compiler,
    message: "Constant `{}` is private in module `{}`.",
    hint: Some("Use `pub const` to make it accessible from other modules."),
};

pub const CONST_INVALID_EXPR: ErrorCode = ErrorCode {
    code: "E048",
    title: "INVALID CONSTANT EXPRESSION",
    error_type: ErrorType::Compiler,
    message: "Invalid expression in constant declaration: {}.",
    hint: Some("Constants can only use literals, arithmetic, and other constants."),
};

pub const CONST_TYPE_ERROR: ErrorCode = ErrorCode {
    code: "E049",
    title: "CONSTANT TYPE ERROR",
    error_type: ErrorType::Compiler,
    message: "Type error in constant evaluation: {}.",
    hint: None,
};

pub const CONST_SCOPE_ERROR: ErrorCode = ErrorCode {
    code: "E050",
    title: "CONSTANT SCOPE ERROR",
    error_type: ErrorType::Compiler,
    message: "Constants can only be declared at module level.",
    hint: Some("Move the constant declaration to the top level of the module."),
};

pub const DIVISION_BY_ZERO_COMPILE: ErrorCode = ErrorCode {
    code: "E051",
    title: "DIVISION BY ZERO",
    error_type: ErrorType::Compiler,
    message: "Division by zero detected at compile time.",
    hint: Some("Check divisor is non-zero before division."),
};

pub const MODULO_BY_ZERO_COMPILE: ErrorCode = ErrorCode {
    code: "E052",
    title: "MODULO BY ZERO",
    error_type: ErrorType::Compiler,
    message: "Modulo by zero detected at compile time.",
    hint: Some("Check divisor is non-zero before modulo operation."),
};

pub const EITHER_UNWRAP_ERROR_LEFT: ErrorCode = ErrorCode {
    code: "E053",
    title: "EITHER UNWRAP ERROR",
    error_type: ErrorType::Compiler,
    message: "Cannot unwrap Left value as Right.",
    hint: Some("Use pattern matching to handle both Left and Right cases."),
};

pub const EITHER_UNWRAP_ERROR_RIGHT: ErrorCode = ErrorCode {
    code: "E054",
    title: "EITHER UNWRAP ERROR",
    error_type: ErrorType::Compiler,
    message: "Cannot unwrap Right value as Left.",
    hint: Some("Use pattern matching to handle both Left and Right cases."),
};

pub const TYPE_MISMATCH: ErrorCode = ErrorCode {
    code: "E055",
    title: "TYPE MISMATCH",
    error_type: ErrorType::Compiler,
    message: "Expected {}, got {}.",
    hint: None,
};

pub const TYPE_ERROR: ErrorCode = ErrorCode {
    code: "E056",
    title: "TYPE ERROR",
    error_type: ErrorType::Compiler,
    message: "Type error: {}.",
    hint: None,
};

pub const INCOMPATIBLE_TYPES: ErrorCode = ErrorCode {
    code: "E057",
    title: "INCOMPATIBLE TYPES",
    error_type: ErrorType::Compiler,
    message: "Cannot {} {} and {} values.",
    hint: None,
};

pub const CONST_RUNTIME_ERROR: ErrorCode = ErrorCode {
    code: "E058",
    title: "CONSTANT RUNTIME ERROR",
    error_type: ErrorType::Compiler,
    message: "Runtime error while evaluating constant: {}.",
    hint: None,
};

pub const CONST_DIVISION_BY_ZERO: ErrorCode = ErrorCode {
    code: "E059",
    title: "CONSTANT DIVISION BY ZERO",
    error_type: ErrorType::Compiler,
    message: "Division by zero in constant evaluation.",
    hint: Some("Ensure all constant expressions have non-zero divisors."),
};

pub const CONST_OVERFLOW: ErrorCode = ErrorCode {
    code: "E060",
    title: "CONSTANT OVERFLOW",
    error_type: ErrorType::Compiler,
    message: "Integer overflow in constant evaluation.",
    hint: Some("Use smaller numbers or break the computation into parts."),
};

// ============================================================================
// INTERNAL COMPILER ERRORS (E061-E070)
// ============================================================================

pub const ICE_SYMBOL_SCOPE_LET: ErrorCode = ErrorCode {
    code: "E061",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected symbol scope for let binding.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_SYMBOL_SCOPE_ASSIGN: ErrorCode = ErrorCode {
    code: "E062",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected symbol scope for assignment.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_MATCH: ErrorCode = ErrorCode {
    code: "E063",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in match scrutinee.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_SOME_PATTERN: ErrorCode = ErrorCode {
    code: "E064",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Some pattern.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_SYMBOL_SCOPE_PATTERN: ErrorCode = ErrorCode {
    code: "E065",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected symbol scope for pattern binding.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_SOME_BINDING: ErrorCode = ErrorCode {
    code: "E066",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Some binding.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_LEFT_PATTERN: ErrorCode = ErrorCode {
    code: "E067",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Left pattern.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_RIGHT_PATTERN: ErrorCode = ErrorCode {
    code: "E068",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Right pattern.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_LEFT_BINDING: ErrorCode = ErrorCode {
    code: "E069",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Left binding.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const ICE_TEMP_SYMBOL_RIGHT_BINDING: ErrorCode = ErrorCode {
    code: "E070",
    title: "INTERNAL COMPILER ERROR",
    error_type: ErrorType::Compiler,
    message: "Unexpected temp symbol scope in Right binding.",
    hint: Some(
        "This is a compiler bug. Please report at: https://github.com/flux-lang/flux/issues",
    ),
};

pub const UNTERMINATED_STRING: ErrorCode = ErrorCode {
    code: "E071",
    title: "UNTERMINATED STRING",
    error_type: ErrorType::Compiler,
    message: "String literal is missing closing quote.",
    hint: Some("Add a closing \" at the end of the string."),
};

pub const UNTERMINATED_INTERPOLATION: ErrorCode = ErrorCode {
    code: "E072",
    title: "UNTERMINATED INTERPOLATION",
    error_type: ErrorType::Compiler,
    message: "Expected string continuation or end after interpolation.",
    hint: Some(
        "String interpolation must be followed by more string content or the closing quote.",
    ),
};

pub const MISSING_COMMA: ErrorCode = ErrorCode {
    code: "E073",
    title: "MISSING COMMA",
    error_type: ErrorType::Compiler,
    message: "Missing comma between {}.",
    hint: Some("Insert a comma between adjacent items, e.g. `a, b`."),
};

// ============================================================================
// Error Constructor Functions
// ============================================================================
// These functions provide a clean API for creating diagnostics with proper
// error codes. Use these instead of Diagnostic::error() in production code.

use super::diagnostic::Diagnostic;
use super::registry::diag_enhanced;
use crate::frontend::position::Span;

// Parser Errors

/// Create an "unknown keyword" error for unrecognized keywords
pub fn unknown_keyword(span: Span, keyword: &str, suggestion: Option<(&str, &str)>) -> Diagnostic {
    let mut diag = diag_enhanced(&UNKNOWN_KEYWORD)
        .with_span(span)
        .with_message(format!("Unknown keyword: `{}`.", keyword));

    if let Some((correct_keyword, description)) = suggestion {
        diag = diag.with_suggestion_message(span, correct_keyword, description);
    }

    diag
}

/// Create an "unexpected token" error
pub fn unexpected_token(span: Span, message: impl Into<String>) -> Diagnostic {
    diag_enhanced(&UNEXPECTED_TOKEN)
        .with_span(span)
        .with_message(message.into())
}

/// Create an "invalid integer" error
pub fn invalid_integer(span: Span, literal: &str) -> Diagnostic {
    diag_enhanced(&INVALID_INTEGER)
        .with_span(span)
        .with_message(format!("Could not parse `{}` as an integer.", literal))
}

/// Create an "invalid float" error
pub fn invalid_float(span: Span, literal: &str) -> Diagnostic {
    diag_enhanced(&INVALID_FLOAT)
        .with_span(span)
        .with_message(format!("Could not parse `{}` as a float.", literal))
}

/// Create a "pipe target error"
pub fn pipe_target_error(span: Span) -> Diagnostic {
    diag_enhanced(&PIPE_TARGET_ERROR)
        .with_span(span)
        .with_message("Pipe operator expects a function or function call.")
        .with_hint_text("Use `value |> func` or `value |> func(arg)`")
}

/// Create an "invalid pattern" error
pub fn invalid_pattern(span: Span, found: &str) -> Diagnostic {
    diag_enhanced(&INVALID_PATTERN)
        .with_span(span)
        .with_message(format!("Expected a pattern, found `{}`.", found))
}

/// Create a "lambda syntax error"
pub fn lambda_syntax_error(span: Span, message: impl Into<String>) -> Diagnostic {
    diag_enhanced(&LAMBDA_SYNTAX_ERROR)
        .with_span(span)
        .with_message(message.into())
        .with_hint_text("Use `\\x -> expr` or `\\(x, y) -> expr`.")
}

/// Create an "unterminated interpolation" error
pub fn unterminated_interpolation(span: Span) -> Diagnostic {
    diag_enhanced(&UNTERMINATED_INTERPOLATION)
        .with_span(span)
        .with_message("Expected string continuation or end after interpolation.")
}

/// Create a "missing comma" error for adjacent list items/arguments
pub fn missing_comma(span: Span, context: &str, example: &str) -> Diagnostic {
    diag_enhanced(&MISSING_COMMA)
        .with_span(span)
        .with_message(format!("Missing comma between {}.", context))
        .with_hint_text(format!("Add a comma between items, e.g. {}.", example))
}
