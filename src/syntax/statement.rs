use std::fmt;

use crate::{
    diagnostics::position::{Position, Span},
    syntax::{
        Identifier,
        block::Block,
        expression::{Expression, Pattern},
        interner::Interner,
    },
};

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        name: Identifier,
        value: Expression,
        span: Span,
    },
    LetDestructure {
        pattern: Pattern,
        value: Expression,
        span: Span,
    },
    Return {
        value: Option<Expression>,
        span: Span,
    },
    Expression {
        expression: Expression,
        has_semicolon: bool,
        span: Span,
    },
    Function {
        name: Identifier,
        parameters: Vec<Identifier>,
        body: Block,
        span: Span,
    },
    Assign {
        name: Identifier,
        value: Expression,
        span: Span,
    },
    Module {
        name: Identifier,
        body: Block,
        span: Span,
    },
    Import {
        name: Identifier,
        alias: Option<Identifier>,
        span: Span,
    },
}

impl Statement {
    pub fn position(&self) -> Position {
        match self {
            Statement::Let { span, .. } => span.start,
            Statement::LetDestructure { span, .. } => span.start,
            Statement::Return { span, .. } => span.start,
            Statement::Expression { span, .. } => span.start,
            Statement::Function { span, .. } => span.start,
            Statement::Assign { span, .. } => span.start,
            Statement::Module { span, .. } => span.start,
            Statement::Import { span, .. } => span.start,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Statement::Let { span, .. } => *span,
            Statement::LetDestructure { span, .. } => *span,
            Statement::Return { span, .. } => *span,
            Statement::Expression { span, .. } => *span,
            Statement::Function { span, .. } => *span,
            Statement::Assign { span, .. } => *span,
            Statement::Module { span, .. } => *span,
            Statement::Import { span, .. } => *span,
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Let { name, value, .. } => {
                write!(f, "let {} = {};", name, value)
            }
            Statement::LetDestructure { pattern, value, .. } => {
                write!(f, "let {} = {};", pattern, value)
            }
            Statement::Return { value: Some(v), .. } => {
                write!(f, "return {};", v)
            }
            Statement::Return { value: None, .. } => {
                write!(f, "return;")
            }
            Statement::Expression {
                expression,
                has_semicolon,
                ..
            } => {
                if *has_semicolon {
                    write!(f, "{};", expression)
                } else {
                    write!(f, "{}", expression)
                }
            }
            Statement::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let params: Vec<String> = parameters.iter().map(|p| p.to_string()).collect();
                write!(f, "fn {}({}) {}", name, params.join(", "), body)
            }
            Statement::Assign { name, value, .. } => {
                write!(f, "{} = {};", name, value)
            }
            Statement::Module { name, body, .. } => {
                write!(f, "module {} {}", name, body)
            }
            Statement::Import { name, .. } => {
                if let Some(alias) = &self.get_import_alias() {
                    write!(f, "import {} as {}", name, alias)
                } else {
                    write!(f, "import {}", name)
                }
            }
        }
    }
}

impl Statement {
    fn get_import_alias(&self) -> Option<&Identifier> {
        match self {
            Statement::Import { alias, .. } => alias.as_ref(),
            _ => None,
        }
    }

    /// Formats this statement using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Statement::Let { name, value, .. } => {
                format!(
                    "let {} = {};",
                    interner.resolve(*name),
                    value.display_with(interner)
                )
            }
            Statement::LetDestructure { pattern, value, .. } => {
                format!(
                    "let {} = {};",
                    pattern.display_with(interner),
                    value.display_with(interner)
                )
            }
            Statement::Return { value: Some(v), .. } => {
                format!("return {};", v.display_with(interner))
            }
            Statement::Return { value: None, .. } => "return;".to_string(),
            Statement::Expression {
                expression,
                has_semicolon,
                ..
            } => {
                if *has_semicolon {
                    format!("{};", expression.display_with(interner))
                } else {
                    expression.display_with(interner)
                }
            }
            Statement::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let params: Vec<&str> = parameters.iter().map(|p| interner.resolve(*p)).collect();
                format!(
                    "fn {}({}) {}",
                    interner.resolve(*name),
                    params.join(", "),
                    body
                )
            }
            Statement::Assign { name, value, .. } => {
                format!(
                    "{} = {};",
                    interner.resolve(*name),
                    value.display_with(interner)
                )
            }
            Statement::Module { name, body, .. } => {
                format!("module {} {}", interner.resolve(*name), body)
            }
            Statement::Import { name, alias, .. } => {
                if let Some(alias) = alias {
                    format!(
                        "import {} as {}",
                        interner.resolve(*name),
                        interner.resolve(*alias)
                    )
                } else {
                    format!("import {}", interner.resolve(*name))
                }
            }
        }
    }
}
