//! Proposal 0165: route effectful prelude primops through `perform` and
//! synthesize default entrypoint handlers backed by internal intrinsics.

use std::collections::{HashMap, HashSet};

use crate::diagnostics::position::Span;
use crate::syntax::{
    Identifier,
    block::Block,
    builtin_effects as be,
    expression::{ExprId, ExprIdGen, Expression, HandleArm, StringPart},
    interner::Interner,
    program::Program,
    statement::Statement,
    type_expr::TypeExpr,
};

#[derive(Clone, Copy)]
struct RoutedPrimop {
    effect: &'static str,
    operation: &'static str,
    internal_name: &'static str,
    arity: usize,
    returns_unit: bool,
}

const ROUTED_PRIMOPS: &[RoutedPrimop] = &[
    RoutedPrimop {
        effect: be::CONSOLE,
        operation: "print",
        internal_name: "__primop_print",
        arity: 1,
        returns_unit: true,
    },
    RoutedPrimop {
        effect: be::CONSOLE,
        operation: "println",
        internal_name: "__primop_println",
        arity: 1,
        returns_unit: true,
    },
    RoutedPrimop {
        effect: be::FILESYSTEM,
        operation: "read_file",
        internal_name: "__primop_read_file",
        arity: 1,
        returns_unit: false,
    },
    RoutedPrimop {
        effect: be::FILESYSTEM,
        operation: "read_lines",
        internal_name: "__primop_read_lines",
        arity: 1,
        returns_unit: false,
    },
    RoutedPrimop {
        effect: be::FILESYSTEM,
        operation: "write_file",
        internal_name: "__primop_write_file",
        arity: 2,
        returns_unit: true,
    },
    RoutedPrimop {
        effect: be::STDIN,
        operation: "read_stdin",
        internal_name: "__primop_read_stdin",
        arity: 0,
        returns_unit: false,
    },
    RoutedPrimop {
        effect: be::CLOCK,
        operation: "clock_now",
        internal_name: "__primop_clock_now",
        arity: 0,
        returns_unit: false,
    },
    RoutedPrimop {
        effect: be::CLOCK,
        operation: "now_ms",
        internal_name: "__primop_now_ms",
        arity: 0,
        returns_unit: false,
    },
];

/// Result of the routing + handler synthesis pass.
///
/// `routed_call_perform_ids` captures the `ExprId`s of `Perform` nodes that
/// were synthesized from direct user calls (e.g. `println(x)` rewritten to
/// `perform Console.println(x)`). Downstream diagnostics use this set to
/// render errors in call-shape terminology instead of the lowered
/// `perform`-shape terminology.
pub struct RoutingResult {
    pub program: Program,
    pub changed: bool,
    pub routed_call_perform_ids: std::collections::HashSet<ExprId>,
}

pub fn route_effectful_primops_and_synthesize_handlers(
    program: &Program,
    interner: &mut Interner,
) -> RoutingResult {
    let mut owned = program.clone();
    let user_effect_ops = collect_user_effect_ops(&owned, interner);
    let function_effects = collect_function_effects(&owned, interner);
    let mut ids = ExprIdGen::resuming_past_program(&owned);
    let mut changed = false;
    let mut routed_call_perform_ids: HashSet<ExprId> = HashSet::new();
    for stmt in &mut owned.statements {
        changed |= route_stmt(stmt, interner, false, &mut routed_call_perform_ids);
    }
    for stmt in &mut owned.statements {
        changed |= synthesize_stmt_entry_handlers(
            stmt,
            interner,
            &mut ids,
            &user_effect_ops,
            &function_effects,
        );
    }
    RoutingResult {
        program: owned,
        changed,
        routed_call_perform_ids,
    }
}

fn collect_user_effect_ops(
    program: &Program,
    interner: &Interner,
) -> HashMap<Identifier, HashSet<Identifier>> {
    let mut out = HashMap::new();
    for stmt in &program.statements {
        collect_user_effect_ops_stmt(stmt, interner, &mut out);
    }
    out
}

fn collect_function_effects(
    program: &Program,
    interner: &Interner,
) -> HashMap<Identifier, HashSet<&'static str>> {
    let mut out = HashMap::new();
    for stmt in &program.statements {
        collect_function_effects_stmt(stmt, interner, &mut out);
    }
    out
}

fn collect_function_effects_stmt(
    stmt: &Statement,
    interner: &Interner,
    out: &mut HashMap<Identifier, HashSet<&'static str>>,
) {
    match stmt {
        Statement::Function { name, effects, .. } => {
            let mut default_effects = HashSet::new();
            for effect in effects {
                collect_default_effects_from_annotation(effect, interner, &mut default_effects);
            }
            if !default_effects.is_empty() {
                out.insert(*name, default_effects);
            }
        }
        Statement::Module { body, .. } => {
            for stmt in &body.statements {
                collect_function_effects_stmt(stmt, interner, out);
            }
        }
        _ => {}
    }
}

fn collect_default_effects_from_annotation(
    effect: &crate::syntax::effect_expr::EffectExpr,
    interner: &Interner,
    out: &mut HashSet<&'static str>,
) {
    match effect {
        crate::syntax::effect_expr::EffectExpr::Named { name, .. } => {
            match interner.try_resolve(*name) {
                Some(be::CONSOLE) => {
                    out.insert(be::CONSOLE);
                }
                Some(be::FILESYSTEM) => {
                    out.insert(be::FILESYSTEM);
                }
                Some(be::STDIN) => {
                    out.insert(be::STDIN);
                }
                Some(be::CLOCK) => {
                    out.insert(be::CLOCK);
                }
                Some(be::IO) => {
                    out.extend([be::CONSOLE, be::FILESYSTEM, be::STDIN]);
                }
                Some(be::TIME) => {
                    out.insert(be::CLOCK);
                }
                _ => {}
            }
        }
        crate::syntax::effect_expr::EffectExpr::Add { left, right, .. } => {
            collect_default_effects_from_annotation(left, interner, out);
            collect_default_effects_from_annotation(right, interner, out);
        }
        crate::syntax::effect_expr::EffectExpr::Subtract { left, right, .. } => {
            collect_default_effects_from_annotation(left, interner, out);
            let mut removed = HashSet::new();
            collect_default_effects_from_annotation(right, interner, &mut removed);
            out.retain(|effect| !removed.contains(effect));
        }
        crate::syntax::effect_expr::EffectExpr::RowVar { .. } => {}
    }
}

fn collect_user_effect_ops_stmt(
    stmt: &Statement,
    interner: &Interner,
    out: &mut HashMap<Identifier, HashSet<Identifier>>,
) {
    match stmt {
        Statement::EffectDecl { name, ops, .. } => {
            let compatible_ops = ops
                .iter()
                .filter(|op| {
                    routed_effect_op(*name, op.name, interner).is_some_and(|entry| {
                        effect_op_type_matches_routed_primop(&op.type_expr, entry, interner)
                    })
                })
                .map(|op| op.name)
                .collect();
            out.insert(*name, compatible_ops);
        }
        Statement::Module { body, .. } => {
            for stmt in &body.statements {
                collect_user_effect_ops_stmt(stmt, interner, out);
            }
        }
        _ => {}
    }
}

fn routed_effect_op(
    effect: Identifier,
    operation: Identifier,
    interner: &Interner,
) -> Option<RoutedPrimop> {
    let effect = interner.try_resolve(effect)?;
    let operation = interner.try_resolve(operation)?;
    ROUTED_PRIMOPS
        .iter()
        .copied()
        .find(|entry| entry.effect == effect && entry.operation == operation)
}

fn effect_op_type_matches_routed_primop(
    type_expr: &TypeExpr,
    entry: RoutedPrimop,
    interner: &Interner,
) -> bool {
    let TypeExpr::Function { params, ret, .. } = type_expr else {
        return false;
    };
    params.len() == entry.arity && routed_return_type_matches(ret, entry, interner)
}

fn routed_return_type_matches(ret: &TypeExpr, entry: RoutedPrimop, interner: &Interner) -> bool {
    if entry.returns_unit {
        return matches!(ret, TypeExpr::Tuple { elements, .. } if elements.is_empty())
            || type_expr_is_named(ret, "Unit", interner);
    }
    match entry.operation {
        "read_file" | "read_stdin" => type_expr_is_named(ret, "String", interner),
        "read_lines" => match ret {
            TypeExpr::Named { name, args, .. } => {
                interner.try_resolve(*name) == Some("Array")
                    && args.len() == 1
                    && type_expr_is_named(&args[0], "String", interner)
            }
            _ => false,
        },
        "clock_now" | "now_ms" => type_expr_is_named(ret, "Int", interner),
        _ => false,
    }
}

fn type_expr_is_named(type_expr: &TypeExpr, expected: &str, interner: &Interner) -> bool {
    matches!(
        type_expr,
        TypeExpr::Named { name, args, .. }
            if args.is_empty() && interner.try_resolve(*name) == Some(expected)
    )
}

fn route_stmt(
    stmt: &mut Statement,
    interner: &mut Interner,
    skip: bool,
    routed_ids: &mut HashSet<ExprId>,
) -> bool {
    match stmt {
        Statement::Let { value, .. }
        | Statement::LetDestructure { value, .. }
        | Statement::Assign { value, .. } => route_expr(value, interner, skip, routed_ids),
        Statement::Return {
            value: Some(value), ..
        } => route_expr(value, interner, skip, routed_ids),
        Statement::Return { value: None, .. } => false,
        Statement::Expression { expression, .. } => {
            route_expr(expression, interner, skip, routed_ids)
        }
        Statement::Function {
            intrinsic,
            body,
            name,
            ..
        } => {
            let skip_body = skip
                || intrinsic.is_some()
                || interner
                    .try_resolve(*name)
                    .is_some_and(|name| name.starts_with("__primop_"));
            route_block(body, interner, skip_body, routed_ids)
        }
        Statement::Module { name, body, .. } => {
            let skip_module = skip || interner.try_resolve(*name) == Some("Flow.Primops");
            route_block(body, interner, skip_module, routed_ids)
        }
        Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. }
        | Statement::EffectAlias { .. }
        | Statement::Class { .. }
        | Statement::Instance { .. } => false,
    }
}

fn route_block(
    block: &mut Block,
    interner: &mut Interner,
    skip: bool,
    routed_ids: &mut HashSet<ExprId>,
) -> bool {
    let mut changed = false;
    for stmt in &mut block.statements {
        changed |= route_stmt(stmt, interner, skip, routed_ids);
    }
    changed
}

fn route_expr(
    expr: &mut Expression,
    interner: &mut Interner,
    skip: bool,
    routed_ids: &mut HashSet<ExprId>,
) -> bool {
    if skip {
        return false;
    }
    let mut changed = false;
    match expr {
        Expression::Call {
            function,
            arguments,
            span,
            id,
        } => {
            changed |= route_expr(function, interner, false, routed_ids);
            for arg in arguments.iter_mut() {
                changed |= route_expr(arg, interner, false, routed_ids);
            }
            if let Some(routed) = routed_call(function, arguments.len(), interner) {
                let args = std::mem::take(arguments);
                let perform_id = *id;
                *expr = Expression::Perform {
                    effect: interner.intern(routed.effect),
                    operation: interner.intern(routed.operation),
                    args,
                    span: *span,
                    id: perform_id,
                };
                routed_ids.insert(perform_id);
                changed = true;
            }
        }
        Expression::Function { body, .. } | Expression::DoBlock { block: body, .. } => {
            changed |= route_block(body, interner, false, routed_ids);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            changed |= route_expr(condition, interner, false, routed_ids);
            changed |= route_block(consequence, interner, false, routed_ids);
            if let Some(alt) = alternative {
                changed |= route_block(alt, interner, false, routed_ids);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            changed |= route_expr(scrutinee, interner, false, routed_ids);
            for arm in arms {
                if let Some(guard) = arm.guard.as_mut() {
                    changed |= route_expr(guard, interner, false, routed_ids);
                }
                changed |= route_expr(&mut arm.body, interner, false, routed_ids);
            }
        }
        Expression::Infix { left, right, .. } => {
            changed |= route_expr(left, interner, false, routed_ids);
            changed |= route_expr(right, interner, false, routed_ids);
        }
        Expression::Prefix { right, .. } => {
            changed |= route_expr(right, interner, false, routed_ids)
        }
        Expression::Perform { args, .. } => {
            for arg in args {
                changed |= route_expr(arg, interner, false, routed_ids);
            }
        }
        Expression::Handle {
            expr: handled,
            parameter,
            arms,
            ..
        } => {
            changed |= route_expr(handled, interner, false, routed_ids);
            if let Some(parameter) = parameter {
                changed |= route_expr(parameter, interner, false, routed_ids);
            }
            for arm in arms {
                changed |= route_expr(&mut arm.body, interner, false, routed_ids);
            }
        }
        Expression::Sealing { expr, .. }
        | Expression::MemberAccess { object: expr, .. }
        | Expression::TupleFieldAccess { object: expr, .. }
        | Expression::Some { value: expr, .. }
        | Expression::Left { value: expr, .. }
        | Expression::Right { value: expr, .. } => {
            changed |= route_expr(expr, interner, false, routed_ids)
        }
        Expression::Index { left, index, .. } => {
            changed |= route_expr(left, interner, false, routed_ids);
            changed |= route_expr(index, interner, false, routed_ids);
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for elem in elements {
                changed |= route_expr(elem, interner, false, routed_ids);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                changed |= route_expr(key, interner, false, routed_ids);
                changed |= route_expr(value, interner, false, routed_ids);
            }
        }
        Expression::Cons { head, tail, .. } => {
            changed |= route_expr(head, interner, false, routed_ids);
            changed |= route_expr(tail, interner, false, routed_ids);
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    changed |= route_expr(expr, interner, false, routed_ids);
                }
            }
        }
        Expression::NamedConstructor { fields, .. } => {
            for field in fields {
                if let Some(value) = field.value.as_mut() {
                    changed |= route_expr(value, interner, false, routed_ids);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            changed |= route_expr(base, interner, false, routed_ids);
            for field in overrides {
                if let Some(value) = field.value.as_mut() {
                    changed |= route_expr(value, interner, false, routed_ids);
                }
            }
        }
        Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
    }
    changed
}

fn routed_call(function: &Expression, arity: usize, interner: &Interner) -> Option<RoutedPrimop> {
    let name = function_name(function, interner)?;
    ROUTED_PRIMOPS
        .iter()
        .copied()
        .find(|entry| entry.operation == name && entry.arity == arity)
}

fn function_name<'a>(function: &'a Expression, interner: &'a Interner) -> Option<&'a str> {
    match function {
        Expression::Identifier { name, .. } => {
            let name = interner.try_resolve(*name)?;
            (!name.starts_with("__primop_")).then_some(name)
        }
        Expression::MemberAccess { object, member, .. } => {
            if module_path_string(object, interner).as_deref() == Some("Flow.Primops") {
                interner.try_resolve(*member)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn module_path_string(expr: &Expression, interner: &Interner) -> Option<String> {
    match expr {
        Expression::Identifier { name, .. } => interner.try_resolve(*name).map(str::to_string),
        Expression::MemberAccess { object, member, .. } => {
            let mut path = module_path_string(object, interner)?;
            path.push('.');
            path.push_str(interner.try_resolve(*member)?);
            Some(path)
        }
        _ => None,
    }
}

fn synthesize_stmt_entry_handlers(
    stmt: &mut Statement,
    interner: &mut Interner,
    ids: &mut ExprIdGen,
    user_effect_ops: &HashMap<Identifier, HashSet<Identifier>>,
    function_effects: &HashMap<Identifier, HashSet<&'static str>>,
) -> bool {
    match stmt {
        Statement::Function {
            name,
            effects,
            body,
            span,
            ..
        } if is_entry_name(*name, interner) => {
            let mut entry_effects = HashSet::new();
            for effect in effects {
                collect_default_effects_from_annotation(effect, interner, &mut entry_effects);
            }
            wrap_block_with_default_handlers(
                body,
                *span,
                interner,
                ids,
                user_effect_ops,
                function_effects,
                &entry_effects,
            )
        }
        Statement::Module { body, .. } => {
            let mut changed = false;
            for stmt in &mut body.statements {
                changed |= synthesize_stmt_entry_handlers(
                    stmt,
                    interner,
                    ids,
                    user_effect_ops,
                    function_effects,
                );
            }
            changed
        }
        _ => false,
    }
}

fn is_entry_name(name: Identifier, interner: &Interner) -> bool {
    interner
        .try_resolve(name)
        .is_some_and(|name| name == "main" || name.starts_with("test_"))
}

fn wrap_block_with_default_handlers(
    block: &mut Block,
    span: Span,
    interner: &mut Interner,
    ids: &mut ExprIdGen,
    user_effect_ops: &HashMap<Identifier, HashSet<Identifier>>,
    function_effects: &HashMap<Identifier, HashSet<&'static str>>,
    entry_effects: &HashSet<&'static str>,
) -> bool {
    let mut required_effects = default_effects_in_block(block, interner, function_effects);
    required_effects.extend(entry_effects.iter().copied());
    if required_effects.is_empty() {
        return false;
    }

    let original = std::mem::replace(
        block,
        Block {
            statements: Vec::new(),
            span,
        },
    );
    let mut expr = Expression::DoBlock {
        block: original,
        span,
        id: ids.next_id(),
    };

    for effect in [be::CONSOLE, be::FILESYSTEM, be::STDIN, be::CLOCK] {
        if !required_effects.contains(effect) {
            continue;
        }
        let arms = default_handler_arms(effect, interner, ids, user_effect_ops, span);
        if arms.is_empty() {
            continue;
        }
        expr = Expression::Handle {
            expr: Box::new(expr),
            effect: interner.intern(effect),
            parameter: None,
            arms,
            span,
            id: ids.next_id(),
        };
    }

    *block = Block {
        statements: vec![Statement::Expression {
            expression: expr,
            has_semicolon: false,
            span,
        }],
        span,
    };
    true
}

fn default_effects_in_block(
    block: &Block,
    interner: &Interner,
    function_effects: &HashMap<Identifier, HashSet<&'static str>>,
) -> HashSet<&'static str> {
    let mut out = HashSet::new();
    for stmt in &block.statements {
        collect_default_effects_stmt(stmt, interner, function_effects, &mut out);
    }
    out
}

fn collect_default_effects_stmt(
    stmt: &Statement,
    interner: &Interner,
    function_effects: &HashMap<Identifier, HashSet<&'static str>>,
    out: &mut HashSet<&'static str>,
) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::LetDestructure { value, .. }
        | Statement::Assign { value, .. } => {
            collect_default_effects_expr(value, interner, function_effects, out);
        }
        Statement::Return {
            value: Some(value), ..
        } => collect_default_effects_expr(value, interner, function_effects, out),
        Statement::Return { value: None, .. } => {}
        Statement::Expression { expression, .. } => {
            collect_default_effects_expr(expression, interner, function_effects, out);
        }
        Statement::Module { body, .. } => {
            for stmt in &body.statements {
                collect_default_effects_stmt(stmt, interner, function_effects, out);
            }
        }
        Statement::Function { .. }
        | Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. }
        | Statement::EffectAlias { .. }
        | Statement::Class { .. }
        | Statement::Instance { .. } => {}
    }
}

fn collect_default_effects_expr(
    expr: &Expression,
    interner: &Interner,
    function_effects: &HashMap<Identifier, HashSet<&'static str>>,
    out: &mut HashSet<&'static str>,
) {
    match expr {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_default_effects_expr(function, interner, function_effects, out);
            if let Expression::Identifier { name, .. } = function.as_ref()
                && let Some(effects) = function_effects.get(name)
            {
                out.extend(effects.iter().copied());
            }
            for arg in arguments {
                collect_default_effects_expr(arg, interner, function_effects, out);
            }
        }
        Expression::Function { body, .. } | Expression::DoBlock { block: body, .. } => {
            for stmt in &body.statements {
                collect_default_effects_stmt(stmt, interner, function_effects, out);
            }
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_default_effects_expr(condition, interner, function_effects, out);
            for stmt in &consequence.statements {
                collect_default_effects_stmt(stmt, interner, function_effects, out);
            }
            if let Some(alt) = alternative {
                for stmt in &alt.statements {
                    collect_default_effects_stmt(stmt, interner, function_effects, out);
                }
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_default_effects_expr(scrutinee, interner, function_effects, out);
            for arm in arms {
                if let Some(guard) = arm.guard.as_ref() {
                    collect_default_effects_expr(guard, interner, function_effects, out);
                }
                collect_default_effects_expr(&arm.body, interner, function_effects, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_default_effects_expr(left, interner, function_effects, out);
            collect_default_effects_expr(right, interner, function_effects, out);
        }
        Expression::Prefix { right, .. } => {
            collect_default_effects_expr(right, interner, function_effects, out);
        }
        Expression::Perform { effect, args, .. } => {
            match interner.try_resolve(*effect) {
                Some(be::CONSOLE) => {
                    out.insert(be::CONSOLE);
                }
                Some(be::FILESYSTEM) => {
                    out.insert(be::FILESYSTEM);
                }
                Some(be::STDIN) => {
                    out.insert(be::STDIN);
                }
                Some(be::CLOCK) => {
                    out.insert(be::CLOCK);
                }
                _ => {}
            }
            for arg in args {
                collect_default_effects_expr(arg, interner, function_effects, out);
            }
        }
        Expression::Handle {
            expr: handled,
            parameter,
            arms,
            ..
        } => {
            collect_default_effects_expr(handled, interner, function_effects, out);
            if let Some(parameter) = parameter {
                collect_default_effects_expr(parameter, interner, function_effects, out);
            }
            for arm in arms {
                collect_default_effects_expr(&arm.body, interner, function_effects, out);
            }
        }
        Expression::Sealing { expr, .. }
        | Expression::MemberAccess { object: expr, .. }
        | Expression::TupleFieldAccess { object: expr, .. }
        | Expression::Some { value: expr, .. }
        | Expression::Left { value: expr, .. }
        | Expression::Right { value: expr, .. } => {
            collect_default_effects_expr(expr, interner, function_effects, out);
        }
        Expression::Index { left, index, .. } => {
            collect_default_effects_expr(left, interner, function_effects, out);
            collect_default_effects_expr(index, interner, function_effects, out);
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for elem in elements {
                collect_default_effects_expr(elem, interner, function_effects, out);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                collect_default_effects_expr(key, interner, function_effects, out);
                collect_default_effects_expr(value, interner, function_effects, out);
            }
        }
        Expression::Cons { head, tail, .. } => {
            collect_default_effects_expr(head, interner, function_effects, out);
            collect_default_effects_expr(tail, interner, function_effects, out);
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    collect_default_effects_expr(expr, interner, function_effects, out);
                }
            }
        }
        Expression::NamedConstructor { fields, .. } => {
            for field in fields {
                if let Some(value) = field.value.as_ref() {
                    collect_default_effects_expr(value, interner, function_effects, out);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            collect_default_effects_expr(base, interner, function_effects, out);
            for field in overrides {
                if let Some(value) = field.value.as_ref() {
                    collect_default_effects_expr(value, interner, function_effects, out);
                }
            }
        }
        Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
    }
}

fn default_handler_arms(
    effect: &str,
    interner: &mut Interner,
    ids: &mut ExprIdGen,
    user_effect_ops: &HashMap<Identifier, HashSet<Identifier>>,
    span: Span,
) -> Vec<HandleArm> {
    let effect_sym = interner.intern(effect);
    let allowed_ops = user_effect_ops.get(&effect_sym);
    let entries = ROUTED_PRIMOPS
        .iter()
        .copied()
        .filter(|entry| entry.effect == effect)
        .filter(|entry| {
            allowed_ops.is_none_or(|ops| {
                interner
                    .lookup(entry.operation)
                    .is_some_and(|op| ops.contains(&op))
            })
        })
        .collect::<Vec<_>>();
    entries
        .into_iter()
        .map(|entry| default_handler_arm(entry, interner, ids, span))
        .collect()
}

fn default_handler_arm(
    entry: RoutedPrimop,
    interner: &mut Interner,
    ids: &mut ExprIdGen,
    span: Span,
) -> HandleArm {
    let resume = interner.intern("__resume_default");
    let params = (0..entry.arity)
        .map(|idx| interner.intern(&format!("__arg{idx}")))
        .collect::<Vec<_>>();
    let primop_call = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: interner.intern(entry.internal_name),
            span,
            id: ids.next_id(),
        }),
        arguments: params
            .iter()
            .map(|param| Expression::Identifier {
                name: *param,
                span,
                id: ids.next_id(),
            })
            .collect(),
        span,
        id: ids.next_id(),
    };
    let resume_arg = if entry.returns_unit {
        Expression::TupleLiteral {
            elements: Vec::new(),
            span,
            id: ids.next_id(),
        }
    } else {
        primop_call.clone()
    };
    let resume_call = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: resume,
            span,
            id: ids.next_id(),
        }),
        arguments: vec![resume_arg],
        span,
        id: ids.next_id(),
    };
    let body = if entry.returns_unit {
        Expression::DoBlock {
            block: Block {
                statements: vec![
                    Statement::Expression {
                        expression: primop_call,
                        has_semicolon: true,
                        span,
                    },
                    Statement::Expression {
                        expression: resume_call,
                        has_semicolon: false,
                        span,
                    },
                ],
                span,
            },
            span,
            id: ids.next_id(),
        }
    } else {
        resume_call
    };
    HandleArm {
        operation_name: interner.intern(entry.operation),
        resume_param: resume,
        params,
        body,
        span,
    }
}
