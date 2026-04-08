use core::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, effect_expr::EffectExpr, interner::Interner},
};

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Named {
        name: Identifier,
        args: Vec<TypeExpr>,
        span: Span,
    },
    Tuple {
        elements: Vec<TypeExpr>,
        span: Span,
    },
    Function {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
        effects: Vec<EffectExpr>,
        span: Span,
    },
}

impl TypeExpr {
    /// Structural equality ignoring source spans.
    ///
    /// Two type expressions are structurally equal if they have the same
    /// shape and identifiers, regardless of where they appear in the source.
    pub fn structural_eq(&self, other: &TypeExpr) -> bool {
        match (self, other) {
            (
                TypeExpr::Named { name: n1, args: a1, .. },
                TypeExpr::Named { name: n2, args: a2, .. },
            ) => n1 == n2 && a1.len() == a2.len() && a1.iter().zip(a2).all(|(x, y)| x.structural_eq(y)),
            (
                TypeExpr::Tuple { elements: e1, .. },
                TypeExpr::Tuple { elements: e2, .. },
            ) => e1.len() == e2.len() && e1.iter().zip(e2).all(|(x, y)| x.structural_eq(y)),
            (
                TypeExpr::Function { params: p1, ret: r1, .. },
                TypeExpr::Function { params: p2, ret: r2, .. },
            ) => {
                p1.len() == p2.len()
                    && p1.iter().zip(p2).all(|(x, y)| x.structural_eq(y))
                    && r1.structural_eq(r2)
            }
            _ => false,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. }
            | TypeExpr::Tuple { span, .. }
            | TypeExpr::Function { span, .. } => *span,
        }
    }

    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            TypeExpr::Named { name, args, .. } => {
                if args.is_empty() {
                    interner.resolve(*name).to_string()
                } else {
                    let renderer: Vec<String> =
                        args.iter().map(|arg| arg.display_with(interner)).collect();
                    format!("{}<{}>", interner.resolve(*name), renderer.join(", "))
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                let renderer: Vec<String> = elements
                    .iter()
                    .map(|elem| elem.display_with(interner))
                    .collect();
                match renderer.len() {
                    0 => "()".to_string(),
                    1 => format!("({},)", renderer[0]),
                    _ => format!("({})", renderer.join(", ")),
                }
            }
            TypeExpr::Function {
                params,
                ret,
                effects,
                ..
            } => {
                let params_text = if params.len() == 1 {
                    params[0].display_with(interner)
                } else {
                    let renderer: Vec<String> = params
                        .iter()
                        .map(|param| param.display_with(interner))
                        .collect();
                    format!("({})", renderer.join(", "))
                };
                if effects.is_empty() {
                    format!("{params_text} -> {}", ret.display_with(interner))
                } else {
                    let effects_text: Vec<String> = effects
                        .iter()
                        .map(|effect| effect.display_with(interner))
                        .collect();
                    format!(
                        "{params_text} -> {} with {}",
                        ret.display_with(interner),
                        effects_text.join(", ")
                    )
                }
            }
        }
    }
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeExpr::Named { name, args, .. } => {
                if args.is_empty() {
                    write!(f, "{}", name)
                } else {
                    let renderer: Vec<String> = args.iter().map(ToString::to_string).collect();
                    write!(f, "{}<{}>", name, renderer.join(", "))
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                let renderer: Vec<String> = elements.iter().map(ToString::to_string).collect();
                match renderer.len() {
                    0 => write!(f, "()"),
                    1 => write!(f, "({},)", renderer[0]),
                    _ => write!(f, "({})", renderer.join(", ")),
                }
            }
            TypeExpr::Function {
                params,
                ret,
                effects,
                ..
            } => {
                let params_text = if params.len() == 1 {
                    params[0].to_string()
                } else {
                    let renderer: Vec<String> = params.iter().map(ToString::to_string).collect();
                    format!("({})", renderer.join(", "))
                };
                if effects.is_empty() {
                    write!(f, "{} -> {}", params_text, ret)
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(ToString::to_string).collect();
                    write!(
                        f,
                        "{} -> {} with {}",
                        params_text,
                        ret,
                        effects_text.join(", ")
                    )
                }
            }
        }
    }
}
