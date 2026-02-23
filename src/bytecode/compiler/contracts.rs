use std::{collections::HashMap, vec};

use crate::syntax::{Identifier, effect_expr::EffectExpr, interner::Interner, type_expr::TypeExpr};

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
