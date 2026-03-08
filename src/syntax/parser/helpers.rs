use crate::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, DiagnosticCategory, EXPECTED_EXPRESSION,
        UNTERMINATED_BLOCK_COMMENT, UNTERMINATED_STRING, missing_comma, missing_lambda_close_paren,
        position::{Position, Span},
        text_similarity::levenshtein_distance,
        unclosed_delimiter, unexpected_token, unexpected_token_with_details,
    },
    syntax::{
        Identifier, block::Block, effect_expr::EffectExpr, expression::Expression,
        precedence::Precedence, statement::Statement, token_type::TokenType, type_expr::TypeExpr,
    },
};

use super::{Parser, RecoveryBoundary};

const LIST_ERROR_LIMIT: usize = 50;
const DELIMITER_RECOVERY_BUDGET: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParameterListContext {
    Function,
    Lambda,
}

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
    fn parser_identifier_typo_hint(&self, name: &str) -> Option<String> {
        const KEYWORDS: &[&str] = &[
            "let", "fn", "if", "else", "match", "handle", "perform", "return", "module", "import",
            "effect", "data", "type", "with", "do",
        ];
        const BUILTINS: &[&str] = &[
            "print",
            "println",
            "read_file",
            "read_lines",
            "read_stdin",
            "to_string",
            "to_int",
            "to_float",
            "len",
            "map",
            "filter",
            "fold",
            "range",
        ];

        if name.len() < 3 {
            return None;
        }

        let mut best: Option<(&str, usize)> = None;
        for candidate in KEYWORDS.iter().chain(BUILTINS.iter()) {
            let distance = levenshtein_distance(&name.to_lowercase(), &candidate.to_lowercase());
            if distance > 2 {
                continue;
            }
            match best {
                Some((_, best_distance)) if distance >= best_distance => {}
                _ => best = Some((candidate, distance)),
            }
        }

        best.map(|(candidate, _)| format!("did you mean `{candidate}`?"))
    }

    pub(super) fn describe_token_type_for_diagnostic(&self, token_type: TokenType) -> String {
        match token_type {
            TokenType::Ident => "an identifier".to_string(),
            TokenType::Int => "an integer literal".to_string(),
            TokenType::Float => "a float literal".to_string(),
            TokenType::String => "a string literal".to_string(),
            TokenType::Eof => "the end of the file".to_string(),
            TokenType::Illegal => "an invalid token".to_string(),
            TokenType::UnterminatedString => "an unterminated string literal".to_string(),
            TokenType::UnterminatedBlockComment => "an unterminated block comment".to_string(),
            other => format!("`{other}`"),
        }
    }

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

    /// Consumes `peek_token` when it matches `token_type`, otherwise emits a
    /// contextual unexpected-token diagnostic with a stable help hint.
    pub(super) fn expect_peek_context(
        &mut self,
        token_type: TokenType,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.emit_parser_diagnostic(
                unexpected_token(self.peek_token.span(), message.into())
                    .with_hint_text(hint.into()),
            );
            false
        }
    }

    /// Contextual `expect_peek` variant for parser errors that need an
    /// explicit display title and category.
    pub(super) fn expect_peek_context_with_details(
        &mut self,
        token_type: TokenType,
        display_title: impl Into<String>,
        category: DiagnosticCategory,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.peek_token.span(),
                    display_title.into(),
                    category,
                    message.into(),
                )
                .with_hint_text(hint.into()),
            );
            false
        }
    }

    /// Contextual `expect_peek` variant that can derive message and hint from
    /// parser state at the failure site.
    pub(super) fn expect_peek_contextf<M, H>(
        &mut self,
        token_type: TokenType,
        message_fn: M,
        hint_fn: H,
    ) -> bool
    where
        M: FnOnce(&Self) -> String,
        H: FnOnce(&Self) -> String,
    {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.emit_parser_diagnostic(
                unexpected_token(self.peek_token.span(), message_fn(self))
                    .with_hint_text(hint_fn(self)),
            );
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

    pub(super) fn eof_anchor_span(&self, preferred: Span) -> Span {
        if self.peek_token.token_type == TokenType::Eof {
            preferred
        } else if self.current_token.token_type != TokenType::Eof {
            self.current_token.span()
        } else {
            self.peek_token.span()
        }
    }

    /// Runs a required parser step and synchronizes on failure.
    pub(super) fn parse_required<T, F>(&mut self, parse: F, sync_mode: SyncMode) -> Option<T>
    where
        F: FnOnce(&mut Parser) -> Option<T>,
    {
        match parse(self) {
            Some(value) => Some(value),
            None => {
                if let Some(boundary) = self.take_recovery_boundary() {
                    self.synchronize_recovery_boundary(boundary);
                } else {
                    self.synchronize(sync_mode);
                }
                None
            }
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

        self.emit_parser_diagnostic(unexpected_token(
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

    /// Returns whether `token_type` can begin a type expression.
    pub(super) fn token_starts_type(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Ident | TokenType::LParen | TokenType::None
        )
    }

    fn looks_like_type_annotation_start(&self) -> bool {
        match self.peek_token.token_type {
            TokenType::Ident => self
                .peek_token
                .literal
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase()),
            TokenType::LParen | TokenType::None => true,
            _ => false,
        }
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
                        TokenType::Semicolon
                            | TokenType::RParen
                            | TokenType::RBracket
                            | TokenType::RBrace
                            | TokenType::Eof
                    ) || self.token_starts_statement(self.peek_token.token_type)
                }
                SyncMode::Block => matches!(token_type, TokenType::RBrace | TokenType::Eof),
            };

            if at_boundary {
                break;
            }

            self.next_token();
        }
    }

    pub(super) fn set_recovery_boundary(&mut self, boundary: RecoveryBoundary) {
        self.pending_recovery_boundary = Some(boundary);
    }

    pub(super) fn take_recovery_boundary(&mut self) -> Option<RecoveryBoundary> {
        self.pending_recovery_boundary.take()
    }

    pub(super) fn synchronize_recovery_boundary(&mut self, boundary: RecoveryBoundary) {
        self.used_custom_recovery = true;
        match boundary {
            RecoveryBoundary::Statement => self.synchronize(SyncMode::Stmt),
            RecoveryBoundary::NextLineOrBlock => {
                while !self.is_current_token(TokenType::Eof) {
                    if matches!(
                        self.current_token.token_type,
                        TokenType::Semicolon | TokenType::RBrace | TokenType::Eof
                    ) {
                        break;
                    }

                    if self.peek_token.position.line > self.current_token.end_position.line
                        && (self.peek_token.token_type == TokenType::RBrace
                            || self.token_starts_statement(self.peek_token.token_type)
                            || self.token_starts_expression(self.peek_token.token_type))
                    {
                        break;
                    }

                    self.next_token();
                }
            }
            RecoveryBoundary::MissingBlockOpener => {
                let mut paren_depth = 0usize;
                let mut bracket_depth = 0usize;
                let mut brace_depth = 0usize;

                while !self.is_current_token(TokenType::Eof) {
                    if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                        if self.current_token.token_type == TokenType::RBrace {
                            self.suppress_top_level_rbrace_once = true;
                            break;
                        }

                        if self.peek_token.position.line > self.current_token.end_position.line
                            && (self.peek_token.token_type == TokenType::RBrace
                                || self.token_starts_statement(self.peek_token.token_type)
                                || self.token_starts_expression(self.peek_token.token_type))
                        {
                            if self.peek_token.token_type == TokenType::RBrace {
                                self.suppress_top_level_rbrace_once = true;
                                self.next_token();
                            }
                            break;
                        }
                    }

                    match self.current_token.token_type {
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
        }
        self.clear_structural_suppression();
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
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected identifier after `.` in qualified name.".to_string(),
                "Qualified names use `Module.Name`.".to_string(),
            ) {
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
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();

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
                    let followup = unexpected_token(
                        self.current_token.span(),
                        "Expected `,` or `)` after function parameter.",
                    )
                    .with_hint_text(
                        "Separate parameters with `,` and close the parameter list with `)`.",
                    );
                    if !self.push_followup_unless_structural_root(construct_checkpoint, followup) {
                        return Some(identifiers);
                    }
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
        context: ParameterListContext,
    ) -> Option<(Vec<Identifier>, Vec<Option<TypeExpr>>)> {
        debug_assert!(self.is_current_token(TokenType::LParen));

        let mut identifiers = Vec::new();
        let mut types = Vec::new();
        let diag_start = self.errors.len();
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();

        if self.consume_if_peek(TokenType::RParen) {
            return Some((identifiers, types));
        }

        loop {
            self.next_token();

            if self.is_current_token(TokenType::RParen) {
                return Some((identifiers, types));
            }

            if let Some(param) = self.parse_parameter_identifier_or_recover() {
                let param_name = self.lexer.interner().resolve(param).to_string();
                let type_annotation = self.parse_type_annotation_opt_with_missing_colon(
                    &[
                        TokenType::Comma,
                        TokenType::RParen,
                        TokenType::Arrow,
                        TokenType::With,
                        TokenType::LBrace,
                        TokenType::Eof,
                    ],
                    "function parameter",
                    Some(param_name.as_str()),
                );

                identifiers.push(param);
                types.push(type_annotation);
                // Move from the parameter/type tail token to the parameter-list delimiter.
                // Keep recovery anchors (e.g. `,` from malformed `a: , b`) in place.
                let current_is_delimiter = matches!(
                    self.current_token.token_type,
                    TokenType::Comma | TokenType::RParen | TokenType::Eof
                );
                if current_is_delimiter {
                    let peek_is_delimiter = matches!(
                        self.peek_token.token_type,
                        TokenType::Comma | TokenType::RParen | TokenType::Eof
                    );
                    if peek_is_delimiter {
                        self.next_token();
                    }
                } else {
                    self.next_token();
                }
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
                    if context == ParameterListContext::Lambda
                        && self.current_token.token_type == TokenType::Arrow
                    {
                        self.errors
                            .push(missing_lambda_close_paren(self.current_token.span()));
                    } else {
                        let followup = unexpected_token(
                            self.current_token.span(),
                            "Expected `,` or `)` after function parameter.",
                        )
                        .with_hint_text(
                            "Separate parameters with `,` and close the parameter list with `)`.",
                        );
                        if !self
                            .push_followup_unless_structural_root(construct_checkpoint, followup)
                        {
                            return Some((identifiers, types));
                        }
                    }
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
        if self.is_current_token(TokenType::With) {
            // already positioned at `with` (possible recovery path)
        } else if self.is_peek_token(TokenType::With) {
            self.next_token(); // with
        } else {
            return Some(Vec::new());
        }

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

    /// Parses an optional `: Type` annotation and recovers to top-level anchors
    /// on malformed type expressions.
    pub(super) fn parse_type_annotation_opt(&mut self, anchors: &[TokenType]) -> Option<TypeExpr> {
        if !self.is_peek_token(TokenType::Colon) {
            return None;
        }

        self.next_token(); // :
        self.next_token(); // start of type

        match self.parse_type_expr() {
            Some(ty) => Some(ty),
            None => {
                self.recover_to_type_anchor(anchors);
                None
            }
        }
    }

    /// Parses an optional type annotation and emits a targeted diagnostic when
    /// a likely `:` separator is missing (e.g. `let x Int = 1`).
    pub(super) fn parse_type_annotation_opt_with_missing_colon(
        &mut self,
        anchors: &[TokenType],
        context: &str,
        binding_name: Option<&str>,
    ) -> Option<TypeExpr> {
        if self.is_peek_token(TokenType::Colon) {
            return self.parse_type_annotation_opt(anchors);
        }

        if !self.looks_like_type_annotation_start() {
            return None;
        }

        let message = if let Some(name) = binding_name {
            format!(
                "Missing `:` in {} type annotation. Write it as `{name}: Type`.",
                context
            )
        } else {
            format!(
                "Missing `:` in {} type annotation. Write it as `name: Type`.",
                context
            )
        };
        self.errors
            .push(unexpected_token(self.peek_token.span(), message));

        self.next_token(); // move to start of type expression
        match self.parse_type_expr() {
            Some(ty) => Some(ty),
            None => {
                self.recover_to_type_anchor(anchors);
                None
            }
        }
    }

    /// Recovers malformed type parsing until `current_token` is a top-level anchor.
    /// The anchor token is not consumed.
    pub(super) fn recover_to_type_anchor(&mut self, anchors: &[TokenType]) {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;
        let mut scanned = 0usize;

        while self.current_token.token_type != TokenType::Eof && scanned < DELIMITER_RECOVERY_BUDGET
        {
            let token_type = self.current_token.token_type;
            let at_top_level = paren_depth == 0 && bracket_depth == 0 && brace_depth == 0;

            if at_top_level
                && (anchors.contains(&token_type)
                    || token_type == TokenType::Semicolon
                    || token_type == TokenType::RBrace
                    || token_type == TokenType::Eof
                    || self.token_starts_statement(token_type))
            {
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
            scanned += 1;
        }
    }

    /// Parses an effect expression used in `with` clauses.
    ///
    /// Supported forms are concrete atoms (`IO`), row tails (`|e`), and left-associative
    /// arithmetic via `+` / `-`. At most one explicit row tail is allowed per expression.
    ///
    /// Lowercase identifiers are rejected unless introduced by `|`; implicit row variables
    /// are intentionally disallowed.
    fn parse_effect_expr(&mut self) -> Option<EffectExpr> {
        let is_lowercase_ident = |literal: &str| {
            literal
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase())
        };

        let mut saw_row_tail;
        let mut expr = if self.is_peek_token(TokenType::Bar) {
            self.next_token(); // |
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected row variable name after `|` in effect expression. ".to_string(),
                "Write row tails as `| e` where `e` is a lowercase identifier.".to_string(),
            ) {
                return None;
            }

            if !is_lowercase_ident(&self.current_token.literal) {
                self.emit_parser_diagnostic(
                    unexpected_token(
                        self.current_token.span(),
                        "Effect row tail variables must be lowecase identifiers.".to_string(),
                    )
                    .with_hint_text("Use a lowercase tail name, for example `with |e`."),
                );
                return None;
            }
            saw_row_tail = true;

            EffectExpr::RowVar {
                name: self
                    .current_token
                    .symbol
                    .expect("ident token should have symbol"),
                span: self.current_token.span(),
            }
        } else {
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected effect name after in effect expression. ".to_string(),
                "Effect expressions use names like `IO`, `IO + Net`, or `|e`.".to_string(),
            ) {
                return None;
            }

            if is_lowercase_ident(&self.current_token.literal) {
                self.emit_parser_diagnostic(
                    unexpected_token(
                        self.current_token.span(),
                        "Implicit row variables are no longer supported in `with` clauses."
                            .to_string(),
                    )
                    .with_hint_text("Rewrite using an explicit tail variable: `with |e`."),
                );
                return None;
            }

            saw_row_tail = false;

            let name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            EffectExpr::Named {
                name,
                span: self.current_token.span(),
            }
        };

        loop {
            if self.is_peek_token(TokenType::Bar) {
                if saw_row_tail {
                    self.emit_parser_diagnostic(
                        unexpected_token(
                            self.current_token.span(),
                            "Effect expressions can contain only one row variable tail."
                                .to_string(),
                        )
                        .with_hint_text("Use a single tail variable, for example `with |e`."),
                    );
                    return None;
                }
                saw_row_tail = true;
                self.next_token(); // |
                if !self.expect_peek_context(
                    TokenType::Ident,
                    "Expected row variable name after `|` in effect expression.".to_string(),
                    "Write row tails as `... | e` where `e` is a lowercase identifier.".to_string(),
                ) {
                    return None;
                }

                if !is_lowercase_ident(&self.current_token.literal) {
                    self.emit_parser_diagnostic(
                        unexpected_token(
                            self.current_token.span(),
                            "Effect row tail variables must be lowercase identifiers.".to_string(),
                        )
                        .with_hint_text("Use a lowercase tail name, for example `with IO | e`."),
                    );
                    return None;
                }

                let tail = EffectExpr::RowVar {
                    name: self
                        .current_token
                        .symbol
                        .expect("ident token should have symbol"),
                    span: self.current_token.span(),
                };

                let span = Span::new(expr.span().start, tail.span().end);
                expr = EffectExpr::Add {
                    left: Box::new(expr),
                    right: Box::new(tail),
                    span,
                };
                continue;
            }

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

            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected effect name after effect operator.".to_string(),
                "Effect expressions use `Left + Right`, `Left - Right`, and optional `| e` tails."
                    .to_string(),
            ) {
                return None;
            }

            if is_lowercase_ident(&self.current_token.literal) {
                self.emit_parser_diagnostic(
                    unexpected_token(
                        self.current_token.span(),
                        "Implicit row variables are no longer supported in `with` clauses."
                            .to_string(),
                    )
                    .with_hint_text("Rewrite using an explicit tail variable: `with |e`."),
                );
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
                self.emit_parser_diagnostic(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "I was expecting a type here, but I found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                ));
                // For annotation contexts, callers may already anchor recovery on
                // tokens like `=`/`with`/`{`; avoid consuming those anchors here.
                let at_safe_anchor = matches!(
                    self.current_token.token_type,
                    TokenType::Assign
                        | TokenType::Semicolon
                        | TokenType::Comma
                        | TokenType::RParen
                        | TokenType::RBracket
                        | TokenType::RBrace
                        | TokenType::Arrow
                        | TokenType::With
                        | TokenType::LBrace
                        | TokenType::Eof
                );
                if !at_safe_anchor {
                    self.synchronize(SyncMode::Expr);
                }
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

            if !self.expect_peek_context(
                TokenType::Gt,
                "Expected `>` to close generic type arguments.".to_string(),
                "Generic types use `Name<T, U>`.".to_string(),
            ) {
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
            if !self.expect_peek_context_with_details(
                TokenType::RParen,
                "Missing Closing Delimiter",
                DiagnosticCategory::ParserDelimiter,
                "Expected `)` to close parenthesized type.".to_string(),
                "Grouped types use `(Type)`.".to_string(),
            ) {
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

        if !self.expect_peek_context_with_details(
            TokenType::RParen,
            "Missing Closing Delimiter",
            DiagnosticCategory::ParserDelimiter,
            "Expected `)` to close tuple type.".to_string(),
            "Tuple types use `(A, B, ...)`.".to_string(),
        ) {
            return None;
        }

        Some(TypeExpr::Tuple {
            elements,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parses a block body until `}`/`EOF`, including post-expression `where` clauses.
    pub(super) fn parse_block(&mut self) -> Block {
        self.parse_block_with_context(None)
    }

    /// Like `parse_block`, but accepts an optional context label (e.g. a function
    /// name) to produce richer "unclosed delimiter" diagnostics.
    pub(super) fn parse_block_with_context(&mut self, context: Option<&str>) -> Block {
        let start = self.current_token.position;
        let mut statements = Vec::new();
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();
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
                self.emit_parser_diagnostic(unexpected_token(
                    self.current_token.span(),
                    "`where` must follow an expression",
                ));
                statements.extend(where_bindings);
            }
        }

        // Detect unclosed block: reached EOF without finding closing `}`
        if self.is_current_token(TokenType::Eof)
            && !self.reported_unclosed_brace
            && !self.has_error_since(construct_checkpoint)
        {
            self.reported_unclosed_brace = true;
            let open_span = Span::new(start, start);
            let msg = if let Some(name) = context {
                format!(
                    "Expected a closing `}}` to match this opening `{{` in function `{}`.",
                    name
                )
            } else {
                "Expected a closing `}` to match this opening `{`.".to_string()
            };
            let mut diag = unclosed_delimiter(open_span, "{", "}", None);
            diag.message = Some(msg);
            if let Some(name) = context {
                diag.hints
                    .push(crate::diagnostics::types::Hint::help(format!(
                        "Add `}}` to close the body of function `{}`.",
                        name
                    )));
            }
            self.emit_parser_diagnostic(diag);
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
                self.emit_parser_diagnostic(unexpected_token(
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
                self.emit_parser_diagnostic(unexpected_token(
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
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();
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
                    self.emit_parser_diagnostic(unexpected_token(
                        self.current_token.span(),
                        "Expected expression after `,`, got `,`.",
                    ));
                    if self.check_list_error_limit(diag_start, end, "list") {
                        return Some(list);
                    }
                    continue;
                }

                if self.is_current_token(TokenType::Eof) {
                    self.emit_parser_diagnostic(unexpected_token(
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
                    self.emit_parser_diagnostic(missing_comma(
                        self.peek_token.span(),
                        context,
                        example,
                    ));
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
                if self.has_structural_error_since(construct_checkpoint) {
                    self.set_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
                    return Some(list);
                }
                let (open, close) = Self::delimiter_chars(end);
                self.emit_parser_diagnostic(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    Some(self.peek_token.span()),
                ));
                self.set_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
                return Some(list);
            }

            if self.has_structural_error_since(construct_checkpoint) {
                while matches!(
                    self.peek_token.token_type,
                    TokenType::RParen | TokenType::RBracket | TokenType::RBrace
                ) && self.peek_token.token_type != end
                {
                    self.next_token();
                }

                if self.recover_to_matching_delimiter(end, &[TokenType::Comma])
                    || self.consume_if_peek(end)
                {
                    return Some(list);
                }
            }

            let context = match end {
                TokenType::RParen => "argument",
                TokenType::RBracket => "array element",
                _ => "item",
            };
            let followup = unexpected_token(
                self.peek_token.span(),
                format!(
                    "I was expecting `,` or `{}` after this {}, but I found {}.",
                    end,
                    context,
                    self.describe_token_type_for_diagnostic(self.peek_token.token_type)
                ),
            );
            if !self.push_followup_unless_structural_root(construct_checkpoint, followup) {
                return Some(list);
            }
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
                if self.has_structural_error_since(construct_checkpoint) {
                    self.set_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
                    return Some(list);
                }
                let (open, close) = Self::delimiter_chars(end);
                self.emit_parser_diagnostic(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    Some(self.peek_token.span()),
                ));
                self.set_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
                return Some(list);
            }

            let message = if self.peek_token.token_type == TokenType::Eof {
                if self.has_structural_error_since(construct_checkpoint) {
                    self.set_recovery_boundary(RecoveryBoundary::Statement);
                    return Some(list);
                }
                let (open, close) = Self::delimiter_chars(end);
                self.emit_parser_diagnostic(unclosed_delimiter(
                    Span::new(open_pos, open_pos),
                    open,
                    close,
                    None,
                ));
                self.set_recovery_boundary(RecoveryBoundary::Statement);
                return None;
            } else {
                format!("Expected `,` or `{}` in expression list.", end)
            };
            if !self.push_followup_unless_structural_root(
                construct_checkpoint,
                unexpected_token(self.peek_token.span(), message),
            ) {
                return Some(list);
            }
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

        if matches!(end, TokenType::RParen | TokenType::RBracket)
            && self.peek_token.position.line > self.current_token.end_position.line
            && self.token_starts_expression(self.peek_token.token_type)
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
        if matches!(
            self.current_token.token_type,
            TokenType::RParen | TokenType::RBracket | TokenType::RBrace
        ) && self.errors.last().is_some_and(|diag| {
            matches!(diag.code(), Some("E034") | Some("E076"))
                && diag
                    .span()
                    .is_some_and(|span| span.start.line == self.current_token.position.line)
        }) {
            return;
        }

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
            self.emit_parser_diagnostic(diag);
            return;
        }

        let error_spec = &EXPECTED_EXPRESSION;
        let mut diag = Diagnostic::make_error(
            error_spec,
            &[&self.describe_token_type_for_diagnostic(self.current_token.token_type)],
            String::new(), // No file context in parser
            self.current_token.span(),
        );
        if self.current_token.token_type == TokenType::Ident {
            if let Some(suggestion) = self.parser_identifier_typo_hint(&self.current_token.literal)
            {
                diag = diag.with_help(suggestion);
            }
        }
        self.emit_parser_diagnostic(diag);
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
        self.emit_parser_diagnostic(diag);
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
        self.emit_parser_diagnostic(diag);
        self.synchronize_after_error();
    }

    /// Statement-level synchronization entry point used after parse errors.
    pub(super) fn synchronize_after_error(&mut self) {
        self.synchronize(SyncMode::Stmt);
    }

    pub(super) fn emit_expected_token(
        &mut self,
        _expected: TokenType,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.emit_parser_diagnostic(
            unexpected_token(self.peek_token.span(), message.into()).with_hint_text(hint.into()),
        );
    }

    pub(super) fn emit_expected_token_with_details(
        &mut self,
        _expected: TokenType,
        display_title: impl Into<String>,
        category: DiagnosticCategory,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.emit_parser_diagnostic(
            unexpected_token_with_details(
                self.peek_token.span(),
                display_title.into(),
                category,
                message.into(),
            )
            .with_hint_text(hint.into()),
        );
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
            self.emit_parser_diagnostic(unexpected_token(
                self.current_token.span(),
                format!(
                    "I was expecting a parameter name here, but I found {}.",
                    self.describe_token_type_for_diagnostic(self.current_token.token_type)
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
