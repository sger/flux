use core::fmt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectExpr {
    /// A concrete effect atom, such as `IO` or `Time`.
    Named { name: Identifier, span: Span },
    /// An open row variable, rendered as `|e`.
    RowVar { name: Identifier, span: Span },
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
    /// Returns the source span that produced this effect expression.
    pub fn span(&self) -> Span {
        match self {
            EffectExpr::Named { span, .. } => *span,
            EffectExpr::RowVar { span, .. } => *span,
            EffectExpr::Add { span, .. } => *span,
            EffectExpr::Subtract { span, .. } => *span,
        }
    }

    /// Pretty-prints the expression using interned symbol names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            EffectExpr::Named { name, .. } => interner.resolve(*name).to_string(),
            EffectExpr::RowVar { name, .. } => format!("|{}", interner.resolve(*name)),
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

    /// Returns the first row variable referenced by this expression, if any.
    pub fn row_var(&self) -> Option<Identifier> {
        match self {
            EffectExpr::RowVar { name, .. } => Some(*name),
            EffectExpr::Named { .. } => None,
            EffectExpr::Add { left, right, .. } => left.row_var().or_else(|| right.row_var()),
            EffectExpr::Subtract { left, right, .. } => left.row_var().or_else(|| right.row_var()),
        }
    }

    /// Returns `true` when the effect expression contains a row variable.
    pub fn is_open(&self) -> bool {
        self.row_var().is_some()
    }

    /// Normalizes to concrete effect atoms only, excluding row variables.
    pub fn normalized_concrete_names(&self) -> HashSet<Identifier> {
        match self {
            EffectExpr::Named { name, .. } => HashSet::from([*name]),
            EffectExpr::RowVar { .. } => HashSet::new(),
            EffectExpr::Add { left, right, .. } => {
                let mut out = left.normalized_concrete_names();
                out.extend(right.normalized_concrete_names());
                out
            }
            EffectExpr::Subtract { left, right, .. } => {
                let mut out = left.normalized_concrete_names();
                for name in right.normalized_concrete_names() {
                    out.remove(&name);
                }
                out
            }
        }
    }

    /// Returns `true` if this expression contains at least one `Add` node.
    pub fn contains_add(&self) -> bool {
        match self {
            EffectExpr::Add { .. } => true,
            EffectExpr::Named { .. } | EffectExpr::RowVar { .. } => false,
            EffectExpr::Subtract { left, right, .. } => {
                left.contains_add() || right.contains_add()
            }
        }
    }

    /// Returns `true` if this expression contains at least one `Subtract` node.
    /// Used by the linter to distinguish genuine row arithmetic (which keeps
    /// `+`) from a `+`-only list that should use `,` in `with` clauses.
    pub fn contains_subtract(&self) -> bool {
        match self {
            EffectExpr::Subtract { .. } => true,
            EffectExpr::Named { .. } | EffectExpr::RowVar { .. } => false,
            EffectExpr::Add { left, right, .. } => {
                left.contains_subtract() || right.contains_subtract()
            }
        }
    }

    pub fn referenced_names(&self) -> HashSet<Identifier> {
        let mut out = HashSet::new();
        self.collect_referenced_names(&mut out);
        out
    }

    /// Normalizes effect atoms using `+`/`-` semantics.
    ///
    /// Row variables are intentionally excluded from the result.
    pub fn normalized_names(&self) -> HashSet<Identifier> {
        match self {
            EffectExpr::Named { name, .. } => HashSet::from([*name]),
            EffectExpr::RowVar { .. } => HashSet::new(),
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

    /// Expand any `Named` atom whose identifier is a key in `aliases` into
    /// the aliased expansion, recursively. Non-alias atoms and row variables
    /// pass through unchanged. Alias expansions may not reference other
    /// aliases (Proposal 0161 B1 — non-recursive aliases); if they do, this
    /// expander will not follow the chain.
    ///
    /// Span of each replacement inherits the original reference's span so
    /// that downstream diagnostics point at the user-written source, not at
    /// the alias definition.
    pub fn expand_aliases(&self, aliases: &HashMap<Identifier, EffectExpr>) -> EffectExpr {
        match self {
            EffectExpr::Named { name, span } => {
                if let Some(expansion) = aliases.get(name) {
                    expansion.with_span(*span)
                } else {
                    self.clone()
                }
            }
            EffectExpr::RowVar { .. } => self.clone(),
            EffectExpr::Add { left, right, span } => EffectExpr::Add {
                left: Box::new(left.expand_aliases(aliases)),
                right: Box::new(right.expand_aliases(aliases)),
                span: *span,
            },
            EffectExpr::Subtract { left, right, span } => EffectExpr::Subtract {
                left: Box::new(left.expand_aliases(aliases)),
                right: Box::new(right.expand_aliases(aliases)),
                span: *span,
            },
        }
    }

    /// Clone this expression replacing every internal span with `span`.
    /// Used when inlining an alias body at a call site so the substituted
    /// row points at the reference's source location.
    fn with_span(&self, span: Span) -> EffectExpr {
        match self {
            EffectExpr::Named { name, .. } => EffectExpr::Named { name: *name, span },
            EffectExpr::RowVar { name, .. } => EffectExpr::RowVar { name: *name, span },
            EffectExpr::Add { left, right, .. } => EffectExpr::Add {
                left: Box::new(left.with_span(span)),
                right: Box::new(right.with_span(span)),
                span,
            },
            EffectExpr::Subtract { left, right, .. } => EffectExpr::Subtract {
                left: Box::new(left.with_span(span)),
                right: Box::new(right.with_span(span)),
                span,
            },
        }
    }

    fn collect_referenced_names(&self, out: &mut HashSet<Identifier>) {
        match self {
            EffectExpr::Named { name, .. } => {
                out.insert(*name);
            }
            EffectExpr::RowVar { name, .. } => {
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
            EffectExpr::RowVar { name, .. } => write!(f, "|{}", name),
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
        let io = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let console = interner.intern("Console");
        let time = crate::syntax::builtin_effects::time_effect_symbol(&mut interner);
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

    #[test]
    fn contains_add_and_subtract_detect_row_arithmetic() {
        let mut interner = Interner::new();
        let console = interner.intern("Console");
        let clock = interner.intern("Clock");
        let span = Span::default();

        let atom = EffectExpr::Named {
            name: console,
            span,
        };
        assert!(!atom.contains_add());
        assert!(!atom.contains_subtract());

        let add = EffectExpr::Add {
            left: Box::new(EffectExpr::Named {
                name: console,
                span,
            }),
            right: Box::new(EffectExpr::Named { name: clock, span }),
            span,
        };
        assert!(add.contains_add());
        assert!(!add.contains_subtract());

        let add_sub = EffectExpr::Subtract {
            left: Box::new(add.clone()),
            right: Box::new(EffectExpr::Named { name: clock, span }),
            span,
        };
        assert!(add_sub.contains_add());
        assert!(add_sub.contains_subtract());
    }

    #[test]
    fn row_var_is_excluded_from_normalized_concrete_names() {
        let mut interner = Interner::new();
        let io = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let e = interner.intern("e");
        let span = Span::default();

        let expr = EffectExpr::Add {
            left: Box::new(EffectExpr::Named { name: io, span }),
            right: Box::new(EffectExpr::RowVar { name: e, span }),
            span,
        };

        assert_eq!(expr.row_var(), Some(e));

        let concrete = expr.normalized_concrete_names();
        assert!(concrete.contains(&io));
        assert!(!concrete.contains(&e));
    }
}
