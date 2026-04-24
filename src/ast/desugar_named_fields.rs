//! Proposal 0152, Phase 3: post-inference AST desugaring for named-field
//! data types. Rewrites:
//!
//! - `Variant { x: a, y: b }` → `Variant(a, b)` (positional `Call`)
//! - `{ ...base, x: a }` → `match base { Variant(_0, _1) -> Variant(a, _1) }`
//! - `p.x` on a named-field ADT → `match p { Variant(_0, _1) -> _0 }`
//! - `Variant { x, y: inner } pattern` → `Variant(x, inner) positional pattern`
//!
//! Runs between Phase 3 (type inference) and Phase 4 (IR lowering) so every
//! downstream stage (AST-fallback bytecode, Core lowering, LLVM) sees only
//! positional forms.

use std::collections::HashMap;

use crate::syntax::{
    Identifier,
    block::Block,
    expression::{ExprId, Expression, MatchArm, NamedFieldInit, Pattern, StringPart},
    program::Program,
    statement::Statement,
};
use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

/// Metadata used by the desugaring pass.
pub struct NamedFieldDesugarCtx<'a> {
    /// Variant name → declared field names in declaration order.
    pub ctor_field_names: &'a HashMap<Identifier, Vec<Identifier>>,
    /// ADT name → list of variant names.
    pub adt_variants: &'a HashMap<Identifier, Vec<Identifier>>,
    /// HM-inferred types, keyed by `ExprId`.
    pub hm_expr_types: &'a HashMap<ExprId, InferType>,
}

/// Rewrite `program` in place to eliminate all named-field AST nodes.
pub fn desugar_named_fields_in_program(program: &mut Program, ctx: &mut NamedFieldDesugarCtx<'_>) {
    for stmt in &mut program.statements {
        desugar_stmt(stmt, ctx);
    }
}

fn desugar_stmt(stmt: &mut Statement, ctx: &mut NamedFieldDesugarCtx<'_>) {
    match stmt {
        Statement::Let { value, .. } => desugar_expr(value, ctx),
        Statement::Expression { expression, .. } => desugar_expr(expression, ctx),
        Statement::Return { value: Some(e), .. } => desugar_expr(e, ctx),
        Statement::Return { value: None, .. } => {}
        Statement::Function { body, .. } => desugar_block(body, ctx),
        Statement::Module { body, .. } => {
            for s in &mut body.statements {
                desugar_stmt(s, ctx);
            }
        }
        Statement::Instance { methods, .. } => {
            for m in methods {
                desugar_block(&mut m.body, ctx);
            }
        }
        _ => {}
    }
}

fn desugar_block(block: &mut Block, ctx: &mut NamedFieldDesugarCtx<'_>) {
    for stmt in &mut block.statements {
        desugar_stmt(stmt, ctx);
    }
}

fn desugar_expr(expr: &mut Expression, ctx: &mut NamedFieldDesugarCtx<'_>) {
    // Recurse first so children are normalized before the current node is
    // (potentially) rewritten. This is safe because all recursion paths take
    // `&mut` refs that persist through rewriting.
    walk_children(expr, ctx);

    match expr {
        Expression::NamedConstructor {
            name, fields, span, ..
        } => {
            let rewritten = rewrite_named_constructor(*name, fields, *span, ctx);
            *expr = rewritten;
        }
        Expression::Spread {
            base,
            overrides,
            span,
            ..
        } => {
            let rewritten = rewrite_spread(base, overrides, *span, ctx);
            *expr = rewritten;
        }
        Expression::MemberAccess {
            object,
            member,
            span,
            id,
        } => {
            if let Some(rewritten) = try_rewrite_adt_field_access(object, *member, *span, *id, ctx)
            {
                *expr = rewritten;
            }
        }
        _ => {}
    }
}

fn walk_children(expr: &mut Expression, ctx: &mut NamedFieldDesugarCtx<'_>) {
    match expr {
        Expression::Prefix { right, .. } => desugar_expr(right, ctx),
        Expression::Infix { left, right, .. } => {
            desugar_expr(left, ctx);
            desugar_expr(right, ctx);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            desugar_expr(condition, ctx);
            desugar_block(consequence, ctx);
            if let Some(alt) = alternative {
                desugar_block(alt, ctx);
            }
        }
        Expression::DoBlock { block, .. } => desugar_block(block, ctx),
        Expression::Function { body, .. } => desugar_block(body, ctx),
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            desugar_expr(function, ctx);
            for a in arguments {
                desugar_expr(a, ctx);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                desugar_expr(e, ctx);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                desugar_expr(k, ctx);
                desugar_expr(v, ctx);
            }
        }
        Expression::Index { left, index, .. } => {
            desugar_expr(left, ctx);
            desugar_expr(index, ctx);
        }
        Expression::MemberAccess { object, .. } => desugar_expr(object, ctx),
        Expression::TupleFieldAccess { object, .. } => desugar_expr(object, ctx),
        Expression::Match {
            scrutinee, arms, ..
        } => {
            desugar_expr(scrutinee, ctx);
            for arm in arms {
                desugar_pattern(&mut arm.pattern, ctx);
                if let Some(g) = &mut arm.guard {
                    desugar_expr(g, ctx);
                }
                desugar_expr(&mut arm.body, ctx);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => desugar_expr(value, ctx),
        Expression::Cons { head, tail, .. } => {
            desugar_expr(head, ctx);
            desugar_expr(tail, ctx);
        }
        Expression::Perform { args, .. } => {
            for a in args {
                desugar_expr(a, ctx);
            }
        }
        Expression::Handle {
            expr: inner,
            parameter,
            arms,
            ..
        } => {
            desugar_expr(inner, ctx);
            if let Some(parameter) = parameter {
                desugar_expr(parameter, ctx);
            }
            for arm in arms {
                desugar_expr(&mut arm.body, ctx);
            }
        }
        Expression::Sealing { expr: inner, .. } => {
            desugar_expr(inner, ctx);
        }
        Expression::NamedConstructor { fields, .. } => {
            for f in fields {
                if let Some(v) = &mut f.value {
                    desugar_expr(v, ctx);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            desugar_expr(base, ctx);
            for f in overrides {
                if let Some(v) = &mut f.value {
                    desugar_expr(v, ctx);
                }
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for p in parts {
                if let StringPart::Interpolation(e) = p {
                    desugar_expr(e, ctx);
                }
            }
        }
        _ => {}
    }
}

fn desugar_pattern(pat: &mut Pattern, ctx: &mut NamedFieldDesugarCtx<'_>) {
    match pat {
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => desugar_pattern(pattern, ctx),
        Pattern::Cons { head, tail, .. } => {
            desugar_pattern(head, ctx);
            desugar_pattern(tail, ctx);
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                desugar_pattern(e, ctx);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                desugar_pattern(f, ctx);
            }
        }
        Pattern::NamedConstructor {
            name, fields, span, ..
        } => {
            let declared = ctx.ctor_field_names.get(name).cloned().unwrap_or_default();
            let mut positional: Vec<Pattern> = (0..declared.len())
                .map(|_| Pattern::Wildcard { span: *span })
                .collect();
            for f in fields.iter_mut() {
                if let Some(sub) = &mut f.pattern {
                    desugar_pattern(sub, ctx);
                }
                if let Some(index) = declared.iter().position(|n| *n == f.name) {
                    positional[index] = match f.pattern.take() {
                        Some(p) => p,
                        None => Pattern::Identifier {
                            name: f.name,
                            span: f.span,
                        },
                    };
                }
            }
            *pat = Pattern::Constructor {
                name: *name,
                fields: positional,
                span: *span,
            };
        }
        _ => {}
    }
}

fn rewrite_named_constructor(
    name: Identifier,
    fields: &[NamedFieldInit],
    span: crate::diagnostics::position::Span,
    ctx: &mut NamedFieldDesugarCtx<'_>,
) -> Expression {
    let declared = ctx.ctor_field_names.get(&name).cloned().unwrap_or_default();
    let mut arg_slots: Vec<Option<Expression>> = (0..declared.len()).map(|_| None).collect();
    for f in fields {
        let Some(index) = declared.iter().position(|n| *n == f.name) else {
            continue;
        };
        let value = match &f.value {
            Some(v) => (**v).clone(),
            None => Expression::Identifier {
                name: f.name,
                span: f.span,
                id: crate::syntax::expression::ExprId::UNSET,
            },
        };
        arg_slots[index] = Some(value);
    }
    let arguments: Vec<Expression> = arg_slots
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| placeholder(span)))
        .collect();
    let func = Expression::Identifier {
        name,
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    };
    Expression::Call {
        function: Box::new(func),
        arguments,
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    }
}

fn rewrite_spread(
    base: &mut Expression,
    overrides: &[NamedFieldInit],
    span: crate::diagnostics::position::Span,
    ctx: &mut NamedFieldDesugarCtx<'_>,
) -> Expression {
    let variant = match resolve_spread_variant(base, ctx) {
        Some(v) => v,
        None => return std::mem::replace(base, placeholder(span)),
    };
    let declared = ctx
        .ctor_field_names
        .get(&variant)
        .cloned()
        .unwrap_or_default();
    let override_map: HashMap<Identifier, &NamedFieldInit> =
        overrides.iter().map(|o| (o.name, o)).collect();

    // Synthesize a single-arm match: match base { Variant(b0, b1, ...) -> Variant(effective0, effective1, ...) }
    let binder_names: Vec<Identifier> = declared.clone();
    let pattern_fields: Vec<Pattern> = binder_names
        .iter()
        .map(|name| Pattern::Identifier { name: *name, span })
        .collect();

    let args: Vec<Expression> = declared
        .iter()
        .map(|field_name| {
            if let Some(init) = override_map.get(field_name) {
                match &init.value {
                    Some(v) => (**v).clone(),
                    None => Expression::Identifier {
                        name: *field_name,
                        span: init.span,
                        id: crate::syntax::expression::ExprId::UNSET,
                    },
                }
            } else {
                Expression::Identifier {
                    name: *field_name,
                    span,
                    id: crate::syntax::expression::ExprId::UNSET,
                }
            }
        })
        .collect();

    let ctor_call = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: variant,
            span,
            id: crate::syntax::expression::ExprId::UNSET,
        }),
        arguments: args,
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    };

    let arm = MatchArm {
        pattern: Pattern::Constructor {
            name: variant,
            fields: pattern_fields,
            span,
        },
        guard: None,
        body: ctor_call,
        span,
    };

    Expression::Match {
        scrutinee: Box::new(base.clone()),
        arms: vec![arm],
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    }
}

fn try_rewrite_adt_field_access(
    object: &mut Expression,
    member: Identifier,
    span: crate::diagnostics::position::Span,
    _id: ExprId,
    ctx: &mut NamedFieldDesugarCtx<'_>,
) -> Option<Expression> {
    let object_ty = ctx.hm_expr_types.get(&object.expr_id())?;
    let adt_name = match object_ty {
        InferType::Con(TypeConstructor::Adt(n)) => *n,
        InferType::App(TypeConstructor::Adt(n), _) => *n,
        _ => return None,
    };
    let variants = ctx.adt_variants.get(&adt_name)?.clone();
    let named_variants: Vec<Identifier> = variants
        .iter()
        .copied()
        .filter(|v| ctx.ctor_field_names.contains_key(v))
        .collect();
    if named_variants.is_empty() {
        return None;
    }
    let mut per_variant: Vec<(Identifier, Vec<Identifier>, usize)> = Vec::new();
    for variant in &named_variants {
        let field_names = ctx.ctor_field_names.get(variant)?.clone();
        let index = field_names.iter().position(|n| *n == member)?;
        per_variant.push((*variant, field_names, index));
    }

    let arms: Vec<MatchArm> = per_variant
        .into_iter()
        .map(|(variant, field_names, target_index)| {
            let pattern_fields: Vec<Pattern> = field_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    if i == target_index {
                        Pattern::Identifier { name: *name, span }
                    } else {
                        Pattern::Wildcard { span }
                    }
                })
                .collect();
            let target_name = field_names[target_index];
            MatchArm {
                pattern: Pattern::Constructor {
                    name: variant,
                    fields: pattern_fields,
                    span,
                },
                guard: None,
                body: Expression::Identifier {
                    name: target_name,
                    span,
                    id: crate::syntax::expression::ExprId::UNSET,
                },
                span,
            }
        })
        .collect();

    Some(Expression::Match {
        scrutinee: Box::new(object.clone()),
        arms,
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    })
}

fn resolve_spread_variant(base: &Expression, ctx: &NamedFieldDesugarCtx<'_>) -> Option<Identifier> {
    if let Expression::NamedConstructor { name, .. } = base {
        return Some(*name);
    }
    // After the post-order walk, a NamedConstructor base has already been
    // rewritten into a `Call(Ident(name), args)`. Detect that pattern too.
    if let Expression::Call { function, .. } = base
        && let Expression::Identifier { name, .. } = function.as_ref()
        && ctx.ctor_field_names.contains_key(name)
    {
        return Some(*name);
    }
    let base_ty = ctx.hm_expr_types.get(&base.expr_id())?;
    let adt_name = match base_ty {
        InferType::Con(TypeConstructor::Adt(n)) => *n,
        InferType::App(TypeConstructor::Adt(n), _) => *n,
        _ => return None,
    };
    let variants = ctx.adt_variants.get(&adt_name)?;
    let named_variants: Vec<Identifier> = variants
        .iter()
        .copied()
        .filter(|v| ctx.ctor_field_names.contains_key(v))
        .collect();
    if named_variants.len() == 1 {
        Some(named_variants[0])
    } else {
        None
    }
}

fn placeholder(span: crate::diagnostics::position::Span) -> Expression {
    Expression::Integer {
        value: 0,
        span,
        id: crate::syntax::expression::ExprId::UNSET,
    }
}

/// Collect (variant → declared field names) and (adt → variant list) from a
/// program's `data` declarations.
pub fn collect_named_field_metadata(
    program: &Program,
) -> (
    HashMap<Identifier, Vec<Identifier>>,
    HashMap<Identifier, Vec<Identifier>>,
) {
    fn walk(
        stmts: &[Statement],
        field_names: &mut HashMap<Identifier, Vec<Identifier>>,
        adt_variants: &mut HashMap<Identifier, Vec<Identifier>>,
    ) {
        for stmt in stmts {
            match stmt {
                Statement::Data { name, variants, .. } => {
                    adt_variants.insert(*name, variants.iter().map(|v| v.name).collect());
                    for variant in variants {
                        if let Some(names) = variant.field_names.as_ref() {
                            field_names.insert(variant.name, names.clone());
                        }
                    }
                }
                Statement::Module { body, .. } => walk(&body.statements, field_names, adt_variants),
                _ => {}
            }
        }
    }
    let mut field_names = HashMap::new();
    let mut adt_variants = HashMap::new();
    walk(&program.statements, &mut field_names, &mut adt_variants);
    (field_names, adt_variants)
}
