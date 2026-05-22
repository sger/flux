use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use flux::types::{infer_type::InferType, type_constructor::TypeConstructor};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::keywords::{effect_doc, is_offset_in_comment_or_string, keyword_doc, word_at_offset};
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

    // Built-in effect hover: a structured doc card for `IO`, `Console`,
    // `FileSystem`, etc. Keyed off the AST effect-name node, so it fires
    // only in effect position — never on an identical identifier elsewhere.
    if let Some(doc) = builtin_effect_doc(snapshot, &node) {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: doc.to_string(),
            }),
            range: None,
        });
    }

    let value = render(snapshot, &node)?;
    // The top-level `fn main` is the program entry point — the compiler's
    // `validate_main_entrypoint` singles it out. Render its hover in the
    // same card shape as keyword docs: a `**`main`** — summary` header,
    // prose, then a ```flux block (here holding the inferred signature).
    let markdown = if is_entry_point_decl(snapshot, &node) {
        format!(
            "**`main`** — Program entry point.\n\n\
             The Flux runtime calls `main` to start execution; a top-level \
             `fn main` is what makes a file runnable.\n\n\
             ```flux\n{value}\n```",
        )
    } else {
        // Prepend the declaration's `///` doc comment as prose, above the
        // signature block, when one is found.
        match node_doc_comment(snapshot, &node) {
            Some(doc) => format!("{doc}\n\n```flux\n{value}\n```"),
            None => format!("```flux\n{value}\n```"),
        }
    };
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    })
}

/// The `///` doc comment to show above the hover signature, if any.
///
/// At a declaration site (`fn`, `data`, `effect`, variant) the comment is
/// scanned from the current buffer's own source above the decl span — so it
/// works for any open file, user modules included. At a module-member *use*
/// site (`Module.member`) the comment is scanned from that module's cached
/// source, the same way `completionItem/resolve` does.
fn node_doc_comment(snapshot: &Snapshot, node: &NodeRef) -> Option<String> {
    match node {
        NodeRef::DeclName {
            binding_span: span, ..
        }
        | NodeRef::DataName { span, .. }
        | NodeRef::DataVariantName { span, .. }
        | NodeRef::EffectDeclName { span, .. }
        // A `class`/`instance` method declaration: its doc run sits above the
        // method's `fn` line in this buffer.
        | NodeRef::ClassMethodName { span, .. }
        | NodeRef::InstanceMethodName { span, .. } => {
            crate::doc_comments::doc_for(&snapshot.program, *span)
        }
        // A type used in an annotation (`let x: MyType`): resolve it to its
        // declaration in this buffer (`data` / `alias` / `class`) and show that
        // declaration's doc comment. Built-in types have no buffer decl → None.
        NodeRef::TypeExprNamed { name, .. } => {
            let type_name = snapshot.interner.try_resolve(*name)?;
            let span = type_decl_span(&snapshot.program, &snapshot.interner, type_name)?;
            crate::doc_comments::doc_for(&snapshot.program, span)
        }
        NodeRef::MemberAccessMember { object, member, .. } => {
            let key = module_key_for(snapshot, object)?;
            let (program, _, _) = snapshot.module_programs.get(&key)?;
            let member_name = snapshot.interner.try_resolve(*member)?;
            crate::doc_comments::member_doc(program, &snapshot.interner, member_name)
        }
        _ => None,
    }
}

/// The `module_programs` key for the module referenced by `object` (the left
/// side of a `Module.member` access): a direct module name, or the underlying
/// module of an `import … as A` alias. `None` for a non-module object.
pub(crate) fn module_key_for(snapshot: &Snapshot, object: &Expression) -> Option<String> {
    let Expression::Identifier { name, .. } = object else {
        return None;
    };
    let resolved = snapshot.interner.try_resolve(*name)?;
    if snapshot.module_programs.contains_key(resolved) {
        return Some(resolved.to_string());
    }
    for stmt in &snapshot.program.statements {
        if let Statement::Import {
            name: qualified,
            alias: Some(alias),
            ..
        } = stmt
            && snapshot.interner.try_resolve(*alias) == Some(resolved)
        {
            let qualified = snapshot.interner.try_resolve(*qualified)?;
            if snapshot.module_programs.contains_key(qualified) {
                return Some(qualified.to_string());
            }
            let short = qualified.rsplit('.').next().unwrap_or(qualified);
            if snapshot.module_programs.contains_key(short) {
                return Some(short.to_string());
            }
        }
    }
    None
}

/// The declaration span of the type named `type_name` in `program` — a `data`,
/// `alias`, or `class`. Its start sits on the keyword line, so a `///` run above
/// the declaration is found by [`doc_comment_above`](crate::doc_comments::doc_comment_above).
/// `None` for a built-in type, which has no buffer declaration.
fn type_decl_span(program: &Program, interner: &Interner, type_name: &str) -> Option<FluxSpan> {
    for stmt in &program.statements {
        let (name, span) = match stmt {
            Statement::Data { name, span, .. } => (*name, *span),
            Statement::Class { name, span, .. } => (*name, *span),
            Statement::TypeAlias(alias) => (alias.name, alias.span),
            _ => continue,
        };
        if interner.try_resolve(name) == Some(type_name) {
            return Some(span);
        }
    }
    None
}

/// Markdown doc card for a built-in effect, if `node` is an effect-name use
/// or declaration whose label is one of the built-ins (`IO`, `Console`, …).
/// Returns `None` for a user-declared `effect`, which has no static doc.
fn builtin_effect_doc(snapshot: &Snapshot, node: &NodeRef) -> Option<&'static str> {
    let name = match node {
        NodeRef::EffectName { name, .. } | NodeRef::EffectDeclName { name, .. } => *name,
        _ => return None,
    };
    effect_doc(snapshot.interner.try_resolve(name)?)
}

/// Whether `node` is the declaration name of the top-level `fn main` — the
/// program's entry point. Matches on both the `main` symbol and the
/// declaration span, so a nested or module-scoped `fn main` (which is not an
/// entry point) is not annotated.
fn is_entry_point_decl(snapshot: &Snapshot, node: &NodeRef) -> bool {
    let NodeRef::DeclName { name, binding_span } = node else {
        return false;
    };
    if snapshot.interner.try_resolve(*name) != Some("main") {
        return false;
    }
    snapshot.program.statements.iter().any(|stmt| {
        matches!(
            stmt,
            Statement::Function { name: n, span, .. } if n == name && span == binding_span
        )
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
                    && *span == *function_span
                {
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
                            && let Some(scheme) = infer.resolved_binding_schemes_by_span.get(&key)
                            && let flux::types::infer_type::InferType::Fun(param_infer_types, _, _) =
                                &scheme.infer_type
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

            Some(format!("parameter: {resolved}"))
        }
        NodeRef::ClassMethodName {
            name, parent_class, ..
        }
        | NodeRef::InstanceMethodName {
            name, parent_class, ..
        } => {
            // Both render the class's declared signature for the method (an
            // instance method has no annotations of its own); fall back to a
            // bare label when the class isn't in this buffer.
            let method_name = snapshot.interner.try_resolve(*name)?;
            class_method_signature(snapshot, *parent_class, *name)
                .or_else(|| Some(format!("method: {method_name}")))
        }
    }
}

/// The declared signature of `method` in the `class` named `class`, found in
/// this buffer's own statements — `fn name(p: T, …) -> Ret`.
fn class_method_signature(
    snapshot: &Snapshot,
    class: Identifier,
    method: Identifier,
) -> Option<String> {
    for stmt in &snapshot.program.statements {
        if let Statement::Class { name, methods, .. } = stmt
            && *name == class
        {
            let m = methods.iter().find(|m| m.name == method)?;
            let method_name = snapshot.interner.try_resolve(m.name)?;
            let params: Vec<String> = m
                .params
                .iter()
                .zip(m.param_types.iter())
                .map(|(p, t)| {
                    let pn = snapshot.interner.try_resolve(*p).unwrap_or("_");
                    format!("{pn}: {}", render_type_expr(t, &snapshot.interner))
                })
                .collect();
            let ret = render_type_expr(&m.return_type, &snapshot.interner);
            return Some(format!("fn {method_name}({}) -> {ret}", params.join(", ")));
        }
    }
    None
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
