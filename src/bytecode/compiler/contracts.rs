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
                ("Any", 0) => Some(RuntimeType::Any),
                ("Int", 0) => Some(RuntimeType::Int),
                ("Float", 0) => Some(RuntimeType::Float),
                ("Bool", 0) => Some(RuntimeType::Bool),
                ("String", 0) => Some(RuntimeType::String),
                ("Unit", 0) => Some(RuntimeType::Unit),
                ("Option", 1) => Some(RuntimeType::Option(Box::new(convert_type_expr(
                    &args[0], interner,
                )?))),
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
        TypeExpr::Function { .. } => None,
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

    if params.iter().all(Option::is_none) && ret.is_none() {
        return None;
    }

    Some(FunctionContract { params, ret })
}
