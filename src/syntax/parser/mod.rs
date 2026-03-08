use crate::{
    diagnostics::{
        Diagnostic, DiagnosticCategory,
        position::{Position, Span},
        quality::with_parser_breadcrumb,
        unexpected_token_with_details,
    },
    syntax::{
        expression::{ExprId, ExprIdGen},
        interner::Interner,
        lexer::Lexer,
        program::Program,
        token::Token,
        token_type::TokenType,
    },
};

mod expression;
mod helpers;
mod literal;
mod statement;

pub(super) struct ParserContextGuard {
    parser: std::ptr::NonNull<Parser>,
    depth: usize,
}

impl Drop for ParserContextGuard {
    fn drop(&mut self) {
        unsafe {
            self.parser.as_mut().parser_contexts.truncate(self.depth);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RecoveryBoundary {
    Statement,
    NextLineOrBlock,
    MissingBlockOpener,
}

#[derive(Debug, Clone)]
pub(super) enum ParserContext {
    Function(String),
    Module(String),
    IfBranch,
    ElseBranch,
    MatchExpression,
    Lambda,
    HandleExpression,
    Effect(String),
    Data(String),
}

impl ParserContext {
    pub(super) fn breadcrumb(&self) -> String {
        match self {
            ParserContext::Function(name) => format!("function `{name}`"),
            ParserContext::Module(name) => format!("module `{name}`"),
            ParserContext::IfBranch => "`if` expression".to_string(),
            ParserContext::ElseBranch => "`else` branch".to_string(),
            ParserContext::MatchExpression => "`match` expression".to_string(),
            ParserContext::Lambda => "lambda expression".to_string(),
            ParserContext::HandleExpression => "`handle` expression".to_string(),
            ParserContext::Effect(name) => format!("effect `{name}`"),
            ParserContext::Data(name) => format!("data declaration `{name}`"),
        }
    }
}

pub struct Parser {
    pub(super) lexer: Lexer,
    pub(super) current_token: Token,
    pub(super) peek_token: Token,
    pub(super) peek2_token: Token,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub(super) suppress_unterminated_string_error_at: Option<Position>,
    pub(super) reported_unclosed_brace: bool,
    pub(super) pending_recovery_boundary: Option<RecoveryBoundary>,
    pub(super) used_custom_recovery: bool,
    pub(super) suppress_top_level_rbrace_once: bool,
    pub(super) parser_contexts: Vec<ParserContext>,
    pub(super) suppress_structural_followups: bool,
    expr_id_gen: ExprIdGen,
}

impl Parser {
    pub fn new(lexer: Lexer) -> Self {
        let mut parser = Parser {
            lexer,
            current_token: Token::new_static(TokenType::Eof, "", 0, 0),
            peek_token: Token::new_static(TokenType::Eof, "", 0, 0),
            peek2_token: Token::new_static(TokenType::Eof, "", 0, 0),
            errors: Vec::new(),
            warnings: Vec::new(),
            suppress_unterminated_string_error_at: None,
            reported_unclosed_brace: false,
            pending_recovery_boundary: None,
            used_custom_recovery: false,
            suppress_top_level_rbrace_once: false,
            parser_contexts: Vec::new(),
            suppress_structural_followups: false,
            expr_id_gen: ExprIdGen::new(),
        };
        parser.prime();
        parser
    }

    fn prime(&mut self) {
        // Skip doc comments during initialization
        self.current_token = self.next_non_doc_token();
        self.peek_token = self.next_non_doc_token();
        self.peek2_token = self.next_non_doc_token();
    }

    fn next_non_doc_token(&mut self) -> Token {
        let mut token = self.lexer.next_token();
        while token.token_type == TokenType::DocComment {
            token = self.lexer.next_token();
        }
        token
    }

    /// Takes ownership of the interner from the parser's lexer,
    /// leaving a default interner in its place.
    pub fn take_interner(&mut self) -> Interner {
        self.lexer.take_interner()
    }

    /// Returns an immutable reference to the parser's interner.
    pub fn interner(&self) -> &Interner {
        self.lexer.interner()
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings)
    }

    pub(super) fn next_expr_id(&mut self) -> ExprId {
        self.expr_id_gen.next_id()
    }

    pub fn expr_id_gen(&self) -> u32 {
        self.expr_id_gen.counter()
    }

    pub fn parse_program(&mut self) -> Program {
        let start = self.current_token.position;
        let mut program = Program::new();

        while self.current_token.token_type != TokenType::Eof {
            if self.current_token.token_type == TokenType::RBrace {
                if self.suppress_top_level_rbrace_once {
                    self.suppress_top_level_rbrace_once = false;
                    self.next_token();
                    continue;
                }
                self.emit_parser_diagnostic(unexpected_token_with_details(
                    self.current_token.span(),
                    "Unexpected Closing Delimiter",
                    DiagnosticCategory::ParserDelimiter,
                    "Unexpected `}` outside of a block.",
                ));
                self.next_token();
                continue;
            }
            if let Some(statement) = self.parse_statement() {
                program.statements.push(statement);
            }
            self.next_token();
        }

        program.span = Span::new(start, self.current_token.end_position);
        program
    }

    pub(super) fn start_construct_diagnostics_checkpoint(&self) -> usize {
        self.errors.len()
    }

    pub(super) fn has_structural_error_since(&self, checkpoint: usize) -> bool {
        self.errors
            .iter()
            .skip(checkpoint)
            .any(|diag| matches!(diag.code(), Some("E034") | Some("E076")))
    }

    pub(super) fn has_error_since(&self, checkpoint: usize) -> bool {
        self.errors
            .iter()
            .skip(checkpoint)
            .any(|diag| diag.severity() == crate::diagnostics::types::Severity::Error)
    }

    pub(super) fn push_followup_unless_structural_root(
        &mut self,
        checkpoint: usize,
        diag: Diagnostic,
    ) -> bool {
        if self.has_structural_error_since(checkpoint) {
            false
        } else {
            self.emit_parser_diagnostic(diag)
        }
    }

    fn push_parser_context(&mut self, context: ParserContext) {
        self.parser_contexts.push(context);
    }

    pub(super) fn enter_parser_context(&mut self, context: ParserContext) -> ParserContextGuard {
        let depth = self.parser_contexts.len();
        self.push_parser_context(context);
        ParserContextGuard {
            parser: std::ptr::NonNull::from(self),
            depth,
        }
    }

    pub(super) fn current_parser_breadcrumb(&self) -> Option<String> {
        self.parser_contexts.last().map(ParserContext::breadcrumb)
    }

    pub(super) fn begin_structural_suppression(&mut self) {
        self.suppress_structural_followups = true;
    }

    pub(super) fn clear_structural_suppression(&mut self) {
        self.suppress_structural_followups = false;
    }

    pub(super) fn push_parser_diagnostic(&mut self, diag: Diagnostic) -> bool {
        let is_structural = matches!(diag.code(), Some("E034") | Some("E076"));
        if self.suppress_structural_followups && !is_structural {
            return false;
        }
        if is_structural {
            self.begin_structural_suppression();
        }
        self.errors.push(diag);
        true
    }

    /// Emit a parser diagnostic through the centralized parser error path so
    /// breadcrumb notes and structural-followup suppression stay consistent.
    pub(super) fn emit_parser_diagnostic(&mut self, diag: Diagnostic) -> bool {
        let breadcrumb = self.current_parser_breadcrumb();
        self.push_parser_diagnostic(with_parser_breadcrumb(diag, breadcrumb.as_deref()))
    }
}

fn is_uppercase_ident(token: &Token) -> bool {
    if token.token_type != TokenType::Ident {
        return false;
    }
    token
        .literal
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

/// Check if token is PascalCase (starts uppercase, contains lowercase)
/// This distinguishes module names like "Math", "Constants" from
/// ALL_CAPS constants like "PI", "TAU", "MAX"
fn is_pascal_case_ident(token: &Token) -> bool {
    if token.token_type != TokenType::Ident {
        return false;
    }
    let literal = &token.literal;
    let mut chars = literal.chars();
    // First char must be uppercase
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    // Must contain at least one lowercase letter (to distinguish from ALL_CAPS)
    literal.chars().any(|ch| ch.is_ascii_lowercase())
}

#[cfg(test)]
mod parser_test;
