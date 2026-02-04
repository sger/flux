use std::fmt;

use crate::frontend::{position::Span, statement::Statement};

#[derive(Debug, Clone)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub span: Span,
}

impl Program {
    pub fn new() -> Self {
        Self {
            statements: Vec::new(),
            span: Span::default(),
        }
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for statement in &self.statements {
            write!(f, "{}", statement)?;
        }
        Ok(())
    }
}
