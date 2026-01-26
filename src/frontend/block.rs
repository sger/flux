use std::fmt;

use crate::frontend::statement::Statement;

#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{ ")?;
        for statement in &self.statements {
            write!(f, "{} ", statement)?;
        }
        write!(f, "}}")
    }
}
