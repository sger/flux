use std::collections::HashMap;

use crate::frontend::{
    diagnostic::Diagnostic,
    expression::Expression,
    module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
    position::Position,
    program::Program,
    statement::Statement,
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

pub struct Linter {
    scopes: Vec<HashMap<String, BindingInfo>>,
    warnings: Vec<Diagnostic>,
    file: Option<String>,
}

impl Linter {
    pub fn new(file: Option<String>) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            warnings: Vec::new(),
            file,
        }
    }

    pub fn lint(mut self, program: &Program) -> Vec<Diagnostic> {
        for statement in &program.statements {
            self.lint_statement(statement);
        }
        self.finish_scope();
        self.warnings
    }

    fn lint_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Let { name, value, span } => {
                self.lint_expression(value);
                self.define_binding(name, span.start, BindingKind::Let);
            }
            Statement::Assign {
                name,
                value,
                span: _,
            } => {
                self.mark_used(name);
                self.lint_expression(value);
            }
            Statement::Return { value, .. } => {
                if let Some(expr) = value {
                    self.lint_expression(expr);
                }
            }
            Statement::Expression { expression, .. } => {
                self.lint_expression(expression);
            }
            Statement::Function {
                name,
                parameters,
                body,
                span,
                ..
            } => {
                if !is_snake_case(name) {
                    self.push_warning(
                        "FUNCTION NAME STYLE",
                        "W005",
                        span.start,
                        format!("`{}` should be snake_case.", name),
                    );
                }
                self.define_binding(name, span.start, BindingKind::Function);
                self.enter_scope();
                for param in parameters {
                    self.define_binding(param, span.start, BindingKind::Param);
                }
                for stmt in &body.statements {
                    self.lint_statement(stmt);
                }
                self.finish_scope();
            }
            Statement::Module { name, body, span } => {
                let binding = module_binding_name(name);
                self.define_binding(binding, span.start, BindingKind::Function);
                self.enter_scope();
                for stmt in &body.statements {
                    self.lint_statement(stmt);
                }
                self.finish_scope();
            }
            Statement::Import { name, alias, span } => {
                if !is_valid_module_name(name) {
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
                let binding = import_binding_name(name, alias.as_deref());
                self.define_binding(binding, span.start, BindingKind::Import);
            }
        }
    }

    fn lint_expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier { name, .. } => {
                self.mark_used(name);
            }
            Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. } => {}
            Expression::Prefix { right, .. } => self.lint_expression(right),
            Expression::Infix { left, right, .. } => {
                self.lint_expression(left);
                self.lint_expression(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.lint_expression(condition);
                for stmt in &consequence.statements {
                    self.lint_statement(stmt);
                }
                if let Some(alt) = alternative {
                    for stmt in &alt.statements {
                        self.lint_statement(stmt);
                    }
                }
            }
            Expression::Function {
                parameters, body, ..
            } => {
                self.enter_scope();
                for param in parameters {
                    self.define_binding(param, Position::default(), BindingKind::Param);
                }
                for stmt in &body.statements {
                    self.lint_statement(stmt);
                }
                self.finish_scope();
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.lint_expression(function);
                for arg in arguments {
                    self.lint_expression(arg);
                }
            }
            Expression::Array { elements, .. } => {
                for el in elements {
                    self.lint_expression(el);
                }
            }
            Expression::Index { left, index, .. } => {
                self.lint_expression(left);
                self.lint_expression(index);
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.lint_expression(k);
                    self.lint_expression(v);
                }
            }
            Expression::MemberAccess { object, .. } => {
                self.lint_expression(object);
            }
            Expression::Some { value, .. } => {
                self.lint_expression(value);
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.lint_expression(scrutinee);
                for arm in arms {
                    // TODO: Handle pattern bindings properly
                    self.lint_expression(&arm.body);
                }
            }
        }
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn finish_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            for (name, info) in scope {
                if info.used {
                    continue;
                }
                if name.starts_with('_') {
                    continue;
                }
                match info.kind {
                    BindingKind::Let => self.push_warning(
                        "UNUSED VARIABLE",
                        "W001",
                        info.position,
                        format!("`{}` is never used.", name),
                    ),
                    BindingKind::Param => self.push_warning(
                        "UNUSED PARAMETER",
                        "W002",
                        info.position,
                        format!("`{}` is never used.", name),
                    ),
                    BindingKind::Import => self.push_warning(
                        "UNUSED IMPORT",
                        "W003",
                        info.position,
                        format!("`{}` is never used.", name),
                    ),
                    BindingKind::Function => {}
                }
            }
        }
    }

    fn define_binding(&mut self, name: &str, position: Position, kind: BindingKind) {
        if self.is_shadowing(name) {
            self.push_warning(
                "SHADOWED NAME",
                "W004",
                position,
                format!("`{}` shadows a binding from an outer scope.", name),
            );
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name.to_string(),
                BindingInfo {
                    position,
                    used: false,
                    kind,
                },
            );
        }
    }

    fn is_shadowing(&self, name: &str) -> bool {
        if self.scopes.len() <= 1 {
            return false;
        }
        for scope in self.scopes.iter().take(self.scopes.len() - 1).rev() {
            if scope.contains_key(name) {
                return true;
            }
        }
        false
    }

    fn mark_used(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.used = true;
                return;
            }
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
