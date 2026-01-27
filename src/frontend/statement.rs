use std::fmt;

use crate::frontend::{Identifier, block::Block, expression::Expression, position::Position};

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        name: Identifier,
        value: Expression,
        position: Position,
    },
    Return {
        value: Option<Expression>,
        position: Position,
    },
    Expression {
        expression: Expression,
        position: Position,
    },
    Function {
        name: Identifier,
        parameters: Vec<Identifier>,
        body: Block,
        position: Position,
    },
    Assign {
        name: Identifier,
        value: Expression,
        position: Position,
    },
}

impl Statement {
    pub fn position(&self) -> Position {
        match self {
            Statement::Let { position, .. } => *position,
            Statement::Return { position, .. } => *position,
            Statement::Expression { position, .. } => *position,
            Statement::Function { position, .. } => *position,
            Statement::Assign { position, .. } => *position,
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
        }
    }
}
