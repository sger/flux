use std::collections::HashMap;
use std::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{interner::Interner, statement::Statement},
};

#[derive(Clone)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub span: Span,
    /// `///` / `/** */` doc comments captured by the parser, keyed by the
    /// 1-based source line of the declaration each contiguous run sits
    /// immediately above (i.e. the line directly below the run). Lets tooling
    /// recover a declaration's documentation straight from the parsed AST
    /// rather than re-scanning source text. A derived index, not structural
    /// AST — it is intentionally excluded from `Debug` (see the impl below) so
    /// AST snapshots stay stable.
    pub doc_comments: HashMap<usize, String>,
}

impl Program {
    pub fn new() -> Self {
        Self {
            statements: Vec::new(),
            span: Span::default(),
            doc_comments: HashMap::new(),
        }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    /// The doc comment attached to a declaration starting on `line` (1-based),
    /// if the parser captured a `///`/`/** */` run immediately above it.
    pub fn doc_for_line(&self, line: usize) -> Option<&str> {
        self.doc_comments.get(&line).map(String::as_str)
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

// Manual `Debug` that omits `doc_comments`: it is a derived doc index, not part
// of the structural AST, and excluding it keeps the `Program { statements, span }`
// shape that AST snapshots assert — so adding the field churns no snapshots.
impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program")
            .field("statements", &self.statements)
            .field("span", &self.span)
            .finish()
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
