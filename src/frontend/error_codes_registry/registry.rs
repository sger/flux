use crate::frontend::diagnostic::Diagnostic;

use super::compiler_errors::*;
use super::runtime_errors::*;
use super::types::ErrorCode;

/// Central registry of all error codes (compiler + runtime)
pub const ERROR_CODES: &[ErrorCode] = &[
    // Compiler errors (E001-E999)
    DUPLICATE_NAME,
    IMMUTABLE_BINDING,
    OUTER_ASSIGNMENT,
    UNDEFINED_VARIABLE,
    UNKNOWN_PREFIX_OPERATOR,
    UNKNOWN_INFIX_OPERATOR,
    DUPLICATE_PARAMETER,
    INVALID_MODULE_NAME,
    MODULE_NAME_CLASH,
    INVALID_MODULE_CONTENT,
    PRIVATE_MEMBER,
    UNKNOWN_MODULE_MEMBER,
    MODULE_NOT_IMPORTED,
    EMPTY_MATCH,
    NON_EXHAUSTIVE_MATCH,
    CATCHALL_NOT_LAST,
    IMPORT_SCOPE,
    IMPORT_NOT_FOUND,
    IMPORT_READ_FAILED,
    INVALID_PATTERN,
    IMPORT_CYCLE,
    SCRIPT_NOT_IMPORTABLE,
    MULTIPLE_MODULES,
    MODULE_PATH_MISMATCH,
    MODULE_SCOPE,
    INVALID_MODULE_ALIAS,
    DUPLICATE_MODULE,
    INVALID_MODULE_FILE,
    IMPORT_NAME_COLLISION,
    UNKNOWN_KEYWORD,
    EXPECTED_EXPRESSION,
    INVALID_INTEGER,
    INVALID_FLOAT,
    UNEXPECTED_TOKEN,
    INVALID_PATTERN_LEGACY,
    LAMBDA_SYNTAX_ERROR,
    LAMBDA_PARAMETER_ERROR,
    LAMBDA_BODY_ERROR,
    PIPE_OPERATOR_ERROR,
    PIPE_TARGET_ERROR,
    EITHER_CONSTRUCTOR_ERROR,
    EITHER_VALUE_ERROR,
    SHORT_CIRCUIT_ERROR,
    CIRCULAR_DEPENDENCY,
    CONST_EVAL_ERROR,
    CONST_NOT_FOUND,
    CONST_NOT_PUBLIC,
    CONST_INVALID_EXPR,
    CONST_TYPE_ERROR,
    CONST_SCOPE_ERROR,
    DIVISION_BY_ZERO_COMPILE,
    MODULO_BY_ZERO_COMPILE,
    EITHER_UNWRAP_ERROR_LEFT,
    EITHER_UNWRAP_ERROR_RIGHT,
    TYPE_MISMATCH,
    TYPE_ERROR,
    INCOMPATIBLE_TYPES,
    CONST_RUNTIME_ERROR,
    CONST_DIVISION_BY_ZERO,
    CONST_OVERFLOW,
    // Runtime errors (E1000+)
    WRONG_NUMBER_OF_ARGUMENTS,
    NOT_A_FUNCTION,
    FUNCTION_NOT_FOUND,
    BUILTIN_ERROR,
    RUNTIME_TYPE_ERROR,
    NOT_INDEXABLE,
    KEY_NOT_HASHABLE,
    NOT_ITERABLE,
    DIVISION_BY_ZERO_RUNTIME,
    INVALID_OPERATION,
    INTEGER_OVERFLOW,
    MODULO_BY_ZERO_RUNTIME,
    INDEX_OUT_OF_BOUNDS,
    KEY_NOT_FOUND,
    NEGATIVE_INDEX,
    INVALID_SLICE,
    MATCH_ERROR,
    OPTION_UNWRAP_ERROR,
    EITHER_UNWRAP_ERROR,
    STRING_INDEX_ERROR,
    STRING_ENCODING_ERROR,
    INVALID_SUBSTRING,
];

/// Look up error code by code string (e.g., "E007", "E1001")
pub fn get_enhanced(code: &str) -> Option<&'static ErrorCode> {
    ERROR_CODES.iter().find(|item| item.code == code)
}

/// Create a diagnostic from an error code (without message formatting)
pub fn diag_enhanced(code: &'static ErrorCode) -> Diagnostic {
    Diagnostic::error(code.title)
        .with_code(code.code)
        .with_error_type(code.error_type)
}

/// Convenience function to build error with formatted message
pub fn diag_with_message(
    code: &str,
    values: &[&str],
) -> Option<Diagnostic> {
    use super::format::format_message;

    get_enhanced(code).map(|err| {
        let mut diag = Diagnostic::error(err.title)
            .with_code(err.code)
            .with_error_type(err.error_type)
            .with_message(format_message(err.message, values));

        if let Some(hint) = err.hint {
            diag = diag.with_hint(hint);
        }

        diag
    })
}
