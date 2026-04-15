use std::collections::HashMap;

use crate::{
    runtime::{function_contract::FunctionContract, runtime_type::RuntimeType},
    syntax::{Identifier, effect_expr::EffectExpr, interner::Interner, type_expr::TypeExpr},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContractKey {
    pub module_name: Option<Identifier>,
    pub function_name: Identifier,
    pub arity: usize,
}

#[derive(Debug, Clone)]
pub struct FnContract {
    pub type_params: Vec<Identifier>,
    pub params: Vec<Option<TypeExpr>>,
    pub ret: Option<TypeExpr>,
    pub effects: Vec<EffectExpr>,
}

pub type ModuleContractTable = HashMap<ContractKey, FnContract>;

pub fn convert_type_expr(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let name = interner.resolve(*name);
            match (name, args.len()) {
                ("Int", 0) => Some(RuntimeType::Int),
                ("Float", 0) => Some(RuntimeType::Float),
                ("Bool", 0) => Some(RuntimeType::Bool),
                ("String", 0) => Some(RuntimeType::String),
                ("None", 0) => Some(RuntimeType::Unit),
                ("Option", 1) => Some(RuntimeType::Option(Box::new(convert_type_expr(
                    &args[0], interner,
                )?))),
                ("List", 1) => Some(RuntimeType::List(Box::new(convert_type_expr(
                    &args[0], interner,
                )?))),
                ("Either", 2) => Some(RuntimeType::Either(
                    Box::new(convert_type_expr(&args[0], interner)?),
                    Box::new(convert_type_expr(&args[1], interner)?),
                )),
                ("Array", 1) => Some(RuntimeType::Array(Box::new(convert_type_expr(
                    &args[0], interner,
                )?))),
                ("Map", 2) => Some(RuntimeType::Map(
                    Box::new(convert_type_expr(&args[0], interner)?),
                    Box::new(convert_type_expr(&args[1], interner)?),
                )),
                _ => None,
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(RuntimeType::Tuple(
            elements
                .iter()
                .map(|e| convert_type_expr(e, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            // Row variables can't be represented in RuntimeType bail to HM path.
            if effects.iter().any(|e| e.row_var().is_some()) {
                return None;
            }
            let param_types = params
                .iter()
                .map(|param| convert_type_expr(param, interner))
                .collect::<Option<Vec<_>>>()?;
            let ret_type = convert_type_expr(ret, interner)?;
            let mut effect_set = effects
                .iter()
                .flat_map(EffectExpr::normalized_names)
                .collect::<Vec<_>>();
            effect_set.sort_by_key(|sym| sym.as_u32());
            effect_set.dedup();
            Some(RuntimeType::Function {
                params: param_types,
                ret: Box::new(ret_type),
                effects: effect_set,
            })
        }
    }
}

pub fn to_runtime_contract(contract: &FnContract, interner: &Interner) -> Option<FunctionContract> {
    let params = contract
        .params
        .iter()
        .map(|p| p.as_ref().and_then(|ty| convert_type_expr(ty, interner)))
        .collect::<Vec<_>>();

    let ret = contract
        .ret
        .as_ref()
        .and_then(|ty| convert_type_expr(ty, interner));

    if params.iter().all(Option::is_none) && ret.is_none() && contract.effects.is_empty() {
        return None;
    }

    let effects = contract
        .effects
        .iter()
        .flat_map(EffectExpr::normalized_names)
        .collect::<Vec<_>>();

    Some(FunctionContract {
        params,
        ret,
        effects,
    })
}

#[cfg(test)]
mod tests {
    use super::convert_type_expr;
    use crate::{
        runtime::runtime_type::RuntimeType,
        syntax::{interner::Interner, type_expr::TypeExpr},
    };

    #[test]
    fn converts_list_and_either_type_expr_to_runtime_types() {
        let mut interner = Interner::new();
        let list_sym = interner.intern("List");
        let either_sym = interner.intern("Either");
        let int_sym = interner.intern("Int");
        let string_sym = interner.intern("String");

        let list_int = TypeExpr::Named {
            name: list_sym,
            args: vec![TypeExpr::Named {
                name: int_sym,
                args: vec![],
                span: Default::default(),
            }],
            span: Default::default(),
        };
        let either_string_int = TypeExpr::Named {
            name: either_sym,
            args: vec![
                TypeExpr::Named {
                    name: string_sym,
                    args: vec![],
                    span: Default::default(),
                },
                TypeExpr::Named {
                    name: int_sym,
                    args: vec![],
                    span: Default::default(),
                },
            ],
            span: Default::default(),
        };

        assert_eq!(
            convert_type_expr(&list_int, &interner),
            Some(RuntimeType::List(Box::new(RuntimeType::Int)))
        );
        assert_eq!(
            convert_type_expr(&either_string_int, &interner),
            Some(RuntimeType::Either(
                Box::new(RuntimeType::String),
                Box::new(RuntimeType::Int)
            ))
        );
    }
}
