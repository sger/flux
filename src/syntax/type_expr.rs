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
