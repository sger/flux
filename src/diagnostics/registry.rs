use super::compiler_errors::*;
use super::diagnostic::Diagnostic;
use super::runtime_errors::*;
use super::types::{DiagnosticCategory, ErrorCode, Severity};
use std::collections::HashMap;
use std::sync::OnceLock;

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
    ICE_SYMBOL_SCOPE_LET,
    ICE_SYMBOL_SCOPE_ASSIGN,
    ICE_TEMP_SYMBOL_MATCH,
    ICE_TEMP_SYMBOL_SOME_PATTERN,
    ICE_SYMBOL_SCOPE_PATTERN,
    ICE_TEMP_SYMBOL_SOME_BINDING,
    ICE_TEMP_SYMBOL_LEFT_PATTERN,
    ICE_TEMP_SYMBOL_RIGHT_PATTERN,
    ICE_TEMP_SYMBOL_LEFT_BINDING,
    ICE_TEMP_SYMBOL_RIGHT_BINDING,
    UNTERMINATED_STRING,
    UNTERMINATED_INTERPOLATION,
    UNTERMINATED_BLOCK_COMMENT,
    MISSING_COMMA,
    DUPLICATE_PATTERN_BINDING,
    UNCLOSED_DELIMITER,
    LEGACY_LIST_TAIL_NONE,
    BASE_ALIAS_FORBIDDEN,
    DUPLICATE_BASE_EXCLUSION,
    UNKNOWN_BASE_MEMBER,
    UNKNOWN_CONSTRUCTOR,
    CONSTRUCTOR_ARITY_MISMATCH,
    ADT_NON_EXHAUSTIVE_MATCH,
    MODULE_ADT_CONSTRUCTOR_NOT_EXPORTED,
    CONSTRUCTOR_PATTERN_ARITY_MISMATCH,
    CROSS_MODULE_CONSTRUCTOR_ACCESS,
    CROSS_MODULE_CONSTRUCTOR_ACCESS_WARNING,
    UNKNOWN_FUNCTION_EFFECT,
    // Type inference errors (E300–E399)
    TYPE_UNIFICATION_ERROR,
    OCCURS_CHECK_FAILURE,
    UNDEFINED_TYPE_VAR,
    INVALID_TYPE_ANNOTATION,
    INVALID_EFFECT_ROW,
    RIGID_VAR_ESCAPE,
    // Strict-types errors (E430+)
    STRICT_TYPES_ANY_INFERRED,
    CORE_LINT_FAILURE,
    // Type class errors (E440–E449)
    DUPLICATE_CLASS,
    INSTANCE_UNKNOWN_CLASS,
    INSTANCE_MISSING_METHOD,
    DUPLICATE_INSTANCE,
    NO_INSTANCE,
    INSTANCE_EXTRA_METHOD,
    INSTANCE_TYPE_ARG_ARITY,
    INSTANCE_METHOD_ARITY,
    MISSING_SUPERCLASS_INSTANCE,
    ORPHAN_INSTANCE,
    PUBLIC_INSTANCE_OF_PRIVATE_CLASS,
    PUBLIC_CLASS_LEAKS_PRIVATE_TYPE,
    INSTANCE_METHOD_EFFECT_FLOOR,
    PUBLIC_INSTANCE_HAS_PRIVATE_HEAD,
    AMBIGUOUS_CLASS_CONSTRAINT,
    EXPOSING_LOCAL_COLLISION,
    IMPORT_NAME_COLLISION_FILE_VS_MODULE,
    // Named-field data types (Proposal 0152, E460–E468)
    NAMED_FIELD_MISSING,
    NAMED_FIELD_UNKNOWN,
    NAMED_FIELD_DUPLICATE,
    NAMED_FIELD_NOT_ON_TYPE,
    SPREAD_NON_NAMED_ADT,
    DATA_MIXED_FIELD_FORMS,
    NAMED_FIELD_PUN_UNBOUND,
    NAMED_FIELD_TYPE_DIVERGES,
    SPREAD_UNKNOWN_VARIANT,
    // Runtime errors (E1000+)
    WRONG_NUMBER_OF_ARGUMENTS,
    NOT_A_FUNCTION,
    FUNCTION_NOT_FOUND,
    BASE_FUNCTION_ERROR,
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

fn error_code_map() -> &'static HashMap<&'static str, &'static ErrorCode> {
    static MAP: OnceLock<HashMap<&'static str, &'static ErrorCode>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::with_capacity(ERROR_CODES.len());
        for code in ERROR_CODES {
            map.insert(code.code, code);
        }
        map
    })
}

/// Return the default category associated with a stable diagnostic code.
pub fn default_diagnostic_category(code: &str) -> Option<DiagnosticCategory> {
    match code {
        "E001" | "E002" | "E003" | "E004" | "E005" | "E006" | "E007" | "E012" | "E080" | "E085" => {
            Some(DiagnosticCategory::NameResolution)
        }
        "E008" | "E009" | "E010" | "E011" | "E013" | "E017" | "E024" | "E029" | "E078" | "E079"
        | "E086" | "E410" | "E411" | "E412" | "E413" | "E414" | "E415" | "E416" | "E417"
        | "E418" => Some(DiagnosticCategory::ModuleSystem),
        "E030" => Some(DiagnosticCategory::ParserKeyword),
        "E031" | "E032" | "E033" | "E036" | "E071" | "E072" => {
            Some(DiagnosticCategory::ParserExpression)
        }
        "E034" => Some(DiagnosticCategory::ParserExpression),
        "E035" => Some(DiagnosticCategory::ParserPattern),
        "E073" => Some(DiagnosticCategory::ParserSeparator),
        "E076" => Some(DiagnosticCategory::ParserDelimiter),
        "E423" => Some(DiagnosticCategory::TypeInference),
        "E426" => Some(DiagnosticCategory::Internal),
        "E056" | "E300" | "E301" | "E430" | "E440" | "E441" | "E442" | "E443" | "E444" | "E445"
        | "E446" | "E447" | "E448" | "E449" | "E450" | "E451" | "E452" | "E455" | "E456" => {
            Some(DiagnosticCategory::TypeInference)
        }
        "E457" | "E458" => Some(DiagnosticCategory::ModuleSystem),
        "E460" | "E461" | "E462" | "E463" | "E464" | "E465" | "E466" | "E467" | "E468" => {
            Some(DiagnosticCategory::TypeInference)
        }
        "E400" | "E401" | "E402" | "E403" | "E404" | "E405" | "E406" | "E407" | "E419" | "E420"
        | "E421" | "E422" | "E425" => Some(DiagnosticCategory::Effects),
        "E1004" => Some(DiagnosticCategory::RuntimeType),
        _ if code.starts_with("E100") || code.starts_with("E101") || code.starts_with("E102") => {
            Some(DiagnosticCategory::RuntimeExecution)
        }
        _ => None,
    }
}

/// Look up error code by code string (e.g., "E007", "E1001")
pub fn lookup_error_code(code: &str) -> Option<&'static ErrorCode> {
    error_code_map().get(code).copied()
}

/// Create a diagnostic from an error code (without message formatting)
pub fn diagnostic_for(code: &'static ErrorCode) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        title: code.title.to_string(),
        display_title: None,
        category: default_diagnostic_category(code.code),
        code: Some(code.code.to_string()),
        error_type: Some(code.error_type),
        message: None,
        file: None,
        span: None,
        labels: Vec::new(),
        hints: Vec::new(),
        suggestions: Vec::new(),
        hint_chains: Vec::new(),
        related: Vec::new(),
        stack_trace: Vec::new(),
        phase: None,
    }
}
