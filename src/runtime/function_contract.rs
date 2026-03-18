use crate::syntax::Identifier;
use crate::syntax::interner::Interner;
use crate::syntax::type_expr::TypeExpr;

use crate::runtime::runtime_type::RuntimeType;

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionContract {
    pub params: Vec<Option<RuntimeType>>,
    pub ret: Option<RuntimeType>,
    pub effects: Vec<Identifier>,
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
        .map(|ty| {
            ty.as_ref()
                .and_then(|t| convert_type_expr_for_contract(t, interner))
        })
        .collect::<Vec<_>>();
    let ret = return_type
        .as_ref()
        .and_then(|ty| convert_type_expr_for_contract(ty, interner));
    if !params.iter().any(|t| t.is_some()) && ret.is_none() && effects.is_empty() {
        None
    } else {
        let effects = effects
            .iter()
            .flat_map(crate::syntax::effect_expr::EffectExpr::normalized_names)
            .collect::<Vec<_>>();
        Some(FunctionContract {
            params,
            ret,
            effects,
        })
    }
}

fn convert_type_expr_for_contract(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let name_str = interner.try_resolve(*name)?;
            match (name_str, args.len()) {
                ("Any", 0) => Some(RuntimeType::Any),
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
                ("Array", 1) => Some(RuntimeType::Array(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
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
