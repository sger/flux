use std::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{interner::Interner, statement::Statement},
};

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

    /// Formats this program using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        self.statements
            .iter()
            .map(|s| s.display_with(interner))
            .collect::<Vec<_>>()
            .join("")
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
