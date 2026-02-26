use crate::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, EXPECTED_EXPRESSION, UNTERMINATED_BLOCK_COMMENT,
        UNTERMINATED_STRING, missing_comma,
        position::{Position, Span},
        unclosed_delimiter, unexpected_token,
    },
    syntax::{
        Identifier, block::Block, effect_expr::EffectExpr, expression::Expression,
        precedence::Precedence, statement::Statement, token_type::TokenType, type_expr::TypeExpr,
    },
};

use super::Parser;

const LIST_ERROR_LIMIT: usize = 50;
const DELIMITER_RECOVERY_BUDGET: usize = 256;

#[derive(Debug, Clone, Copy)]
pub(super) enum SyncMode {
    /// Synchronize at expression boundaries (commas, closers, EOF).
    Expr,
    /// Synchronize at statement boundaries (`;`, `}`, EOF).
    Stmt,
    /// Synchronize until the end of the current block (`}` or EOF).
    Block,
}

impl Parser {
    // Token navigation
    /// Advances the 3-token parser window, skipping over doc comment tokens.
    pub(super) fn next_token(&mut self) {
        let mut next = self.lexer.next_token();
        // Skip doc comments - they're lexed but not parsed
        while next.token_type == TokenType::DocComment {
            next = self.lexer.next_token();
        }
        self.current_token = std::mem::replace(
            &mut self.peek_token,
            std::mem::replace(&mut self.peek2_token, next),
        );
    }

    /// Returns `true` when `current_token` matches `token_type`.
    pub(super) fn is_current_token(&self, token_type: TokenType) -> bool {
        self.current_token.token_type == token_type
    }

    /// Returns `true` when `peek_token` matches `token_type`.
    pub(super) fn is_peek_token(&self, token_type: TokenType) -> bool {
        self.peek_token.token_type == token_type
    }

    /// Consumes `peek_token` when it matches `token_type`, otherwise emits a peek error.
    pub(super) fn expect_peek(&mut self, token_type: TokenType) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.peek_error(token_type);
            false
        }
    }

    /// Consumes `peek_token` only if it matches `token_type`.
    pub(super) fn consume_if_peek(&mut self, token_type: TokenType) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            false
        }
    }

    /// Returns the maximum number of list-related diagnostics emitted before bailing out.
    pub(super) fn list_error_limit(&self) -> usize {
        LIST_ERROR_LIMIT
    }

    /// Returns how many diagnostics were emitted since `diag_start`.
    pub(super) fn list_diag_count_since(&self, diag_start: usize) -> usize {
        self.errors.len().saturating_sub(diag_start)
    }

    /// Enforces the per-list diagnostic budget and synchronizes to `end` when exceeded.
    ///
    /// Returns `true` when parsing should stop for the current list.
    pub(super) fn check_list_error_limit(
        &mut self,
        diag_start: usize,
        end: TokenType,
        partial_reason: &str,
    ) -> bool {
        if self.list_diag_count_since(diag_start) < LIST_ERROR_LIMIT {
            return false;
        }

        let span = if self.peek_token.token_type != TokenType::Eof {
            self.peek_token.span()
        } else {
            self.current_token.span()
        };

        self.errors.push(unexpected_token(
            span,
            format!(
                "Too many errors in this {}; stopping after {} errors.",
                partial_reason, LIST_ERROR_LIMIT
            ),
        ));
        self.sync_to_list_end(end);
        true
    }

    // Span/position utilities
    /// Builds a span from `start` to the end position of `current_token`.
    pub(super) fn span_from(&self, start: Position) -> Span {
        Span::new(start, self.current_token.end_position)
    }

    /// Returns whether `token_type` ends an expression in the current grammar.
    pub(super) fn is_expression_terminator(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Semicolon
                | TokenType::Comma
                | TokenType::RParen
                | TokenType::RBracket
                | TokenType::RBrace
                | TokenType::Colon
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    /// Returns whether `token_type` can begin an expression.
    pub(super) fn token_starts_expression(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Ident
                | TokenType::Int
                | TokenType::Float
                | TokenType::String
                | TokenType::UnterminatedString
                | TokenType::InterpolationStart
                | TokenType::True
                | TokenType::False
                | TokenType::None
                | TokenType::Some
                | TokenType::Left
                | TokenType::Right
                | TokenType::Bang
                | TokenType::Minus
                | TokenType::LParen
                | TokenType::Hash
                | TokenType::LBracket
                | TokenType::LBrace
                | TokenType::If
                | TokenType::Do
                | TokenType::Fn
                | TokenType::Match
                | TokenType::Backslash
        )
    }

    /// Returns whether `token_type` can begin a statement.
    pub(super) fn token_starts_statement(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Let
                | TokenType::Return
                | TokenType::Fn
                | TokenType::Import
                | TokenType::Module
        )
    }

    /// Attempts delimiter-aware recovery until `expected` is found at top level.
    ///
    /// Returns `true` if `expected` was consumed, or `false` if recovery stopped
    /// at a boundary token from `stop_on` (or another hard boundary).
    pub(super) fn recover_to_matching_delimiter(
        &mut self,
        expected: TokenType,
        stop_on: &[TokenType],
    ) -> bool {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;
        let mut scanned = 0usize;

        while self.peek_token.token_type != TokenType::Eof && scanned < DELIMITER_RECOVERY_BUDGET {
            let token_type = self.peek_token.token_type;
            let at_top_level = paren_depth == 0 && bracket_depth == 0 && brace_depth == 0;

            if at_top_level && token_type == expected {
                self.next_token(); // consume expected delimiter
                return true;
            }

            if at_top_level
                && (stop_on.contains(&token_type)
                    || token_type == TokenType::RBrace
                    || token_type == TokenType::Eof
                    || token_type == TokenType::Semicolon
                    || self.token_starts_statement(token_type))
            {
                return false;
            }

            match token_type {
                TokenType::LParen => paren_depth += 1,
                TokenType::LBracket => bracket_depth += 1,
                TokenType::LBrace => brace_depth += 1,
                TokenType::RParen => paren_depth = paren_depth.saturating_sub(1),
                TokenType::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenType::RBrace => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }

            self.next_token();
            scanned += 1;
        }

        false
    }

    /// Advances tokens until the parser reaches a boundary appropriate for `mode`.
    pub(super) fn synchronize(&mut self, mode: SyncMode) {
        while !self.is_current_token(TokenType::Eof) {
            let token_type = self.current_token.token_type;
            let at_boundary = match mode {
                SyncMode::Expr => matches!(
                    token_type,
                    TokenType::Comma
                        | TokenType::Semicolon
                        | TokenType::RParen
                        | TokenType::RBracket
                        | TokenType::RBrace
                        | TokenType::Arrow
                        | TokenType::Eof
                ),
                SyncMode::Stmt => {
                    matches!(
                        token_type,
                        TokenType::Semicolon | TokenType::RBrace | TokenType::Eof
                    )
                }
                SyncMode::Block => matches!(token_type, TokenType::RBrace | TokenType::Eof),
            };

            if at_boundary {
                break;
            }

            self.next_token();
        }
    }

    // Complex parsing helpers
    /// Parses either a simple identifier or a dotted qualified identifier.
    pub(super) fn parse_qualified_name(&mut self) -> Option<Identifier> {
        let first_sym = self
            .current_token
            .symbol
            .expect("ident token should have symbol");
        if !self.is_peek_token(TokenType::Dot) {
            return Some(first_sym);
        }

        // Build dotted name, then intern the whole thing
        let mut name = self.current_token.literal.to_string();
        while self.is_peek_token(TokenType::Dot) {
            self.next_token(); // consume '.'
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            name.push('.');
            name.push_str(&self.current_token.literal);
        }
        Some(self.lexer.interner_mut().intern(&name))
    }

    /// Parses an untyped function parameter list inside `(` ... `)`.
    ///
    /// Supports trailing commas and delimiter-aware recovery.
    pub(super) fn parse_function_parameters(&mut self) -> Option<Vec<Identifier>> {
        debug_assert!(self.is_current_token(TokenType::LParen));
        let mut identifiers = Vec::new();
        let diag_start = self.errors.len();

        // Empty list: ()
        if self.consume_if_peek(TokenType::RParen) {
            return Some(identifiers);
        }

        loop {
            // Move to parameter candidate token
            self.next_token();

            // Allow trailing comma: fn f(a, ) { ... }
            if self.is_current_token(TokenType::RParen) {
                return Some(identifiers);
            }

            // Parse identifier or recover (recovery stops on Comma/RParen/Eof)
            if let Some(param) = self.parse_parameter_identifier_or_recover() {
                identifiers.push(param);
                self.next_token(); // move to delimiter after a valid parameter
            }
            if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list") {
                return Some(identifiers);
            }
            // else: current_token is already at delimiter due to recovery

            match self.current_token.token_type {
                TokenType::Comma => {
                    // Allow trailing comma in parameter lists.
                    if self.consume_if_peek(TokenType::RParen) {
                        return Some(identifiers);
                    }

                    // Continue with next parameter (comma is current delimiter; next loop consumes next token)
                    continue;
                }

                TokenType::RParen | TokenType::Eof => return Some(identifiers),

                _ => {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        format!(
                            "Expected `,` or `)` after parameter, got {}.",
                            self.current_token.token_type
                        ),
                    ));
                    if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list")
                    {
                        return Some(identifiers);
                    }

                    // Recover to a safe delimiter
                    while !matches!(
                        self.current_token.token_type,
                        TokenType::Comma | TokenType::RParen | TokenType::Eof
                    ) {
                        self.next_token();
                    }

                    // If we recovered to comma, attempt to continue parsing more params.
                    if self.current_token.token_type == TokenType::Comma {
                        continue;
                    }

                    return Some(identifiers);
                }
            }
        }
    }

    /// Parses a function parameter list where each parameter may include
    /// an optional type annotation (`name: Type`).
    ///
    /// Returns a tuple of `(parameter_names, parameter_types)` where
    /// `parameter_types[i]` corresponds to `parameter_names[i]`.
    pub(super) fn parse_typed_function_parameters(
        &mut self,
    ) -> Option<(Vec<Identifier>, Vec<Option<TypeExpr>>)> {
        debug_assert!(self.is_current_token(TokenType::LParen));

        let mut identifiers = Vec::new();
        let mut types = Vec::new();
        let diag_start = self.errors.len();

        if self.consume_if_peek(TokenType::RParen) {
            return Some((identifiers, types));
        }

        loop {
            self.next_token();

            if self.is_current_token(TokenType::RParen) {
                return Some((identifiers, types));
            }

            if let Some(param) = self.parse_parameter_identifier_or_recover() {
                let mut type_annotation = None;
                if self.is_peek_token(TokenType::Colon) {
                    self.next_token(); // :
                    self.next_token(); // start of type
                    type_annotation = self.parse_type_expr();
                }

                identifiers.push(param);
                types.push(type_annotation);
                self.next_token();
            }

            if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list") {
                return Some((identifiers, types));
            }

            match self.current_token.token_type {
                TokenType::Comma => {
                    if self.consume_if_peek(TokenType::RParen) {
                        return Some((identifiers, types));
                    }
                    continue;
                }
                TokenType::RParen | TokenType::Eof => return Some((identifiers, types)),
                _ => {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        format!(
                            "Expected `,` or `)` after parameter, got {}.",
                            self.current_token.token_type
                        ),
                    ));
                    if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list")
                    {
                        return Some((identifiers, types));
                    }
                    while !matches!(
                        self.current_token.token_type,
                        TokenType::Comma | TokenType::RParen | TokenType::Eof
                    ) {
                        self.next_token();
                    }
                    if self.current_token.token_type == TokenType::Comma {
                        continue;
                    }
                    return Some((identifiers, types));
                }
            }
        }
    }

    /// Parses an optional effect list in the form `with EffectA, EffectB`.
    ///
    /// If no `with` keyword is present, returns an empty effect list.
    pub(super) fn parse_effect_list(&mut self) -> Option<Vec<EffectExpr>> {
        if !self.is_peek_token(TokenType::With) {
            return Some(Vec::new());
        }

        self.next_token(); // with
        let mut effects = Vec::new();

        loop {
            let effect = self.parse_effect_expr()?;
            effects.push(effect);

            if self.is_peek_token(TokenType::Comma) {
                self.next_token();
                continue;
            }
            break;
        }

        Some(effects)
    }

    fn parse_effect_expr(&mut self) -> Option<EffectExpr> {
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");
        let mut expr = EffectExpr::Named {
            name,
            span: self.current_token.span(),
        };

        loop {
            let token = if self.is_peek_token(TokenType::Plus) {
                Some(TokenType::Plus)
            } else if self.is_peek_token(TokenType::Minus) {
                Some(TokenType::Minus)
            } else {
                None
            };

            let Some(operator) = token else {
                break;
            };

            self.next_token(); // operator
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }

            let rhs_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");
            let rhs = EffectExpr::Named {
                name: rhs_name,
                span: self.current_token.span(),
            };
            let span = Span::new(expr.span().start, rhs.span().end);
            expr = match operator {
                TokenType::Plus => EffectExpr::Add {
                    left: Box::new(expr),
                    right: Box::new(rhs),
                    span,
                },
                TokenType::Minus => EffectExpr::Subtract {
                    left: Box::new(expr),
                    right: Box::new(rhs),
                    span,
                },
                _ => unreachable!("only + or - are parsed here"),
            };
        }

        Some(expr)
    }

    /// Parses a type expression, including function types with optional effects.
    ///
    /// Function type arrows are right-associative, so `A -> B -> C` parses as
    /// `A -> (B -> C)`.
    pub(super) fn parse_type_expr(&mut self) -> Option<TypeExpr> {
        let start = self.current_token.position;
        let left = self.parse_non_function_type()?;

        if !self.is_peek_token(TokenType::Arrow) {
            return Some(left);
        }

        let params = match left {
            TypeExpr::Tuple { elements, .. } => elements,
            other => vec![other],
        };

        self.next_token(); // ->
        self.next_token(); // return type start
        let ret = self.parse_type_expr()?;
        let effects = self.parse_effect_list()?;

        Some(TypeExpr::Function {
            params,
            ret: Box::new(ret),
            effects,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parses a non-function type atom (`Ident` or parenthesized type/tuple type).
    fn parse_non_function_type(&mut self) -> Option<TypeExpr> {
        match self.current_token.token_type {
            TokenType::Ident => self.parse_named_type_expr(),
            TokenType::LParen => self.parse_paren_type_expr(),
            TokenType::None => {
                // Allow `None` as a type annotation (void return type)
                let start = self.current_token.position;
                let name = self.lexer.interner_mut().intern("None");
                let end = self.current_token.end_position;
                Some(TypeExpr::Named {
                    name,
                    args: vec![],
                    span: Span::new(start, end),
                })
            }
            _ => {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "Expected a type expression, got {}.",
                        self.current_token.token_type
                    ),
                ));
                None
            }
        }
    }

    /// Parses a named type, including optional generic arguments (`Name<T, U>`).
    fn parse_named_type_expr(&mut self) -> Option<TypeExpr> {
        let start = self.current_token.position;
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        let mut args = Vec::new();

        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // <
            if self.consume_if_peek(TokenType::Gt) {
                return Some(TypeExpr::Named {
                    name,
                    args,
                    span: Span::new(start, self.current_token.end_position),
                });
            }

            loop {
                self.next_token();
                args.push(self.parse_type_expr()?);

                if self.is_peek_token(TokenType::Comma) {
                    self.next_token();
                    continue;
                }
                break;
            }

            if !self.expect_peek(TokenType::Gt) {
                return None;
            }
        }

        Some(TypeExpr::Named {
            name,
            args,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parses either:
    /// - a grouped type `(T)`, or
    /// - a tuple type `(T, U, ...)`.
    fn parse_paren_type_expr(&mut self) -> Option<TypeExpr> {
        let start = self.current_token.position;
        if self.consume_if_peek(TokenType::RParen) {
            return Some(TypeExpr::Tuple {
                elements: vec![],
                span: Span::new(start, self.current_token.end_position),
            });
        }

        self.next_token();
        let first = self.parse_type_expr()?;
        if !self.is_peek_token(TokenType::Comma) {
            if !self.expect_peek(TokenType::RParen) {
                return None;
            }
            return Some(first);
        }

        let mut elements = vec![first];

        while self.is_peek_token(TokenType::Comma) {
            self.next_token(); // comma
            if self.is_peek_token(TokenType::RParen) {
                break;
            }

            self.next_token();
            elements.push(self.parse_type_expr()?);
        }

        if !self.expect_peek(TokenType::RParen) {
            return None;
        }

        Some(TypeExpr::Tuple {
            elements,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parses a block body until `}`/`EOF`, including post-expression `where` clauses.
    pub(super) fn parse_block(&mut self) -> Block {
        let start = self.current_token.position;
        let mut statements = Vec::new();
        self.next_token();

        while !self.is_current_token(TokenType::RBrace)
            && !self.is_current_token(TokenType::Eof)
            && !self.is_current_token(TokenType::Where)
        {
            if let Some(statement) = self.parse_statement() {
                statements.push(statement);
            }
            self.next_token();
        }

        // `where x = val ...` — collect bindings and reorder before the body expression
        if self.is_current_token(TokenType::Where) {
            let where_bindings = self.parse_where_clauses();
            // The body expression is the last statement; inject bindings before it
            if let Some(body) = statements.pop() {
                statements.extend(where_bindings);
                statements.push(body);
            } else {
                // `where` without a preceding expression — emit an error, still collect bindings
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    "`where` must follow an expression",
                ));
                statements.extend(where_bindings);
            }
        }

        // Detect unclosed block: reached EOF without finding closing `}`
        if self.is_current_token(TokenType::Eof) && !self.reported_unclosed_brace {
            self.reported_unclosed_brace = true;
            self.errors
                .push(unclosed_delimiter(Span::new(start, start), "{", "}", None));
        }

        Block {
            statements,
            span: Span::new(start, self.current_token.end_position),
        }
    }

    /// Parse one or more `where ident = expr` clauses, returning them as `Let` statements.
    /// Called when `current_token` is `Where`. Leaves `current_token` at `}` or `EOF`.
    fn parse_where_clauses(&mut self) -> Vec<Statement> {
        let mut bindings = Vec::new();

        while self.is_current_token(TokenType::Where) {
            let clause_start = self.current_token.position;
            self.next_token(); // consume `where`

            if !self.is_current_token(TokenType::Ident) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    "expected identifier after `where`",
                ));
                break;
            }

            let name: Identifier = self
                .current_token
                .symbol
                .expect("ident token must have symbol");
            self.next_token(); // consume identifier

            if !self.is_current_token(TokenType::Assign) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    "expected `=` after where binding name",
                ));
                break;
            }
            self.next_token(); // consume `=`

            let value = match self.parse_expression(Precedence::Lowest) {
                Some(v) => v,
                None => break,
            };

            let clause_end = self.current_token.end_position;
            bindings.push(Statement::Let {
                name,
                type_annotation: None,
                value,
                span: Span::new(clause_start, clause_end),
            });

            self.next_token(); // advance to next `where` or `}`
        }

        bindings
    }

    /// Parses a comma-separated expression list terminated by `end`.
    ///
    /// `open_pos` is used for diagnostics when the closing delimiter is missing.
    pub(super) fn parse_expression_list(
        &mut self,
        end: TokenType,
        open_pos: Position,
    ) -> Option<Vec<Expression>> {
        if self.consume_if_peek(end) {
            return Some(vec![]);
        }
        self.parse_expression_list_core(vec![], end, true, open_pos)
    }

    /// Like `parse_expression_list`, but the first element has already been
    /// parsed by the caller. Provides identical error recovery (missing-comma
    /// detection, error limiting, etc.).
    pub(super) fn parse_expression_list_with_first(
        &mut self,
        first: Expression,
        end: TokenType,
        open_pos: Position,
    ) -> Option<Vec<Expression>> {
        self.parse_expression_list_core(vec![first], end, false, open_pos)
    }

    /// Core loop for comma-separated expression lists.
    ///
    /// When `advance_first` is true, the loop starts by advancing and parsing
    /// the first expression (normal path). When false, the first element is
    /// already in `list` and the loop starts at the "after expression" phase.
    fn parse_expression_list_core(
        &mut self,
        mut list: Vec<Expression>,
        end: TokenType,
        advance_first: bool,
        open_pos: Position,
    ) -> Option<Vec<Expression>> {
        let mut last_missing_comma_at = None;
        let diag_start = self.errors.len();
        let mut need_advance = advance_first;

        loop {
            // === Phase A: advance to next token and parse expression ===
            if need_advance {
                self.next_token();

                // Allow trailing comma in list contexts: f(a, ), [a, ], ...
                if self.is_current_token(end) {
                    return Some(list);
                }

                if self.is_current_token(TokenType::Comma) {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        "Expected expression after `,`, got `,`.",
                    ));
                    if self.check_list_error_limit(diag_start, end, "list") {
                        return Some(list);
                    }
                    continue;
                }

                if self.is_current_token(TokenType::Eof) {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        format!("Expected `{}` before end of file.", end),
                    ));
                    if self.check_list_error_limit(diag_start, end, "list") {
                        return Some(list);
                    }
                    return None;
                }

                list.push(self.parse_expression(Precedence::Lowest)?);
            }
            need_advance = true;

            // === Phase B: handle separator after expression ===
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma

                if self.consume_if_peek(end) {
                    return Some(list);
                }

                continue;
            }

            if self.consume_if_peek(end) {
                return Some(list);
            }

            // Dangling member access already emitted a targeted error in
            // `parse_member_access` (e.g. `point.\nnext`), so skip emitting
            // a second generic list-delimiter error here.
            if self.current_token.token_type == TokenType::Dot
                && self.peek_token.position.line > self.current_token.end_position.line
            {
                return Some(list);
            }

            // Adjacent expression-starting token inside a delimited list strongly
            // indicates a missing comma: f(a b), [a b], etc.
            if self.token_starts_expression(self.peek_token.token_type)
                && !self.token_can_continue_expression(self.peek_token.token_type)
                && !self.likely_missing_closing_delimiter(end)
            {
                let (context, example) = match end {
                    TokenType::RParen => ("arguments", "`f(a, b)`"),
                    TokenType::RBracket => ("items", "`[a, b]`"),
                    _ => ("items", "`a, b`"),
                };
                let missing_comma_at = self.peek_token.position;
                if last_missing_comma_at != Some(missing_comma_at) {
                    self.errors
                        .push(missing_comma(self.peek_token.span(), context, example));
                    last_missing_comma_at = Some(missing_comma_at);
                    if self.check_list_error_limit(diag_start, end, "list") {
                        return Some(list);
                    }
                }
                // Pretend a comma existed and continue parsing the next list item.
                continue;
            }

            // Heuristic: if the next token is on a different line or starts
            // a statement, the closing delimiter was likely forgotten.
            // Point the error at the opening delimiter instead of the
            // unexpected token (Rust-style).
            if self.likely_missing_closing_delimiter(end) {
                let (open, close) = Self::delimiter_chars(end);
                self.errors.push(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    Some(self.peek_token.span()),
                ));
                return Some(list);
            }

            let context = match end {
                TokenType::RParen => "argument",
                TokenType::RBracket => "array element",
                _ => "item",
            };
            self.errors.push(unexpected_token(
                self.peek_token.span(),
                format!(
                    "Expected `,` or `{}` after {}, got {}.",
                    end, context, self.peek_token.token_type
                ),
            ));
            if self.check_list_error_limit(diag_start, end, "list") {
                return Some(list);
            }

            if self.recover_to_matching_delimiter(end, &[TokenType::Comma]) {
                return Some(list);
            }

            if self.is_peek_token(TokenType::Comma) {
                self.next_token();

                if self.consume_if_peek(end) {
                    return Some(list);
                }

                continue;
            }

            if self.consume_if_peek(end) {
                return Some(list);
            }

            // If recovery stopped at a statement boundary, the closing
            // delimiter was simply forgotten. Emit unclosed-delimiter error
            // pointing at the opener (Rust-style).
            if matches!(
                self.peek_token.token_type,
                TokenType::Semicolon
                    | TokenType::Let
                    | TokenType::Fn
                    | TokenType::Import
                    | TokenType::Module
            ) {
                let (open, close) = Self::delimiter_chars(end);
                self.errors.push(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    Some(self.peek_token.span()),
                ));
                return Some(list);
            }

            let message = if self.peek_token.token_type == TokenType::Eof {
                let (open, close) = Self::delimiter_chars(end);
                self.errors.push(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    None,
                ));
                return None;
            } else {
                format!("Expected `,` or `{}` in expression list.", end)
            };
            self.errors
                .push(unexpected_token(self.peek_token.span(), message));
            if self.check_list_error_limit(diag_start, end, "list") {
                return Some(list);
            }
            return None;
        }
    }

    /// Returns whether `token_type` can legally continue an already-started expression.
    fn token_can_continue_expression(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::LParen
                | TokenType::LBracket
                | TokenType::Dot
                | TokenType::Pipe
                | TokenType::Or
                | TokenType::And
                | TokenType::Eq
                | TokenType::NotEq
                | TokenType::Lt
                | TokenType::Gt
                | TokenType::Lte
                | TokenType::Gte
                | TokenType::Plus
                | TokenType::Minus
                | TokenType::Asterisk
                | TokenType::Slash
                | TokenType::Percent
        )
    }

    /// Returns the opening and closing delimiter strings associated with `end`.
    fn delimiter_chars(end: TokenType) -> (&'static str, &'static str) {
        match end {
            TokenType::RParen => ("(", ")"),
            TokenType::RBracket => ("[", "]"),
            _ => ("{", "}"),
        }
    }

    /// Heuristic for deciding whether a closing delimiter was likely omitted.
    fn likely_missing_closing_delimiter(&self, end: TokenType) -> bool {
        // In call argument lists, a newline before the next expression-starter
        // is more likely to be a forgotten closing ')' than a missing comma.
        if end == TokenType::RParen
            && self.peek_token.position.line > self.current_token.end_position.line
        {
            return true;
        }

        matches!(
            self.peek_token.token_type,
            TokenType::Semicolon
                | TokenType::RBrace
                | TokenType::Let
                | TokenType::Fn
                | TokenType::Return
                | TokenType::Import
                | TokenType::Module
        )
    }

    /// Fast-forwards to the matching list terminator `end` while tracking
    /// nested delimiters.
    pub(super) fn sync_to_list_end(&mut self, end: TokenType) {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        while self.peek_token.token_type != TokenType::Eof {
            let at_top_level = paren_depth == 0 && bracket_depth == 0 && brace_depth == 0;
            let token_type = self.peek_token.token_type;

            if at_top_level && token_type == end {
                self.next_token(); // consume end delimiter
                return;
            }

            match token_type {
                TokenType::LParen => paren_depth += 1,
                TokenType::LBracket => bracket_depth += 1,
                TokenType::LBrace => brace_depth += 1,
                TokenType::RParen => paren_depth = paren_depth.saturating_sub(1),
                TokenType::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenType::RBrace => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }

            self.next_token();
        }
    }

    // Error handling
    /// Emits an "expected expression" diagnostic for the current token.
    pub(super) fn no_prefix_parse_error(&mut self) {
        if self.current_token.token_type == TokenType::Let {
            let diag = Diagnostic::make_error(
                &EXPECTED_EXPRESSION,
                &[&self.current_token.token_type.to_string()],
                String::new(),
                self.current_token.span(),
            )
            .with_message("`let` is a statement and cannot appear in an expression position.")
            .with_hint_text(
                "Move `let` above this expression, or use a block body `{ ... }` if this construct supports it. \
For lambdas, write `\\x -> { let y = ...; y }`. Match arms require an expression; extract the `let` before `match` \
(or use a block only if match arms support it).",
            );
            self.errors.push(diag);
            return;
        }

        let error_spec = &EXPECTED_EXPRESSION;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&self.current_token.token_type.to_string()],
            String::new(), // No file context in parser
            self.current_token.span(),
        );
        self.errors.push(diag);
    }

    /// Emits the lexer-driven unterminated-string diagnostic and synchronizes.
    pub(super) fn unterminated_string_error(&mut self) {
        // Unterminated strings are a lexical error; use the lexer-provided end position
        // where the closing quote should have appeared.
        let token_span = self.current_token.span();

        let error_spec = &UNTERMINATED_STRING;
        let diag = Diagnostic::make_error(
            error_spec,
            &[],           // No message formatting args needed
            String::new(), // No file context in parser
            token_span,
        );
        self.errors.push(diag);
        self.synchronize_after_error();
    }

    /// Emits the lexer-driven unterminated-block-comment diagnostic and synchronizes.
    pub(super) fn unterminated_block_comment_error(&mut self) {
        let token_span = self.current_token.span();
        let error_spec = &UNTERMINATED_BLOCK_COMMENT;
        let diag = Diagnostic::make_error(
            error_spec,
            &[],           // No message formatting args needed
            String::new(), // No file context in parser
            token_span,
        );
        self.errors.push(diag);
        self.synchronize_after_error();
    }

    /// Statement-level synchronization entry point used after parse errors.
    pub(super) fn synchronize_after_error(&mut self) {
        self.synchronize(SyncMode::Stmt);
    }

    /// Emits a diagnostic for an unexpected `peek_token`.
    pub(super) fn peek_error(&mut self, expected: TokenType) {
        self.errors.push(unexpected_token(
            self.peek_token.span(),
            format!("Expected {}, got {}.", expected, self.peek_token.token_type),
        ));
    }

    /// Validates that `current_token` is an identifier parameter and returns its symbol.
    pub(super) fn validate_parameter_identifier(&mut self) -> Option<Identifier> {
        if self.current_token.token_type == TokenType::Ident {
            Some(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            )
        } else {
            self.errors.push(unexpected_token(
                self.current_token.span(),
                format!(
                    "Expected identifier as parameter, got {}.",
                    self.current_token.token_type
                ),
            ));
            None
        }
    }

    /// Parses a parameter identifier or recovers to the next parameter delimiter.
    fn parse_parameter_identifier_or_recover(&mut self) -> Option<Identifier> {
        if let Some(identifier) = self.validate_parameter_identifier() {
            return Some(identifier);
        }

        while !matches!(
            self.current_token.token_type,
            TokenType::Comma | TokenType::RParen | TokenType::Eof
        ) {
            self.next_token();
        }

        None
    }
}
