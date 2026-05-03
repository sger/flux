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
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                write!(out, "letrec {{").unwrap();
                for (var, rhs) in bindings {
                    out.push('\n');
                    push_indent(out, indent + 1);
                    write!(out, "{} = ", self.resolve_binder(var)).unwrap();
                    self.write_expr_inline(out, rhs, indent + 1);
                    out.push(';');
                }
                out.push('\n');
                push_indent(out, indent);
                out.push_str("} in");
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
                write_primop_name(out, op, self.interner);
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
                parameter,
                handlers,
                ..
            } => {
                write!(out, "handle {}", self.resolve_name(*effect)).unwrap();
                if let Some(parameter) = parameter {
                    out.push('(');
                    self.write_expr_inline(out, parameter, indent);
                    out.push(')');
                }
                out.push_str(" {");
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
            CoreExpr::MemberAccess { object, member, .. } => {
                self.write_expr_inline(out, object, indent);
                out.push('.');
                out.push_str(&self.resolve_name(*member));
            }
            CoreExpr::TupleField { object, index, .. } => {
                self.write_expr_inline(out, object, indent);
                write!(out, ".{index}").unwrap();
            }
        }
    }

    fn write_expr_inline(&mut self, out: &mut String, expr: &CoreExpr, indent: usize) {
        let needs_parens = matches!(
            expr,
            CoreExpr::Lam { .. }
                | CoreExpr::Let { .. }
                | CoreExpr::LetRec { .. }
                | CoreExpr::LetRecGroup { .. }
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
        if let Some(state) = &h.state {
            if !h.params.is_empty() {
                out.push_str(", ");
            }
            out.push_str(&self.resolve_binder(state));
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
            CoreDisplayMode::Debug => {
                let rep = format_rep(binder.rep);
                match name {
                    Some(name) => format!("{name}#{}{rep}", binder.id.0),
                    None => format!("%t{}{rep}", self.temp_name(binder.id)),
                }
            }
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

fn write_primop_name(out: &mut String, op: &CorePrimOp, _interner: &Interner) {
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
        CorePrimOp::Abs => out.push_str("Abs"),
        CorePrimOp::FSqrt => out.push_str("FSqrt"),
        CorePrimOp::FSin => out.push_str("FSin"),
        CorePrimOp::FCos => out.push_str("FCos"),
        CorePrimOp::FExp => out.push_str("FExp"),
        CorePrimOp::FLog => out.push_str("FLog"),
        CorePrimOp::FFloor => out.push_str("FFloor"),
        CorePrimOp::FCeil => out.push_str("FCeil"),
        CorePrimOp::FRound => out.push_str("FRound"),
        CorePrimOp::FTan => out.push_str("FTan"),
        CorePrimOp::FAsin => out.push_str("FAsin"),
        CorePrimOp::FAcos => out.push_str("FAcos"),
        CorePrimOp::FAtan => out.push_str("FAtan"),
        CorePrimOp::FSinh => out.push_str("FSinh"),
        CorePrimOp::FCosh => out.push_str("FCosh"),
        CorePrimOp::FTanh => out.push_str("FTanh"),
        CorePrimOp::FTruncate => out.push_str("FTruncate"),
        CorePrimOp::BitAnd => out.push_str("BitAnd"),
        CorePrimOp::BitOr => out.push_str("BitOr"),
        CorePrimOp::BitXor => out.push_str("BitXor"),
        CorePrimOp::BitShl => out.push_str("BitShl"),
        CorePrimOp::BitShr => out.push_str("BitShr"),
        CorePrimOp::Min => out.push_str("Min"),
        CorePrimOp::Max => out.push_str("Max"),
        CorePrimOp::Neg => out.push_str("Neg"),
        CorePrimOp::Not => out.push_str("Not"),
        CorePrimOp::Eq => out.push_str("Eq"),
        CorePrimOp::NEq => out.push_str("NEq"),
        CorePrimOp::Lt => out.push_str("Lt"),
        CorePrimOp::Le => out.push_str("Le"),
        CorePrimOp::Gt => out.push_str("Gt"),
        CorePrimOp::Ge => out.push_str("Ge"),
        CorePrimOp::ICmpEq => out.push_str("ICmpEq"),
        CorePrimOp::ICmpNe => out.push_str("ICmpNe"),
        CorePrimOp::ICmpLt => out.push_str("ICmpLt"),
        CorePrimOp::ICmpLe => out.push_str("ICmpLe"),
        CorePrimOp::ICmpGt => out.push_str("ICmpGt"),
        CorePrimOp::ICmpGe => out.push_str("ICmpGe"),
        CorePrimOp::FCmpEq => out.push_str("FCmpEq"),
        CorePrimOp::FCmpNe => out.push_str("FCmpNe"),
        CorePrimOp::FCmpLt => out.push_str("FCmpLt"),
        CorePrimOp::FCmpLe => out.push_str("FCmpLe"),
        CorePrimOp::FCmpGt => out.push_str("FCmpGt"),
        CorePrimOp::FCmpGe => out.push_str("FCmpGe"),
        CorePrimOp::And => out.push_str("And"),
        CorePrimOp::Or => out.push_str("Or"),
        CorePrimOp::Concat => out.push_str("Concat"),
        CorePrimOp::Interpolate => out.push_str("Interpolate"),
        CorePrimOp::MakeList => out.push_str("MakeList"),
        CorePrimOp::MakeArray => out.push_str("MakeArray"),
        CorePrimOp::MakeTuple => out.push_str("MakeTuple"),
        CorePrimOp::MakeHash => out.push_str("MakeHash"),
        CorePrimOp::Index => out.push_str("Index"),
        // Promoted primops (Proposal 0120)
        CorePrimOp::Print => out.push_str("Print"),
        CorePrimOp::Println => out.push_str("Println"),
        CorePrimOp::DebugTrace => out.push_str("DebugTrace"),
        CorePrimOp::ReadFile => out.push_str("ReadFile"),
        CorePrimOp::WriteFile => out.push_str("WriteFile"),
        CorePrimOp::ReadStdin => out.push_str("ReadStdin"),
        CorePrimOp::ReadLines => out.push_str("ReadLines"),
        CorePrimOp::StringLength => out.push_str("StringLength"),
        CorePrimOp::StringConcat => out.push_str("StringConcat"),
        CorePrimOp::StringSlice => out.push_str("StringSlice"),
        CorePrimOp::ToString => out.push_str("ToString"),
        CorePrimOp::Split => out.push_str("Split"),
        CorePrimOp::Trim => out.push_str("Trim"),
        CorePrimOp::Upper => out.push_str("Upper"),
        CorePrimOp::Lower => out.push_str("Lower"),
        CorePrimOp::Replace => out.push_str("Replace"),
        CorePrimOp::Substring => out.push_str("Substring"),
        CorePrimOp::ArrayLen => out.push_str("ArrayLen"),
        CorePrimOp::ArrayGet => out.push_str("ArrayGet"),
        CorePrimOp::ArraySet => out.push_str("ArraySet"),
        CorePrimOp::ArrayPush => out.push_str("ArrayPush"),
        CorePrimOp::ArrayConcat => out.push_str("ArrayConcat"),
        CorePrimOp::ArraySlice => out.push_str("ArraySlice"),
        CorePrimOp::HamtGet => out.push_str("HamtGet"),
        CorePrimOp::HamtSet => out.push_str("HamtSet"),
        CorePrimOp::HamtDelete => out.push_str("HamtDelete"),
        CorePrimOp::HamtKeys => out.push_str("HamtKeys"),
        CorePrimOp::HamtValues => out.push_str("HamtValues"),
        CorePrimOp::HamtMerge => out.push_str("HamtMerge"),
        CorePrimOp::HamtSize => out.push_str("HamtSize"),
        CorePrimOp::HamtContains => out.push_str("HamtContains"),
        CorePrimOp::TypeOf => out.push_str("TypeOf"),
        CorePrimOp::IsInt => out.push_str("IsInt"),
        CorePrimOp::IsFloat => out.push_str("IsFloat"),
        CorePrimOp::IsString => out.push_str("IsString"),
        CorePrimOp::IsBool => out.push_str("IsBool"),
        CorePrimOp::IsArray => out.push_str("IsArray"),
        CorePrimOp::IsNone => out.push_str("IsNone"),
        CorePrimOp::IsSome => out.push_str("IsSome"),
        CorePrimOp::IsList => out.push_str("IsList"),
        CorePrimOp::IsMap => out.push_str("IsMap"),
        CorePrimOp::Panic => out.push_str("Panic"),
        CorePrimOp::ClockNow => out.push_str("ClockNow"),
        CorePrimOp::Time => out.push_str("Time"),
        CorePrimOp::ParseInt => out.push_str("ParseInt"),
        CorePrimOp::Len => out.push_str("Len"),
        CorePrimOp::CmpEq => out.push_str("CmpEq"),
        CorePrimOp::CmpNe => out.push_str("CmpNe"),
        CorePrimOp::Try => out.push_str("Try"),
        CorePrimOp::AssertThrows => out.push_str("AssertThrows"),
        // Effect handlers (Koka-style yield model)
        CorePrimOp::EvvGet => out.push_str("EvvGet"),
        CorePrimOp::EvvSet => out.push_str("EvvSet"),
        CorePrimOp::FreshMarker => out.push_str("FreshMarker"),
        CorePrimOp::EvvInsert => out.push_str("EvvInsert"),
        CorePrimOp::YieldTo => out.push_str("YieldTo"),
        CorePrimOp::YieldExtend => out.push_str("YieldExtend"),
        CorePrimOp::YieldPrompt => out.push_str("YieldPrompt"),
        CorePrimOp::IsYielding => out.push_str("IsYielding"),
        CorePrimOp::PerformDirect => out.push_str("PerformDirect"),
        CorePrimOp::Unwrap => out.push_str("Unwrap"),
        CorePrimOp::SafeDiv => out.push_str("SafeDiv"),
        CorePrimOp::SafeMod => out.push_str("SafeMod"),
        // Concurrency (proposal 0174 D5-a)
        CorePrimOp::TaskSpawn => out.push_str("TaskSpawn"),
        CorePrimOp::TaskBlockingJoin => out.push_str("TaskBlockingJoin"),
        CorePrimOp::TaskCancel => out.push_str("TaskCancel"),
    }
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

fn format_rep(rep: super::FluxRep) -> &'static str {
    use super::FluxRep;
    match rep {
        FluxRep::IntRep => ":Int",
        FluxRep::FloatRep => ":Float",
        FluxRep::BoolRep => ":Bool",
        FluxRep::BoxedRep => ":Box",
        FluxRep::TaggedRep => "", // default — don't clutter output
        FluxRep::UnitRep => ":Unit",
    }
}

fn alpha_name(index: usize) -> String {
    let letter = ((index % 26) as u8 + b'a') as char;
    let suffix = index / 26;
    if suffix == 0 {
        letter.to_string()
    } else {
        format!("{letter}{suffix}")
    }
}

fn collect_core_bound_var_order(
    ty: &super::CoreType,
    bound: &[crate::types::TypeVarId],
    order: &mut Vec<crate::types::TypeVarId>,
) {
    use super::CoreType;
    match ty {
        CoreType::Var(var) => {
            if bound.contains(var) && !order.contains(var) {
                order.push(*var);
            }
        }
        CoreType::Forall(_, body) => collect_core_bound_var_order(body, bound, order),
        CoreType::List(elem) | CoreType::Array(elem) | CoreType::Option(elem) => {
            collect_core_bound_var_order(elem, bound, order)
        }
        CoreType::Either(left, right) | CoreType::Map(left, right) => {
            collect_core_bound_var_order(left, bound, order);
            collect_core_bound_var_order(right, bound, order);
        }
        CoreType::Tuple(elems) => {
            for elem in elems {
                collect_core_bound_var_order(elem, bound, order);
            }
        }
        CoreType::Function(params, ret) => {
            for param in params {
                collect_core_bound_var_order(param, bound, order);
            }
            collect_core_bound_var_order(ret, bound, order);
        }
        CoreType::Adt(_, args) => {
            for arg in args {
                collect_core_bound_var_order(arg, bound, order);
            }
        }
        CoreType::Abstract(super::CoreAbstractType::Named(_, args)) => {
            for arg in args {
                collect_core_bound_var_order(arg, bound, order);
            }
        }
        CoreType::Int
        | CoreType::Float
        | CoreType::Bool
        | CoreType::String
        | CoreType::Unit
        | CoreType::Never
        | CoreType::Abstract(_) => {}
    }
}

fn format_core_type(ty: &super::CoreType, interner: &Interner) -> String {
    use super::{CoreAbstractType, CoreType};
    use std::collections::HashMap;
    fn go(
        ty: &CoreType,
        interner: &Interner,
        names: &mut HashMap<crate::types::TypeVarId, String>,
        next: &mut usize,
    ) -> String {
        match ty {
            CoreType::Int => "Int".to_string(),
            CoreType::Float => "Float".to_string(),
            CoreType::Bool => "Bool".to_string(),
            CoreType::String => "String".to_string(),
            CoreType::Unit => "Unit".to_string(),
            CoreType::Never => "Never".to_string(),
            CoreType::Var(var) => names
                .entry(*var)
                .or_insert_with(|| {
                    let name = alpha_name(*next);
                    *next += 1;
                    name
                })
                .clone(),
            CoreType::Forall(vars, body) => {
                let mut ordered = Vec::new();
                collect_core_bound_var_order(body, vars, &mut ordered);
                for var in vars {
                    if !ordered.contains(var) {
                        ordered.push(*var);
                    }
                }
                let mut inserted = Vec::new();
                for var in &ordered {
                    if !names.contains_key(var) {
                        let name = alpha_name(*next);
                        *next += 1;
                        names.insert(*var, name);
                        inserted.push(*var);
                    }
                }
                let vars = ordered
                    .iter()
                    .map(|var| names.get(var).cloned().unwrap())
                    .collect::<Vec<_>>();
                let rendered = format!(
                    "forall {}. {}",
                    vars.join(", "),
                    go(body, interner, names, next)
                );
                for var in inserted {
                    names.remove(&var);
                }
                rendered
            }
            CoreType::Abstract(CoreAbstractType::ConstructorHead(tc)) => format!("{tc:?}"),
            CoreType::Abstract(CoreAbstractType::HigherKindedApp) => "Abstract<HKT>".to_string(),
            CoreType::Abstract(CoreAbstractType::UnsupportedApp(tc)) => {
                format!("Abstract<{tc:?}>")
            }
            CoreType::Abstract(CoreAbstractType::Named(name, args)) => {
                let name = interner
                    .try_resolve(*name)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Abstract#{}", name.as_u32()));
                if args.is_empty() {
                    name
                } else {
                    let rendered = args
                        .iter()
                        .map(|arg| go(arg, interner, names, next))
                        .collect::<Vec<_>>();
                    format!("{name}<{}>", rendered.join(", "))
                }
            }
            CoreType::List(elem) => format!("List<{}>", go(elem, interner, names, next)),
            CoreType::Array(elem) => format!("Array<{}>", go(elem, interner, names, next)),
            CoreType::Option(elem) => format!("Option<{}>", go(elem, interner, names, next)),
            CoreType::Either(l, r) => format!(
                "Either<{}, {}>",
                go(l, interner, names, next),
                go(r, interner, names, next)
            ),
            CoreType::Map(k, v) => format!(
                "Map<{}, {}>",
                go(k, interner, names, next),
                go(v, interner, names, next)
            ),
            CoreType::Tuple(elems) => {
                let parts: Vec<_> = elems.iter().map(|e| go(e, interner, names, next)).collect();
                format!("({})", parts.join(", "))
            }
            CoreType::Function(params, ret) => {
                let parts: Vec<_> = params
                    .iter()
                    .map(|p| go(p, interner, names, next))
                    .collect();
                format!(
                    "({}) -> {}",
                    parts.join(", "),
                    go(ret, interner, names, next)
                )
            }
            CoreType::Adt(name, args) => {
                let name = interner
                    .try_resolve(*name)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Adt#{}", name.as_u32()));
                if args.is_empty() {
                    name
                } else {
                    let rendered = args
                        .iter()
                        .map(|arg| go(arg, interner, names, next))
                        .collect::<Vec<_>>();
                    format!("{name}<{}>", rendered.join(", "))
                }
            }
        }
    }

    let mut names = HashMap::new();
    let mut next = 0;
    go(ty, interner, &mut names, &mut next)
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{CoreAbstractType, CoreType},
        syntax::interner::Interner,
    };

    use super::format_core_type;

    #[test]
    fn format_core_type_renders_explicit_semantic_forms() {
        let mut interner = Interner::new();
        let result = interner.intern("Result");
        let elem = interner.intern("a");

        let ty = CoreType::Forall(
            vec![3],
            Box::new(CoreType::Function(
                vec![
                    CoreType::Adt(result, vec![CoreType::Var(3), CoreType::Int]),
                    CoreType::Abstract(CoreAbstractType::Named(elem, vec![])),
                ],
                Box::new(CoreType::Var(3)),
            )),
        );

        assert_eq!(
            format_core_type(&ty, &interner),
            "forall a. (Result<a, Int>, a) -> a"
        );
    }
}
