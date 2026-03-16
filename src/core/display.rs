/// Pretty-printer for Core IR.
///
/// Produces a human-readable, indented representation of a `CoreProgram` or
/// individual `CoreExpr`. The output is intended for debugging and for the
/// `--dump-core` CLI flag — it is not a round-trippable surface syntax.
use std::fmt::Write as FmtWrite;

use crate::syntax::interner::Interner;

use super::{
    CoreAlt, CoreBinder, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreProgram, CoreTag,
    CoreVarRef,
};

/// Pretty-print a complete `CoreProgram` to a `String`.
pub fn display_program(program: &CoreProgram, interner: &Interner) -> String {
    let mut out = String::new();
    for (i, def) in program.defs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let name = resolve(interner, def.name);
        let keyword = if def.is_recursive { "letrec" } else { "def" };
        writeln!(out, "{keyword} {name} =").unwrap();
        write_expr(&mut out, &def.expr, interner, 2);
        out.push('\n');
    }
    out
}

/// Pretty-print a single `CoreExpr` to a `String` (for tests / one-off use).
pub fn display_expr(expr: &CoreExpr, interner: &Interner) -> String {
    let mut out = String::new();
    write_expr(&mut out, expr, interner, 0);
    out
}

/// Resolve a symbol to its string form, falling back to `#<n>` for synthetic
/// symbols that were never registered in the interner (e.g. fresh temporaries).
fn resolve(interner: &Interner, id: crate::syntax::Identifier) -> String {
    interner
        .try_resolve(id)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("#{}", id.as_u32()))
}

fn write_expr(out: &mut String, expr: &CoreExpr, interner: &Interner, indent: usize) {
    match expr {
        CoreExpr::Var { var, .. } => {
            out.push_str(&resolve_var(interner, var));
        }
        CoreExpr::Lit(lit, _) => {
            write_lit(out, lit);
        }
        CoreExpr::Lam { params, body, .. } => {
            out.push('λ');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&resolve_binder(interner, p));
            }
            out.push('.');
            out.push('\n');
            push_indent(out, indent + 2);
            write_expr(out, body, interner, indent + 2);
        }
        CoreExpr::App { func, args, .. } => {
            write_expr(out, func, interner, indent);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr_inline(out, a, interner, indent);
            }
            out.push(')');
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            write!(out, "let {} = ", &resolve_binder(interner, var)).unwrap();
            write_expr_inline(out, rhs, interner, indent);
            out.push('\n');
            push_indent(out, indent);
            write_expr(out, body, interner, indent);
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            write!(out, "letrec {} = ", &resolve_binder(interner, var)).unwrap();
            write_expr_inline(out, rhs, interner, indent);
            out.push('\n');
            push_indent(out, indent);
            write_expr(out, body, interner, indent);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            out.push_str("case ");
            write_expr_inline(out, scrutinee, interner, indent);
            out.push_str(" of");
            for alt in alts {
                out.push('\n');
                write_alt(out, alt, interner, indent + 2);
            }
        }
        CoreExpr::Con { tag, fields, .. } => {
            write_tag(out, tag, interner);
            if !fields.is_empty() {
                out.push('(');
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_expr_inline(out, f, interner, indent);
                }
                out.push(')');
            }
        }
        CoreExpr::PrimOp { op, args, .. } => {
            write_primop_name(out, op);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr_inline(out, a, interner, indent);
            }
            out.push(')');
        }
        CoreExpr::Return { value, .. } => {
            out.push_str("return ");
            write_expr_inline(out, value, interner, indent);
        }
        CoreExpr::Perform {
            effect,
            operation,
            args,
            ..
        } => {
            write!(
                out,
                "perform {}.{}(",
                &resolve(interner, *effect),
                &resolve(interner, *operation)
            )
            .unwrap();
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr_inline(out, a, interner, indent);
            }
            out.push(')');
        }
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            ..
        } => {
            write!(out, "handle {} {{", &resolve(interner, *effect)).unwrap();
            for h in handlers {
                out.push('\n');
                write_handler(out, h, interner, indent + 2);
            }
            out.push('\n');
            push_indent(out, indent);
            out.push_str("} with\n");
            push_indent(out, indent + 2);
            write_expr(out, body, interner, indent + 2);
        }
    }
}

fn write_expr_inline(out: &mut String, expr: &CoreExpr, interner: &Interner, indent: usize) {
    let needs_parens = matches!(
        expr,
        CoreExpr::Lam { .. }
            | CoreExpr::Let { .. }
            | CoreExpr::LetRec { .. }
            | CoreExpr::Case { .. }
            | CoreExpr::Return { .. }
            | CoreExpr::Handle { .. }
    );
    if needs_parens {
        out.push('(');
        write_expr(out, expr, interner, indent);
        out.push(')');
    } else {
        write_expr(out, expr, interner, indent);
    }
}

fn write_alt(out: &mut String, alt: &CoreAlt, interner: &Interner, indent: usize) {
    push_indent(out, indent);
    write_pat(out, &alt.pat, interner);
    if let Some(guard) = &alt.guard {
        out.push_str(" if ");
        write_expr_inline(out, guard, interner, indent);
    }
    out.push_str(" →\n");
    push_indent(out, indent + 2);
    write_expr(out, &alt.rhs, interner, indent + 2);
}

fn write_handler(out: &mut String, h: &CoreHandler, interner: &Interner, indent: usize) {
    push_indent(out, indent);
    write!(out, "{}(", &resolve(interner, h.operation)).unwrap();
    for (i, p) in h.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&resolve_binder(interner, p));
    }
    writeln!(out, "; {}) →", &resolve_binder(interner, &h.resume)).unwrap();
    push_indent(out, indent + 2);
    write_expr(out, &h.body, interner, indent + 2);
}

fn write_pat(out: &mut String, pat: &CorePat, interner: &Interner) {
    match pat {
        CorePat::Wildcard => out.push('_'),
        CorePat::Var(binder) => out.push_str(&resolve_binder(interner, binder)),
        CorePat::Lit(lit) => write_lit(out, lit),
        CorePat::EmptyList => out.push_str("[]"),
        CorePat::Con { tag, fields } => {
            write_tag(out, tag, interner);
            if !fields.is_empty() {
                out.push('(');
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_pat(out, f, interner);
                }
                out.push(')');
            }
        }
        CorePat::Tuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_pat(out, e, interner);
            }
            out.push(')');
        }
    }
}

fn resolve_binder(interner: &Interner, binder: &CoreBinder) -> String {
    format!("{}#{}", resolve(interner, binder.name), binder.id.0)
}

fn resolve_var(interner: &Interner, var: &CoreVarRef) -> String {
    match var.binder {
        Some(id) => format!("{}#{}", resolve(interner, var.name), id.0),
        None => format!("{}#?", resolve(interner, var.name)),
    }
}

fn write_lit(out: &mut String, lit: &CoreLit) {
    match lit {
        CoreLit::Int(n) => write!(out, "{n}").unwrap(),
        CoreLit::Float(f) => write!(out, "{f}").unwrap(),
        CoreLit::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CoreLit::String(s) => write!(out, "{s:?}").unwrap(),
        CoreLit::Unit => out.push_str("()"),
    }
}

fn write_tag(out: &mut String, tag: &CoreTag, interner: &Interner) {
    match tag {
        CoreTag::Named(id) => out.push_str(&resolve(interner, *id)),
        CoreTag::None => out.push_str("None"),
        CoreTag::Some => out.push_str("Some"),
        CoreTag::Left => out.push_str("Left"),
        CoreTag::Right => out.push_str("Right"),
        CoreTag::Nil => out.push_str("Nil"),
        CoreTag::Cons => out.push_str("Cons"),
    }
}

fn write_primop_name(out: &mut String, op: &CorePrimOp) {
    let name = match op {
        CorePrimOp::Add => "Add",
        CorePrimOp::Sub => "Sub",
        CorePrimOp::Mul => "Mul",
        CorePrimOp::Div => "Div",
        CorePrimOp::Mod => "Mod",
        CorePrimOp::IAdd => "IAdd",
        CorePrimOp::ISub => "ISub",
        CorePrimOp::IMul => "IMul",
        CorePrimOp::IDiv => "IDiv",
        CorePrimOp::IMod => "IMod",
        CorePrimOp::FAdd => "FAdd",
        CorePrimOp::FSub => "FSub",
        CorePrimOp::FMul => "FMul",
        CorePrimOp::FDiv => "FDiv",
        CorePrimOp::Neg => "Neg",
        CorePrimOp::Not => "Not",
        CorePrimOp::Eq => "Eq",
        CorePrimOp::NEq => "NEq",
        CorePrimOp::Lt => "Lt",
        CorePrimOp::Le => "Le",
        CorePrimOp::Gt => "Gt",
        CorePrimOp::Ge => "Ge",
        CorePrimOp::And => "And",
        CorePrimOp::Or => "Or",
        CorePrimOp::Concat => "Concat",
        CorePrimOp::Interpolate => "Interpolate",
        CorePrimOp::MakeList => "MakeList",
        CorePrimOp::MakeArray => "MakeArray",
        CorePrimOp::MakeTuple => "MakeTuple",
        CorePrimOp::MakeHash => "MakeHash",
        CorePrimOp::Index => "Index",
        CorePrimOp::MemberAccess(id) => {
            let _ = id;
            "MemberAccess"
        }
        CorePrimOp::TupleField(n) => {
            write!(out, "TupleField[{n}]").unwrap();
            return;
        }
    };
    out.push_str(name);
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}
