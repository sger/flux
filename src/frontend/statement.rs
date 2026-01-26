use std::fmt;

use crate::frontend::{Identifier, block::Block, expression::Expression};

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        name: Identifier,
        value: Expression,
    },
    Return {
        value: Option<Expression>,
    },
    Expression {
        expression: Expression,
    },
    Function {
        name: Identifier,
        parameters: Vec<Identifier>,
        body: Block,
    },
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Let { name, value } => {
                write!(f, "let {} = {};", name, value)
            }
            Statement::Return { value: Some(v) } => {
                write!(f, "return {};", v)
            }
            Statement::Return { value: None } => {
                write!(f, "return;")
            }
            Statement::Expression { expression } => {
                write!(f, "{}", expression)
            }
            Statement::Function {
                name,
                parameters,
                body,
            } => {
                write!(f, "fun {}({}) {}", name, parameters.join(", "), body)
            }
        }
    }
}
