#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticPhase {
    /// Lexer and parser errors
    Parse,
    /// Module graph resolution diagnostics
    ModuleGraph,
    /// Compiler validation diagnostics
    Validation,
    /// HM type inference diagnostics
    TypeInference,
    /// Compiler type-check/boundary diagnostics
    TypeCheck,
    /// Effect system diagnostics
    Effect,
    /// Runtime diagnostics
    Runtime,
}
