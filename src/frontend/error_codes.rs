#[derive(Debug, Clone, Copy)]
pub struct ErrorCode {
    pub code: &'static str,
    pub title: &'static str,
}

pub const DUPLICATE_NAME: ErrorCode = ErrorCode {
    code: "E001",
    title: "DUPLICATE NAME",
};
pub const IMMUTABLE_BINDING: ErrorCode = ErrorCode {
    code: "E003",
    title: "IMMUTABLE BINDING",
};
pub const OUTER_ASSIGNMENT: ErrorCode = ErrorCode {
    code: "E004",
    title: "OUTER ASSIGNMENT",
};
pub const UNDEFINED_VARIABLE: ErrorCode = ErrorCode {
    code: "E007",
    title: "UNDEFINED VARIABLE",
};
pub const UNKNOWN_PREFIX_OPERATOR: ErrorCode = ErrorCode {
    code: "E010",
    title: "UNKNOWN PREFIX OPERATOR",
};
pub const UNKNOWN_INFIX_OPERATOR: ErrorCode = ErrorCode {
    code: "E011",
    title: "UNKNOWN INFIX OPERATOR",
};
pub const DUPLICATE_PARAMETER: ErrorCode = ErrorCode {
    code: "E012",
    title: "DUPLICATE PARAMETER",
};
pub const INVALID_MODULE_NAME: ErrorCode = ErrorCode {
    code: "E016",
    title: "INVALID MODULE NAME",
};
pub const MODULE_NAME_CLASH: ErrorCode = ErrorCode {
    code: "E018",
    title: "MODULE NAME CLASH",
};
pub const INVALID_MODULE_CONTENT: ErrorCode = ErrorCode {
    code: "E019",
    title: "INVALID MODULE CONTENT",
};
pub const PRIVATE_MEMBER: ErrorCode = ErrorCode {
    code: "E021",
    title: "PRIVATE MEMBER",
};
pub const EMPTY_MATCH: ErrorCode = ErrorCode {
    code: "E030",
    title: "EMPTY MATCH",
};
pub const IMPORT_SCOPE: ErrorCode = ErrorCode {
    code: "E031",
    title: "IMPORT SCOPE",
};
pub const IMPORT_NOT_FOUND: ErrorCode = ErrorCode {
    code: "E032",
    title: "IMPORT NOT FOUND",
};
pub const IMPORT_READ_FAILED: ErrorCode = ErrorCode {
    code: "E033",
    title: "IMPORT READ FAILED",
};
pub const INVALID_PATTERN: ErrorCode = ErrorCode {
    code: "E034",
    title: "INVALID PATTERN",
};
pub const IMPORT_CYCLE: ErrorCode = ErrorCode {
    code: "E035",
    title: "IMPORT CYCLE",
};
pub const SCRIPT_NOT_IMPORTABLE: ErrorCode = ErrorCode {
    code: "E036",
    title: "SCRIPT NOT IMPORTABLE",
};
pub const MULTIPLE_MODULES: ErrorCode = ErrorCode {
    code: "E037",
    title: "MULTIPLE MODULES",
};
pub const MODULE_PATH_MISMATCH: ErrorCode = ErrorCode {
    code: "E038",
    title: "MODULE PATH MISMATCH",
};
pub const MODULE_SCOPE: ErrorCode = ErrorCode {
    code: "E039",
    title: "MODULE SCOPE",
};
pub const INVALID_MODULE_ALIAS: ErrorCode = ErrorCode {
    code: "E040",
    title: "INVALID MODULE ALIAS",
};
pub const DUPLICATE_MODULE: ErrorCode = ErrorCode {
    code: "E041",
    title: "DUPLICATE MODULE",
};
pub const INVALID_MODULE_FILE: ErrorCode = ErrorCode {
    code: "E042",
    title: "INVALID MODULE FILE",
};
pub const UNKNOWN_KEYWORD: ErrorCode = ErrorCode {
    code: "E101",
    title: "UNKNOWN KEYWORD",
};
pub const EXPECTED_EXPRESSION: ErrorCode = ErrorCode {
    code: "E102",
    title: "EXPECTED EXPRESSION",
};
pub const INVALID_INTEGER: ErrorCode = ErrorCode {
    code: "E103",
    title: "INVALID INTEGER",
};
pub const INVALID_FLOAT: ErrorCode = ErrorCode {
    code: "E104",
    title: "INVALID FLOAT",
};
pub const UNEXPECTED_TOKEN: ErrorCode = ErrorCode {
    code: "E105",
    title: "UNEXPECTED TOKEN",
};
pub const INVALID_PATTERN_LEGACY: ErrorCode = ErrorCode {
    code: "E106",
    title: "INVALID PATTERN",
};

pub const ERROR_CODES: &[ErrorCode] = &[
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
    EMPTY_MATCH,
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
    UNKNOWN_KEYWORD,
    EXPECTED_EXPRESSION,
    INVALID_INTEGER,
    INVALID_FLOAT,
    UNEXPECTED_TOKEN,
    INVALID_PATTERN_LEGACY,
];

pub fn get(code: &str) -> Option<&'static ErrorCode> {
    ERROR_CODES.iter().find(|item| item.code == code)
}

pub fn diag(code: &'static ErrorCode) -> crate::frontend::diagnostic::Diagnostic {
    crate::frontend::diagnostic::Diagnostic::error(code.title).with_code(code.code)
}
