use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::syntax::Identifier;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use flux::types::{infer_type::InferType, type_constructor::TypeConstructor};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::keywords::{is_offset_in_comment_or_string, keyword_doc, word_at_offset};
use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

pub fn hover_at(snapshot: &Snapshot, position: Position) -> Option<Hover> {
    // Keyword check before AST lookup: the locator returns the enclosing
    // statement for keywords (since they don't have their own AST node),
    // which produces less-useful hover content than dedicated docs.
    // Reject hover-in-comment/string so prose like `// use let to bind`
    // doesn't surface keyword docs for the prose mention.
    if let Some(offset) = snapshot.position_map.lsp_to_offset(position) {
        let off: usize = offset.into();
        if !is_offset_in_comment_or_string(snapshot.text.as_ref(), off)
            && let Some(word) = word_at_offset(snapshot.text.as_ref(), off)
            && let Some(doc) = keyword_doc(word)
        {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc.to_string(),
                }),
                range: None,
            });
        }
    }

    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let value = render(snapshot, &node)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```flux\n{value}\n```"),
        }),
        range: None,
    })
}

fn render(snapshot: &Snapshot, node: &NodeRef) -> Option<String> {
    match node {
        NodeRef::Expr(expr) => render_expr(snapshot, expr),
        NodeRef::Pattern(pattern) => render_pattern(snapshot, pattern),
        NodeRef::Statement(_) => None,
        NodeRef::TypeExprNamed { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("type: {resolved}"))
        }
        NodeRef::EffectName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("effect: {resolved}"))
        }
        NodeRef::EffectRowVar { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("row var: |{resolved}"))
        }
        NodeRef::MemberAccessMember { object, member, .. } => {
            let member_name = snapshot.interner.try_resolve(*member)?;
            // Best-effort: use the object's inferred type to find the data
            // declaration the field lives on, then render its declared
            // `TypeExpr`. Falls back to a bare label when the object's type
            // isn't a known ADT or the field isn't named there.
            if let Some(adt_sym) = inferred_adt_symbol(snapshot, object)
                && let Some(fields) = snapshot.variant_fields.get(&adt_sym)
                && let Some((_, ty)) = fields.iter().find(|(n, _)| *n == *member)
            {
                return Some(format!(
                    "{member_name}: {}",
                    render_type_expr(ty, &snapshot.interner)
                ));
            }
            // Module member: look up the scheme from inference results.
            if let Expression::Identifier {
                name: module_id, ..
            } = object
                && let Some(infer) = snapshot.infer.as_ref()
                && let Some(scheme) = infer.module_member_schemes.get(&(*module_id, *member))
            {
                return Some(render_scheme_canonical(&snapshot.interner, scheme));
            }
            Some(format!("field: {member_name}"))
        }
        NodeRef::NamedFieldInitName {
            field_name,
            parent_constructor,
            ..
        } => {
            let resolved = snapshot.interner.try_resolve(*field_name)?;
            if let Some(parent) = parent_constructor
                && let Some(fields) = snapshot.variant_fields.get(parent)
                && let Some((_, ty)) = fields.iter().find(|(n, _)| *n == *field_name)
            {
                return Some(format!(
                    "{resolved}: {}",
                    render_type_expr(ty, &snapshot.interner)
                ));
            }
            Some(format!("field: {resolved}"))
        }
        NodeRef::NamedConstructorName { name, expr_id, .. } => {
            if let Some(infer) = snapshot.infer.as_ref()
                && let Some(ty) = infer.expr_types.get(expr_id)
            {
                return Some(display_infer_type(ty, &snapshot.interner));
            }
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("constructor: {resolved}"))
        }
        NodeRef::PerformOpName { name, .. } | NodeRef::HandleArmOpName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("operation: {resolved}"))
        }
        NodeRef::ImportName { qualified, .. } => {
            let resolved = snapshot.interner.try_resolve(*qualified)?;
            Some(format!("module: {resolved}"))
        }
        NodeRef::ImportAlias {
            alias, qualified, ..
        } => {
            let a = snapshot.interner.try_resolve(*alias)?;
            let q = snapshot.interner.try_resolve(*qualified)?;
            Some(format!("alias {a} = {q}"))
        }
        NodeRef::DataName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("data: {resolved}"))
        }
        NodeRef::DataVariantName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("variant: {resolved}"))
        }
        NodeRef::DataFieldName {
            name,
            ty_index,
            parent_variant,
            ..
        } => {
            let field_name = snapshot.interner.try_resolve(*name)?;
            if let Some(fields) = snapshot.variant_fields.get(parent_variant)
                && let Some((_, ty)) = fields.get(*ty_index)
            {
                return Some(format!(
                    "{field_name}: {}",
                    render_type_expr(ty, &snapshot.interner)
                ));
            }
            Some(format!("field: {field_name}"))
        }
        NodeRef::EffectDeclName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("effect: {resolved}"))
        }
        NodeRef::EffectOpName {
            name,
            parent_effect,
            ..
        } => {
            let op_name = snapshot.interner.try_resolve(*name)?;
            if let Some(op_ty) = find_effect_op_type(snapshot, *parent_effect, *name) {
                return Some(format!(
                    "{op_name}: {}",
                    render_type_expr(&op_ty, &snapshot.interner)
                ));
            }
            Some(format!("op: {op_name}"))
        }
        NodeRef::DeclName { name, binding_span } => {
            let key = (
                binding_span.start.line,
                binding_span.start.column,
                binding_span.end.line,
                binding_span.end.column,
            );
            if let Some(infer) = snapshot.infer.as_ref()
                && let Some(scheme) = infer.resolved_binding_schemes_by_span.get(&key)
            {
                return Some(render_scheme_canonical(&snapshot.interner, scheme));
            }
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("decl: {resolved}"))
        }
        NodeRef::FunctionParameter {
            name,
            function_span,
            ..
        } => {
            let resolved = snapshot.interner.try_resolve(*name)?;

            for stmt in &snapshot.program.statements {
                if let Statement::Function {
                    span,
                    parameters,
                    parameter_types,
                    ..
                } = stmt
                {
                    if *span == *function_span {
                        if let Some(idx) = parameters.iter().position(|p| *p == *name) {
                            // Prefer the written type annotation.
                            if let Some(Some(ty)) = parameter_types.get(idx) {
                                return Some(format!(
                                    "{resolved}: {}",
                                    ty.display_with(&snapshot.interner)
                                ));
                            }
                            // Fall back to the inferred parameter type from the
                            // function's scheme (Fun(param_types, _, _)).
                            let key = (
                                span.start.line,
                                span.start.column,
                                span.end.line,
                                span.end.column,
                            );
                            if let Some(infer) = snapshot.infer.as_ref()
                                && let Some(scheme) =
                                    infer.resolved_binding_schemes_by_span.get(&key)
                                && let flux::types::infer_type::InferType::Fun(
                                    param_infer_types,
                                    _,
                                    _,
                                ) = &scheme.infer_type
                                && let Some(param_ty) = param_infer_types.get(idx)
                            {
                                return Some(format!(
                                    "{resolved}: {}",
                                    display_infer_type(param_ty, &snapshot.interner)
                                ));
                            }
                        }
                        break;
                    }
                }
            }

            Some(format!("parameter: {resolved}"))
        }
    }
}

/// Extract the ADT symbol from `expr`'s inferred type, if it has one. Used
/// to map a record-access target back to its data declaration for field
/// lookups. Handles both `Con(Adt(s))` and `App(Adt(s), _)` forms.
fn inferred_adt_symbol(snapshot: &Snapshot, expr: &Expression) -> Option<Identifier> {
    let infer = snapshot.infer.as_ref()?;
    let ty = infer.expr_types.get(&expr.expr_id())?;
    match ty {
        InferType::Con(TypeConstructor::Adt(s)) | InferType::App(TypeConstructor::Adt(s), _) => {
            Some(*s)
        }
        _ => None,
    }
}

/// Walk `Statement::EffectDecl` blocks in the program for one named
/// `parent_effect`, return the declared type of operation `op_name` if
/// found.
fn find_effect_op_type(
    snapshot: &Snapshot,
    parent_effect: Identifier,
    op_name: Identifier,
) -> Option<TypeExpr> {
    for stmt in &snapshot.program.statements {
        if let Statement::EffectDecl { name, ops, .. } = stmt
            && *name == parent_effect
        {
            for op in ops {
                if op.name == op_name {
                    return Some(op.type_expr.clone());
                }
            }
        }
    }
    None
}

/// Surface-syntax rendering of a `TypeExpr`. We have no compiler-side
/// pretty-printer that takes an interner, so produce one ourselves for the
/// small subset we encounter in field/op type rendering.
fn render_type_expr(ty: &TypeExpr, interner: &flux::syntax::interner::Interner) -> String {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let base = interner.try_resolve(*name).unwrap_or("?").to_string();
            if args.is_empty() {
                base
            } else {
                let rendered: Vec<String> =
                    args.iter().map(|a| render_type_expr(a, interner)).collect();
                format!("{base}<{}>", rendered.join(", "))
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            let parts: Vec<String> = elements
                .iter()
                .map(|e| render_type_expr(e, interner))
                .collect();
            format!("({})", parts.join(", "))
        }
        TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            let params_str: Vec<String> = params
                .iter()
                .map(|p| render_type_expr(p, interner))
                .collect();
            let ret_str = render_type_expr(ret, interner);
            let core = format!("({}) -> {}", params_str.join(", "), ret_str);
            if effects.is_empty() {
                core
            } else {
                // Best-effort: print the effect rows by their root names.
                format!("{core} with ...")
            }
        }
    }
}

fn render_pattern(snapshot: &Snapshot, pat: &Pattern) -> Option<String> {
    match pat {
        Pattern::Identifier { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            if let Some(infer) = snapshot.infer.as_ref()
                && let Some(scheme) = infer.resolved_binding_schemes.get(name)
            {
                return Some(render_scheme_canonical(&snapshot.interner, scheme));
            }
            Some(format!("binding: {resolved}"))
        }
        _ => None,
    }
}

fn render_expr(snapshot: &Snapshot, expr: &Expression) -> Option<String> {
    if let Some(infer) = snapshot.infer.as_ref()
        && let Some(ty) = infer.expr_types.get(&expr.expr_id())
    {
        return Some(display_infer_type(ty, &snapshot.interner));
    }
    // Module-name fallback: identifier expression on the LHS of a member
    // access whose name matches a loaded module. The locator emits a more
    // specific `MemberAccessMember` for the RHS; this branch handles the
    // LHS (the bare identifier).
    if let Expression::Identifier { name, .. } = expr
        && let Some(resolved) = snapshot.interner.try_resolve(*name)
        && snapshot.module_short_names.contains(resolved)
    {
        return Some(format!("module: {resolved}"));
    }
    None
}
