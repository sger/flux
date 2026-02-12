use std::fmt;

use crate::{diagnostics::position::Span, syntax::statement::Statement};

#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
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

impl Block {
    pub fn span(&self) -> Span {
        self.span
    }
}
