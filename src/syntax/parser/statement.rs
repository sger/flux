use crate::{
    core::CorePrimOp,
    diagnostics::{
        DiagnosticBuilder, DiagnosticCategory, compiler_errors::DATA_MIXED_FIELD_FORMS,
        diagnostic_for, missing_fn_param_list, missing_function_body_brace, missing_let_assign,
        orphan_constructor_pattern, position::Span, unexpected_end_keyword, unexpected_token,
        unexpected_token_with_details, unknown_keyword, unknown_keyword_alias,
    },
    syntax::{
        data_variant::DataVariant,
        effect_ops::EffectOp,
        expression::Expression,
        precedence::Precedence,
        statement::{FunctionTypeParam, Statement},
        token_type::TokenType,
        type_class::{ClassConstraint, ClassMethod, InstanceMethod},
        type_expr::TypeExpr,
    },
};

use super::{
    Parser, ParserContext, RecoveryBoundary,
    helpers::{ParameterListContext, SyncMode},
};

type ParsedFunctionSignature = (
    crate::syntax::Identifier,
    Vec<FunctionTypeParam>,
    Vec<crate::syntax::Identifier>,
    Vec<Option<TypeExpr>>,
    Option<TypeExpr>,
    Vec<crate::syntax::effect_expr::EffectExpr>,
    Span,
);

impl Parser {
    fn looks_like_text_block_juxtaposition(&self) -> bool {
        if self.current_token.token_type != TokenType::Ident
            || self.peek_token.token_type != TokenType::Ident
        {
            return false;
        }

        // Strong signal: at least three bare words on one line (`Roses are red`).
        self.peek2_token.token_type == TokenType::Ident
            && self.peek2_token.position.line == self.peek_token.position.line
    }

    fn build_juxtaposed_identifier_error(
        &self,
        ident_name: &str,
        span: crate::diagnostics::position::Span,
    ) -> crate::diagnostics::Diagnostic {
        let mut diag = unexpected_token(
            span,
            format!("Unexpected identifier `{ident_name}` after expression."),
        )
        .with_hint_text("If you intended a function call, use parentheses: `f(x)`.")
        .with_hint_text("If you intended separate expressions, add an operator or a separator.");

        if self.looks_like_text_block_juxtaposition() {
            diag = diag.with_hint_text(
                "If this is text, wrap lines in quotes or use multiline strings with triple quotes: `\"\"\"...\"\"\"`.",
            );
        }

        diag
    }

    pub(super) fn parse_statement(&mut self) -> Option<Statement> {
        self.begin_statement_recovery();
        let statement = match self.current_token.token_type {
            TokenType::Module => self.parse_module_statement(),
            TokenType::Import => self.parse_import_statement(),
            TokenType::Let => self.parse_let_statement(false),
            TokenType::Return => self.parse_return_statement(),
            TokenType::Type => self.parse_type_adt_statement(),
            TokenType::Data => self.parse_data_statement(false),
            TokenType::Effect => self.parse_effect_statement(),
            TokenType::Alias => self.parse_effect_alias_statement(),
            TokenType::Class => self.parse_class_statement(false),
            TokenType::Instance => self.parse_instance_statement(false),
            TokenType::Fn if self.is_peek_token(TokenType::Ident) => {
                self.parse_function_statement(false, None)
            }
            TokenType::Intrinsic if self.is_peek_token(TokenType::Fn) => {
                self.next_token(); // fn
                self.parse_intrinsic_function_statement(false)
            }
            TokenType::Public if self.is_peek_token(TokenType::Fn) => {
                self.next_token(); // fn
                self.parse_function_statement(true, None)
            }
            TokenType::Public if self.is_peek_token(TokenType::Intrinsic) => {
                self.next_token(); // intrinsic
                if !self.expect_peek_context(
                    TokenType::Fn,
                    "Expected `fn` after `public intrinsic`.".to_string(),
                    "Intrinsic declarations use `public intrinsic fn name(...) = primop PrimOp`."
                        .to_string(),
                ) {
                    return None;
                }
                self.parse_intrinsic_function_statement(true)
            }
            TokenType::Public if self.is_peek_token(TokenType::Let) => {
                self.next_token(); // let
                self.parse_let_statement(true)
            }
            // Proposal 0151: `public class` and `public instance` declarations.
            TokenType::Public if self.is_peek_token(TokenType::Class) => {
                self.next_token(); // class
                self.parse_class_statement(true)
            }
            TokenType::Public if self.is_peek_token(TokenType::Instance) => {
                self.next_token(); // instance
                self.parse_instance_statement(true)
            }
            // Proposal 0151, Phase 2: `public data` declarations.
            TokenType::Public if self.is_peek_token(TokenType::Data) => {
                self.next_token(); // data
                self.parse_data_statement(true)
            }
            TokenType::At => self.parse_annotated_function(),
            TokenType::Ident if self.current_token.literal == "fn" => {
                // Defensive path: `fn` should lex as TokenType::Fn.
                None
            }
            TokenType::Ident
                if self.current_token.literal == "def" && self.is_peek_token(TokenType::Ident) =>
            {
                self.emit_parser_diagnostic(unknown_keyword_alias(
                    self.current_token.span(),
                    "def",
                    "fn",
                    "function declarations",
                ));
                None
            }
            TokenType::Ident
                if matches!(self.current_token.literal.as_ref(), "var" | "const" | "val")
                    && self.is_peek_token(TokenType::Ident) =>
            {
                self.emit_parser_diagnostic(unknown_keyword_alias(
                    self.current_token.span(),
                    &self.current_token.literal,
                    "let",
                    "bindings",
                ));
                None
            }
            TokenType::Ident
                if matches!(
                    self.current_token.literal.as_ref(),
                    "case" | "switch" | "when"
                ) =>
            {
                self.emit_parser_diagnostic(unknown_keyword_alias(
                    self.current_token.span(),
                    &self.current_token.literal,
                    "match",
                    "pattern matching",
                ));
                None
            }
            TokenType::Ident
                if self.current_token.literal != "fn"
                    && (self.current_token.literal.starts_with("fn")
                        || self.current_token.literal.starts_with("fun"))
                    && self.is_peek_token(TokenType::Ident) =>
            {
                self.emit_parser_diagnostic(
                    unknown_keyword(self.current_token.span(), &self.current_token.literal, None)
                        .with_message(format!(
                            "Unknown keyword `{}`. Flux uses `fn` for function declarations.",
                            self.current_token.literal
                        ))
                        .with_hint_text("Did you mean `fn`?"),
                );
                None
            }

            // Check if we have `identifier = expression` (reassignment without 'let')
            TokenType::Ident if self.is_peek_token(TokenType::Assign) => {
                self.parse_assignment_statement()
            }
            _ => self.parse_expression_statement(),
        };

        if let Some(boundary) = self.take_requested_recovery_boundary() {
            self.synchronize_recovery_boundary(boundary);
        } else if statement.is_none() && !self.used_custom_recovery() {
            self.synchronize(SyncMode::Stmt);
        }

        statement
    }

    pub(super) fn parse_expression_statement(&mut self) -> Option<Statement> {
        if self.current_token.token_type == TokenType::Ident && self.current_token.literal == "end"
        {
            self.errors
                .push(unexpected_end_keyword(self.current_token.span()));
            return None;
        }

        let start = self.current_token.position;
        let expression = match self.parse_expression(Precedence::Lowest) {
            Some(expression) => expression,
            None => {
                self.synchronize(SyncMode::Expr);
                return None;
            }
        };

        // Detect juxtaposed identifiers on the same line: `foo bar` with no operator.
        // In Flux, function calls require parentheses, so two bare identifiers on the
        // same line without a separator is always a parse error. Emitting the error here
        // (at the expression start) rather than at the downstream failure token prevents
        // cascade errors — e.g. "Expected expression, found ," several tokens later.
        if self.peek_token.token_type == TokenType::Ident
            && self.peek_token.position.line == self.current_token.end_position.line
        {
            let ident_name = self.peek_token.literal.to_string();
            if matches!(ident_name.as_str(), "elif" | "elsif") {
                self.emit_parser_diagnostic(
                    unknown_keyword_alias(
                        self.peek_token.span(),
                        &ident_name,
                        "else if",
                        "chained conditionals",
                    )
                    .with_hint_text("Replace `elif`/`elsif` with `else if`."),
                );
                while !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.position.line == self.current_token.end_position.line
                {
                    self.next_token();
                }
                return Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    span: self.span_from(start),
                });
            }
            if ident_name == "end" {
                self.errors
                    .push(unexpected_end_keyword(self.peek_token.span()));
                while !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.position.line == self.current_token.end_position.line
                {
                    self.next_token();
                }
                return Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    span: self.span_from(start),
                });
            }
            let error_span = self.span_from(start);
            self.errors
                .push(self.build_juxtaposed_identifier_error(&ident_name, error_span));
            // Skip the current line AND any subsequent lines that also consist of
            // bare identifier sequences (e.g. lines of prose inside a `""` that should
            // have been `"""`). This emits a single error for the whole group instead
            // of one per line.
            let mut skip_line = self.current_token.end_position.line;
            loop {
                // Advance past everything remaining on `skip_line`.
                while !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.position.line == skip_line
                {
                    self.next_token();
                }
                // If the next line also opens with two consecutive identifiers on the
                // same line, it is part of the same erroneous block — skip it silently.
                if !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.token_type == TokenType::Ident
                    && self.peek2_token.token_type == TokenType::Ident
                    && self.peek2_token.position.line == self.peek_token.position.line
                {
                    skip_line = self.peek_token.position.line;
                    self.next_token(); // step onto the next line
                } else {
                    break;
                }
            }
            // Return Some so parse_statement does not call synchronize() and consume
            // valid code on subsequent lines.
            return Some(Statement::Expression {
                expression,
                has_semicolon: false,
                span: self.span_from(start),
            });
        }

        let has_semicolon = if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
            true
        } else {
            false
        };

        if matches!(
            expression,
            crate::syntax::expression::Expression::Some { .. }
                | crate::syntax::expression::Expression::Left { .. }
                | crate::syntax::expression::Expression::Right { .. }
        ) && (has_semicolon || self.is_peek_token(TokenType::Eof))
        {
            let name = match &expression {
                crate::syntax::expression::Expression::Some { .. } => "Some",
                crate::syntax::expression::Expression::Left { .. } => "Left",
                crate::syntax::expression::Expression::Right { .. } => "Right",
                _ => unreachable!(),
            };
            self.errors
                .push(orphan_constructor_pattern(expression.span(), name));
        }

        Some(Statement::Expression {
            expression,
            has_semicolon,
            span: self.span_from(start),
        })
    }

    /// Parse `@fip fn ...` or `@fbip fn ...`.
    fn parse_annotated_function(&mut self) -> Option<Statement> {
        use crate::syntax::statement::FipAnnotation;
        let annotation_start = self.current_token.position;
        // Current token is '@'. Next should be 'fip' or 'fbip' (as an identifier).
        self.next_token();
        if self.current_token.token_type != TokenType::Ident {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.current_token.span(),
                    "Invalid Function Annotation",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Expected a function annotation name after `@`, but found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                )
                .with_hint_text("Supported function annotations are `@fip` and `@fbip`."),
            );
            return None;
        }

        let annotation_name = self.current_token.literal.to_string();
        let annotation_span = self.span_from(annotation_start);
        let annotation = match annotation_name.as_str() {
            "fip" => Some(FipAnnotation::Fip),
            "fbip" => Some(FipAnnotation::Fbip),
            _ => {
                self.emit_parser_diagnostic(
                    unexpected_token_with_details(
                        annotation_span,
                        "Unknown Function Annotation",
                        DiagnosticCategory::ParserDeclaration,
                        format!(
                            "Unknown annotation `@{annotation_name}` before function declaration."
                        ),
                    )
                    .with_hint_text("Supported function annotations are `@fip` and `@fbip`."),
                );
                return None;
            }
        };
        // Next token must be 'fn'
        self.next_token();
        if self.current_token.token_type != TokenType::Fn {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.current_token.span(),
                    "Malformed Annotated Function",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Annotation `@{annotation_name}` must be followed by `fn`, but found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                )
                .with_hint_text("Write annotated functions as `@fip fn name(...) { ... }` or `@fbip fn name(...) { ... }`."),
            );
            return None;
        }
        self.parse_function_statement(false, annotation)
    }

    fn parse_function_signature(
        &mut self,
        declaration_keyword: &'static str,
    ) -> Option<ParsedFunctionSignature> {
        let start = self.current_token.position;
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Function Name",
            DiagnosticCategory::ParserDeclaration,
            format!("Expected function name after `{declaration_keyword}`."),
            format!("Function declarations start with `{declaration_keyword} name(...)`."),
        ) {
            return None;
        }

        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        let type_params = self.parse_function_type_params_angle_bracket()?;

        if self.is_peek_token(TokenType::Arrow)
            || (!self.is_peek_token(TokenType::LParen)
                && self.token_starts_type(self.peek_token.token_type))
        {
            let fn_name = self.lexer.interner().resolve(name).to_string();
            self.emit_parser_diagnostic(missing_fn_param_list(self.peek_token.span(), &fn_name));
            return None;
        }

        if !self.expect_peek_context_with_details(
            TokenType::LParen,
            "Missing Function Parameter List",
            DiagnosticCategory::ParserDeclaration,
            "Expected `(` after function name.".to_string(),
            format!("Function declarations use `{declaration_keyword} name(params)`."),
        ) {
            return None;
        }

        let (parameters, parameter_types) =
            self.parse_typed_function_parameters(ParameterListContext::Function)?;

        let return_type = if self.is_peek_token(TokenType::Arrow) {
            self.next_token(); // ->
            self.next_token(); // start of return type
            let ty = self.parse_type_expr();
            if ty.is_none() {
                self.recover_to_type_anchor(&[
                    TokenType::With,
                    TokenType::Assign,
                    TokenType::LBrace,
                    TokenType::Semicolon,
                    TokenType::RBrace,
                    TokenType::Eof,
                ]);
            }
            ty
        } else if self.token_starts_type(self.peek_token.token_type) {
            self.emit_parser_diagnostic(unexpected_token_with_details(
                self.peek_token.span(),
                "Missing Return Type Arrow",
                DiagnosticCategory::ParserSeparator,
                "Missing `->` before function return type. Write it as `fn name(...) -> Type`.",
            ));
            self.next_token(); // start of return type
            let ty = self.parse_type_expr();
            if ty.is_none() {
                self.recover_to_type_anchor(&[
                    TokenType::With,
                    TokenType::Assign,
                    TokenType::LBrace,
                    TokenType::Semicolon,
                    TokenType::RBrace,
                    TokenType::Eof,
                ]);
            }
            ty
        } else {
            None
        };

        let effects = self.parse_effect_list()?;

        Some((
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            self.span_from(start),
        ))
    }

    fn parse_intrinsic_function_statement(&mut self, is_public: bool) -> Option<Statement> {
        let context_name = if self.is_peek_token(TokenType::Ident) {
            Some(self.peek_token.literal.to_string())
        } else {
            None
        };
        let _function_context = context_name
            .clone()
            .map(|name| self.enter_parser_context(ParserContext::Function(name)));

        let (name, type_params, parameters, parameter_types, return_type, effects, span) =
            self.parse_function_signature("intrinsic fn")?;

        if !self.expect_peek_context_with_details(
            TokenType::Assign,
            "Missing Intrinsic PrimOp Binding",
            DiagnosticCategory::ParserDeclaration,
            "Expected `=` after intrinsic function signature.".to_string(),
            "Intrinsic declarations use `intrinsic fn name(...) = primop PrimOpName`.".to_string(),
        ) {
            return None;
        }

        if !self.expect_peek_context_with_details(
            TokenType::Primop,
            "Missing PrimOp Keyword",
            DiagnosticCategory::ParserDeclaration,
            "Expected `primop` after `=` in intrinsic declaration.".to_string(),
            "Intrinsic declarations bind to a primop with `= primop PrimOpName`.".to_string(),
        ) {
            return None;
        }

        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing PrimOp Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected a primop name after `primop`.".to_string(),
            "Use an internal primop name like `ArrayLen` or `StringConcat`.".to_string(),
        ) {
            return None;
        }

        let primop_name = self.current_token.literal.to_string();
        let Some(primop) = CorePrimOp::from_intrinsic_name(&primop_name) else {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.current_token.span(),
                    "Unknown PrimOp Name",
                    DiagnosticCategory::ParserDeclaration,
                    format!("Unknown primop `{primop_name}` in intrinsic declaration."),
                )
                .with_hint_text("Use a valid internal primop name such as `ArrayLen`."),
            );
            return None;
        };

        if parameters.len() != primop.arity() {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.current_token.span(),
                    "INTRINSIC ARITY MISMATCH",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Intrinsic `{}` declares {} parameter(s), but primop `{}` expects {}.",
                        self.lexer.interner().resolve(name),
                        parameters.len(),
                        primop_name,
                        primop.arity()
                    ),
                )
                .with_hint_text("Make the intrinsic parameter list match the primop arity."),
            );
            return None;
        }

        let Some(helper_name) = primop.intrinsic_helper_name() else {
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.current_token.span(),
                    "UNSUPPORTED INTRINSIC PRIMOP",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Primop `{primop_name}` is not available for `intrinsic fn` declarations."
                    ),
                )
                .with_hint_text(
                    "Use a function-shaped primop intended for stdlib intrinsic wrappers.",
                ),
            );
            return None;
        };

        let helper_symbol = self.lexer.interner_mut().intern(helper_name);
        // Allocate fresh ExprIds for every synthesized node. Reusing
        // `ExprId::UNSET` across multiple intrinsic declarations in the same
        // module made HM's per-expression type map alias across them, so the
        // first intrinsic's call expression could be unified with the
        // second's body type. This is the structural fix referenced by
        // Proposal 0167 Part 6.
        let helper_expr = Expression::Identifier {
            name: helper_symbol,
            span,
            id: self.next_expr_id(),
        };
        let args = parameters
            .iter()
            .map(|param| Expression::Identifier {
                name: *param,
                span,
                id: self.next_expr_id(),
            })
            .collect();
        let call = Expression::Call {
            function: Box::new(helper_expr),
            arguments: args,
            span,
            id: self.next_expr_id(),
        };
        let body = crate::syntax::block::Block {
            statements: vec![Statement::Expression {
                expression: call,
                has_semicolon: false,
                span,
            }],
            span,
        };

        Some(Statement::Function {
            is_public,
            fip: None,
            intrinsic: Some(primop),
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
        })
    }

    pub(super) fn parse_function_statement(
        &mut self,
        is_public: bool,
        fip: Option<crate::syntax::statement::FipAnnotation>,
    ) -> Option<Statement> {
        let context_name = if self.is_peek_token(TokenType::Ident) {
            Some(self.peek_token.literal.to_string())
        } else {
            None
        };
        let _function_context = context_name
            .clone()
            .map(|name| self.enter_parser_context(ParserContext::Function(name)));
        let (name, type_params, parameters, parameter_types, return_type, effects, span) =
            self.parse_function_signature("fn")?;

        if !self.is_current_token(TokenType::LBrace) && !self.is_peek_token(TokenType::LBrace) {
            let fn_name = self.lexer.interner().resolve(name).to_string();
            let fn_span = Span::new(span.start, span.start);
            let found_desc = match self.peek_token.token_type {
                TokenType::Ident => format!("`{}`", self.peek_token.literal),
                TokenType::Eof => "end of file".to_string(),
                _ => format!("`{}`", self.peek_token.token_type),
            };
            self.emit_parser_diagnostic(missing_function_body_brace(
                fn_span,
                &fn_name,
                self.peek_token.span(),
                &found_desc,
            ));
            self.suppress_top_level_rbrace_once = true;
            self.request_recovery_boundary(RecoveryBoundary::MissingBlockOpener);
            return None;
        }
        if self.is_peek_token(TokenType::LBrace) {
            self.next_token(); // consume `{`
        }

        let fn_name_str = self.lexer.interner().resolve(name).to_string();
        let body = self.parse_block_with_context(Some(&fn_name_str));

        Some(Statement::Function {
            is_public,
            name,
            intrinsic: None,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            fip,
        })
    }

    pub(super) fn parse_return_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        self.next_token();

        let value = if self.is_current_token(TokenType::Semicolon) {
            None
        } else {
            match self.parse_expression(Precedence::Lowest) {
                Some(expression) => Some(expression),
                None => {
                    self.synchronize(SyncMode::Stmt);
                    return None;
                }
            }
        };

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Return {
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_let_statement(&mut self, is_public: bool) -> Option<Statement> {
        let start = self.current_token.position;

        if self.is_peek_token(TokenType::LParen) {
            self.next_token(); // consume '(' so parse_pattern sees tuple pattern start
            let pattern = self.parse_pattern()?;

            if !self.expect_peek_context(
                TokenType::Assign,
                "Expected `=` in tuple destructuring `let` binding.".to_string(),
                "Tuple destructuring uses `let (a, b) = value`.".to_string(),
            ) {
                return None;
            }

            self.next_token();
            let value = self.parse_required(
                |parser| parser.parse_expression(Precedence::Lowest),
                SyncMode::Stmt,
            )?;

            if self.is_peek_token(TokenType::Semicolon) {
                self.next_token();
            }

            return Some(Statement::LetDestructure {
                is_public,
                pattern,
                value,
                span: self.span_from(start),
            });
        }

        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected binding name after `let`.".to_string(),
            "Let bindings use `let name = value`.".to_string(),
        ) {
            return None;
        }

        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        let binding_name = self.lexer.interner().resolve(name).to_string();
        let type_annotation = self.parse_type_annotation_opt_with_missing_colon(
            &[
                TokenType::Assign,
                TokenType::Semicolon,
                TokenType::RBrace,
                TokenType::Eof,
            ],
            "let binding",
            Some(binding_name.as_str()),
        );

        if !self.is_current_token(TokenType::Assign) {
            if self.is_peek_token(TokenType::Assign) {
                self.next_token();
            } else {
                self.errors
                    .push(missing_let_assign(self.peek_token.span(), &binding_name));
                return None;
            }
        }

        self.next_token();

        let value = self.parse_required(
            |parser| parser.parse_expression(Precedence::Lowest),
            SyncMode::Stmt,
        )?;

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Let {
            is_public,
            name,
            type_annotation,
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_assignment_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        if !self.expect_peek_context(
            TokenType::Assign,
            "Expected `=` in assignment statement.".to_string(),
            "Assignments use `name = value`.".to_string(),
        ) {
            return None;
        }

        self.next_token();

        let value = match self.parse_expression(Precedence::Lowest) {
            Some(expression) => expression,
            None => {
                self.synchronize(SyncMode::Stmt);
                return None;
            }
        };

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Assign {
            name,
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_module_statement(&mut self) -> Option<Statement> {
        let context_name = if self.is_peek_token(TokenType::Ident) {
            Some(self.peek_token.literal.to_string())
        } else {
            None
        };
        let _module_context = context_name
            .clone()
            .map(|name| self.enter_parser_context(ParserContext::Module(name)));
        let start = self.current_token.position;

        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected module name after `module`.".to_string(),
            "Module declarations use `module Name { ... }`.".to_string(),
        ) {
            return None;
        }

        let name = self.parse_qualified_name()?;

        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Module Body",
            DiagnosticCategory::ParserDeclaration,
            "This module body needs to start with `{`.".to_string(),
            "Module declarations use `module Name { ... }`.".to_string(),
        ) {
            self.suppress_top_level_rbrace_once = true;
            self.request_recovery_boundary(RecoveryBoundary::MissingBlockOpener);
            return None;
        }

        let body = self.parse_block();

        Some(Statement::Module {
            name,
            body,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_import_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Import Path",
            DiagnosticCategory::ParserDeclaration,
            "This import needs a module path after `import`.".to_string(),
            "Import statements use `import Module.Name`.".to_string(),
        ) {
            if self.peek_token.position.line > self.current_token.end_position.line
                || self.peek_token.token_type == TokenType::Eof
            {
                self.errors.pop();
                self.emit_parser_diagnostic(
                    unexpected_token_with_details(
                        self.current_token.span(),
                        "Missing Import Path",
                        DiagnosticCategory::ParserDeclaration,
                        "This import needs a module path after `import`.",
                    )
                    .with_hint_text("Import statements use `import Module.Name`."),
                );
                self.request_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
            }
            return None;
        }

        let name = self.parse_qualified_name()?;
        let mut alias = None;
        let mut except = Vec::new();

        if self.is_peek_token(TokenType::As) {
            self.next_token(); // consume 'as'
            if !self.is_peek_token(TokenType::Ident) {
                let alias_span = if self.peek_token.position.line
                    > self.current_token.end_position.line
                    || self.peek_token.token_type == TokenType::Eof
                {
                    self.current_token.span()
                } else {
                    self.peek_token.span()
                };
                self.emit_parser_diagnostic(
                    unexpected_token_with_details(
                        alias_span,
                        "Missing Import Alias",
                        DiagnosticCategory::ParserDeclaration,
                        "This import alias needs a name after `as`.",
                    )
                    .with_hint_text("Import aliases use `import Module as Alias`."),
                );
                self.request_recovery_boundary(RecoveryBoundary::NextLineOrBlock);
                return None;
            }
            self.next_token();
            alias = Some(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            );
        }

        if self.peek_token.token_type == TokenType::Ident && self.peek_token.literal == "except" {
            self.next_token(); // consume `except`
            except = self.parse_import_except_list()?;
        }

        let mut exposing = crate::syntax::statement::ImportExposing::None;
        if self.peek_token.token_type == TokenType::Ident && self.peek_token.literal == "exposing" {
            self.next_token(); // consume `exposing`
            exposing = self.parse_import_exposing()?;
        } else if self.peek_token.token_type == TokenType::Ident
            && matches!(
                self.peek_token.literal.as_ref(),
                "expose" | "exports" | "exporting" | "using" | "open"
            )
        {
            let typo = self.peek_token.literal.to_string();
            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.peek_token.span(),
                    "Unknown Import Clause",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Unknown keyword `{}` in import statement.",
                        typo
                    ),
                )
                .with_hint_text(
                    "Did you mean `exposing`? Use `import Module exposing (..)` or `import Module exposing (name1, name2)`."),
            );
            return None;
        }

        // No semicolon required for import statements

        Some(Statement::Import {
            name,
            alias,
            except,
            exposing,
            span: self.span_from(start),
        })
    }

    fn parse_import_except_list(&mut self) -> Option<Vec<crate::syntax::Identifier>> {
        if !self.expect_peek_context_with_details(
            TokenType::LBracket,
            "Missing Import Except List",
            DiagnosticCategory::ParserDeclaration,
            "Expected `[` after `except` in import.".to_string(),
            "Import exclusions use `import Module except [Name1, Name2]`.".to_string(),
        ) {
            return None;
        }

        let mut names = Vec::new();
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();
        if self.is_peek_token(TokenType::RBracket) {
            self.next_token();
            return Some(names);
        }

        loop {
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Invalid Import Except List",
                DiagnosticCategory::ParserDeclaration,
                "Expected identifier in import `except` list.".to_string(),
                "Import exclusions use `import Module except [Name1, Name2]`.".to_string(),
            ) {
                return None;
            }
            names.push(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            );

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma
                continue;
            }

            if self.is_peek_token(TokenType::RBracket) {
                self.next_token(); // consume closing bracket
                break;
            }

            if !self.push_followup_unless_structural_root(
                construct_checkpoint,
                unexpected_token(
                    self.peek_token.span(),
                    format!(
                        "I was expecting `,` or `]` in the import except list, but I found {}.",
                        self.describe_token_type_for_diagnostic(self.peek_token.token_type)
                    ),
                ),
            ) {
                return Some(names);
            }
            return None;
        }

        Some(names)
    }

    /// Parses the `exposing (..)` or `exposing (name, name)` clause of an import.
    fn parse_import_exposing(&mut self) -> Option<crate::syntax::statement::ImportExposing> {
        use crate::syntax::statement::ImportExposing;

        if !self.expect_peek_context_with_details(
            TokenType::LParen,
            "Missing Exposing List",
            DiagnosticCategory::ParserDeclaration,
            "Expected `(` after `exposing` in import.".to_string(),
            "Use `exposing (..)` for all members or `exposing (name1, name2)` for selective."
                .to_string(),
        ) {
            return None;
        }

        // Check for `(..)` — wildcard: two Dot tokens followed by RParen
        if self.is_peek_token(TokenType::Dot) {
            self.next_token(); // consume first `.`
            if !self.expect_peek_context_with_details(
                TokenType::Dot,
                "Invalid Exposing Clause",
                DiagnosticCategory::ParserDeclaration,
                "Expected `..` for wildcard exposing.".to_string(),
                "Use `exposing (..)` to expose all public members.".to_string(),
            ) {
                return None;
            }
            if !self.expect_peek_context_with_details(
                TokenType::RParen,
                "Missing Closing Paren",
                DiagnosticCategory::ParserDeclaration,
                "Expected `)` after `..` in exposing clause.".to_string(),
                "Wildcard exposing uses `exposing (..)`.".to_string(),
            ) {
                return None;
            }
            return Some(ImportExposing::All);
        }

        // Parse selective list: `(name1, name2, ...)`
        let mut names = Vec::new();
        if self.is_peek_token(TokenType::RParen) {
            self.next_token();
            return Some(ImportExposing::Names(names));
        }

        loop {
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Invalid Exposing List",
                DiagnosticCategory::ParserDeclaration,
                "Expected identifier in `exposing` list.".to_string(),
                "Use `exposing (name1, name2)` to expose specific members.".to_string(),
            ) {
                return None;
            }
            names.push(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            );

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma
                continue;
            }

            if self.is_peek_token(TokenType::RParen) {
                self.next_token(); // consume closing paren
                break;
            }

            self.emit_parser_diagnostic(
                unexpected_token_with_details(
                    self.peek_token.span(),
                    "Invalid Exposing List",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "Expected `,` or `)` in exposing list, but found {}.",
                        self.describe_token_type_for_diagnostic(self.peek_token.token_type)
                    ),
                )
                .with_hint_text("Use `exposing (name1, name2)` for selective imports."),
            );
            return None;
        }

        Some(ImportExposing::Names(names))
    }

    /// Parses a `data` declaration with optional type parameters and constructor
    /// variants, for example `data Option<T> { Some(T), None }`.
    pub(super) fn parse_data_statement(&mut self, is_public: bool) -> Option<Statement> {
        let context_name = if self.is_peek_token(TokenType::Ident) {
            Some(self.peek_token.literal.to_string())
        } else {
            None
        };
        let _data_context = context_name
            .clone()
            .map(|name| self.enter_parser_context(ParserContext::Data(name)));
        let start = self.current_token.position;

        // current: 'data' — advance to type name
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Data Type Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected type name after `data`.".to_string(),
            "Data declarations use `data TypeName { Ctor, ... }`.".to_string(),
        ) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // Optional generic type parameters <T, U, ...> (parsed but stored only as names for now)
        let mut type_params = Vec::new();

        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume '<'
            loop {
                if !self.expect_peek_context_with_details(
                    TokenType::Ident,
                    "Missing Generic Parameter Name",
                    DiagnosticCategory::ParserDeclaration,
                    "Expected generic type parameter name.".to_string(),
                    "Data generics use `data Type<T, U> { ... }`.".to_string(),
                ) {
                    return None;
                }
                type_params.push(
                    self.current_token
                        .symbol
                        .expect("ident token should have symbol"),
                );
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume ','
                } else {
                    break;
                }
            }
            if !self.expect_peek_context_with_details(
                TokenType::Gt,
                "Missing Generic Parameter List",
                DiagnosticCategory::ParserDelimiter,
                "Expected `>` to close data type parameters.".to_string(),
                "Data generics use `data Type<T, U> { ... }`.".to_string(),
            ) {
                return None;
            }
        }

        // Expect opening brace
        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Data Body",
            DiagnosticCategory::ParserDeclaration,
            "Expected `{` to begin data constructors.".to_string(),
            "Data declarations use `data TypeName { Ctor, ... }`.".to_string(),
        ) {
            return None;
        }
        self.next_token(); // move past '{'

        let mut variants = Vec::new();
        // Tracks the form of the first variant (None = unknown, Some(true) = named,
        // Some(false) = positional). Used to enforce uniform form (E465).
        let mut first_variant_named: Option<bool> = None;

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            let var_start = self.current_token.position;
            let variant_checkpoint = self.start_construct_diagnostics_checkpoint();

            if self.current_token.token_type != TokenType::Ident {
                self.emit_parser_diagnostic(unexpected_token_with_details(
                    self.current_token.span(),
                    "Invalid Data Constructor",
                    DiagnosticCategory::ParserDeclaration,
                    format!(
                        "I was expecting a constructor name in this `data` declaration, but I found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                ));
                return None;
            }

            let var_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            // Optional field types:
            //   Positional: VariantName(Type1, Type2, ...)
            //   Named:      VariantName { field1: Type1, field2: Type2, ... }
            let mut fields = Vec::new();
            let mut field_names: Option<Vec<crate::syntax::Identifier>> = None;
            let this_variant_named;

            if self.is_peek_token(TokenType::LParen) {
                this_variant_named = false;
                self.next_token(); // advance to '('
                // Empty parens: Variant() — skip past ')'
                if !self.consume_if_peek(TokenType::RParen) {
                    self.next_token(); // move to start of first type

                    loop {
                        if self.is_current_token(TokenType::RParen)
                            || self.is_current_token(TokenType::Eof)
                        {
                            break;
                        }
                        let ty = self.parse_type_expr()?;
                        fields.push(ty);
                        // parse_type_expr leaves current at last token of type; advance past it
                        self.next_token();
                        match self.current_token.token_type {
                            TokenType::Comma => {
                                self.next_token(); // move to start of next type
                            }
                            TokenType::RParen | TokenType::Eof => break,
                            _ => {
                                if !self.push_followup_unless_structural_root(
                                    variant_checkpoint,
                                    unexpected_token_with_details(
                                        self.current_token.span(),
                                        "Missing Constructor Field Separator",
                                        DiagnosticCategory::ParserSeparator,
                                        format!(
                                            "I was expecting `,` or `)` between constructor fields, but I found {}.",
                                            self.describe_token_type_for_diagnostic(
                                                self.current_token.token_type
                                            )
                                        ),
                                    ),
                                ) {
                                    break;
                                }
                                let _ = self.recover_to_matching_delimiter(
                                    TokenType::RParen,
                                    &[TokenType::Comma, TokenType::RBrace],
                                );
                                break;
                            }
                        }
                    }
                }

                // current_token is '(' after consume_if_peek or ')' after the loop;
                // in both cases we need current to be ')' to leave this block correctly.
            } else if self.is_peek_token(TokenType::LBrace) {
                // Named-field variant: VariantName { field: Type, ... }
                // See proposal 0152.
                this_variant_named = true;
                let mut names: Vec<crate::syntax::Identifier> = Vec::new();
                self.next_token(); // advance to '{'
                if !self.consume_if_peek(TokenType::RBrace) {
                    self.next_token(); // move to first field name
                    loop {
                        if self.is_current_token(TokenType::RBrace)
                            || self.is_current_token(TokenType::Eof)
                        {
                            break;
                        }
                        if self.current_token.token_type != TokenType::Ident {
                            self.emit_parser_diagnostic(unexpected_token_with_details(
                                self.current_token.span(),
                                "Invalid Named Field",
                                DiagnosticCategory::ParserDeclaration,
                                format!(
                                    "I was expecting a field name inside a named-field variant, but I found {}.",
                                    self.describe_token_type_for_diagnostic(
                                        self.current_token.token_type
                                    )
                                ),
                            ));
                            let _ = self.recover_to_matching_delimiter(
                                TokenType::RBrace,
                                &[TokenType::Comma],
                            );
                            break;
                        }
                        let field_name = self
                            .current_token
                            .symbol
                            .expect("ident token should have symbol");
                        let field_name_span = self.current_token.span();
                        // Reject duplicate field names within the same variant (E462).
                        if names.contains(&field_name) {
                            self.emit_parser_diagnostic(unexpected_token_with_details(
                                field_name_span,
                                "Duplicate Field",
                                DiagnosticCategory::ParserDeclaration,
                                "This named-field variant already declares a field with this name."
                                    .to_string(),
                            ));
                        }
                        if !self.expect_peek_context_with_details(
                            TokenType::Colon,
                            "Missing Field Type Annotation",
                            DiagnosticCategory::ParserSeparator,
                            "Expected `:` between field name and its type.".to_string(),
                            "Named-field variants use `field_name: FieldType` pairs.".to_string(),
                        ) {
                            let _ = self.recover_to_matching_delimiter(
                                TokenType::RBrace,
                                &[TokenType::Comma],
                            );
                            break;
                        }
                        self.next_token(); // move to start of type
                        let ty = self.parse_type_expr()?;
                        names.push(field_name);
                        fields.push(ty);
                        self.next_token(); // advance past the type
                        match self.current_token.token_type {
                            TokenType::Comma => {
                                self.next_token(); // move to next field name
                            }
                            TokenType::RBrace | TokenType::Eof => break,
                            _ => {
                                if !self.push_followup_unless_structural_root(
                                    variant_checkpoint,
                                    unexpected_token_with_details(
                                        self.current_token.span(),
                                        "Missing Named-Field Separator",
                                        DiagnosticCategory::ParserSeparator,
                                        format!(
                                            "I was expecting `,` or `}}` between named fields, but I found {}.",
                                            self.describe_token_type_for_diagnostic(
                                                self.current_token.token_type
                                            )
                                        ),
                                    ),
                                ) {
                                    break;
                                }
                                let _ = self.recover_to_matching_delimiter(
                                    TokenType::RBrace,
                                    &[TokenType::Comma],
                                );
                                break;
                            }
                        }
                    }
                }
                field_names = Some(names);
            } else {
                // Zero-field variant: VariantName
                this_variant_named = false;
            }

            // Enforce uniform form across variants (E465).
            match first_variant_named {
                None => first_variant_named = Some(this_variant_named),
                Some(expected) if expected != this_variant_named => {
                    let msg = format!(
                        "Data type `{}` mixes positional and named-field variants.",
                        self.interner().resolve(name)
                    );
                    self.emit_parser_diagnostic(
                        diagnostic_for(&DATA_MIXED_FIELD_FORMS)
                            .with_span(self.span_from(var_start))
                            .with_message(msg),
                    );
                }
                _ => {}
            }

            variants.push(DataVariant {
                name: var_name,
                fields,
                field_names,
                span: self.span_from(var_start),
            });

            // Optional trailing comma between variants
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume ','
            }
            self.next_token(); // advance to next variant or '}'
        }

        // Optional `deriving (Eq, Show, ...)`
        let deriving = if self.is_peek_token(TokenType::Deriving) {
            self.next_token(); // consume `deriving`
            self.parse_deriving_list()
        } else {
            vec![]
        };

        Some(Statement::Data {
            is_public,
            name,
            type_params,
            variants,
            deriving,
            span: self.span_from(start),
        })
    }

    /// Parse `(Eq, Show, Ord)` after `deriving` keyword.
    fn parse_deriving_list(&mut self) -> Vec<crate::syntax::Identifier> {
        let mut classes = Vec::new();
        if !self.expect_peek_context(
            TokenType::LParen,
            "Expected `(` after `deriving`.".to_string(),
            "Use `deriving (Eq, Show)` to auto-derive instances.".to_string(),
        ) {
            return classes;
        }
        loop {
            self.next_token(); // move to class name or `)`
            if self.is_current_token(TokenType::RParen) {
                break;
            }
            if let Some(sym) = self.current_token.symbol {
                classes.push(sym);
            }
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume `,`
            } else {
                break;
            }
        }
        // Consume `)`
        if self.is_peek_token(TokenType::RParen) {
            self.next_token();
        }
        classes
    }

    /// Parses ADT sugar:
    /// `type Result<T, E> = Ok(T) | Err(E)`
    ///
    /// This desugars directly into `Statement::Data`.
    pub(super) fn parse_type_adt_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        // current: 'type' — advance to type name
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Type Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected type name after `type`.".to_string(),
            "ADT sugar uses `type TypeName = Ctor(...) | Other`.".to_string(),
        ) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // Optional generic type parameters <T, U, ...>
        let mut type_params = Vec::new();
        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume '<'
            loop {
                if !self.expect_peek_context_with_details(
                    TokenType::Ident,
                    "Missing Generic Parameter Name",
                    DiagnosticCategory::ParserDeclaration,
                    "Expected generic type parameter name.".to_string(),
                    "Type generics use `type Name<T, U> = ...`.".to_string(),
                ) {
                    return None;
                }
                type_params.push(
                    self.current_token
                        .symbol
                        .expect("ident token should have symbol"),
                );
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume ','
                } else {
                    break;
                }
            }
            if !self.expect_peek_context_with_details(
                TokenType::Gt,
                "Missing Generic Parameter List",
                DiagnosticCategory::ParserDelimiter,
                "Expected `>` to close type parameters.".to_string(),
                "Type generics use `type Name<T, U> = ...`.".to_string(),
            ) {
                return None;
            }
        }

        // Expect '='
        if !self.expect_peek_context_with_details(
            TokenType::Assign,
            "Missing Type Definition",
            DiagnosticCategory::ParserDeclaration,
            "Expected `=` after type declaration name.".to_string(),
            "ADT sugar uses `type Name = Ctor(...) | Other`.".to_string(),
        ) {
            return None;
        }
        self.next_token(); // move past '=' to first constructor token

        let mut variants = Vec::new();
        loop {
            let var_start = self.current_token.position;
            let variant_checkpoint = self.start_construct_diagnostics_checkpoint();
            if self.current_token.token_type != TokenType::Ident {
                self.emit_parser_diagnostic(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "I was expecting a constructor name in this `type` declaration, but I found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                ));
                return None;
            }
            let var_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            let mut fields = Vec::new();
            if self.is_peek_token(TokenType::LParen) {
                self.next_token(); // move to '('
                // Empty Variant()
                if !self.consume_if_peek(TokenType::RParen) {
                    self.next_token(); // first field type
                    loop {
                        if self.is_current_token(TokenType::RParen)
                            || self.is_current_token(TokenType::Eof)
                        {
                            break;
                        }
                        let ty = self.parse_type_expr()?;
                        fields.push(ty);
                        self.next_token(); // past field type
                        match self.current_token.token_type {
                            TokenType::Comma => self.next_token(),
                            TokenType::RParen | TokenType::Eof => break,
                            _ => {
                                if !self.push_followup_unless_structural_root(
                                    variant_checkpoint,
                                    unexpected_token_with_details(
                                        self.current_token.span(),
                                        "Missing Constructor Field Separator",
                                        DiagnosticCategory::ParserSeparator,
                                        format!(
                                            "I was expecting `,` or `)` between constructor fields, but I found {}.",
                                            self.describe_token_type_for_diagnostic(
                                                self.current_token.token_type
                                            )
                                        ),
                                    ),
                                ) {
                                    break;
                                }
                                let _ = self.recover_to_matching_delimiter(
                                    TokenType::RParen,
                                    &[TokenType::Bar, TokenType::Semicolon],
                                );
                                break;
                            }
                        }
                    }
                }
            }

            variants.push(DataVariant {
                name: var_name,
                fields,
                field_names: None,
                span: self.span_from(var_start),
            });

            // Continue on '|', stop at statement end.
            if self.is_peek_token(TokenType::Bar) {
                self.next_token(); // consume '|'
                self.next_token(); // next constructor start
                continue;
            }
            if self.is_peek_token(TokenType::Semicolon) {
                self.next_token(); // consume ';'
            }
            break;
        }

        Some(Statement::Data {
            // `type Foo = ...` form is the legacy ADT syntax — defaults
            // to private under the new visibility model.
            is_public: false,
            name,
            type_params,
            variants,
            span: self.span_from(start),
            deriving: vec![],
        })
    }

    // ── Type class declarations ──────────────────────────────────────────────

    /// Parses `class [Constraint =>] Name<params> { methods... }`.
    /// current_token is `class` on entry. The `is_public` parameter is set
    /// by the dispatcher in `parse_statement` based on whether `public`
    /// preceded `class` (Proposal 0151).
    pub(super) fn parse_class_statement(&mut self, is_public: bool) -> Option<Statement> {
        let start = self.current_token.position;

        // Parse the head: either `Name<a>` or `Constraint<a> => Name<a>`
        // We parse the first Name<args>, then check for `=>`.
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Class Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected class name or constraint after `class`.".to_string(),
            "Class declarations use `class Name<a> { ... }`.".to_string(),
        ) {
            return None;
        }

        let first_name = self.current_token.symbol.expect("ident should have symbol");
        let first_args = self.parse_type_params_angle_bracket();

        // Check for `=>` — if present, the first name was a superclass constraint.
        // Disambiguation: `class Eq<a> => Ord<a> { ... }`
        //   first_name = Eq, first_args = [a] → superclass constraint
        //   then parse Ord<a> as the actual class name
        let (superclasses, class_name, type_params) = if self.is_peek_token(TokenType::FatArrow) {
            let constraint_span = self.span_from(start);
            let superclass = ClassConstraint {
                class_name: first_name,
                type_args: first_args
                    .iter()
                    .map(|&id| TypeExpr::Named {
                        name: id,
                        args: vec![],
                        span: constraint_span,
                    })
                    .collect(),
                span: constraint_span,
            };
            self.next_token(); // consume `=>`

            // Parse the actual class name and type params.
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Missing Class Name",
                DiagnosticCategory::ParserDeclaration,
                "Expected class name after `=>`.".to_string(),
                "Superclass syntax: `class Eq<a> => Ord<a> { ... }`.".to_string(),
            ) {
                return None;
            }
            let actual_name = self.current_token.symbol.expect("ident should have symbol");
            let actual_params = self.parse_type_params_angle_bracket();
            (vec![superclass], actual_name, actual_params)
        } else {
            (vec![], first_name, first_args)
        };

        // Expect `{`
        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Class Body",
            DiagnosticCategory::ParserDeclaration,
            "Expected `{` to begin class body.".to_string(),
            "Class declarations use `class Name<a> { fn method(x: a) -> ReturnType }`.".to_string(),
        ) {
            return None;
        }
        self.next_token(); // move past `{`

        // Parse methods
        let mut methods: Vec<ClassMethod> = Vec::new();
        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            if let Some(method) = self.parse_class_method() {
                methods.push(method);
            } else {
                // Skip to next method or closing brace
                self.next_token();
            }
        }

        Some(Statement::Class {
            // Proposal 0151: populated from the dispatcher in `parse_statement`.
            is_public,
            name: class_name,
            type_params,
            superclasses,
            methods,
            span: self.span_from(start),
        })
    }

    /// Parse a single method inside a class declaration.
    /// Expects `fn name(params) -> ReturnType` or `fn name(params) -> ReturnType { body }`.
    fn parse_class_method(&mut self) -> Option<ClassMethod> {
        // Expect `fn`
        if !self.is_current_token(TokenType::Fn) {
            return None;
        }
        let start = self.current_token.position;

        // Method name
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Method Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected method name after `fn` in class declaration.".to_string(),
            "Class methods use `fn name(x: a, y: a) -> ReturnType`.".to_string(),
        ) {
            return None;
        }
        let name = self.current_token.symbol.expect("ident should have symbol");

        // Optional per-method type parameters: `<a, b>`
        // parse_type_params_angle_bracket checks for `<` as peek token internally.
        let method_type_params = self.parse_type_params_angle_bracket();

        // `(`
        if !self.expect_peek_context(
            TokenType::LParen,
            "Expected `(` after method name.".to_string(),
            "Class methods use `fn name(x: a, y: a) -> ReturnType`.".to_string(),
        ) {
            return None;
        }

        // Parse parameters with types: (x: a, y: a)
        let mut params = Vec::new();
        let mut param_types = Vec::new();
        self.next_token(); // move past `(`
        while !self.is_current_token(TokenType::RParen) && !self.is_current_token(TokenType::Eof) {
            // Parameter name
            let param_name = self.current_token.symbol.expect("ident should have symbol");
            params.push(param_name);

            // `:` type
            if self.is_peek_token(TokenType::Colon) {
                self.next_token(); // consume `:`
                self.next_token(); // move to type
                let ty = self.parse_type_expr()?;
                param_types.push(ty);
            }

            // Comma or end
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume `,`
                self.next_token(); // next param
            } else {
                break;
            }
        }
        // Consume `)`
        if !self.expect_peek_context(
            TokenType::RParen,
            "Expected `)` after method parameters.".to_string(),
            "".to_string(),
        ) && !self.is_current_token(TokenType::RParen)
        {
            return None;
        }

        // `->` return type
        let return_type = if self.is_peek_token(TokenType::Arrow) {
            self.next_token(); // consume `->`
            self.next_token(); // move to type
            self.parse_type_expr()?
        } else {
            // Default to Unit if no return type
            crate::syntax::type_expr::TypeExpr::Named {
                name: self.lexer.interner_mut().intern("Unit"),
                args: vec![],
                span: Span::default(),
            }
        };

        // Optional `with EffectA, EffectB` clause (Proposal 0151, Phase 4a).
        // Class-method effects act as a *floor* on conforming instances.
        let effects = self.parse_effect_list()?;

        // Optional default body `{ ... }`
        let default_body = if self.is_peek_token(TokenType::LBrace) {
            self.next_token(); // move current to `{`
            let body = self.parse_block();
            // parse_block leaves current_token on `}` — advance past it
            self.next_token();
            Some(body)
        } else {
            // Skip optional comma/newline
            if self.is_peek_token(TokenType::Comma) {
                self.next_token();
            }
            self.next_token(); // advance to next method or `}`
            None
        };

        Some(ClassMethod {
            name,
            type_params: method_type_params,
            params,
            param_types,
            return_type,
            effects,
            default_body,
            span: self.span_from(start),
        })
    }

    /// Parses `instance [Constraint =>] ClassName<TypeArgs> { methods... }`.
    /// current_token is `instance` on entry. The `is_public` parameter is set
    /// by the dispatcher in `parse_statement` based on whether `public`
    /// preceded `instance` (Proposal 0151).
    pub(super) fn parse_instance_statement(&mut self, is_public: bool) -> Option<Statement> {
        let start = self.current_token.position;

        // Parse first name + type args. Could be the class name or a context constraint.
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Instance Class Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected class name after `instance`.".to_string(),
            "Instance declarations use `instance ClassName<Type> { ... }`.".to_string(),
        ) {
            return None;
        }
        let first_name = self.current_token.symbol.expect("ident should have symbol");
        let first_type_args = self.parse_instance_type_args();

        // Check for `=>` — if present, first_name was a context constraint.
        let (context, class_name, type_args) = if self.is_peek_token(TokenType::FatArrow) {
            let constraint_span = self.span_from(start);
            let ctx_constraint = ClassConstraint {
                class_name: first_name,
                type_args: first_type_args,
                span: constraint_span,
            };
            self.next_token(); // consume `=>`

            // Parse the actual class name and type args.
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Missing Instance Class Name",
                DiagnosticCategory::ParserDeclaration,
                "Expected class name after `=>`.".to_string(),
                "Constrained instance syntax: `instance Eq<a> => Eq<List<a>> { ... }`.".to_string(),
            ) {
                return None;
            }
            let actual_name = self.current_token.symbol.expect("ident should have symbol");
            let actual_type_args = self.parse_instance_type_args();
            (vec![ctx_constraint], actual_name, actual_type_args)
        } else {
            (vec![], first_name, first_type_args)
        };

        // Expect `{`
        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Instance Body",
            DiagnosticCategory::ParserDeclaration,
            "Expected `{` to begin instance body.".to_string(),
            "Instance declarations use `instance ClassName<Type> { fn method(...) { ... } }`."
                .to_string(),
        ) {
            return None;
        }
        self.next_token(); // move past `{`

        // Parse methods
        let mut methods: Vec<InstanceMethod> = Vec::new();
        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            if let Some(method) = self.parse_instance_method() {
                methods.push(method);
            } else {
                self.next_token();
            }
        }

        Some(Statement::Instance {
            // Proposal 0151: populated from the dispatcher in `parse_statement`.
            is_public,
            class_name,
            type_args,
            context,
            methods,
            span: self.span_from(start),
        })
    }

    /// Parse `<TypeArgs>` for an instance declaration head.
    fn parse_instance_type_args(&mut self) -> Vec<TypeExpr> {
        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume `<`
            let mut args = Vec::new();
            loop {
                self.next_token();
                if let Some(ty) = self.parse_type_expr() {
                    args.push(ty);
                }
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume `,`
                    continue;
                }
                break;
            }
            let _ = self.expect_peek_context(
                TokenType::Gt,
                "Expected `>` after instance type arguments.".to_string(),
                "".to_string(),
            );
            args
        } else {
            vec![]
        }
    }

    /// Parse a single method inside an instance declaration.
    /// Expects `fn name(params) { body }`.
    fn parse_instance_method(&mut self) -> Option<InstanceMethod> {
        if !self.is_current_token(TokenType::Fn) {
            return None;
        }
        let start = self.current_token.position;

        // Method name
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Method Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected method name after `fn` in instance declaration.".to_string(),
            "Instance methods use `fn name(x, y) { body }`.".to_string(),
        ) {
            return None;
        }
        let name = self.current_token.symbol.expect("ident should have symbol");

        // `(`
        if !self.expect_peek_context(
            TokenType::LParen,
            "Expected `(` after method name.".to_string(),
            "".to_string(),
        ) {
            return None;
        }

        // Parse parameter names (no types needed in instance methods)
        let mut params = Vec::new();
        self.next_token(); // move past `(`
        while !self.is_current_token(TokenType::RParen) && !self.is_current_token(TokenType::Eof) {
            let param_name = self.current_token.symbol.expect("ident should have symbol");
            params.push(param_name);

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume `,`
                self.next_token(); // next param
            } else {
                break;
            }
        }
        // Consume `)`
        if !self.expect_peek_context(
            TokenType::RParen,
            "Expected `)` after method parameters.".to_string(),
            "".to_string(),
        ) && !self.is_current_token(TokenType::RParen)
        {
            return None;
        }

        // Optional `with EffectA, EffectB` clause (Proposal 0151, Phase 4a).
        // Instance-method effects must be a *superset* of the class method's
        // declared row (floor semantics) — checked by the E452 walker.
        let effects = self.parse_effect_list()?;

        // `{` body `}`
        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Method Body",
            DiagnosticCategory::ParserDeclaration,
            "Expected `{` for method body.".to_string(),
            "Instance methods require a body: `fn name(x, y) { ... }`.".to_string(),
        ) {
            return None;
        }
        let body = self.parse_block();
        // parse_block leaves current_token on `}` — advance past it
        // so the instance loop can see the next `fn` or closing `}`.
        self.next_token();

        Some(InstanceMethod {
            name,
            params,
            effects,
            body,
            span: self.span_from(start),
        })
    }

    /// Parse optional type parameters in angle brackets: `<a, b>`.
    /// Returns empty vec if no `<` follows.
    fn parse_type_params_angle_bracket(&mut self) -> Vec<crate::syntax::Identifier> {
        let mut type_params = Vec::new();
        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume `<`
            loop {
                self.next_token(); // move to param name
                if self.is_current_token(TokenType::Gt) {
                    break;
                }
                if let Some(sym) = self.current_token.symbol {
                    type_params.push(sym);
                }
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume `,`
                } else {
                    break;
                }
            }
            // Expect `>`
            if self.is_peek_token(TokenType::Gt) {
                self.next_token();
            } else if !self.is_current_token(TokenType::Gt) {
                // Try to recover
            }
        }
        type_params
    }

    fn parse_function_type_params_angle_bracket(&mut self) -> Option<Vec<FunctionTypeParam>> {
        let mut type_params = Vec::new();
        if !self.is_peek_token(TokenType::Lt) {
            return Some(type_params);
        }

        self.next_token(); // consume `<`
        loop {
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Missing Generic Parameter Name",
                DiagnosticCategory::ParserDeclaration,
                "Expected generic type parameter name.".to_string(),
                "Generic parameters use `fn name<T, U>(...) { ... }` or `fn name<T: Eq + Show>(...) { ... }`.".to_string(),
            ) {
                return None;
            }

            let name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");
            let mut constraints = Vec::new();

            if self.is_peek_token(TokenType::Colon) {
                self.next_token(); // consume `:`
                loop {
                    if !self.expect_peek_context_with_details(
                        TokenType::Ident,
                        "Missing Generic Constraint",
                        DiagnosticCategory::ParserDeclaration,
                        "Expected a type class name after `:` in generic bounds.".to_string(),
                        "Use `fn name<T: Eq>(...) { ... }` or `fn name<T: Eq + Show>(...) { ... }`.".to_string(),
                    ) {
                        return None;
                    }
                    constraints.push(
                        self.current_token
                            .symbol
                            .expect("ident token should have symbol"),
                    );

                    if self.is_peek_token(TokenType::Plus) {
                        self.next_token(); // consume `+`
                    } else {
                        break;
                    }
                }
            }

            type_params.push(FunctionTypeParam { name, constraints });

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume `,`
            } else {
                break;
            }
        }

        if !self.expect_peek_context_with_details(
            TokenType::Gt,
            "Missing Generic Parameter List",
            DiagnosticCategory::ParserDelimiter,
            "Expected `>` to close generic parameter list.".to_string(),
            "Generic parameters use `fn name<T, U>(...) { ... }` or `fn name<T: Eq + Show>(...) { ... }`.".to_string(),
        ) {
            return None;
        }

        Some(type_params)
    }

    /// Parses `effect Name { op: TypeExpr, ... }`.
    /// current_token is `effect` on entry.
    pub(super) fn parse_effect_statement(&mut self) -> Option<Statement> {
        let context_name = if self.is_peek_token(TokenType::Ident) {
            Some(self.peek_token.literal.to_string())
        } else {
            None
        };
        let _effect_context = context_name
            .clone()
            .map(|name| self.enter_parser_context(ParserContext::Effect(name)));
        let start = self.current_token.position;

        // Effect name
        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Effect Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected effect name after `effect`.".to_string(),
            "Effect declarations use `effect Name { op: Type -> Return, ... }`.".to_string(),
        ) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        let mut ops: Vec<EffectOp> = Vec::new();

        // Proposal 0161: bodiless `effect Name` declares a phantom label —
        // no operations, tracked via effect rows only (e.g. Console, FileSystem).
        if !self.is_peek_token(TokenType::LBrace) {
            return Some(Statement::EffectDecl {
                name,
                ops,
                span: self.span_from(start),
            });
        }

        // `{`
        if !self.expect_peek_context_with_details(
            TokenType::LBrace,
            "Missing Effect Body",
            DiagnosticCategory::ParserDeclaration,
            "Expected `{` to begin effect declaration body.".to_string(),
            "Effect declarations use `effect Name { op: Type -> Return, ... }`.".to_string(),
        ) {
            return None;
        }
        self.next_token(); // move past `{`

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            // Optional `fn` keyword prefix (e.g. `fn print: String -> ()`)
            if self.is_current_token(TokenType::Fn) {
                self.next_token(); // skip `fn`
            }

            if self.current_token.token_type != TokenType::Ident {
                self.emit_parser_diagnostic(unexpected_token_with_details(
                    self.current_token.span(),
                    "Invalid Effect Operation",
                    DiagnosticCategory::ParserExpression,
                    format!(
                        "I was expecting an operation name in this `effect` declaration, but I found {}.",
                        self.describe_token_type_for_diagnostic(self.current_token.token_type)
                    ),
                ));
                return None;
            }
            let op_start = self.current_token.position;
            let op_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            // `:` before the type expression
            if self.is_peek_token(TokenType::Colon) {
                self.next_token(); // consume `:`
                self.next_token(); // move to start of TypeExpr
            } else if self.token_starts_type(self.peek_token.token_type) {
                self.emit_parser_diagnostic(unexpected_token_with_details(
                    self.peek_token.span(),
                    "Missing Effect Operation Colon",
                    DiagnosticCategory::ParserSeparator,
                    "Missing `:` in effect operation signature. Write it as `op: Type -> Return`.",
                ));
                self.next_token(); // move to start of TypeExpr
            } else if !self.expect_peek_context(
                TokenType::Colon,
                "Expected `:` after effect operation name.".to_string(),
                "Effect operation signatures use `op: Type -> Return`.".to_string(),
            ) {
                return None;
            } else {
                self.next_token(); // move to start of TypeExpr
            }

            let type_expr = self.parse_type_expr()?;
            let op_end = self.current_token.span().end;

            ops.push(EffectOp {
                name: op_name,
                type_expr,
                span: crate::diagnostics::position::Span::new(op_start, op_end),
            });

            // Optional comma or newline separator
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume ','
            }
            self.next_token(); // advance to next op or `}`
        }

        Some(Statement::EffectDecl {
            name,
            ops,
            span: self.span_from(start),
        })
    }

    /// Parses `alias Name = <E1 | E2 | ...>` (Proposal 0161 B1).
    ///
    /// Effect-row aliases let users give a short name to a decomposed row.
    /// Expansion happens at type-inference time; see
    /// `Compiler::effect_alias_table`.
    pub(super) fn parse_effect_alias_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Effect Alias Name",
            DiagnosticCategory::ParserDeclaration,
            "Expected alias name after `alias`.".to_string(),
            "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
        ) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        if !self.expect_peek_context_with_details(
            TokenType::Assign,
            "Missing `=` in Effect Alias",
            DiagnosticCategory::ParserSeparator,
            "Expected `=` after alias name.".to_string(),
            "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
        ) {
            return None;
        }

        if !self.expect_peek_context_with_details(
            TokenType::Lt,
            "Missing `<` in Effect Alias Body",
            DiagnosticCategory::ParserSeparator,
            "Expected `<` to begin the effect-row body.".to_string(),
            "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
        ) {
            return None;
        }

        let expansion = self.parse_effect_alias_body()?;

        Some(Statement::EffectAlias {
            name,
            expansion,
            span: self.span_from(start),
        })
    }

    /// Parses the `<E1 | E2 | ...>` body of an alias declaration.
    ///
    /// Each atom is an identifier (either an effect label like `Console` or
    /// another alias). `|` joins atoms via `EffectExpr::Add`. A trailing
    /// `>` closes the row. This is a stripped-down parser specific to the
    /// alias body — it does not accept row-tail variables or subtraction,
    /// which are meaningful at use-site rows but not inside an alias.
    fn parse_effect_alias_body(&mut self) -> Option<crate::syntax::effect_expr::EffectExpr> {
        use crate::syntax::effect_expr::EffectExpr;

        if !self.expect_peek_context_with_details(
            TokenType::Ident,
            "Missing Effect Name in Alias",
            DiagnosticCategory::ParserDeclaration,
            "Expected an effect label name inside `<...>`.".to_string(),
            "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
        ) {
            return None;
        }
        let first_name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");
        let first_span = self.current_token.span();
        let mut expr = EffectExpr::Named {
            name: first_name,
            span: first_span,
        };

        while self.is_peek_token(TokenType::Bar) {
            self.next_token(); // consume `|`
            if !self.expect_peek_context_with_details(
                TokenType::Ident,
                "Missing Effect Name After `|`",
                DiagnosticCategory::ParserDeclaration,
                "Expected an effect label name after `|` in an alias body.".to_string(),
                "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
            ) {
                return None;
            }
            let atom_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");
            let atom_span = self.current_token.span();
            let atom = EffectExpr::Named {
                name: atom_name,
                span: atom_span,
            };
            let combined_span = Span::new(expr.span().start, atom_span.end);
            expr = EffectExpr::Add {
                left: Box::new(expr),
                right: Box::new(atom),
                span: combined_span,
            };
        }

        if !self.expect_peek_context_with_details(
            TokenType::Gt,
            "Missing `>` in Effect Alias Body",
            DiagnosticCategory::ParserSeparator,
            "Expected `>` to close the effect-row body.".to_string(),
            "Effect aliases use `alias Name = <E1 | E2 | ...>`.".to_string(),
        ) {
            return None;
        }

        Some(expr)
    }
}
