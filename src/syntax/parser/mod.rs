use crate::{
    diagnostics::{
        Diagnostic,
        position::{Position, Span},
        unexpected_token,
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

#[derive(Debug, Clone, Copy)]
pub(super) enum RecoveryBoundary {
    Statement,
    NextLineOrBlock,
    MissingBlockOpener,
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
                self.errors.push(unexpected_token(
                    self.current_token.span(),
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
            self.errors.push(diag);
            true
        }
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
