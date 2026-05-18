use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::data_variant::DataVariant;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{Location, Position, Uri};

use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

pub fn find_references(snapshot: &Snapshot, uri: &Uri, position: Position) -> Vec<Location> {
    let Some(target) = snapshot.position_map.lsp_to_flux(position) else {
        return vec![];
    };
    let Some(node) = find_at(&snapshot.program, &snapshot.interner, target) else {
        return vec![];
    };
    let Some(target_id) = node_identifier(&node) else {
        return vec![];
    };
    let mut spans: Vec<FluxSpan> = Vec::new();
    collect_all_uses(&snapshot.program, target_id, &mut spans);
    spans
        .into_iter()
        .map(|span| Location {
            uri: uri.clone(),
            range: snapshot.position_map.flux_span_to_range(span),
        })
        .collect()
}

/// Extract the canonical `Identifier` from a `NodeRef`. Used by both
/// find-references and rename to resolve the cursor's target symbol.
pub(crate) fn node_identifier(node: &NodeRef) -> Option<Identifier> {
    match node {
        NodeRef::DeclName { name, .. }
        | NodeRef::DataName { name, .. }
        | NodeRef::EffectDeclName { name, .. }
        | NodeRef::DataVariantName { name, .. }
        | NodeRef::NamedConstructorName { name, .. }
        | NodeRef::PerformOpName { name, .. }
        | NodeRef::HandleArmOpName { name, .. }
        | NodeRef::FunctionParameter { name, .. } => Some(*name),
        NodeRef::Expr(Expression::Identifier { name, .. }) => Some(*name),
        NodeRef::MemberAccessMember { member, .. } => Some(*member),
        NodeRef::NamedFieldInitName { field_name, .. } => Some(*field_name),
        NodeRef::DataFieldName { name, .. } => Some(*name),
        NodeRef::EffectOpName { name, .. } => Some(*name),
        _ => None,
    }
}

/// Walk the entire program and collect every span where `target` appears.
pub fn collect_all_uses(program: &Program, target: Identifier, out: &mut Vec<FluxSpan>) {
    for stmt in &program.statements {
        collect_in_stmt(stmt, target, out);
    }
}

fn collect_in_stmt(stmt: &Statement, target: Identifier, out: &mut Vec<FluxSpan>) {
    match stmt {
        Statement::Let { name, value, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            collect_in_expr(value, target, out);
        }
        Statement::LetDestructure { pattern, value, .. } => {
            collect_in_pattern(pattern, target, out);
            collect_in_expr(value, target, out);
        }
        Statement::Function { name, parameters, body, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            for param in parameters {
                if *param == target {
                    out.push(*span);
                }
            }
            collect_in_block(body, target, out);
        }
        Statement::Return { value: Some(v), .. } => collect_in_expr(v, target, out),
        Statement::Return { value: None, .. } => {}
        Statement::Expression { expression, .. } => collect_in_expr(expression, target, out),
        Statement::Assign { name, value, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            collect_in_expr(value, target, out);
        }
        Statement::Module { name, body, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            collect_in_block(body, target, out);
        }
        Statement::Data { name, variants, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            for variant in variants {
                collect_in_variant(variant, target, out);
            }
        }
        Statement::EffectDecl { name, ops, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            for op in ops {
                if op.name == target {
                    out.push(op.span);
                }
            }
        }
        Statement::Import { name, alias, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            if let Some(a) = alias {
                if *a == target {
                    out.push(*span);
                }
            }
        }
        _ => {}
    }
}

fn collect_in_variant(variant: &DataVariant, target: Identifier, out: &mut Vec<FluxSpan>) {
    if variant.name == target {
        out.push(variant.span);
    }
    if let Some(field_names) = &variant.field_names {
        for field_name in field_names {
            if *field_name == target {
                out.push(variant.span);
            }
        }
    }
}

fn collect_in_expr(expr: &Expression, target: Identifier, out: &mut Vec<FluxSpan>) {
    match expr {
        Expression::Identifier { name, span, .. } => {
            if *name == target {
                out.push(*span);
            }
        }
        Expression::Call { function, arguments, .. } => {
            collect_in_expr(function, target, out);
            for a in arguments {
                collect_in_expr(a, target, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_in_expr(left, target, out);
            collect_in_expr(right, target, out);
        }
        Expression::Prefix { right, .. } => {
            collect_in_expr(right, target, out);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_in_expr(condition, target, out);
            collect_in_block(consequence, target, out);
            if let Some(b) = alternative {
                collect_in_block(b, target, out);
            }
        }
        Expression::Match { scrutinee, arms, .. } => {
            collect_in_expr(scrutinee, target, out);
            for arm in arms {
                collect_in_pattern(&arm.pattern, target, out);
                collect_in_expr(&arm.body, target, out);
            }
        }
        Expression::MemberAccess { object, member, span, .. } => {
            collect_in_expr(object, target, out);
            if *member == target {
                out.push(*span);
            }
        }
        Expression::NamedConstructor { name, fields, span, .. } => {
            if *name == target {
                out.push(*span);
            }
            for f in fields {
                if f.name == target {
                    out.push(f.span);
                }
                if let Some(v) = &f.value {
                    collect_in_expr(v, target, out);
                }
            }
        }
        Expression::Function { body, .. } => {
            collect_in_block(body, target, out);
        }
        Expression::DoBlock { block, .. } => {
            collect_in_block(block, target, out);
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. } => {
            for e in elements {
                collect_in_expr(e, target, out);
            }
        }
        Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                collect_in_expr(e, target, out);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            collect_in_expr(expr, target, out);
            for arm in arms {
                collect_in_expr(&arm.body, target, out);
            }
        }
        Expression::Perform { operation, args, span, .. } => {
            if *operation == target {
                out.push(*span);
            }
            for a in args {
                collect_in_expr(a, target, out);
            }
        }
        Expression::Spread { base, overrides, .. } => {
            collect_in_expr(base, target, out);
            for f in overrides {
                if f.name == target {
                    out.push(f.span);
                }
                if let Some(v) = &f.value {
                    collect_in_expr(v, target, out);
                }
            }
        }
        Expression::Index { left, index, .. } => {
            collect_in_expr(left, target, out);
            collect_in_expr(index, target, out);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => {
            collect_in_expr(value, target, out);
        }
        Expression::Cons { head, tail, .. } => {
            collect_in_expr(head, target, out);
            collect_in_expr(tail, target, out);
        }
        _ => {}
    }
}

fn collect_in_block(block: &Block, target: Identifier, out: &mut Vec<FluxSpan>) {
    for s in &block.statements {
        collect_in_stmt(s, target, out);
    }
}

fn collect_in_pattern(pat: &Pattern, target: Identifier, out: &mut Vec<FluxSpan>) {
    match pat {
        Pattern::Identifier { name, span } => {
            if *name == target {
                out.push(*span);
            }
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                collect_in_pattern(e, target, out);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                collect_in_pattern(f, target, out);
            }
        }
        Pattern::Cons { head, tail, .. } => {
            collect_in_pattern(head, target, out);
            collect_in_pattern(tail, target, out);
        }
        _ => {}
    }
}
