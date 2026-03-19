/// Pretty-printer for Core IR.
///
/// Produces human-readable and debug-oriented representations of a
/// `CoreProgram` or individual `CoreExpr`. The output is intended for
/// debugging and for the `--dump-core` CLI flag — it is not a round-trippable
/// surface syntax.
use std::{collections::HashMap, fmt::Write as FmtWrite};

use crate::syntax::{Identifier, interner::Interner};

use super::{
    CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp,
    CoreProgram, CoreTag, CoreVarRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreDisplayMode {
    Readable,
    Debug,
}

/// Pretty-print a complete `CoreProgram` to a `String` in readable mode.
pub fn display_program(program: &CoreProgram, interner: &Interner) -> String {
    display_program_readable(program, interner)
}

pub fn display_program_readable(program: &CoreProgram, interner: &Interner) -> String {
    Formatter::new(interner, CoreDisplayMode::Readable).display_program(program)
}

pub fn display_program_debug(program: &CoreProgram, interner: &Interner) -> String {
    Formatter::new(interner, CoreDisplayMode::Debug).display_program(program)
}

/// Pretty-print a single `CoreExpr` to a `String` (for tests / one-off use).
pub fn display_expr(expr: &CoreExpr, interner: &Interner) -> String {
    display_expr_readable(expr, interner)
}

pub fn display_expr_readable(expr: &CoreExpr, interner: &Interner) -> String {
    Formatter::new(interner, CoreDisplayMode::Readable).display_expr(expr)
}

pub fn display_expr_debug(expr: &CoreExpr, interner: &Interner) -> String {
    Formatter::new(interner, CoreDisplayMode::Debug).display_expr(expr)
}

struct Formatter<'a> {
    interner: &'a Interner,
    mode: CoreDisplayMode,
    temp_names: HashMap<CoreBinderId, usize>,
    next_temp: usize,
}

impl<'a> Formatter<'a> {
    fn new(interner: &'a Interner, mode: CoreDisplayMode) -> Self {
        Self {
            interner,
            mode,
            temp_names: HashMap::new(),
            next_temp: 0,
        }
    }

    fn display_program(mut self, program: &CoreProgram) -> String {
        let mut out = String::new();
        for (i, def) in program.defs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if matches!(self.mode, CoreDisplayMode::Readable)
                && is_readable_main_thunk(def.name, def.is_recursive, &def.expr, self.interner)
            {
                writeln!(out, "def main =").unwrap();
                if let CoreExpr::Lam { body, .. } = &def.expr {
                    self.write_expr(&mut out, body, 2);
                    out.push('\n');
                }
                continue;
            }

            let name = self.resolve_name(def.name);
            let keyword = if def.is_recursive { "letrec" } else { "def" };
            if matches!(self.mode, CoreDisplayMode::Debug) {
                if let Some(ty) = &def.result_ty {
                    let ty_str = format_core_type(ty, self.interner);
                    writeln!(out, "{keyword} {name} : {ty_str} =").unwrap();
                } else {
                    writeln!(out, "{keyword} {name} =").unwrap();
                }
            } else {
                writeln!(out, "{keyword} {name} =").unwrap();
            }
            self.write_expr(&mut out, &def.expr, 2);
            out.push('\n');
        }
        out
    }

    fn display_expr(mut self, expr: &CoreExpr) -> String {
        let mut out = String::new();
        self.write_expr(&mut out, expr, 0);
        out
    }

    fn write_expr(&mut self, out: &mut String, expr: &CoreExpr, indent: usize) {
        match expr {
            CoreExpr::Var { var, .. } => out.push_str(&self.resolve_var(var)),
            CoreExpr::Lit(lit, _) => write_lit(out, lit),
            CoreExpr::Lam { params, body, .. } => {
                out.push('λ');
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&self.resolve_binder(p));
                }
                out.push('.');
                out.push('\n');
                push_indent(out, indent + 2);
                self.write_expr(out, body, indent + 2);
            }
            CoreExpr::App { func, args, .. } => {
                self.write_expr(out, func, indent);
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.write_expr_inline(out, a, indent);
                }
                out.push(')');
            }
            CoreExpr::Let { var, rhs, body, .. } => {
                write!(out, "let {} = ", self.resolve_binder(var)).unwrap();
                self.write_expr_inline(out, rhs, indent);
                out.push('\n');
                push_indent(out, indent);
                self.write_expr(out, body, indent);
            }
            CoreExpr::LetRec { var, rhs, body, .. } => {
                write!(out, "letrec {} = ", self.resolve_binder(var)).unwrap();
                self.write_expr_inline(out, rhs, indent);
                out.push('\n');
                push_indent(out, indent);
                self.write_expr(out, body, indent);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                out.push_str("case ");
                self.write_expr_inline(out, scrutinee, indent);
                out.push_str(" of");
                for alt in alts {
                    out.push('\n');
                    self.write_alt(out, alt, indent + 2);
                }
            }
            CoreExpr::Con { tag, fields, .. } => {
                self.write_tag(out, tag);
                if !fields.is_empty() {
                    out.push('(');
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.write_expr_inline(out, f, indent);
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
                    self.write_expr_inline(out, a, indent);
                }
                out.push(')');
            }
            CoreExpr::Return { value, .. } => {
                out.push_str("return ");
                self.write_expr_inline(out, value, indent);
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
                    self.resolve_name(*effect),
                    self.resolve_name(*operation)
                )
                .unwrap();
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.write_expr_inline(out, a, indent);
                }
                out.push(')');
            }
            CoreExpr::Handle {
                body,
                effect,
                handlers,
                ..
            } => {
                write!(out, "handle {} {{", self.resolve_name(*effect)).unwrap();
                for h in handlers {
                    out.push('\n');
                    self.write_handler(out, h, indent + 2);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push_str("} with\n");
                push_indent(out, indent + 2);
                self.write_expr(out, body, indent + 2);
            }
            CoreExpr::Dup { var, body, .. } => {
                write!(out, "dup {}", self.resolve_var(var)).unwrap();
                out.push('\n');
                push_indent(out, indent);
                self.write_expr(out, body, indent);
            }
            CoreExpr::Drop { var, body, .. } => {
                write!(out, "drop {}", self.resolve_var(var)).unwrap();
                out.push('\n');
                push_indent(out, indent);
                self.write_expr(out, body, indent);
            }
            CoreExpr::Reuse {
                token, tag, fields, ..
            } => {
                write!(out, "reuse {} ", self.resolve_var(token)).unwrap();
                self.write_tag(out, tag);
                if !fields.is_empty() {
                    out.push('(');
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.write_expr_inline(out, f, indent);
                    }
                    out.push(')');
                }
            }
        }
    }

    fn write_expr_inline(&mut self, out: &mut String, expr: &CoreExpr, indent: usize) {
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
            self.write_expr(out, expr, indent);
            out.push(')');
        } else {
            self.write_expr(out, expr, indent);
        }
    }

    fn write_alt(&mut self, out: &mut String, alt: &CoreAlt, indent: usize) {
        push_indent(out, indent);
        self.write_pat(out, &alt.pat);
        if let Some(guard) = &alt.guard {
            out.push_str(" if ");
            self.write_expr_inline(out, guard, indent);
        }
        out.push_str(" →\n");
        push_indent(out, indent + 2);
        self.write_expr(out, &alt.rhs, indent + 2);
    }

    fn write_handler(&mut self, out: &mut String, h: &CoreHandler, indent: usize) {
        push_indent(out, indent);
        write!(out, "{}(", self.resolve_name(h.operation)).unwrap();
        for (i, p) in h.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&self.resolve_binder(p));
        }
        writeln!(out, "; {}) →", self.resolve_binder(&h.resume)).unwrap();
        push_indent(out, indent + 2);
        self.write_expr(out, &h.body, indent + 2);
    }

    fn write_pat(&mut self, out: &mut String, pat: &CorePat) {
        match pat {
            CorePat::Wildcard => out.push('_'),
            CorePat::Var(binder) => out.push_str(&self.resolve_binder(binder)),
            CorePat::Lit(lit) => write_lit(out, lit),
            CorePat::EmptyList => out.push_str("[]"),
            CorePat::Con { tag, fields } => {
                self.write_tag(out, tag);
                if !fields.is_empty() {
                    out.push('(');
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        self.write_pat(out, f);
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
                    self.write_pat(out, e);
                }
                out.push(')');
            }
        }
    }

    fn resolve_name(&self, id: Identifier) -> String {
        self.interner
            .try_resolve(id)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("#{}", id.as_u32()))
    }

    fn temp_name(&mut self, binder_id: CoreBinderId) -> usize {
        if let Some(existing) = self.temp_names.get(&binder_id) {
            *existing
        } else {
            self.next_temp += 1;
            self.temp_names.insert(binder_id, self.next_temp);
            self.next_temp
        }
    }

    fn resolve_binder(&mut self, binder: &CoreBinder) -> String {
        let name = self.interner.try_resolve(binder.name);
        match self.mode {
            CoreDisplayMode::Readable => {
                if name.is_some() {
                    self.resolve_name(binder.name)
                } else {
                    format!("%t{}", self.temp_name(binder.id))
                }
            }
            CoreDisplayMode::Debug => match name {
                Some(name) => format!("{name}#{}", binder.id.0),
                None => format!("#{}[synthetic]#{}", binder.name.as_u32(), binder.id.0),
            },
        }
    }

    fn resolve_var(&mut self, var: &CoreVarRef) -> String {
        match self.mode {
            CoreDisplayMode::Readable => match var.binder {
                Some(binder_id) => {
                    if self.interner.try_resolve(var.name).is_some() {
                        self.resolve_name(var.name)
                    } else {
                        format!("%t{}", self.temp_name(binder_id))
                    }
                }
                None => self.resolve_name(var.name),
            },
            CoreDisplayMode::Debug => match var.binder {
                Some(id) => match self.interner.try_resolve(var.name) {
                    Some(name) => format!("{name}#{id}", id = id.0),
                    None => format!("#{}[synthetic]#{}", var.name.as_u32(), id.0),
                },
                None => format!("{}#?[external]", self.resolve_name(var.name)),
            },
        }
    }

    fn write_tag(&self, out: &mut String, tag: &CoreTag) {
        match tag {
            CoreTag::Named(id) => out.push_str(&self.resolve_name(*id)),
            CoreTag::None => out.push_str("None"),
            CoreTag::Some => out.push_str("Some"),
            CoreTag::Left => out.push_str("Left"),
            CoreTag::Right => out.push_str("Right"),
            CoreTag::Nil => out.push_str("[]"),
            CoreTag::Cons => out.push_str("::"),
        }
    }
}

fn is_readable_main_thunk(
    name: Identifier,
    is_recursive: bool,
    expr: &CoreExpr,
    interner: &Interner,
) -> bool {
    interner.try_resolve(name) == Some("main")
        && is_recursive
        && matches!(expr, CoreExpr::Lam { params, .. } if params.is_empty())
}

fn write_lit(out: &mut String, lit: &CoreLit) {
    match lit {
        CoreLit::Int(i) => {
            write!(out, "{i}").unwrap();
        }
        CoreLit::Float(x) => {
            write!(out, "{x}").unwrap();
        }
        CoreLit::Bool(true) => out.push_str("true"),
        CoreLit::Bool(false) => out.push_str("false"),
        CoreLit::String(s) => {
            write!(out, "{s:?}").unwrap();
        }
        CoreLit::Unit => out.push_str("()"),
    }
}

fn write_primop_name(out: &mut String, op: &CorePrimOp) {
    match op {
        CorePrimOp::Add => out.push_str("Add"),
        CorePrimOp::Sub => out.push_str("Sub"),
        CorePrimOp::Mul => out.push_str("Mul"),
        CorePrimOp::Div => out.push_str("Div"),
        CorePrimOp::Mod => out.push_str("Mod"),
        CorePrimOp::IAdd => out.push_str("IAdd"),
        CorePrimOp::ISub => out.push_str("ISub"),
        CorePrimOp::IMul => out.push_str("IMul"),
        CorePrimOp::IDiv => out.push_str("IDiv"),
        CorePrimOp::IMod => out.push_str("IMod"),
        CorePrimOp::FAdd => out.push_str("FAdd"),
        CorePrimOp::FSub => out.push_str("FSub"),
        CorePrimOp::FMul => out.push_str("FMul"),
        CorePrimOp::FDiv => out.push_str("FDiv"),
        CorePrimOp::Neg => out.push_str("Neg"),
        CorePrimOp::Not => out.push_str("Not"),
        CorePrimOp::Eq => out.push_str("Eq"),
        CorePrimOp::NEq => out.push_str("NEq"),
        CorePrimOp::Lt => out.push_str("Lt"),
        CorePrimOp::Le => out.push_str("Le"),
        CorePrimOp::Gt => out.push_str("Gt"),
        CorePrimOp::Ge => out.push_str("Ge"),
        CorePrimOp::And => out.push_str("And"),
        CorePrimOp::Or => out.push_str("Or"),
        CorePrimOp::Concat => out.push_str("Concat"),
        CorePrimOp::Interpolate => out.push_str("Interpolate"),
        CorePrimOp::MakeList => out.push_str("MakeList"),
        CorePrimOp::MakeArray => out.push_str("MakeArray"),
        CorePrimOp::MakeTuple => out.push_str("MakeTuple"),
        CorePrimOp::MakeHash => out.push_str("MakeHash"),
        CorePrimOp::Index => out.push_str("Index"),
        CorePrimOp::MemberAccess(name) => write!(out, "MemberAccess({})", name.as_u32()).unwrap(),
        CorePrimOp::TupleField(index) => write!(out, "TupleField({index})").unwrap(),
    }
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

fn format_core_type(ty: &super::CoreType, interner: &Interner) -> String {
    use super::CoreType;
    match ty {
        CoreType::Int => "Int".to_string(),
        CoreType::Float => "Float".to_string(),
        CoreType::Bool => "Bool".to_string(),
        CoreType::String => "String".to_string(),
        CoreType::Unit => "Unit".to_string(),
        CoreType::Never => "Never".to_string(),
        CoreType::Any => "Any".to_string(),
        CoreType::List(elem) => format!("List<{}>", format_core_type(elem, interner)),
        CoreType::Array(elem) => format!("Array<{}>", format_core_type(elem, interner)),
        CoreType::Option(elem) => format!("Option<{}>", format_core_type(elem, interner)),
        CoreType::Either(l, r) => format!(
            "Either<{}, {}>",
            format_core_type(l, interner),
            format_core_type(r, interner)
        ),
        CoreType::Map(k, v) => format!(
            "Map<{}, {}>",
            format_core_type(k, interner),
            format_core_type(v, interner)
        ),
        CoreType::Tuple(elems) => {
            let parts: Vec<_> = elems
                .iter()
                .map(|e| format_core_type(e, interner))
                .collect();
            format!("({})", parts.join(", "))
        }
        CoreType::Function(params, ret) => {
            let parts: Vec<_> = params
                .iter()
                .map(|p| format_core_type(p, interner))
                .collect();
            format!(
                "({}) -> {}",
                parts.join(", "),
                format_core_type(ret, interner)
            )
        }
        CoreType::Adt(name) => interner
            .try_resolve(*name)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Adt#{}", name.as_u32())),
    }
}
