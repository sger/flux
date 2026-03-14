/// High-level category used to group diagnostics for rendering and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCategory {
    ParserDeclaration,
    ParserDelimiter,
    ParserSeparator,
    ParserExpression,
    ParserPattern,
    ParserKeyword,
    NameResolution,
    ModuleSystem,
    Orchestration,
    TypeInference,
    Effects,
    RuntimeType,
    RuntimeExecution,
    Internal,
}

impl DiagnosticCategory {
    /// Return the stable machine-readable category name used in JSON output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParserDeclaration => "parser_declaration",
            Self::ParserDelimiter => "parser_delimiter",
            Self::ParserSeparator => "parser_separator",
            Self::ParserExpression => "parser_expression",
            Self::ParserPattern => "parser_pattern",
            Self::ParserKeyword => "parser_keyword",
            Self::NameResolution => "name_resolution",
            Self::ModuleSystem => "module_system",
            Self::Orchestration => "orchestration",
            Self::TypeInference => "type_inference",
            Self::Effects => "effects",
            Self::RuntimeType => "runtime_type",
            Self::RuntimeExecution => "runtime_execution",
            Self::Internal => "internal",
        }
    }
}
