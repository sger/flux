use std::rc::Rc;

use super::builders::DiagnosticBuilder;
use super::quality::{
    TypeMismatchNotes, missing_construct_opener_diagnostic, missing_syntax_token_diagnostic,
    occurs_check_diagnostic, type_mismatch_diagnostic,
};
use super::types::{DiagnosticCategory, ErrorCode, ErrorType};

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
        "In Flux, bindings are immutable. Compute a new value with a new binding name (for example: `let next_{} = ...`).",
    ),
};

pub const OUTER_ASSIGNMENT: ErrorCode = ErrorCode {
    code: "E003",
    title: "OUTER ASSIGNMENT",
    error_type: ErrorType::Compiler,
    message: "Cannot assign to variable `{}` from outer scope.",
    hint: Some(
        "Variables captured by closures cannot be reassigned. Return the updated value from the function or bind it to a new name.",
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
        "Private members can only be accessed within the same module. Use `public fn` to export a function.",
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
        "Flux keywords are: let, fn, if, else, match, import, module, return, true, false, None. Common mistakes: use `fn` (not `function` or `def`), use `let` (not `var` or `const`). Check for typos in your keyword.",
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
    title: "WRONG NUMBER OF ARGUMENTS",
    error_type: ErrorType::Compiler,
    message: "Expected {} arguments, got {}.",
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

pub const UNTERMINATED_BLOCK_COMMENT: ErrorCode = ErrorCode {
    code: "E074",
    title: "UNTERMINATED BLOCK COMMENT",
    error_type: ErrorType::Compiler,
    message: "Block comment is missing closing */.",
    hint: Some("Add a closing */ to end the comment."),
};

pub const MISSING_COMMA: ErrorCode = ErrorCode {
    code: "E073",
    title: "MISSING COMMA",
    error_type: ErrorType::Compiler,
    message: "Missing comma between {}.",
    hint: Some("Insert a comma between adjacent items, e.g. `a, b`."),
};

pub const DUPLICATE_PATTERN_BINDING: ErrorCode = ErrorCode {
    code: "E075",
    title: "DUPLICATE PATTERN BINDING",
    error_type: ErrorType::Compiler,
    message: "Pattern binds `{}` more than once.",
    hint: Some("Use unique binding names within a single pattern."),
};

pub const UNCLOSED_DELIMITER: ErrorCode = ErrorCode {
    code: "E076",
    title: "UNCLOSED DELIMITER",
    error_type: ErrorType::Compiler,
    message: "Expected a closing delimiter to match the opening one.",
    hint: Some("Add the missing closing delimiter."),
};

pub const LEGACY_LIST_TAIL_NONE: ErrorCode = ErrorCode {
    code: "E077",
    title: "LEGACY LIST TAIL",
    error_type: ErrorType::Compiler,
    message: "Use `[]` as the empty list tail instead of `None`.",
    hint: Some("Replace `None` with `[]` in cons expressions, for example: `[1 | []]`."),
};

pub const BASE_ALIAS_FORBIDDEN: ErrorCode = ErrorCode {
    code: "E078",
    title: "INVALID BASE DIRECTIVE",
    error_type: ErrorType::Compiler,
    message: "`import Base as {}` is not allowed.",
    hint: Some("Use `import Base` or `import Base except [...]`."),
};

pub const DUPLICATE_BASE_EXCLUSION: ErrorCode = ErrorCode {
    code: "E079",
    title: "INVALID BASE DIRECTIVE",
    error_type: ErrorType::Compiler,
    message: "Duplicate Base exclusion `{}`.",
    hint: Some("Each name in `import Base except [...]` must appear only once."),
};

pub const UNKNOWN_BASE_MEMBER: ErrorCode = ErrorCode {
    code: "E080",
    title: "UNKNOWN BASE MEMBER",
    error_type: ErrorType::Compiler,
    message: "Base has no member named `{}`.",
    hint: Some("Check the Base surface or remove this name from `except`."),
};

// ============================================================================
// ADT ERRORS (E081-E084)
// ============================================================================

pub const UNKNOWN_CONSTRUCTOR: ErrorCode = ErrorCode {
    code: "E081",
    title: "UNKNOWN CONSTRUCTOR",
    error_type: ErrorType::Compiler,
    message: "Unknown constructor `{}`.",
    hint: Some("Check that the constructor is defined in a `data` declaration in scope."),
};

pub const CONSTRUCTOR_ARITY_MISMATCH: ErrorCode = ErrorCode {
    code: "E082",
    title: "CONSTRUCTOR ARITY MISMATCH",
    error_type: ErrorType::Compiler,
    message: "Constructor `{}` expects {} argument(s) but got {}.",
    hint: Some("Check the `data` declaration for the correct number of fields."),
};

pub const ADT_NON_EXHAUSTIVE_MATCH: ErrorCode = ErrorCode {
    code: "E083",
    title: "NON-EXHAUSTIVE ADT MATCH",
    error_type: ErrorType::Compiler,
    message: "Match expression on `{}` does not cover all constructors.",
    hint: Some("Add the missing constructors or a wildcard `_` catch-all arm."),
};

pub const MODULE_ADT_CONSTRUCTOR_NOT_EXPORTED: ErrorCode = ErrorCode {
    code: "E084",
    title: "MODULE ADT CONSTRUCTOR NOT EXPORTED",
    error_type: ErrorType::Compiler,
    message: "Constructor `{}` from module `{}` is not part of the public API.",
    hint: Some(
        "Use the module's `public fn` factory/accessor API instead of direct constructor access.",
    ),
};

pub const CONSTRUCTOR_PATTERN_ARITY_MISMATCH: ErrorCode = ErrorCode {
    code: "E085",
    title: "CONSTRUCTOR PATTERN ARITY MISMATCH",
    error_type: ErrorType::Compiler,
    message: "Constructor pattern `{}` expects {} argument(s) but got {}.",
    hint: Some("Check the constructor's declared pattern field count."),
};

pub const CROSS_MODULE_CONSTRUCTOR_ACCESS: ErrorCode = ErrorCode {
    code: "E086",
    title: "CROSS-MODULE CONSTRUCTOR ACCESS",
    error_type: ErrorType::Compiler,
    message: "Direct constructor access `{}` from module `{}` is not allowed in strict mode.",
    hint: Some("Use the module's public factory/accessor functions instead."),
};

pub const CROSS_MODULE_CONSTRUCTOR_ACCESS_WARNING: ErrorCode = ErrorCode {
    code: "W201",
    title: "CROSS-MODULE CONSTRUCTOR ACCESS",
    error_type: ErrorType::Compiler,
    message: "Direct constructor access `{}` from module `{}` bypasses module API boundaries.",
    hint: Some("Prefer module `public fn` factory/accessor API for cross-module usage."),
};

pub const UNREACHABLE_PATTERN_ARM: ErrorCode = ErrorCode {
    code: "W202",
    title: "UNREACHABLE PATTERN ARM",
    error_type: ErrorType::Compiler,
    message: "This arm is unreachable because an earlier unguarded arm already covers this pattern.",
    hint: Some("Remove or reorder this arm."),
};

pub const UNKNOWN_FUNCTION_EFFECT: ErrorCode = ErrorCode {
    code: "E407",
    title: "UNKNOWN FUNCTION EFFECT",
    error_type: ErrorType::Compiler,
    message: "Function effect annotation references unknown effect `{}`.",
    hint: Some("Use a declared effect name in `with ...` or declare the effect first."),
};

// ============================================================================
// Type Inference Errors (E300–E399)
// ============================================================================

pub const TYPE_UNIFICATION_ERROR: ErrorCode = ErrorCode {
    code: "E300",
    title: "TYPE UNIFICATION ERROR",
    error_type: ErrorType::Compiler,
    message: "Cannot unify {} with {}.",
    hint: None,
};

pub const OCCURS_CHECK_FAILURE: ErrorCode = ErrorCode {
    code: "E301",
    title: "OCCURS CHECK FAILURE",
    error_type: ErrorType::Compiler,
    message: "Infinite type: type variable {} occurs in {}.",
    hint: Some(
        "A type cannot contain itself. This usually indicates a recursive type without a data wrapper.",
    ),
};

pub const UNDEFINED_TYPE_VAR: ErrorCode = ErrorCode {
    code: "E302",
    title: "UNDEFINED TYPE VARIABLE",
    error_type: ErrorType::Compiler,
    message: "Undefined type variable `{}`.",
    hint: Some("Declare the type variable in the function's generic parameter list: fn f<T>(...)"),
};

// ============================================================================
// Error Constructor Functions
// ============================================================================
// These functions provide a clean API for creating diagnostics with proper
// error codes. Use these instead of Diagnostic::error() in production code.

use super::diagnostic::Diagnostic;
use super::registry::diagnostic_for;
use super::types::{Label, RelatedDiagnostic};
use crate::diagnostics::position::Span;

// Parser Errors

/// Create an "unknown keyword" error for unrecognized keywords
pub fn unknown_keyword(span: Span, keyword: &str, suggestion: Option<(&str, &str)>) -> Diagnostic {
    let mut diag = diagnostic_for(&UNKNOWN_KEYWORD)
        .with_category(DiagnosticCategory::ParserKeyword)
        .with_span(span)
        .with_message(format!("Unknown keyword: `{}`.", keyword));

    if let Some((correct_keyword, description)) = suggestion {
        diag = diag.with_suggestion_message(span, correct_keyword, description);
    }

    diag
}

/// Create an alias-style unknown keyword diagnostic (E030).
pub fn unknown_keyword_alias(
    span: Span,
    found: &str,
    replacement: &str,
    context: &str,
) -> Diagnostic {
    unknown_keyword(span, found, None)
        .with_message(format!(
            "Unknown keyword `{found}`. Flux uses `{replacement}` for {context}."
        ))
        .with_hint_text(format!("Did you mean `{replacement}`?"))
}

/// Create an "unexpected token" error
pub fn unexpected_token(span: Span, message: impl Into<String>) -> Diagnostic {
    diagnostic_for(&UNEXPECTED_TOKEN)
        .with_span(span)
        .with_message(message.into())
}

/// Create an "unexpected token" error with explicit parser-facing metadata.
pub fn unexpected_token_with_details(
    span: Span,
    display_title: impl Into<String>,
    category: DiagnosticCategory,
    message: impl Into<String>,
) -> Diagnostic {
    unexpected_token(span, message)
        .with_display_title(display_title.into())
        .with_category(category)
}

/// Create a missing-if-body-brace diagnostic (E034).
pub fn missing_if_body_brace(span: Span) -> Diagnostic {
    missing_construct_opener_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing If Body",
        DiagnosticCategory::ParserDeclaration,
        "This `if` branch needs to start with `{`.",
        "This looks like the `if` body",
        "Try adding `{` after the `if` condition.",
    )
}

/// Create a missing-else-body-brace diagnostic (E034).
pub fn missing_else_body_brace(span: Span) -> Diagnostic {
    missing_construct_opener_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Else Body",
        DiagnosticCategory::ParserDeclaration,
        "This `else` branch needs to start with `{`.",
        "This looks like the `else` body",
        "Try adding `{` after `else`.",
    )
}

/// Create a missing-do-block-brace diagnostic (E034).
pub fn missing_do_block_brace(span: Span) -> Diagnostic {
    missing_construct_opener_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Do Block",
        DiagnosticCategory::ParserDeclaration,
        "This `do` block needs to start with `{`.",
        "This looks like the `do` block body",
        "Try adding `{` after `do`.",
    )
}

/// Create a missing-let-assignment diagnostic (E034).
pub fn missing_let_assign(span: Span, name: &str) -> Diagnostic {
    unexpected_token(
        span,
        format!("Expected `=` after `let {name}`. Did you mean `let {name} = ...`?"),
    )
    .with_category(DiagnosticCategory::ParserDeclaration)
    .with_hint_text("Let bindings require `=`: `let name = value`")
}

/// Create a missing-function-parameter-list diagnostic (E034).
pub fn missing_fn_param_list(span: Span, fn_name: &str) -> Diagnostic {
    missing_syntax_token_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Function Parameter List",
        DiagnosticCategory::ParserDeclaration,
        format!("This function declaration needs a parameter list after `{fn_name}`."),
        format!("Try `fn {fn_name}()` or `fn {fn_name}(x: Type)`."),
    )
}

/// Create a match-arm `|` separator diagnostic (E034).
pub fn match_pipe_separator(span: Span) -> Diagnostic {
    unexpected_token(span, "Match arms are separated by `,` in Flux, not `|`.")
        .with_display_title("Invalid Match Arm Separator")
        .with_category(DiagnosticCategory::ParserSeparator)
        .with_hint_text("Replace `|` with `,`.")
}

/// Create a match-arm `=>` arrow diagnostic (E034).
pub fn match_fat_arrow(span: Span) -> Diagnostic {
    missing_syntax_token_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Match Arm Arrow",
        DiagnosticCategory::ParserSeparator,
        "This match arm needs `->`, not `=>`.",
        "Replace `=>` with `->`.",
    )
}

/// Create a missing-match-arrow diagnostic (E034).
pub fn missing_match_arrow(span: Span, found: &str) -> Diagnostic {
    missing_syntax_token_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Match Arm Arrow",
        DiagnosticCategory::ParserSeparator,
        format!("I was expecting `->` in this match arm, but I found {found}."),
        "Write match arms as `match x { pattern -> body, ... }`.",
    )
}

/// Create a missing-lambda-arrow diagnostic (E034).
pub fn missing_lambda_arrow(span: Span, found: &str) -> Diagnostic {
    missing_syntax_token_diagnostic(
        &UNEXPECTED_TOKEN,
        span,
        "Missing Lambda Arrow",
        DiagnosticCategory::ParserSeparator,
        format!("I was expecting `->` after the lambda parameters, but I found {found}."),
        "Use `\\x -> expr` or `\\(x, y) -> expr`.",
    )
}

/// Create an orphan-constructor-pattern diagnostic (E034).
pub fn orphan_constructor_pattern(span: Span, name: &str) -> Diagnostic {
    unexpected_token(
        span,
        format!("`{name}(...)` looks like a pattern but appears outside `match`."),
    )
    .with_category(DiagnosticCategory::ParserPattern)
    .with_hint_text(format!(
        "Did you mean `match value {{ {name}(x) -> ... }}`?"
    ))
}

/// Create an unexpected `end` keyword diagnostic (E034).
pub fn unexpected_end_keyword(span: Span) -> Diagnostic {
    unexpected_token(
        span,
        "`end` is not a keyword in Flux. Use `}` to close blocks.",
    )
    .with_category(DiagnosticCategory::ParserKeyword)
    .with_hint_text("Replace `end` with `}`.")
}

/// Create a missing-hash-close-brace diagnostic (E034).
pub fn missing_hash_close_brace(span: Span) -> Diagnostic {
    unexpected_token(span, "Expected `}` to close hash literal.")
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_hint_text("Hash literals use `{key: value, ...}` and must end with `}`.")
}

/// Create a missing-array-close-bracket diagnostic (E034).
pub fn missing_array_close_bracket(span: Span) -> Diagnostic {
    unexpected_token(span, "Expected `]` to close array literal.")
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_hint_text("Array literals use `[| ... |]` and must end with `]`.")
}

/// Create a missing-lambda-close-paren diagnostic (E034).
pub fn missing_lambda_close_paren(span: Span) -> Diagnostic {
    unexpected_token(span, "Expected `)` to close lambda parameter list.")
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_hint_text("Use `\\(x, y) -> expr` for parenthesized lambda parameters.")
}

/// Create a missing-string-interpolation-close diagnostic (E034).
pub fn missing_string_interpolation_close(span: Span) -> Diagnostic {
    unexpected_token(span, "Expected `}` to close string interpolation.")
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_hint_text("Interpolation segments use `#{expr}` inside strings.")
}

/// Create a missing-comprehension-close-bracket diagnostic (E034).
pub fn missing_comprehension_close_bracket(span: Span) -> Diagnostic {
    unexpected_token(span, "Expected `]` to close list comprehension.")
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_hint_text("List comprehensions use `[expr | x <- xs, ...]`.")
}

/// Create a constructor-pattern arity mismatch diagnostic (E085).
pub fn constructor_pattern_arity_mismatch(
    span: Span,
    name: &str,
    expected: usize,
    found: usize,
) -> Diagnostic {
    Diagnostic::make_error(
        &CONSTRUCTOR_PATTERN_ARITY_MISMATCH,
        &[name, &expected.to_string(), &found.to_string()],
        "<unknown>",
        span,
    )
}

/// Create a strict cross-module constructor access diagnostic (E086).
pub fn cross_module_constructor_access_error(span: Span, ctor: &str, module: &str) -> Diagnostic {
    Diagnostic::make_error(
        &CROSS_MODULE_CONSTRUCTOR_ACCESS,
        &[ctor, module],
        "<unknown>",
        span,
    )
}

/// Create a non-strict cross-module constructor access warning (W201).
pub fn cross_module_constructor_access_warning(span: Span, ctor: &str, module: &str) -> Diagnostic {
    Diagnostic::make_warning_from_code(
        &CROSS_MODULE_CONSTRUCTOR_ACCESS_WARNING,
        &[ctor, module],
        "<unknown>",
        span,
    )
}

/// Create a guarded-wildcard non-exhaustive match diagnostic (E015).
pub fn guarded_wildcard_non_exhaustive(span: Span) -> Diagnostic {
    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
        .with_span(span)
        .with_message(
            "A guarded wildcard `_ if ...` does not guarantee exhaustiveness because the guard may fail.",
        )
        .with_hint_text("Add an unguarded `_ -> ...` fallback arm after guarded arms.")
}

/// Create an "invalid integer" error
pub fn invalid_integer(span: Span, literal: &str) -> Diagnostic {
    diagnostic_for(&INVALID_INTEGER)
        .with_category(DiagnosticCategory::ParserExpression)
        .with_span(span)
        .with_message(format!("Could not parse `{}` as an integer.", literal))
}

/// Create an "invalid float" error
pub fn invalid_float(span: Span, literal: &str) -> Diagnostic {
    diagnostic_for(&INVALID_FLOAT)
        .with_category(DiagnosticCategory::ParserExpression)
        .with_span(span)
        .with_message(format!("Could not parse `{}` as a float.", literal))
}

/// Create a "pipe target error"
pub fn pipe_target_error(span: Span) -> Diagnostic {
    diagnostic_for(&PIPE_TARGET_ERROR)
        .with_category(DiagnosticCategory::ParserExpression)
        .with_span(span)
        .with_message("Pipe operator expects a function or function call.")
        .with_hint_text("Use `value |> func` or `value |> func(arg)`")
}

/// Create an "invalid pattern" error
pub fn invalid_pattern(span: Span, found: &str) -> Diagnostic {
    diagnostic_for(&INVALID_PATTERN)
        .with_category(DiagnosticCategory::ParserPattern)
        .with_span(span)
        .with_message(format!("Expected a pattern, found `{}`.", found))
}

/// Create a "lambda syntax error"
pub fn lambda_syntax_error(span: Span, message: impl Into<String>) -> Diagnostic {
    diagnostic_for(&LAMBDA_SYNTAX_ERROR)
        .with_category(DiagnosticCategory::ParserExpression)
        .with_span(span)
        .with_message(message.into())
        .with_hint_text("Use `\\x -> expr` or `\\(x, y) -> expr`.")
}

/// Create an "unterminated interpolation" error
pub fn unterminated_interpolation(span: Span) -> Diagnostic {
    diagnostic_for(&UNTERMINATED_INTERPOLATION)
        .with_category(DiagnosticCategory::ParserExpression)
        .with_span(span)
        .with_message("Expected string continuation or end after interpolation.")
}

/// Create an "unterminated block comment" error
pub fn unterminated_block_comment(span: Span) -> Diagnostic {
    diagnostic_for(&UNTERMINATED_BLOCK_COMMENT)
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_span(span)
        .with_message("Block comment is missing closing */.")
}

/// Create a "missing comma" error for adjacent list items/arguments
pub fn missing_comma(span: Span, context: &str, example: &str) -> Diagnostic {
    diagnostic_for(&MISSING_COMMA)
        .with_category(DiagnosticCategory::ParserSeparator)
        .with_span(span)
        .with_message(format!("Missing comma between {}.", context))
        .with_hint_text(format!("Add a comma between items, e.g. {}.", example))
}

/// Create an "unclosed delimiter" error for unmatched `{`, `[`, or `(`
///
/// Points the primary span at the opening delimiter. If `found_span` is
/// provided, adds a related note showing where the mismatch was detected
/// (Rust-style two-location diagnostic).
pub fn unclosed_delimiter(
    open_span: Span,
    open: &str,
    close: &str,
    found_span: Option<Span>,
) -> Diagnostic {
    let mut diag = diagnostic_for(&UNCLOSED_DELIMITER)
        .with_display_title("Missing Closing Delimiter")
        .with_category(DiagnosticCategory::ParserDelimiter)
        .with_span(open_span)
        .with_message(format!(
            "Expected a closing `{}` to match this opening `{}`.",
            close, open
        ));
    if let Some(span) = found_span {
        diag = diag.with_related(
            RelatedDiagnostic::note(format!("Expected `{}` before this token.", close))
                .with_span(span),
        );
    }
    diag
}

/// Create a "missing opening brace" error for a function definition.
///
/// Emits a contextual error when `{` is missing after a function signature,
/// pointing at the unexpected token and providing a help hint.
pub fn missing_function_body_brace(
    fn_span: Span,
    fn_name: &str,
    found_span: Span,
    _found_token: &str,
) -> Diagnostic {
    missing_construct_opener_diagnostic(
        &UNEXPECTED_TOKEN,
        found_span,
        "Missing Function Body",
        DiagnosticCategory::ParserDeclaration,
        "This function body needs to start with `{`.",
        "This looks like the function body",
        "Try adding `{` after the function signature.",
    )
    .with_label(Label::secondary(
        fn_span,
        format!("`{}` starts here", fn_name),
    ))
}

// Type Inference Errors (E300–E399)

/// Create a type unification error (E300) with a source snippet at `span`.
///
/// Used by the HM inference pass when two concrete types cannot be unified,
/// e.g. `Int` vs `String` at a function call site.
pub fn type_unification_error(
    file: impl Into<Rc<str>>,
    span: Span,
    expected: &str,
    actual: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        span,
        "I found a type mismatch.",
        format!("this expression has type `{actual}`"),
        expected,
        actual,
        TypeMismatchNotes::new("expected type", "found type"),
        "These two types are not compatible.",
    )
    .with_display_title("Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
}

/// Create a wrong-argument-count diagnostic (E056).
pub fn wrong_argument_count(
    file: impl Into<Rc<str>>,
    call_span: Span,
    fn_name: &str,
    expected: usize,
    actual: usize,
    def_span: Option<Span>,
) -> Diagnostic {
    let mut diag = diagnostic_for(&TYPE_ERROR)
        .with_display_title("Wrong Number Of Arguments")
        .with_category(DiagnosticCategory::TypeInference)
        .with_phase(super::types::DiagnosticPhase::TypeInference)
        .with_file(file)
        .with_span(call_span)
        .with_message(format!(
            "The `{fn_name}` function takes {expected} arguments, but {actual} were provided."
        ))
        .with_primary_label(
            call_span,
            format!("{actual} arguments provided here, expected {expected}"),
        );

    let expected_call = format_call_skeleton(fn_name, expected);
    if actual > expected {
        let extra = actual - expected;
        diag = diag.with_help(format!(
            "Remove {extra} extra argument(s), for example: `{expected_call}`."
        ));
    } else if actual < expected {
        let missing = expected - actual;
        diag = diag.with_help(format!(
            "Add {missing} missing argument(s), for example: `{expected_call}`."
        ));
    }

    if let Some(span) = def_span {
        diag = diag.with_secondary_label(
            span,
            format!("`{fn_name}` is defined with {expected} parameters"),
        );
    }

    diag
}

fn format_call_skeleton(fn_name: &str, arity: usize) -> String {
    if arity == 0 {
        return format!("{fn_name}()");
    }
    let args: Vec<String> = (1..=arity).map(|i| format!("arg{i}")).collect();
    format!("{fn_name}({})", args.join(", "))
}

fn ordinal(index: usize) -> String {
    let suffix = match index % 100 {
        11..=13 => "th",
        _ => match index % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    };
    format!("{index}{suffix}")
}

/// Create a call-argument type mismatch diagnostic (E300).
pub fn call_arg_type_mismatch(
    file: impl Into<Rc<str>>,
    arg_span: Span,
    fn_name: Option<&str>,
    arg_index: usize,
    fn_def_span: Option<Span>,
    expected: &str,
    actual: &str,
) -> Diagnostic {
    let ord = ordinal(arg_index);
    let message = if let Some(name) = fn_name {
        format!("I found the wrong type in the {ord} argument to `{name}`.")
    } else {
        format!("I found the wrong type in the {ord} argument to this function.")
    };
    let mut diag = type_mismatch_diagnostic(
        file,
        arg_span,
        message,
        format!("this argument has type `{actual}`"),
        expected,
        actual,
        TypeMismatchNotes::new("expected argument type", "found argument type"),
        format!("Pass a `{expected}` value as the {ord} argument."),
    )
    .with_display_title("Argument Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
    .with_note("the actual argument type is inferred from this expression");

    if let Some(def_span) = fn_def_span {
        if let Some(name) = fn_name {
            diag = diag.with_secondary_label(
                def_span,
                format!("`{name}` expects `{expected}` as the {ord} parameter"),
            );
        } else {
            diag =
                diag.with_secondary_label(def_span, format!("this function expects `{expected}`"));
        }
        diag = diag.with_note("the expected argument type comes from the function signature");
    }

    diag
}

/// Create a typed-let annotation mismatch diagnostic (E300).
pub fn let_annotation_type_mismatch(
    file: impl Into<Rc<str>>,
    ann_span: Span,
    value_span: Span,
    name: &str,
    ann_ty: &str,
    value_ty: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        value_span,
        format!("The value of `{name}` does not match its type annotation."),
        format!("this value has type `{value_ty}`"),
        ann_ty,
        value_ty,
        TypeMismatchNotes::new("annotated type", "value type"),
        format!("Change `{name}` to a `{ann_ty}` value or update the annotation."),
    )
    .with_display_title("Annotation Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
    .with_secondary_label(
        ann_span,
        format!("but `{name}` was annotated as `{ann_ty}`"),
    )
}

/// Create a function return-annotation mismatch diagnostic (E300).
pub fn fun_return_annotation_mismatch(
    file: impl Into<Rc<str>>,
    ret_ann_span: Span,
    return_expr_span: Span,
    fn_name: &str,
    declared_ty: &str,
    actual_ty: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        return_expr_span,
        format!("The body of `{fn_name}` does not match its declared return type."),
        format!("this expression has type `{actual_ty}`"),
        declared_ty,
        actual_ty,
        TypeMismatchNotes::new("declared return type", "body type"),
        format!("Return a `{declared_ty}` value from `{fn_name}` or change its annotation."),
    )
    .with_display_title("Return Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
    .with_secondary_label(
        ret_ann_span,
        format!("`{fn_name}` was declared to return `{declared_ty}`"),
    )
}

/// Create an if-branch mismatch diagnostic (E300).
pub fn if_branch_type_mismatch(
    file: impl Into<Rc<str>>,
    then_span: Span,
    else_span: Span,
    then_ty: &str,
    else_ty: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        else_span,
        "The branches of this `if` expression do not agree on a type.",
        format!("the `else` branch has type `{else_ty}`"),
        then_ty,
        else_ty,
        TypeMismatchNotes::new("then branch type", "else branch type"),
        "Both branches of an `if` must produce the same type.",
    )
    .with_display_title("Branch Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
    .with_secondary_label(then_span, format!("`then` branch returns `{then_ty}`"))
}

/// Create a match-arm mismatch diagnostic (E300).
pub fn match_arm_type_mismatch(
    file: impl Into<Rc<str>>,
    first_span: Span,
    arm_span: Span,
    first_ty: &str,
    arm_ty: &str,
    arm_index: usize,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        arm_span,
        "The arms of this `match` expression do not agree on a type.",
        format!("arm {arm_index} has type `{arm_ty}`"),
        first_ty,
        arm_ty,
        TypeMismatchNotes::new("first arm type", "this arm type"),
        format!("Change arm {arm_index} so every arm returns the same type."),
    )
    .with_display_title("Match Arm Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
    .with_secondary_label(first_span, format!("first arm returns `{first_ty}`"))
}

/// Create a function return-type mismatch diagnostic (E300).
pub fn fun_return_type_mismatch(
    file: impl Into<Rc<str>>,
    span: Span,
    expected_ret: &str,
    actual_ret: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        span,
        "The body of this function does not match its return type.",
        format!("this expression has type `{actual_ret}`"),
        expected_ret,
        actual_ret,
        TypeMismatchNotes::new("declared return type", "body type"),
        format!("Change the return annotation to `-> {actual_ret}` or change the body."),
    )
    .with_display_title("Return Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
}

/// Create a function parameter-type mismatch diagnostic (E300).
pub fn fun_param_type_mismatch(
    file: impl Into<Rc<str>>,
    span: Span,
    index: usize,
    expected: &str,
    actual: &str,
) -> Diagnostic {
    type_mismatch_diagnostic(
        file,
        span,
        format!("Parameter {index} has the wrong type."),
        format!("parameter {index} has type `{actual}`"),
        expected,
        actual,
        TypeMismatchNotes::new("expected parameter type", "found parameter type"),
        format!("Change parameter {index} to use `{expected}` consistently."),
    )
    .with_display_title("Parameter Type Mismatch")
    .with_category(DiagnosticCategory::TypeInference)
}

/// Create a function arity mismatch diagnostic (E300).
pub fn fun_arity_mismatch(
    file: impl Into<Rc<str>>,
    span: Span,
    expected: usize,
    actual: usize,
) -> Diagnostic {
    let direction = if actual > expected {
        "too many"
    } else if actual < expected {
        "too few"
    } else {
        "the wrong number of"
    };
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        .with_display_title("Wrong Number Of Arguments")
        .with_category(DiagnosticCategory::TypeInference)
        .with_phase(super::types::DiagnosticPhase::TypeInference)
        .with_file(file)
        .with_span(span)
        .with_message(format!(
            "I am applying a function to {direction} arguments."
        ))
        .with_primary_label(span, format!("this call passes {actual} argument(s)"))
        .with_note(format!("this function takes: {expected} argument(s)"))
        .with_note(format!("but this call passes: {actual} argument(s)"))
        .with_help(if actual > expected {
            format!("Remove {} extra argument(s).", actual - expected)
        } else if actual < expected {
            format!("Add {} missing argument(s).", expected - actual)
        } else {
            "Check the call site and the function definition.".to_string()
        })
}

/// Create an occurs-check failure (E301) with a source snippet at `span`.
///
/// Fires when a type variable would be bound to a type that contains itself,
/// creating an infinite recursive type.
pub fn occurs_check_failure(
    file: impl Into<Rc<str>>,
    span: Span,
    var: &str,
    ty: &str,
) -> Diagnostic {
    let _ = var;
    occurs_check_diagnostic(file, span, ty)
}
