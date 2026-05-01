use std::collections::{HashMap, HashSet};

use crate::{
    runtime::{
        function_contract::FunctionContract,
        runtime_type::{AdtConstructorContract, RuntimeType},
    },
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

/// Categorizes why a `TypeExpr` could not be lowered to a `RuntimeType`.
/// Used directly as the error type of the lowering functions — no wrapping
/// struct needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractLoweringIssue {
    GenericParameter,
    UnsupportedBoundaryType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdtConstructorContractSpec {
    pub name: Identifier,
    pub fields: Vec<TypeExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdtContractSpec {
    pub module_name: Option<Identifier>,
    pub type_name: Identifier,
    pub type_params: Vec<Identifier>,
    pub constructors: Vec<AdtConstructorContractSpec>,
}

pub type ModuleContractTable = HashMap<ContractKey, FnContract>;

struct ContractLoweringEnv<'a> {
    interner: &'a Interner,
    generic_params: HashSet<Identifier>,
    adts: &'a HashMap<Identifier, AdtContractSpec>,
}

impl<'a> ContractLoweringEnv<'a> {
    fn new(
        interner: &'a Interner,
        generic_params: impl IntoIterator<Item = Identifier>,
        adts: &'a HashMap<Identifier, AdtContractSpec>,
    ) -> Self {
        Self {
            interner,
            generic_params: generic_params.into_iter().collect(),
            adts,
        }
    }
}

#[allow(dead_code)]
pub fn convert_type_expr(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    convert_type_expr_checked(ty, interner, &[], &HashMap::new()).ok()
}

pub fn convert_type_expr_checked(
    ty: &TypeExpr,
    interner: &Interner,
    generic_params: &[Identifier],
    adts: &HashMap<Identifier, AdtContractSpec>,
) -> Result<RuntimeType, ContractLoweringIssue> {
    let env = ContractLoweringEnv::new(interner, generic_params.iter().copied(), adts);
    let bindings = HashMap::new();
    let mut active_adts = HashSet::new();
    convert_type_expr_rec(ty, &env, &bindings, &mut active_adts)
}

pub fn to_runtime_contract(contract: &FnContract, interner: &Interner) -> Option<FunctionContract> {
    to_runtime_contract_checked(contract, interner, &HashMap::new())
        .ok()
        .flatten()
}

pub fn to_runtime_contract_checked(
    contract: &FnContract,
    interner: &Interner,
    adts: &HashMap<Identifier, AdtContractSpec>,
) -> Result<Option<FunctionContract>, ContractLoweringIssue> {
    let env = ContractLoweringEnv::new(interner, contract.type_params.iter().copied(), adts);
    let bindings = HashMap::new();
    let mut active_adts = HashSet::new();

    let params = contract
        .params
        .iter()
        .map(|param| {
            param
                .as_ref()
                .map(|param| convert_type_expr_rec(param, &env, &bindings, &mut active_adts))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let ret = contract
        .ret
        .as_ref()
        .map(|ret| convert_type_expr_rec(ret, &env, &bindings, &mut active_adts))
        .transpose()?;

    if params.iter().all(Option::is_none) && ret.is_none() && contract.effects.is_empty() {
        return Ok(None);
    }

    let expanded_outer =
        crate::types::type_env::expand_async_alias_in_effects(&contract.effects, interner);
    let effects = expanded_outer
        .iter()
        .flat_map(EffectExpr::normalized_names)
        .collect::<Vec<_>>();

    Ok(Some(FunctionContract {
        params,
        ret,
        effects,
    }))
}

fn convert_type_expr_rec(
    ty: &TypeExpr,
    env: &ContractLoweringEnv<'_>,
    bindings: &HashMap<Identifier, TypeExpr>,
    active_adts: &mut HashSet<Identifier>,
) -> Result<RuntimeType, ContractLoweringIssue> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            if let Some(bound) = bindings.get(name) {
                if !args.is_empty() {
                    return Err(ContractLoweringIssue::UnsupportedBoundaryType);
                }
                return convert_type_expr_rec(bound, env, bindings, active_adts);
            }

            if env.generic_params.contains(name) {
                return Err(ContractLoweringIssue::GenericParameter);
            }

            let name_text = env.interner.resolve(*name);
            match (name_text, args.len()) {
                ("Int", 0) => Ok(RuntimeType::Int),
                ("Float", 0) => Ok(RuntimeType::Float),
                ("Bool", 0) => Ok(RuntimeType::Bool),
                ("String", 0) => Ok(RuntimeType::String),
                ("None" | "Unit", 0) => Ok(RuntimeType::Unit),
                ("Option", 1) => Ok(RuntimeType::Option(Box::new(convert_type_expr_rec(
                    &args[0],
                    env,
                    bindings,
                    active_adts,
                )?))),
                ("List", 1) => Ok(RuntimeType::List(Box::new(convert_type_expr_rec(
                    &args[0],
                    env,
                    bindings,
                    active_adts,
                )?))),
                ("Either", 2) => Ok(RuntimeType::Either(
                    Box::new(convert_type_expr_rec(&args[0], env, bindings, active_adts)?),
                    Box::new(convert_type_expr_rec(&args[1], env, bindings, active_adts)?),
                )),
                ("Array", 1) => Ok(RuntimeType::Array(Box::new(convert_type_expr_rec(
                    &args[0],
                    env,
                    bindings,
                    active_adts,
                )?))),
                ("Map", 2) => Ok(RuntimeType::Map(
                    Box::new(convert_type_expr_rec(&args[0], env, bindings, active_adts)?),
                    Box::new(convert_type_expr_rec(&args[1], env, bindings, active_adts)?),
                )),
                _ => lower_adt_type(name, args, env, bindings, active_adts),
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            if elements.is_empty() {
                return Ok(RuntimeType::Unit);
            }
            Ok(RuntimeType::Tuple(
                elements
                    .iter()
                    .map(|element| convert_type_expr_rec(element, env, bindings, active_adts))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            if effects.iter().any(|effect| effect.row_var().is_some()) {
                return Err(ContractLoweringIssue::UnsupportedBoundaryType);
            }
            let params = params
                .iter()
                .map(|param| convert_type_expr_rec(param, env, bindings, active_adts))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = convert_type_expr_rec(ret, env, bindings, active_adts)?;
            // Expand the builtin `Async` alias before normalizing, so callers
            // and callees see the same fine-grained row regardless of whether
            // the surface annotation used the alias.
            let expanded =
                crate::types::type_env::expand_async_alias_in_effects(effects, env.interner);
            let mut effect_set = expanded
                .iter()
                .flat_map(EffectExpr::normalized_names)
                .collect::<Vec<_>>();
            effect_set.sort_by_key(|sym| sym.as_u32());
            effect_set.dedup();
            Ok(RuntimeType::Function {
                params,
                ret: Box::new(ret),
                effects: effect_set,
            })
        }
    }
}

fn lower_adt_type(
    name: &Identifier,
    args: &[TypeExpr],
    env: &ContractLoweringEnv<'_>,
    bindings: &HashMap<Identifier, TypeExpr>,
    active_adts: &mut HashSet<Identifier>,
) -> Result<RuntimeType, ContractLoweringIssue> {
    let Some(spec) = env.adts.get(name) else {
        return Err(ContractLoweringIssue::UnsupportedBoundaryType);
    };

    if spec.type_params.len() != args.len() || active_adts.contains(name) {
        return Err(ContractLoweringIssue::UnsupportedBoundaryType);
    }

    let type_args = args
        .iter()
        .map(|arg| convert_type_expr_rec(arg, env, bindings, active_adts))
        .collect::<Result<Vec<_>, _>>()?;

    let mut child_bindings = bindings.clone();
    for (type_param, arg) in spec.type_params.iter().zip(args.iter()) {
        child_bindings.insert(*type_param, arg.clone());
    }

    active_adts.insert(*name);
    let constructors = spec
        .constructors
        .iter()
        .map(|ctor| {
            let fields = ctor
                .fields
                .iter()
                .map(|field| convert_type_expr_rec(field, env, &child_bindings, active_adts))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AdtConstructorContract {
                name: ctor.name,
                display_name: env.interner.resolve(ctor.name).to_string(),
                fields,
            })
        })
        .collect::<Result<Vec<_>, ContractLoweringIssue>>()?;
    active_adts.remove(name);

    Ok(RuntimeType::Adt {
        module_name: spec.module_name,
        module_name_text: spec
            .module_name
            .map(|module_name| env.interner.resolve(module_name).to_string()),
        name: spec.type_name,
        display_name: env.interner.resolve(spec.type_name).to_string(),
        type_args,
        constructors,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        AdtConstructorContractSpec, AdtContractSpec, ContractLoweringIssue, convert_type_expr,
        convert_type_expr_checked,
    };
    use crate::{
        runtime::runtime_type::{AdtConstructorContract, RuntimeType},
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

    #[test]
    fn empty_tuple_annotation_lowers_to_unit_contract() {
        let interner = Interner::new();
        let unit = TypeExpr::Tuple {
            elements: vec![],
            span: Default::default(),
        };

        assert_eq!(convert_type_expr(&unit, &interner), Some(RuntimeType::Unit));
    }

    #[test]
    fn lowers_nominal_adt_contracts() {
        let mut interner = Interner::new();
        let maybe = interner.intern("MaybeInt");
        let just = interner.intern("Just");
        let none = interner.intern("Nope");
        let int = interner.intern("Int");

        let ty = TypeExpr::Named {
            name: maybe,
            args: vec![],
            span: Default::default(),
        };
        let adts = HashMap::from([(
            maybe,
            AdtContractSpec {
                module_name: None,
                type_name: maybe,
                type_params: vec![],
                constructors: vec![
                    AdtConstructorContractSpec {
                        name: just,
                        fields: vec![TypeExpr::Named {
                            name: int,
                            args: vec![],
                            span: Default::default(),
                        }],
                    },
                    AdtConstructorContractSpec {
                        name: none,
                        fields: vec![],
                    },
                ],
            },
        )]);

        let lowered = convert_type_expr_checked(&ty, &interner, &[], &adts).expect("lowers ADT");

        assert_eq!(
            lowered,
            RuntimeType::Adt {
                module_name: None,
                module_name_text: None,
                name: maybe,
                display_name: "MaybeInt".to_string(),
                type_args: vec![],
                constructors: vec![
                    AdtConstructorContract {
                        name: just,
                        display_name: "Just".to_string(),
                        fields: vec![RuntimeType::Int],
                    },
                    AdtConstructorContract {
                        name: none,
                        display_name: "Nope".to_string(),
                        fields: vec![],
                    },
                ],
            }
        );
    }

    #[test]
    fn generic_parameter_boundary_is_rejected() {
        let mut interner = Interner::new();
        let t = interner.intern("T");
        let ty = TypeExpr::Named {
            name: t,
            args: vec![],
            span: Default::default(),
        };

        let err = convert_type_expr_checked(&ty, &interner, &[t], &HashMap::new())
            .expect_err("generic parameter should be rejected");

        assert_eq!(err, ContractLoweringIssue::GenericParameter);
    }
}
