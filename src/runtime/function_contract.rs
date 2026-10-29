use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::syntax::Identifier;
use crate::syntax::interner::Interner;
use crate::syntax::symbol::Symbol;
use crate::syntax::type_expr::TypeExpr;

use crate::runtime::runtime_type::RuntimeType;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionContract {
    pub params: Vec<Option<RuntimeType>>,
    pub ret: Option<RuntimeType>,
    pub effects: Vec<Identifier>,
}

impl FunctionContract {
    pub fn collect_symbols(&self, out: &mut HashSet<Symbol>) {
        for param in self.params.iter().flatten() {
            param.collect_symbols(out);
        }
        if let Some(ret) = &self.ret {
            ret.collect_symbols(out);
        }
        out.extend(self.effects.iter().copied());
    }

    pub fn remap_symbols(&self, remap: &std::collections::HashMap<Symbol, Symbol>) -> Self {
        Self {
            params: self
                .params
                .iter()
                .map(|param| param.as_ref().map(|param| param.remap_symbols(remap)))
                .collect(),
            ret: self.ret.as_ref().map(|ret| ret.remap_symbols(remap)),
            effects: self
                .effects
                .iter()
                .map(|effect| remap.get(effect).copied().unwrap_or(*effect))
                .collect(),
        }
    }
}

/// Build a `FunctionContract` from source-level type annotations.
/// Returns `None` if the function has no annotations at all.
pub fn runtime_contract_from_annotations(
    parameter_types: &[Option<TypeExpr>],
    return_type: &Option<TypeExpr>,
    effects: &[crate::syntax::effect_expr::EffectExpr],
    interner: &Interner,
) -> Option<FunctionContract> {
    let params = parameter_types
        .iter()
        .map(|ty| ty.as_ref().and_then(|t| convert_type_expr_for_contract(t, interner)))
        .collect::<Vec<_>>();
    let ret = return_type.as_ref().and_then(|ty| convert_type_expr_for_contract(ty, interner));
    if !params.iter().any(|t| t.is_some()) && ret.is_none() && effects.is_empty() {
        None
    } else {
        let effects = crate::syntax::effect_expr::EffectExpr::sorted_names(effects);
        Some(FunctionContract { params, ret, effects })
    }
}

fn convert_type_expr_for_contract(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let name_str = interner.try_resolve(*name)?;
            match (name_str, args.len()) {
                ("Int", 0) => Some(RuntimeType::Int),
                ("Float", 0) => Some(RuntimeType::Float),
                ("Bool", 0) => Some(RuntimeType::Bool),
                ("String", 0) => Some(RuntimeType::String),
                ("Unit", 0) => Some(RuntimeType::Unit),
                ("Option", 1) => Some(RuntimeType::Option(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
                ("List", 1) => Some(RuntimeType::List(Box::new(convert_type_expr_for_contract(
                    &args[0], interner,
                )?))),
                ("Either", 2) => Some(RuntimeType::Either(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                ("Array", 1) => Some(RuntimeType::Array(Box::new(convert_type_expr_for_contract(
                    &args[0], interner,
                )?))),
                ("Map", 2) => Some(RuntimeType::Map(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                _ => None,
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(RuntimeType::Tuple(
            elements
                .iter()
                .map(|e| convert_type_expr_for_contract(e, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_contract_from_annotations;
    use crate::diagnostics::position::Span;
    use crate::syntax::{effect_expr::EffectExpr, interner::Interner, symbol::Symbol};

    #[test]
    fn a_contract_lists_its_effects_in_one_order() {
        // `A + B + …` once listed its effects in a HashSet's hash order, so
        // a cached interface differed from one compilation to the next.
        let span = Span::default();
        let row = (1..32)
            .map(|n| EffectExpr::Named { name: Symbol::new(n), span })
            .reduce(|left, right| EffectExpr::Add {
                left: Box::new(left),
                right: Box::new(right),
                span,
            })
            .expect("a row");
        let interner = Interner::new();
        let effects = || {
            runtime_contract_from_annotations(&[], &None, std::slice::from_ref(&row), &interner)
                .expect("a contract")
                .effects
        };
        let first = effects();
        for _ in 0..8 {
            assert_eq!(effects(), first);
        }
    }
}
