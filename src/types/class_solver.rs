//! Type class constraint solver — resolves collected constraints against
//! known instances (Proposal 0145, Step 4).
//!
//! After HM inference generates `WantedClassConstraint` entries, this pass
//! checks each constraint:
//! - Concrete type (e.g., `Num<Int>`) → look up instance in ClassEnv
//! - Type variable (e.g., `Num<a>`) → leave unsolved (future: add to scheme)
//!
//! Unsatisfied concrete constraints produce compile errors.

use crate::{
    ast::type_infer::constraint::WantedClassConstraint,
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for},
    diagnostics::position::Span,
    syntax::interner::Interner,
    types::{
        class_env::ClassEnv,
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
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

        // Check if an instance exists for this class + concrete type(s).
        let instances = class_env.instances_for(constraint.class_name);
        let type_displays: Vec<String> = constraint
            .type_args
            .iter()
            .map(|t| display_type(t, interner))
            .collect();
        let type_display = type_displays.join(", ");

        let has_matching_instance = instances.iter().any(|inst| {
            if inst.type_args.len() != constraint.type_args.len() {
                return false;
            }
            inst.type_args
                .iter()
                .zip(type_displays.iter())
                .all(|(inst_arg, constraint_display)| {
                    inst_arg.display_with(interner) == *constraint_display
                })
        });

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
