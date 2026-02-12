use std::collections::HashMap;

use crate::{
    ast::{Visitor, complexity::analyze_complexity, visit},
    diagnostics::{
        Diagnostic, DiagnosticBuilder,
        position::{Position, Span},
    },
    syntax::{
        Identifier,
        expression::{Expression, Pattern},
        interner::Interner,
        module_graph::is_valid_module_name,
        program::Program,
        statement::Statement,
        symbol::Symbol,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Let,
    Param,
    Import,
    Function,
}

#[derive(Debug, Clone)]
struct BindingInfo {
    position: Position,
    used: bool,
    kind: BindingKind,
}

pub struct Linter<'a> {
    scopes: Vec<HashMap<Symbol, BindingInfo>>,
    warnings: Vec<Diagnostic>,
    file: Option<String>,
    interner: &'a Interner,
}

const MAX_FUNCTION_LINES: usize = 50;
const MAX_FUNCTION_PARAMS: usize = 5;
const MAX_CYCLOMATIC_COMPLEXITY: usize = 10;
const MAX_NESTING_DEPTH: usize = 4;

impl<'a> Linter<'a> {
    pub fn new(file: Option<String>, interner: &'a Interner) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            warnings: Vec::new(),
            file,
            interner,
        }
    }

    pub fn lint(mut self, program: &Program) -> Vec<Diagnostic> {
        // Analyze complexity metrics for all functions
        let complexity_metrics = analyze_complexity(program);
        for metric in complexity_metrics {
            self.check_complexity_metrics(&metric);
        }

        // Then do regular linting
        self.visit_program(program);
        self.finish_scope();
        self.warnings
    }

    #[inline]
    fn sym(&self, s: Symbol) -> &str {
        self.interner.resolve(s)
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn lint_block_statements(&mut self, statements: &[Statement]) {
        let mut unreachable = false;
        for statement in statements {
            if unreachable {
                self.push_warning(
                    "DEAD CODE",
                    "W008",
                    statement.span().start,
                    "Unreachable code after return.".to_string(),
                );
            }
            self.visit_stmt(statement);
            if matches!(statement, Statement::Return { .. }) {
                unreachable = true;
            }
        }
    }

    fn finish_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            for (name, info) in scope {
                if info.used {
                    continue;
                }
                let name_str = self.sym(name);
                if name_str.starts_with('_') {
                    continue;
                }
                match info.kind {
                    BindingKind::Let => self.push_warning(
                        "UNUSED VARIABLE",
                        "W001",
                        info.position,
                        format!("`{}` is never used.", self.sym(name)),
                    ),
                    BindingKind::Param => self.push_warning(
                        "UNUSED PARAMETER",
                        "W002",
                        info.position,
                        format!("`{}` is never used.", self.sym(name)),
                    ),
                    BindingKind::Import => self.push_warning(
                        "UNUSED IMPORT",
                        "W003",
                        info.position,
                        format!("`{}` is never used.", self.sym(name)),
                    ),
                    BindingKind::Function => self.push_warning(
                        "UNUSED FUNCTION",
                        "W007",
                        info.position,
                        format!("`{}` is never called.", self.sym(name)),
                    ),
                }
            }
        }
    }

    fn define_binding(&mut self, name: Symbol, position: Position, kind: BindingKind) {
        if self.is_shadowing(name) {
            let name_str = self.sym(name);
            self.push_warning(
                "SHADOWED NAME",
                "W004",
                position,
                format!("`{}` shadows a binding from an outer scope.", name_str),
            );
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name,
                BindingInfo {
                    position,
                    used: false,
                    kind,
                },
            );
        }
    }

    fn is_shadowing(&self, name: Symbol) -> bool {
        if self.scopes.len() <= 1 {
            return false;
        }
        for scope in self.scopes.iter().take(self.scopes.len() - 1).rev() {
            if scope.contains_key(&name) {
                return true;
            }
        }
        false
    }

    fn mark_used(&mut self, name: Symbol) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(&name) {
                info.used = true;
                return;
            }
        }
    }

    fn extract_pattern_bindings(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.define_binding(*name, Position::default(), BindingKind::Let);
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => {
                self.extract_pattern_bindings(pattern);
            }
            Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
        }
    }

    fn push_warning(&mut self, title: &str, code: &str, position: Position, message: String) {
        let mut diag = Diagnostic::warning(title)
            .with_code(code)
            .with_message(message)
            .with_position(position);
        if let Some(file) = &self.file {
            diag = diag.with_file(file.clone());
        }
        self.warnings.push(diag);
    }

    fn check_complexity_metrics(&mut self, metrics: &crate::ast::complexity::FunctionMetrics) {
        let name_str = if let Some(name) = metrics.name {
            self.sym(name).to_string()
        } else {
            "<anonymous>".to_string()
        };

        // W011: High cyclomatic complexity
        if metrics.cyclomatic_complexity > MAX_CYCLOMATIC_COMPLEXITY {
            self.push_warning(
                "HIGH CYCLOMATIC COMPLEXITY",
                "W011",
                metrics.span.start,
                format!(
                    "Function `{}` has cyclomatic complexity {} (max {}).",
                    name_str, metrics.cyclomatic_complexity, MAX_CYCLOMATIC_COMPLEXITY
                ),
            );
        }

        // W012: Deep nesting
        if metrics.max_nesting_depth > MAX_NESTING_DEPTH {
            self.push_warning(
                "DEEP NESTING",
                "W012",
                metrics.span.start,
                format!(
                    "Function `{}` has nesting depth {} (max {}).",
                    name_str, metrics.max_nesting_depth, MAX_NESTING_DEPTH
                ),
            );
        }
    }

    fn check_function_complexity(
        &mut self,
        name: Option<Identifier>,
        parameters: &[Identifier],
        body: &crate::syntax::block::Block,
        position: Position,
    ) {
        if parameters.len() > MAX_FUNCTION_PARAMS {
            let label = name.map(|n| self.sym(n)).unwrap_or("<anonymous>");
            self.push_warning(
                "TOO MANY PARAMETERS",
                "W010",
                position,
                format!(
                    "Function `{}` has {} parameters (max {}).",
                    label,
                    parameters.len(),
                    MAX_FUNCTION_PARAMS
                ),
            );
        }

        let line_count = span_line_count(body.span());
        if line_count > MAX_FUNCTION_LINES {
            let label = name.map(|n| self.sym(n)).unwrap_or("<anonymous>");
            self.push_warning(
                "FUNCTION TOO LONG",
                "W009",
                position,
                format!(
                    "Function `{}` is {} lines long (max {}).",
                    label, line_count, MAX_FUNCTION_LINES
                ),
            );
        }
    }
}

impl<'ast, 'a> Visitor<'ast> for Linter<'a> {
    fn visit_program(&mut self, program: &'ast Program) {
        self.lint_block_statements(&program.statements);
    }

    fn visit_block(&mut self, block: &'ast super::block::Block) {
        self.lint_block_statements(&block.statements);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Let { name, value, span } => {
                self.visit_expr(value);
                self.define_binding(*name, span.start, BindingKind::Let);
            }
            Statement::Assign {
                name,
                value,
                span: _,
            } => {
                self.mark_used(*name);
                self.visit_expr(value);
            }
            Statement::Function {
                name,
                parameters,
                body,
                span,
            } => {
                self.check_function_complexity(Some(*name), parameters, body, span.start);
                let name_str = self.sym(*name);
                if !is_snake_case(name_str) {
                    self.push_warning(
                        "FUNCTION NAME STYLE",
                        "W005",
                        span.start,
                        format!("`{}` should be snake_case.", name_str),
                    );
                }
                self.define_binding(*name, span.start, BindingKind::Function);
                self.enter_scope();
                for param in parameters {
                    self.define_binding(*param, span.start, BindingKind::Param);
                }
                self.lint_block_statements(&body.statements);
                self.finish_scope();
            }
            Statement::Module { name, body, span } => {
                self.define_binding(*name, span.start, BindingKind::Function);
                self.enter_scope();
                self.lint_block_statements(&body.statements);
                self.finish_scope();
            }
            Statement::Import { name, alias, span } => {
                let name_str = self.sym(*name);
                if !is_valid_module_name(name_str) {
                    self.push_warning(
                        "IMPORT NAME STYLE",
                        "W006",
                        span.start,
                        format!(
                            "`{}` should use UpperCamelCase segments separated by dots.",
                            name
                        ),
                    );
                }
                let binding = alias.unwrap_or(*name);
                self.define_binding(binding, span.start, BindingKind::Import);
            }
            // Return and Expression: walk_stmt recurses into child expressions
            Statement::Return { .. } | Statement::Expression { .. } => {
                visit::walk_stmt(self, stmt);
            }
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Identifier { name, .. } => {
                self.mark_used(*name);
            }
            Expression::Function {
                parameters,
                body,
                span,
            } => {
                self.check_function_complexity(None, parameters, body, span.start);
                self.enter_scope();
                for param in parameters {
                    self.define_binding(*param, span.start, BindingKind::Param);
                }
                self.lint_block_statements(&body.statements);
                self.finish_scope();
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.enter_scope();
                    self.extract_pattern_bindings(&arm.pattern);

                    // Visit guard expression if present
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }

                    self.visit_expr(&arm.body);
                    self.finish_scope();
                }
            }
            // All other variants: walk_expr handles recursion into children.
            // walk_expr exhaustively destructures Expression, so adding a new
            // variant will cause a compile error there.
            _ => visit::walk_expr(self, expr),
        }
    }
}

fn span_line_count(span: Span) -> usize {
    if span.start.line == 0 || span.end.line == 0 {
        return 0;
    }
    span.end.line.saturating_sub(span.start.line) + 1
}

fn is_snake_case(name: &str) -> bool {
    let trimmed = name.trim_start_matches('_');
    if trimmed.is_empty() {
        return true;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && !trimmed.contains("__")
}
