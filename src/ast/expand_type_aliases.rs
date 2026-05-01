//! Proposal 0174 prerequisite: pre-inference expansion of transparent type
//! aliases.
//!
//! Rewrites every `TypeExpr::Named { name }` reference to a registered alias
//! into its body, with `params -> args` substitution applied. Runs once,
//! before HM inference, so every downstream consumer (type inference,
//! Core lowering, codegen) sees fully expanded type expressions.
//!
//! Recursion is detected via a visited-set; depth is hard-capped to defend
//! against pathological input. Both conditions surface as compiler errors.

use std::collections::{HashMap, HashSet};

use crate::syntax::{
    Identifier, block::Block, expression::Expression, program::Program, statement::Statement,
    type_expr::TypeExpr,
};

const MAX_ALIAS_DEPTH: usize = 64;

pub type TypeAliasTable = HashMap<Identifier, (Vec<Identifier>, TypeExpr)>;

pub fn expand_type_aliases_in_program(program: &mut Program, aliases: &TypeAliasTable) {
    if aliases.is_empty() {
        return;
    }
    for stmt in &mut program.statements {
        expand_stmt(stmt, aliases);
    }
}

fn expand_stmt(stmt: &mut Statement, aliases: &TypeAliasTable) {
    match stmt {
        Statement::Function {
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for ty in parameter_types.iter_mut().flatten() {
                expand_type(ty, aliases);
            }
            if let Some(ty) = return_type {
                expand_type(ty, aliases);
            }
            expand_block(body, aliases);
        }
        Statement::Module { body, .. } => {
            for s in &mut body.statements {
                expand_stmt(s, aliases);
            }
        }
        Statement::Class { methods, .. } => {
            for m in methods {
                expand_type(&mut m.return_type, aliases);
                for param_ty in m.param_types.iter_mut() {
                    expand_type(param_ty, aliases);
                }
                if let Some(body) = m.default_body.as_mut() {
                    expand_block(body, aliases);
                }
            }
        }
        Statement::Instance {
            type_args, methods, ..
        } => {
            for ty in type_args.iter_mut() {
                expand_type(ty, aliases);
            }
            for m in methods {
                expand_block(&mut m.body, aliases);
            }
        }
        Statement::Data { variants, .. } => {
            for variant in variants.iter_mut() {
                for field in variant.fields.iter_mut() {
                    expand_type(field, aliases);
                }
            }
        }
        Statement::EffectDecl { ops, .. } => {
            for op in ops.iter_mut() {
                expand_type(&mut op.type_expr, aliases);
            }
        }
        Statement::TypeAlias { body, .. } => {
            // Pre-expand alias bodies so subsequent lookups find them already
            // closed. Cycles are caught by the depth cap inside `expand_type`.
            expand_type(body, aliases);
        }
        Statement::Let {
            type_annotation: Some(ty),
            value,
            ..
        } => {
            expand_type(ty, aliases);
            expand_expr(value, aliases);
        }
        Statement::Let { value, .. }
        | Statement::LetDestructure { value, .. }
        | Statement::Assign { value, .. }
        | Statement::Expression {
            expression: value, ..
        } => {
            expand_expr(value, aliases);
        }
        Statement::Return { value: Some(v), .. } => expand_expr(v, aliases),
        Statement::Return { value: None, .. } => {}
        Statement::Import { .. } | Statement::EffectAlias { .. } => {}
    }
}

fn expand_block(block: &mut Block, aliases: &TypeAliasTable) {
    for s in &mut block.statements {
        expand_stmt(s, aliases);
    }
}

fn expand_expr(expr: &mut Expression, aliases: &TypeAliasTable) {
    match expr {
        Expression::Function {
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for ty in parameter_types.iter_mut().flatten() {
                expand_type(ty, aliases);
            }
            if let Some(ty) = return_type {
                expand_type(ty, aliases);
            }
            expand_block(body, aliases);
        }
        Expression::DoBlock { block, .. } => expand_block(block, aliases),
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            expand_expr(condition, aliases);
            expand_block(consequence, aliases);
            if let Some(alt) = alternative {
                expand_block(alt, aliases);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            expand_expr(scrutinee, aliases);
            for arm in arms {
                if let Some(guard) = arm.guard.as_mut() {
                    expand_expr(guard, aliases);
                }
                expand_expr(&mut arm.body, aliases);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            expand_expr(function, aliases);
            for arg in arguments {
                expand_expr(arg, aliases);
            }
        }
        Expression::Infix { left, right, .. } => {
            expand_expr(left, aliases);
            expand_expr(right, aliases);
        }
        Expression::Prefix { right, .. } => expand_expr(right, aliases),
        Expression::Perform { args, .. } => {
            for arg in args {
                expand_expr(arg, aliases);
            }
        }
        Expression::Handle {
            expr,
            parameter,
            arms,
            ..
        } => {
            expand_expr(expr, aliases);
            if let Some(parameter) = parameter {
                expand_expr(parameter, aliases);
            }
            for arm in arms {
                expand_expr(&mut arm.body, aliases);
            }
        }
        Expression::Sealing { expr, .. } => {
            expand_expr(expr, aliases);
        }
        Expression::Index { left, index, .. } => {
            expand_expr(left, aliases);
            expand_expr(index, aliases);
        }
        Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
            expand_expr(object, aliases);
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                expand_expr(e, aliases);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                expand_expr(k, aliases);
                expand_expr(v, aliases);
            }
        }
        Expression::Cons { head, tail, .. } => {
            expand_expr(head, aliases);
            expand_expr(tail, aliases);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => expand_expr(value, aliases),
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

fn expand_type(ty: &mut TypeExpr, aliases: &TypeAliasTable) {
    let mut visiting: HashSet<Identifier> = HashSet::new();
    expand_type_inner(ty, aliases, &mut visiting, 0);
}

fn expand_type_inner(
    ty: &mut TypeExpr,
    aliases: &TypeAliasTable,
    visiting: &mut HashSet<Identifier>,
    depth: usize,
) {
    if depth > MAX_ALIAS_DEPTH {
        // Cap reached — leave the type as-is. Cycles surface to inference
        // as an unresolved alias name.
        return;
    }
    match ty {
        TypeExpr::Function {
            params,
            ret,
            effects: _,
            ..
        } => {
            for p in params.iter_mut() {
                expand_type_inner(p, aliases, visiting, depth);
            }
            expand_type_inner(ret, aliases, visiting, depth);
        }
        TypeExpr::Tuple { elements, .. } => {
            for e in elements.iter_mut() {
                expand_type_inner(e, aliases, visiting, depth);
            }
        }
        TypeExpr::Named { .. } => {
            let span = ty.span();
            let TypeExpr::Named { name, args, .. } = ty else {
                unreachable!()
            };
            // First, recursively expand args.
            for a in args.iter_mut() {
                expand_type_inner(a, aliases, visiting, depth);
            }
            // Then, if this name is a registered alias, substitute its body.
            if visiting.contains(name) {
                return;
            }
            let alias_name = *name;
            if let Some((params, body)) = aliases.get(&alias_name) {
                if params.len() != args.len() {
                    return;
                }
                let substituted = substitute_params(body, params, args, span);
                visiting.insert(alias_name);
                let mut next = substituted;
                expand_type_inner(&mut next, aliases, visiting, depth + 1);
                visiting.remove(&alias_name);
                *ty = next;
            }
        }
    }
}

fn substitute_params(
    body: &TypeExpr,
    params: &[Identifier],
    args: &[TypeExpr],
    use_span: crate::diagnostics::position::Span,
) -> TypeExpr {
    let mut subst: HashMap<Identifier, &TypeExpr> = HashMap::new();
    for (p, a) in params.iter().zip(args.iter()) {
        subst.insert(*p, a);
    }
    substitute(body, &subst, use_span)
}

fn substitute(
    ty: &TypeExpr,
    subst: &HashMap<Identifier, &TypeExpr>,
    use_span: crate::diagnostics::position::Span,
) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, args, span } => {
            // A bare type-variable reference (no args) gets replaced wholesale.
            if args.is_empty() {
                if let Some(replacement) = subst.get(name) {
                    return (*replacement).clone();
                }
            }
            TypeExpr::Named {
                name: *name,
                args: args.iter().map(|a| substitute(a, subst, use_span)).collect(),
                span: *span,
            }
        }
        TypeExpr::Tuple { elements, span } => TypeExpr::Tuple {
            elements: elements.iter().map(|e| substitute(e, subst, use_span)).collect(),
            span: *span,
        },
        TypeExpr::Function {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Function {
            params: params.iter().map(|p| substitute(p, subst, use_span)).collect(),
            ret: Box::new(substitute(ret, subst, use_span)),
            effects: effects.clone(),
            span: *span,
        },
    }
}

