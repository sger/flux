//! Proposal 0174 Phase 2: transparent type-alias expansion.
//!
//! Rewrites `alias Name<a> = TypeExpr` use sites before HM inference so
//! downstream compiler phases only see the expanded structural type.

use std::{collections::HashMap, sync::Arc};

use crate::{
    diagnostics::{Diagnostic, position::Span, types::ErrorType},
    syntax::{
        Identifier,
        block::Block,
        expression::Expression,
        program::Program,
        statement::{Statement, TypeAliasDecl},
        type_class::ClassConstraint,
        type_expr::TypeExpr,
    },
};

const MAX_ALIAS_EXPANSION_DEPTH: usize = 64;

pub fn expand_type_aliases_in_program(
    program: &mut Program,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
) -> Vec<Diagnostic> {
    if aliases.is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    for stmt in &mut program.statements {
        expand_stmt(stmt, aliases, file_path, &mut diagnostics);
    }
    diagnostics
}

fn expand_stmt(
    stmt: &mut Statement,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match stmt {
        Statement::Let {
            type_annotation,
            value,
            ..
        } => {
            if let Some(ty) = type_annotation {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            expand_expr(value, aliases, file_path, diagnostics);
        }
        Statement::LetDestructure { value, .. } | Statement::Assign { value, .. } => {
            expand_expr(value, aliases, file_path, diagnostics);
        }
        Statement::Return { value: Some(v), .. } => {
            expand_expr(v, aliases, file_path, diagnostics);
        }
        Statement::Return { value: None, .. } => {}
        Statement::Expression { expression, .. } => {
            expand_expr(expression, aliases, file_path, diagnostics);
        }
        Statement::Function {
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for ty in parameter_types.iter_mut().flatten() {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            if let Some(ty) = return_type {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            expand_block(body, aliases, file_path, diagnostics);
        }
        Statement::Module { body, .. } => expand_block(body, aliases, file_path, diagnostics),
        Statement::Data { variants, .. } => {
            for variant in variants {
                for field in &mut variant.fields {
                    expand_type(field, aliases, file_path, diagnostics);
                }
            }
        }
        Statement::Class {
            superclasses,
            methods,
            ..
        } => {
            for constraint in superclasses {
                expand_constraint(constraint, aliases, file_path, diagnostics);
            }
            for method in methods {
                for ty in &mut method.param_types {
                    expand_type(ty, aliases, file_path, diagnostics);
                }
                expand_type(&mut method.return_type, aliases, file_path, diagnostics);
                if let Some(body) = method.default_body.as_mut() {
                    expand_block(body, aliases, file_path, diagnostics);
                }
            }
        }
        Statement::Instance {
            type_args,
            context,
            methods,
            ..
        } => {
            for ty in type_args {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            for constraint in context {
                expand_constraint(constraint, aliases, file_path, diagnostics);
            }
            for method in methods {
                expand_block(&mut method.body, aliases, file_path, diagnostics);
            }
        }
        Statement::TypeAlias(alias) => {
            expand_type(&mut alias.body, aliases, file_path, diagnostics);
        }
        Statement::Import { .. } | Statement::EffectDecl { .. } | Statement::EffectAlias { .. } => {
        }
    }
}

fn expand_block(
    block: &mut Block,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for stmt in &mut block.statements {
        expand_stmt(stmt, aliases, file_path, diagnostics);
    }
}

fn expand_expr(
    expr: &mut Expression,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expression::Function {
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for ty in parameter_types.iter_mut().flatten() {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            if let Some(ty) = return_type {
                expand_type(ty, aliases, file_path, diagnostics);
            }
            expand_block(body, aliases, file_path, diagnostics);
        }
        Expression::DoBlock { block, .. } => expand_block(block, aliases, file_path, diagnostics),
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            expand_expr(condition, aliases, file_path, diagnostics);
            expand_block(consequence, aliases, file_path, diagnostics);
            if let Some(alt) = alternative {
                expand_block(alt, aliases, file_path, diagnostics);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            expand_expr(scrutinee, aliases, file_path, diagnostics);
            for arm in arms {
                if let Some(guard) = arm.guard.as_mut() {
                    expand_expr(guard, aliases, file_path, diagnostics);
                }
                expand_expr(&mut arm.body, aliases, file_path, diagnostics);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            expand_expr(function, aliases, file_path, diagnostics);
            for arg in arguments {
                expand_expr(arg, aliases, file_path, diagnostics);
            }
        }
        Expression::Infix { left, right, .. } => {
            expand_expr(left, aliases, file_path, diagnostics);
            expand_expr(right, aliases, file_path, diagnostics);
        }
        Expression::Prefix { right, .. } => expand_expr(right, aliases, file_path, diagnostics),
        Expression::Perform { args, .. } => {
            for arg in args {
                expand_expr(arg, aliases, file_path, diagnostics);
            }
        }
        Expression::Handle {
            expr,
            parameter,
            arms,
            ..
        } => {
            expand_expr(expr, aliases, file_path, diagnostics);
            if let Some(parameter) = parameter {
                expand_expr(parameter, aliases, file_path, diagnostics);
            }
            for arm in arms {
                expand_expr(&mut arm.body, aliases, file_path, diagnostics);
            }
        }
        Expression::Sealing { expr, .. } => expand_expr(expr, aliases, file_path, diagnostics),
        Expression::Index { left, index, .. } => {
            expand_expr(left, aliases, file_path, diagnostics);
            expand_expr(index, aliases, file_path, diagnostics);
        }
        Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
            expand_expr(object, aliases, file_path, diagnostics);
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for elem in elements {
                expand_expr(elem, aliases, file_path, diagnostics);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                expand_expr(key, aliases, file_path, diagnostics);
                expand_expr(value, aliases, file_path, diagnostics);
            }
        }
        Expression::Cons { head, tail, .. } => {
            expand_expr(head, aliases, file_path, diagnostics);
            expand_expr(tail, aliases, file_path, diagnostics);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => expand_expr(value, aliases, file_path, diagnostics),
        Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::InterpolatedString { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. }
        | Expression::NamedConstructor { .. }
        | Expression::Spread { .. } => {}
    }
}

fn expand_constraint(
    constraint: &mut ClassConstraint,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for ty in &mut constraint.type_args {
        expand_type(ty, aliases, file_path, diagnostics);
    }
}

/// Expand transparent aliases inside a single type.
///
/// Exposed for module-interface export: constructor field types are collected
/// from the raw AST, which still names the alias.
pub fn expand_type(
    ty: &mut TypeExpr,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut stack = Vec::new();
    *ty = expand_type_rec(ty, aliases, file_path, diagnostics, &mut stack, 0);
}

fn expand_type_rec(
    ty: &TypeExpr,
    aliases: &HashMap<Identifier, TypeAliasDecl>,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
    stack: &mut Vec<Identifier>,
    depth: usize,
) -> TypeExpr {
    if depth > MAX_ALIAS_EXPANSION_DEPTH {
        diagnostics.push(alias_error(
            "Transparent Type Alias Cycle",
            "Type alias expansion exceeded the maximum depth of 64.",
            Some("Check for direct or indirect recursive aliases.".to_string()),
            file_path,
            ty.span(),
        ));
        return ty.clone();
    }

    match ty {
        TypeExpr::Named { name, args, span } => {
            let expanded_args: Vec<_> = args
                .iter()
                .map(|arg| expand_type_rec(arg, aliases, file_path, diagnostics, stack, depth + 1))
                .collect();

            let Some(alias) = aliases.get(name) else {
                return TypeExpr::Named {
                    name: *name,
                    args: expanded_args,
                    span: *span,
                };
            };

            if stack.contains(name) {
                diagnostics.push(alias_error(
                    "Transparent Type Alias Cycle",
                    "Recursive type aliases are not supported.",
                    Some("Replace the recursive alias with a nominal `data` type.".to_string()),
                    file_path,
                    *span,
                ));
                return TypeExpr::Named {
                    name: *name,
                    args: expanded_args,
                    span: *span,
                };
            }

            if expanded_args.len() != alias.params.len() {
                diagnostics.push(alias_error(
                    "Wrong Type Alias Arity",
                    format!(
                        "Type alias expects {} type argument(s), but got {}.",
                        alias.params.len(),
                        expanded_args.len()
                    ),
                    None,
                    file_path,
                    *span,
                ));
                return TypeExpr::Named {
                    name: *name,
                    args: expanded_args,
                    span: *span,
                };
            }

            let subst: HashMap<Identifier, TypeExpr> =
                alias.params.iter().copied().zip(expanded_args).collect();
            stack.push(*name);
            let substituted = substitute_alias_params(&alias.body, &subst);
            let expanded = expand_type_rec(
                &substituted,
                aliases,
                file_path,
                diagnostics,
                stack,
                depth + 1,
            );
            stack.pop();
            expanded
        }
        TypeExpr::Tuple { elements, span } => TypeExpr::Tuple {
            elements: elements
                .iter()
                .map(|elem| {
                    expand_type_rec(elem, aliases, file_path, diagnostics, stack, depth + 1)
                })
                .collect(),
            span: *span,
        },
        TypeExpr::Function {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Function {
            params: params
                .iter()
                .map(|param| {
                    expand_type_rec(param, aliases, file_path, diagnostics, stack, depth + 1)
                })
                .collect(),
            ret: Box::new(expand_type_rec(
                ret,
                aliases,
                file_path,
                diagnostics,
                stack,
                depth + 1,
            )),
            effects: effects.clone(),
            span: *span,
        },
    }
}

fn substitute_alias_params(ty: &TypeExpr, subst: &HashMap<Identifier, TypeExpr>) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, args, span } => {
            if let Some(replacement) = subst.get(name) {
                return replacement.clone();
            }
            TypeExpr::Named {
                name: *name,
                args: args
                    .iter()
                    .map(|arg| substitute_alias_params(arg, subst))
                    .collect(),
                span: *span,
            }
        }
        TypeExpr::Tuple { elements, span } => TypeExpr::Tuple {
            elements: elements
                .iter()
                .map(|elem| substitute_alias_params(elem, subst))
                .collect(),
            span: *span,
        },
        TypeExpr::Function {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Function {
            params: params
                .iter()
                .map(|param| substitute_alias_params(param, subst))
                .collect(),
            ret: Box::new(substitute_alias_params(ret, subst)),
            effects: effects.clone(),
            span: *span,
        },
    }
}

fn alias_error(
    title: impl Into<String>,
    message: impl Into<String>,
    hint: Option<String>,
    file_path: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::make_error_dynamic(
        "E308",
        title,
        ErrorType::Compiler,
        message,
        hint,
        Arc::<str>::from(file_path),
        span,
    )
    .with_phase(crate::diagnostics::DiagnosticPhase::TypeCheck)
}
