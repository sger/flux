//! Type class constraint solver — resolves collected constraints against
//! known instances (Proposal 0145, Step 4).
//!
//! After HM inference generates `WantedClassConstraint` entries, this pass
//! checks each constraint:
//! - Concrete type (e.g., `Num<Int>`) → look up instance in ClassEnv
//! - Type variable (e.g., `Num<a>`) → leave unsolved (future: add to scheme)
//!
//! Unsatisfied concrete constraints produce compile errors.

use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::WantedClassConstraint,
    diagnostics::position::Span,
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for},
    syntax::{Identifier, interner::Interner, type_expr::TypeExpr},
    types::{class_env::ClassEnv, infer_type::InferType, type_constructor::TypeConstructor},
};

use super::super::diagnostics::compiler_errors::NO_INSTANCE;

/// Solve class constraints against known instances.
///
/// For each constraint where the type argument is concrete, verifies that
/// a matching instance exists in the ClassEnv. Emits diagnostics for
/// unsatisfied constraints.
///
/// Returns a list of error diagnostics (empty if all constraints are satisfied).
pub fn solve_class_constraints(
    constraints: &[WantedClassConstraint],
    class_env: &ClassEnv,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for constraint in constraints {
        // Only check concrete types — variables are left unsolved for now.
        if !constraint.type_args.iter().all(is_concrete_type) {
            continue;
        }

        // Skip constraints from compiler-generated code (e.g., dispatch functions).
        // These have default spans (line 0, col 0).
        if constraint.span == Span::default() {
            continue;
        }

        // Skip if the class doesn't exist in the environment (already
        // reported by ClassEnv validation as E441).
        if class_env.lookup_class(constraint.class_name).is_none() {
            continue;
        }

        let type_displays: Vec<String> = constraint
            .type_args
            .iter()
            .map(|t| display_type(t, interner))
            .collect();
        let type_display = type_displays.join(", ");

        let has_matching_instance = has_satisfied_instance(
            constraint.class_name,
            &constraint.type_args,
            class_env,
            interner,
            &mut HashSet::new(),
        );

        if !has_matching_instance {
            let class_display = interner.resolve(constraint.class_name);
            diagnostics.push(
                diagnostic_for(&NO_INSTANCE)
                    .with_span(constraint.span)
                    .with_message(format!(
                        "No instance for `{class_display}<{type_display}>`."
                    ))
                    .with_hint_text(format!(
                        "Add an instance: `instance {class_display}<{type_display}> {{ ... }}`"
                    )),
            );
        }
    }

    diagnostics
}

fn has_satisfied_instance(
    class_name: Identifier,
    type_args: &[InferType],
    class_env: &ClassEnv,
    interner: &Interner,
    seen: &mut HashSet<String>,
) -> bool {
    let key = format!(
        "{}<{}>",
        interner.resolve(class_name),
        type_args
            .iter()
            .map(|ty| display_type(ty, interner))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !seen.insert(key.clone()) {
        return true;
    }

    let result = class_env
        .resolve_instance_with_subst(class_name, type_args, interner)
        .is_some_and(|(instance, subst)| {
            instance.context.iter().all(|ctx| {
                let resolved_args: Option<Vec<InferType>> = ctx
                    .type_args
                    .iter()
                    .map(|arg| instantiate_context_type_expr(arg, &subst, interner))
                    .collect();
                resolved_args.is_some_and(|args| {
                    args.iter().all(is_concrete_type)
                        && has_satisfied_instance(ctx.class_name, &args, class_env, interner, seen)
                })
            })
        });

    seen.remove(&key);
    result
}

fn instantiate_context_type_expr(
    expr: &TypeExpr,
    subst: &HashMap<Identifier, InferType>,
    interner: &Interner,
) -> Option<InferType> {
    match expr {
        TypeExpr::Named { name, args, .. } => {
            if args.is_empty()
                && interner
                    .resolve(*name)
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            {
                return subst.get(name).cloned();
            }

            let arg_tys: Option<Vec<InferType>> = args
                .iter()
                .map(|arg| instantiate_context_type_expr(arg, subst, interner))
                .collect();
            let arg_tys = arg_tys?;
            let tc = match interner.resolve(*name) {
                "Int" => TypeConstructor::Int,
                "Float" => TypeConstructor::Float,
                "Bool" => TypeConstructor::Bool,
                "String" => TypeConstructor::String,
                "Unit" | "None" => TypeConstructor::Unit,
                "Never" => TypeConstructor::Never,
                "Any" => TypeConstructor::Any,
                "List" => TypeConstructor::List,
                "Array" => TypeConstructor::Array,
                "Map" => TypeConstructor::Map,
                "Option" => TypeConstructor::Option,
                "Either" => TypeConstructor::Either,
                _ => TypeConstructor::Adt(*name),
            };

            if arg_tys.is_empty() {
                Some(InferType::Con(tc))
            } else {
                Some(InferType::App(tc, arg_tys))
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(InferType::Tuple(
            elements
                .iter()
                .map(|elem| instantiate_context_type_expr(elem, subst, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function {
            params,
            ret,
            effects: _,
            ..
        } => Some(InferType::Fun(
            params
                .iter()
                .map(|param| instantiate_context_type_expr(param, subst, interner))
                .collect::<Option<Vec<_>>>()?,
            Box::new(instantiate_context_type_expr(ret, subst, interner)?),
            crate::types::infer_effect_row::InferEffectRow::closed_empty(),
        )),
    }
}

/// Check if a type is concrete (not a variable, not Any).
fn is_concrete_type(ty: &InferType) -> bool {
    match ty {
        InferType::Var(_) => false,
        InferType::Con(TypeConstructor::Any) => false,
        InferType::Con(_) => true,
        InferType::App(_, args) => args.iter().all(is_concrete_type),
        InferType::Tuple(elems) => elems.iter().all(is_concrete_type),
        InferType::Fun(params, ret, _) => {
            params.iter().all(is_concrete_type) && is_concrete_type(ret)
        }
        InferType::HktApp(head, args) => {
            is_concrete_type(head) && args.iter().all(is_concrete_type)
        }
    }
}

/// Format a type for display in diagnostics.
fn display_type(ty: &InferType, interner: &Interner) -> String {
    crate::ast::type_infer::display_infer_type(ty, interner)
}
