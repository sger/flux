use super::builders::DiagnosticBuilder;
use super::types::{ErrorCode, ErrorType};

pub const WRONG_NUMBER_OF_ARGUMENTS: ErrorCode = ErrorCode {
    code: "E1000",
    title: "WRONG NUMBER OF ARGUMENTS",
    error_type: ErrorType::Runtime,
    message: "function {}/{} expects {} arguments, got {}",
    hint: Some("{}"), // Function signature
};

pub const NOT_A_FUNCTION: ErrorCode = ErrorCode {
    code: "E1001",
    title: "NOT A FUNCTION",
    error_type: ErrorType::Runtime,
    message: "Cannot call non-function value (got {}).",
    hint: None,
};

pub const FUNCTION_NOT_FOUND: ErrorCode = ErrorCode {
    code: "E1002",
    title: "FUNCTION NOT FOUND",
    error_type: ErrorType::Runtime,
    message: "Function `{}` not found.",
    hint: Some("Check that the function is defined and imported."),
};

pub const BUILTIN_ERROR: ErrorCode = ErrorCode {
    code: "E1003",
    title: "BUILTIN FUNCTION ERROR",
    error_type: ErrorType::Runtime,
    message: "Error in builtin function `{}`: {}.",
    hint: None,
};

pub const RUNTIME_TYPE_ERROR: ErrorCode = ErrorCode {
    code: "E1004",
    title: "TYPE ERROR",
    error_type: ErrorType::Runtime,
    message: "Expected {}, got {}.",
    hint: None,
};

pub const NOT_INDEXABLE: ErrorCode = ErrorCode {
    code: "E1005",
    title: "NOT INDEXABLE",
    error_type: ErrorType::Runtime,
    message: "Cannot index {} (not an array or hash).",
    hint: Some("Only arrays and hashes support indexing."),
};

pub const KEY_NOT_HASHABLE: ErrorCode = ErrorCode {
    code: "E1006",
    title: "KEY NOT HASHABLE",
    error_type: ErrorType::Runtime,
    message: "Hash key must be String, Int, or Bool (got {}).",
    hint: None,
};

pub const NOT_ITERABLE: ErrorCode = ErrorCode {
    code: "E1007",
    title: "NOT ITERABLE",
    error_type: ErrorType::Runtime,
    message: "Cannot iterate over {} (not an array).",
    hint: Some("Only arrays can be iterated."),
};

pub const DIVISION_BY_ZERO_RUNTIME: ErrorCode = ErrorCode {
    code: "E1008",
    title: "DIVISION BY ZERO",
    error_type: ErrorType::Runtime,
    message: "Cannot divide by zero.",
    hint: Some("Check divisor is non-zero before division."),
};

pub const INVALID_OPERATION: ErrorCode = ErrorCode {
    code: "E1009",
    title: "INVALID OPERATION",
    error_type: ErrorType::Runtime,
    message: "Cannot {} {} and {} values.", // op, type1, type2
    hint: None,
};

pub const INTEGER_OVERFLOW: ErrorCode = ErrorCode {
    code: "E1010",
    title: "INTEGER OVERFLOW",
    error_type: ErrorType::Runtime,
    message: "Integer overflow in {} operation.",
    hint: Some("Use smaller numbers or handle overflow explicitly."),
};

pub const MODULO_BY_ZERO_RUNTIME: ErrorCode = ErrorCode {
    code: "E1011",
    title: "MODULO BY ZERO",
    error_type: ErrorType::Runtime,
    message: "Cannot compute modulo by zero.",
    hint: Some("Check divisor is non-zero before modulo operation."),
};

pub const INDEX_OUT_OF_BOUNDS: ErrorCode = ErrorCode {
    code: "E1012",
    title: "INDEX OUT OF BOUNDS",
    error_type: ErrorType::Runtime,
    message: "Array index {} out of bounds (length {}).",
    hint: None,
};

pub const KEY_NOT_FOUND: ErrorCode = ErrorCode {
    code: "E1013",
    title: "KEY NOT FOUND",
    error_type: ErrorType::Runtime,
    message: "Hash key `{}` not found.",
    hint: Some("Use has_key() to check before accessing."),
};

pub const NEGATIVE_INDEX: ErrorCode = ErrorCode {
    code: "E1014",
    title: "NEGATIVE INDEX",
    error_type: ErrorType::Runtime,
    message: "Array index cannot be negative (got {}).",
    hint: Some("Use non-negative integers for array indexing."),
};

pub const INVALID_SLICE: ErrorCode = ErrorCode {
    code: "E1015",
    title: "INVALID SLICE",
    error_type: ErrorType::Runtime,
    message: "Invalid slice bounds: start={}, end={}, length={}.",
    hint: Some("Ensure 0 <= start <= end <= length."),
};

pub const MATCH_ERROR: ErrorCode = ErrorCode {
    code: "E1016",
    title: "MATCH ERROR",
    error_type: ErrorType::Runtime,
    message: "No pattern matched the value.",
    hint: Some("Add a wildcard pattern _ to handle all cases."),
};

pub const OPTION_UNWRAP_ERROR: ErrorCode = ErrorCode {
    code: "E1017",
    title: "OPTION UNWRAP ERROR",
    error_type: ErrorType::Runtime,
    message: "Cannot unwrap None value.",
    hint: Some("Use pattern matching or unwrap_or() to handle None."),
};

pub const EITHER_UNWRAP_ERROR: ErrorCode = ErrorCode {
    code: "E1018",
    title: "EITHER UNWRAP ERROR",
    error_type: ErrorType::Runtime,
    message: "Cannot unwrap {} as {}.", // "Left" as "Right" or vice versa
    hint: Some("Use pattern matching to handle both Left and Right cases."),
};

pub const STRING_INDEX_ERROR: ErrorCode = ErrorCode {
    code: "E1019",
    title: "STRING INDEX ERROR",
    error_type: ErrorType::Runtime,
    message: "String index {} out of bounds (length {}).",
    hint: None,
};

pub const STRING_ENCODING_ERROR: ErrorCode = ErrorCode {
    code: "E1020",
    title: "STRING ENCODING ERROR",
    error_type: ErrorType::Runtime,
    message: "Invalid UTF-8 encoding in string operation.",
    hint: None,
};

pub const INVALID_SUBSTRING: ErrorCode = ErrorCode {
    code: "E1021",
    title: "INVALID SUBSTRING",
    error_type: ErrorType::Runtime,
    message: "Invalid substring bounds: start={}, end={}, length={}.",
    hint: Some("Ensure 0 <= start <= end <= length."),
};

// ============================================================================
// Runtime Error Constructor Functions
// ============================================================================
// These functions provide a clean API for creating runtime diagnostics with
// proper error codes. Use these instead of Diagnostic::error() in production code.

use super::diagnostic::Diagnostic;
use super::registry::diag_enhanced;
use crate::frontend::position::Span;

/// Create an "invalid operation" runtime error
pub fn invalid_operation(
    op_name: &str,
    left_type: &str,
    right_type: &str,
    file: String,
    span: Span,
) -> Diagnostic {
    diag_enhanced(&INVALID_OPERATION)
        .with_message(format!(
            "Cannot {} {} and {} values.",
            op_name, left_type, right_type
        ))
        .with_file(file)
        .with_span(span)
}
