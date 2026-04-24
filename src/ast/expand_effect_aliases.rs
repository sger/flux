//! Proposal 0161 B1: pre-inference alias expansion for effect rows.
//!
//! Rewrites every `EffectExpr::Named { name: alias_name }` in the program to
//! the aliased row (from `alias Name = <E1 | E2 | ...>`). Runs once, after
//! collection but before HM inference, so every downstream effect-row
//! consumer (type inference, row solver, diagnostics, Core lowering) sees a
//! fully decomposed row. Aliases are non-recursive in this first pass.

use std::collections::HashMap;

use crate::syntax::{
    Identifier, block::Block, effect_expr::EffectExpr, expression::Expression, program::Program,
    statement::Statement, type_expr::TypeExpr,
};

/// Rewrite every effect expression in `program` in place, replacing any
/// reference to an alias with the alias body.
pub fn expand_effect_aliases_in_program(
    program: &mut Program,
    aliases: &HashMap<Identifier, EffectExpr>,
) {
    if aliases.is_empty() {
        return;
    }
    for stmt in &mut program.statements {
        expand_stmt(stmt, aliases);
    }
}

fn expand_stmt(stmt: &mut Statement, aliases: &HashMap<Identifier, EffectExpr>) {
    match stmt {
        Statement::Function {
            effects,
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for effect in effects.iter_mut() {
                *effect = effect.expand_aliases(aliases);
            }
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
                for effect in m.effects.iter_mut() {
                    *effect = effect.expand_aliases(aliases);
                }
                expand_type(&mut m.return_type, aliases);
                for param_ty in m.param_types.iter_mut() {
                    expand_type(param_ty, aliases);
                }
                if let Some(body) = m.default_body.as_mut() {
                    expand_block(body, aliases);
                }
            }
        }
        Statement::Instance { methods, .. } => {
            for m in methods {
                for effect in m.effects.iter_mut() {
                    *effect = effect.expand_aliases(aliases);
                }
                expand_block(&mut m.body, aliases);
            }
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
        Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. }
        | Statement::EffectAlias { .. } => {}
    }
}

fn expand_block(block: &mut Block, aliases: &HashMap<Identifier, EffectExpr>) {
    for s in &mut block.statements {
        expand_stmt(s, aliases);
    }
}

fn expand_expr(expr: &mut Expression, aliases: &HashMap<Identifier, EffectExpr>) {
    match expr {
        Expression::Function {
            effects,
            parameter_types,
            return_type,
            body,
            ..
        } => {
            for effect in effects.iter_mut() {
                *effect = effect.expand_aliases(aliases);
            }
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
        Expression::Sealing { expr, allowed, .. } => {
            expand_expr(expr, aliases);
            for effect in allowed.iter_mut() {
                *effect = effect.expand_aliases(aliases);
            }
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
        // Leaf / no effect expressions inside.
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

fn expand_type(ty: &mut TypeExpr, aliases: &HashMap<Identifier, EffectExpr>) {
    match ty {
        TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            for effect in effects.iter_mut() {
                *effect = effect.expand_aliases(aliases);
            }
            for p in params {
                expand_type(p, aliases);
            }
            expand_type(ret, aliases);
        }
        TypeExpr::Named { args, .. } => {
            for a in args {
                expand_type(a, aliases);
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            for e in elements {
                expand_type(e, aliases);
            }
        }
    }
}
