use core::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
};

#[derive(Debug, Clone, PartialEq)]
pub enum EffectExpr {
    Named { name: Identifier, span: Span },
}

impl EffectExpr {
    pub fn span(&self) -> Span {
        match self {
            EffectExpr::Named { span, .. } => *span,
        }
    }

    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            EffectExpr::Named { name, .. } => interner.resolve(*name).to_string(),
        }
    }
}

impl fmt::Display for EffectExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectExpr::Named { name, .. } => write!(f, "{}", name),
        }
    }
}
