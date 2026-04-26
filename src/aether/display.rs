use crate::aether::{AetherAlt, AetherDef, AetherExpr, AetherHandler, AetherProgram};
use crate::core::{CoreLit, CorePat, CoreTag, CoreVarRef};
use crate::syntax::Identifier;
use crate::syntax::interner::Interner;

pub fn display_expr_readable(expr: &AetherExpr, interner: &Interner) -> String {
    let mut out = String::new();
    fmt_expr(expr, interner, 0, &mut out);
    out
}

pub fn display_def_readable(def: &AetherDef, interner: &Interner) -> String {
    let mut out = String::new();
    out.push_str("def ");
    out.push_str(&resolve_name(interner, def.name));
    out.push_str(" =\n");
    fmt_expr(&def.expr, interner, 2, &mut out);
    out
}

pub fn display_program_readable(program: &AetherProgram, interner: &Interner) -> String {
    let mut out = String::new();
    for (idx, def) in program.defs.iter().enumerate() {
        if idx > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&display_def_readable(def, interner));
    }
    out
}

pub fn single_line_expr(expr: &AetherExpr, interner: &Interner) -> String {
    let rendered = display_expr_readable(expr, interner);
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn format_var_ref(var: &CoreVarRef, interner: &Interner) -> String {
    match var.binder {
        Some(binder) => format!("{}#{}", resolve_name(interner, var.name), binder.0),
        None => resolve_name(interner, var.name),
    }
}

pub fn tag_label(tag: &CoreTag, interner: &Interner) -> String {
    match tag {
        CoreTag::None => "None".to_string(),
        CoreTag::Some => "Some".to_string(),
        CoreTag::Left => "Left".to_string(),
        CoreTag::Right => "Right".to_string(),
        CoreTag::Nil => "Nil".to_string(),
        CoreTag::Cons => "Cons".to_string(),
        CoreTag::Named(name) => resolve_name(interner, *name),
    }
}

fn fmt_expr(expr: &AetherExpr, interner: &Interner, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match expr {
        AetherExpr::Var { var, .. } => {
            out.push_str(&pad);
            out.push_str(&format_var_ref(var, interner));
        }
        AetherExpr::Lit(lit, _) => {
            out.push_str(&pad);
            out.push_str(&fmt_lit(lit));
        }
        AetherExpr::Lam { params, body, .. } => {
            out.push_str(&pad);
            out.push_str("lam(");
            for (idx, param) in params.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&resolve_name(interner, param.name));
            }
            out.push_str(") ->\n");
            fmt_expr(body, interner, indent + 2, out);
        }
        AetherExpr::App { func, args, .. } => {
            out.push_str(&pad);
            out.push_str("app(");
            out.push_str(&single_line_expr(func, interner));
            if !args.is_empty() {
                out.push_str(", ");
                out.push_str(
                    &args
                        .iter()
                        .map(|arg| single_line_expr(arg, interner))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            out.push(')');
        }
        AetherExpr::AetherCall {
            func,
            args,
            arg_modes,
            ..
        } => {
            out.push_str(&pad);
            out.push_str("aether_call[");
            out.push_str(
                &arg_modes
                    .iter()
                    .map(|mode| format!("{mode:?}").to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push_str("] ");
            out.push_str(&single_line_expr(func, interner));
            out.push('(');
            out.push_str(
                &args
                    .iter()
                    .map(|arg| single_line_expr(arg, interner))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push(')');
        }
        AetherExpr::Let { var, rhs, body, .. } => {
            out.push_str(&pad);
            out.push_str("let ");
            out.push_str(&resolve_name(interner, var.name));
            out.push_str(" =\n");
            fmt_expr(rhs, interner, indent + 2, out);
            out.push('\n');
            fmt_expr(body, interner, indent, out);
        }
        AetherExpr::LetRec { var, rhs, body, .. } => {
            out.push_str(&pad);
            out.push_str("letrec ");
            out.push_str(interner.resolve(var.name));
            out.push_str(" =\n");
            fmt_expr(rhs, interner, indent + 2, out);
            out.push('\n');
            fmt_expr(body, interner, indent, out);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            out.push_str(&pad);
            out.push_str("letrec_group");
            for (binder, rhs) in bindings {
                out.push('\n');
                out.push_str(&" ".repeat(indent + 2));
                out.push_str(interner.resolve(binder.name));
                out.push_str(" =\n");
                fmt_expr(rhs, interner, indent + 4, out);
            }
            out.push('\n');
            fmt_expr(body, interner, indent, out);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            out.push_str(&pad);
            out.push_str("case ");
            out.push_str(&single_line_expr(scrutinee, interner));
            out.push_str(" of");
            for alt in alts {
                out.push('\n');
                fmt_alt(alt, interner, indent + 2, out);
            }
        }
        AetherExpr::Con { tag, fields, .. } => {
            out.push_str(&pad);
            out.push_str(&tag_label(tag, interner));
            if !fields.is_empty() {
                out.push('(');
                out.push_str(
                    &fields
                        .iter()
                        .map(|field| single_line_expr(field, interner))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
        }
        AetherExpr::PrimOp { op, args, .. } => {
            out.push_str(&pad);
            out.push_str(&format!("{op:?}"));
            out.push('(');
            out.push_str(
                &args
                    .iter()
                    .map(|arg| single_line_expr(arg, interner))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push(')');
        }
        AetherExpr::MemberAccess { object, member, .. } => {
            out.push_str(&pad);
            out.push_str(&single_line_expr(object, interner));
            out.push('.');
            out.push_str(&resolve_name(interner, *member));
        }
        AetherExpr::TupleField { object, index, .. } => {
            out.push_str(&pad);
            out.push_str(&single_line_expr(object, interner));
            out.push('.');
            out.push_str(&index.to_string());
        }
        AetherExpr::Return { value, .. } => {
            out.push_str(&pad);
            out.push_str("return ");
            out.push_str(&single_line_expr(value, interner));
        }
        AetherExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            out.push_str(&pad);
            // Annotate with the Phase 3 runtime lowering (yield_to) so the
            // dump makes the evidence-passing vocabulary explicit.
            out.push_str("perform /*yield_to*/ ");
            out.push_str(&resolve_name(interner, *effect));
            out.push('.');
            out.push_str(&resolve_name(interner, *operation));
            out.push('(');
            out.push_str(
                &args
                    .iter()
                    .map(|arg| single_line_expr(arg, interner))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push(')');
        }
        AetherExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            ..
        } => {
            out.push_str(&pad);
            // Annotate with the Phase 3 lowering shape:
            //   evv_insert + fresh_marker → body → yield_prompt
            out.push_str("handle /*evv_insert+yield_prompt*/ ");
            out.push_str(&single_line_expr(body, interner));
            out.push_str(" with ");
            out.push_str(interner.resolve(*effect));
            if let Some(parameter) = parameter {
                out.push('(');
                out.push_str(&single_line_expr(parameter, interner));
                out.push(')');
            }
            for handler in handlers {
                out.push('\n');
                fmt_handler(handler, interner, indent + 2, out);
            }
        }
        AetherExpr::Dup { .. } => {
            let (mut vars, tail) = collect_dup_chain(expr);
            vars.sort_unstable_by_key(|var| format_var_ref(var, interner));
            for (idx, var) in vars.iter().enumerate() {
                out.push_str(&" ".repeat(indent + idx * 2));
                out.push_str("dup ");
                out.push_str(&format_var_ref(var, interner));
                out.push_str(" in\n");
            }
            fmt_expr(tail, interner, indent + vars.len() * 2, out);
        }
        AetherExpr::Drop { .. } => {
            let (mut vars, tail) = collect_drop_chain(expr);
            vars.sort_unstable_by_key(|var| format_var_ref(var, interner));
            for (idx, var) in vars.iter().enumerate() {
                out.push_str(&" ".repeat(indent + idx * 2));
                out.push_str("drop ");
                out.push_str(&format_var_ref(var, interner));
                out.push_str(" in\n");
            }
            fmt_expr(tail, interner, indent + vars.len() * 2, out);
        }
        AetherExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            ..
        } => {
            out.push_str(&pad);
            out.push_str("reuse ");
            out.push_str(&format_var_ref(token, interner));
            out.push_str(" as ");
            out.push_str(&tag_label(tag, interner));
            if let Some(mask) = field_mask {
                out.push_str(&format!(" mask=0x{mask:x}"));
            }
            if !fields.is_empty() {
                out.push('(');
                out.push_str(
                    &fields
                        .iter()
                        .map(|field| single_line_expr(field, interner))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            out.push_str(&pad);
            out.push_str("drop_specialized ");
            out.push_str(&format_var_ref(scrutinee, interner));
            out.push_str(" {\n");
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("unique ->\n");
            fmt_expr(unique_body, interner, indent + 4, out);
            out.push('\n');
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("shared ->\n");
            fmt_expr(shared_body, interner, indent + 4, out);
            out.push('\n');
            out.push_str(&pad);
            out.push('}');
        }
    }
}

fn collect_dup_chain(expr: &AetherExpr) -> (Vec<&CoreVarRef>, &AetherExpr) {
    let mut vars = Vec::new();
    let mut current = expr;
    while let AetherExpr::Dup { var, body, .. } = current {
        vars.push(var);
        current = body;
    }
    (vars, current)
}

fn collect_drop_chain(expr: &AetherExpr) -> (Vec<&CoreVarRef>, &AetherExpr) {
    let mut vars = Vec::new();
    let mut current = expr;
    while let AetherExpr::Drop { var, body, .. } = current {
        vars.push(var);
        current = body;
    }
    (vars, current)
}

fn fmt_alt(alt: &AetherAlt, interner: &Interner, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    out.push_str(&fmt_pat(&alt.pat, interner));
    if let Some(guard) = &alt.guard {
        out.push_str(" if ");
        out.push_str(&single_line_expr(guard, interner));
    }
    out.push_str(" ->\n");
    fmt_expr(&alt.rhs, interner, indent + 2, out);
}

fn fmt_handler(handler: &AetherHandler, interner: &Interner, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    out.push_str(&resolve_name(interner, handler.operation));
    out.push('(');
    out.push_str(&resolve_name(interner, handler.resume.name));
    for param in &handler.params {
        out.push_str(", ");
        out.push_str(&resolve_name(interner, param.name));
    }
    if let Some(state) = &handler.state {
        out.push_str(", ");
        out.push_str(&resolve_name(interner, state.name));
    }
    out.push_str(") ->\n");
    fmt_expr(&handler.body, interner, indent + 2, out);
}

fn fmt_pat(pat: &CorePat, interner: &Interner) -> String {
    match pat {
        CorePat::Wildcard => "_".to_string(),
        CorePat::Var(binder) => resolve_name(interner, binder.name),
        CorePat::Lit(lit) => fmt_lit(lit),
        CorePat::Con { tag, fields } => {
            let mut out = tag_label(tag, interner);
            if !fields.is_empty() {
                out.push('(');
                out.push_str(
                    &fields
                        .iter()
                        .map(|field| fmt_pat(field, interner))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
            out
        }
        CorePat::Tuple(fields) => format!(
            "({})",
            fields
                .iter()
                .map(|field| fmt_pat(field, interner))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CorePat::EmptyList => "[]".to_string(),
    }
}

fn fmt_lit(lit: &CoreLit) -> String {
    match lit {
        CoreLit::Int(value) => value.to_string(),
        CoreLit::Float(value) => value.to_string(),
        CoreLit::Bool(value) => value.to_string(),
        CoreLit::String(value) => format!("{value:?}"),
        CoreLit::Unit => "()".to_string(),
    }
}

fn resolve_name(interner: &Interner, name: Identifier) -> String {
    interner
        .try_resolve(name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("<sym:{}>", name.as_u32()))
}
