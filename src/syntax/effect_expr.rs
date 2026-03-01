use core::fmt;
use std::collections::HashSet;

use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
};

#[derive(Debug, Clone, PartialEq)]
pub enum EffectExpr {
    Named {
        name: Identifier,
        span: Span,
    },
    Add {
        left: Box<EffectExpr>,
        right: Box<EffectExpr>,
        span: Span,
    },
    Subtract {
        left: Box<EffectExpr>,
        right: Box<EffectExpr>,
        span: Span,
    },
}

impl EffectExpr {
    pub fn span(&self) -> Span {
        match self {
            EffectExpr::Named { span, .. } => *span,
            EffectExpr::Add { span, .. } => *span,
            EffectExpr::Subtract { span, .. } => *span,
        }
    }

    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            EffectExpr::Named { name, .. } => interner.resolve(*name).to_string(),
            EffectExpr::Add { left, right, .. } => {
                format!(
                    "{} + {}",
                    left.display_with(interner),
                    right.display_with(interner)
                )
            }
            EffectExpr::Subtract { left, right, .. } => {
                format!(
                    "{} - {}",
                    left.display_with(interner),
                    right.display_with(interner)
                )
            }
        }
    }

    pub fn referenced_names(&self) -> HashSet<Identifier> {
        let mut out = HashSet::new();
        self.collect_referenced_names(&mut out);
        out
    }

    pub fn normalized_names(&self) -> HashSet<Identifier> {
        match self {
            EffectExpr::Named { name, .. } => HashSet::from([*name]),
            EffectExpr::Add { left, right, .. } => {
                let mut out = left.normalized_names();
                out.extend(right.normalized_names());
                out
            }
            EffectExpr::Subtract { left, right, .. } => {
                let mut out = left.normalized_names();
                for name in right.normalized_names() {
                    out.remove(&name);
                }
                out
            }
        }
    }

    fn collect_referenced_names(&self, out: &mut HashSet<Identifier>) {
        match self {
            EffectExpr::Named { name, .. } => {
                out.insert(*name);
            }
            EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
                left.collect_referenced_names(out);
                right.collect_referenced_names(out);
            }
        }
    }
}

impl fmt::Display for EffectExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectExpr::Named { name, .. } => write!(f, "{}", name),
            EffectExpr::Add { left, right, .. } => write!(f, "{} + {}", left, right),
            EffectExpr::Subtract { left, right, .. } => write!(f, "{} - {}", left, right),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        diagnostics::position::Span,
        syntax::{effect_expr::EffectExpr, interner::Interner},
    };

    #[test]
    fn normalized_names_apply_add_and_subtract() {
        let mut interner = Interner::new();
        let io = interner.intern("IO");
        let console = interner.intern("Console");
        let time = interner.intern("Time");
        let span = Span::default();

        let expr = EffectExpr::Subtract {
            left: Box::new(EffectExpr::Add {
                left: Box::new(EffectExpr::Add {
                    left: Box::new(EffectExpr::Named { name: io, span }),
                    right: Box::new(EffectExpr::Named {
                        name: console,
                        span,
                    }),
                    span,
                }),
                right: Box::new(EffectExpr::Named { name: time, span }),
                span,
            }),
            right: Box::new(EffectExpr::Named {
                name: console,
                span,
            }),
            span,
        };

        let names = expr.normalized_names();
        assert!(names.contains(&io));
        assert!(names.contains(&time));
        assert!(!names.contains(&console));
    }
}
