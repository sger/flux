use std::fmt;

use crate::frontend::{Identifier, block::Block, expression::Expression, position::{Position, Span}};

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        name: Identifier,
        value: Expression,
        span: Span,
    },
    Return {
        value: Option<Expression>,
        span: Span,
    },
    Expression {
        expression: Expression,
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
            Statement::Return { value: Some(v), .. } => {
                write!(f, "return {};", v)
            }
            Statement::Return { value: None, .. } => {
                write!(f, "return;")
            }
            Statement::Expression { expression, .. } => {
                write!(f, "{}", expression)
            }
            Statement::Function {
                name,
                parameters,
                body,
                ..
            } => {
                write!(f, "fun {}({}) {}", name, parameters.join(", "), body)
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
}
