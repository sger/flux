/// Primop promotion pass — rewrites `App(Var { binder: None, name }, args)`
/// into `PrimOp(SpecificVariant, args)` for known primitive function names.
///
/// Runs after binder resolution so that `binder: None` reliably identifies
/// names that were never bound to a local/global definition (i.e. true
/// primitives that need hardware access, memory layout knowledge, or OS
/// syscalls).
///
/// Only direct calls with matching arity are promoted; higher-order usage
/// (`let f = println; f(x)`) falls through to the existing `App(Var …)` path.
///
/// Functions NOT promoted here (map, filter, fold, sort, reverse,
/// assert_*, etc.) will be rewritten in Flux (`lib/Flow/*.flx`) using
/// these primops.
use std::collections::HashMap;

use crate::core::{CoreExpr, CorePrimOp, CoreProgram};
use crate::syntax::interner::Interner;

/// Map from (function name, arity) → CorePrimOp variant.
///
/// Only true primitives are listed.  Higher-order functions and algorithms
/// that can be written in Flux are intentionally excluded.
fn builtin_primop_table() -> HashMap<(&'static str, usize), CorePrimOp> {
    let entries: &[(&str, usize, CorePrimOp)] = &[
        // I/O
        ("print", 1, CorePrimOp::Print),
        ("println", 1, CorePrimOp::Println),
        ("read_file", 1, CorePrimOp::ReadFile),
        ("write_file", 2, CorePrimOp::WriteFile),
        ("read_stdin", 0, CorePrimOp::ReadStdin),
        ("read_lines", 1, CorePrimOp::ReadLines),
        // String memory operations
        ("to_string", 1, CorePrimOp::ToString),
        ("split", 2, CorePrimOp::Split),
        ("join", 2, CorePrimOp::Join),
        ("trim", 1, CorePrimOp::Trim),
        ("starts_with", 2, CorePrimOp::StartsWith),
        ("ends_with", 2, CorePrimOp::EndsWith),
        ("substring", 3, CorePrimOp::Substring),
        // TODO: promote when C runtime implements these:
        // ("upper", 1, CorePrimOp::Upper),
        // ("lower", 1, CorePrimOp::Lower),
        // ("replace", 3, CorePrimOp::Replace),
        // ("chars", 1, CorePrimOp::Chars),
        // ("str_contains", 2, CorePrimOp::StrContains),
        // Array memory operations
        ("push", 2, CorePrimOp::ArrayPush),
        ("concat", 2, CorePrimOp::ArrayConcat),
        ("slice", 3, CorePrimOp::ArraySlice),
        // HAMT operations
        ("put", 3, CorePrimOp::HamtSet),
        ("get", 2, CorePrimOp::HamtGet),
        ("has_key", 2, CorePrimOp::HamtContains),
        ("delete", 2, CorePrimOp::HamtDelete),
        ("merge", 2, CorePrimOp::HamtMerge),
        ("keys", 1, CorePrimOp::HamtKeys),
        ("values", 1, CorePrimOp::HamtValues),
        ("size", 1, CorePrimOp::HamtSize),
        // Type tag inspection
        ("type_of", 1, CorePrimOp::TypeOf),
        ("is_int", 1, CorePrimOp::IsInt),
        ("is_float", 1, CorePrimOp::IsFloat),
        ("is_string", 1, CorePrimOp::IsString),
        ("is_bool", 1, CorePrimOp::IsBool),
        ("is_array", 1, CorePrimOp::IsArray),
        ("is_none", 1, CorePrimOp::IsNone),
        ("is_some", 1, CorePrimOp::IsSome),
        ("is_list", 1, CorePrimOp::IsList),
        ("is_map", 1, CorePrimOp::IsMap),
        ("is_hash", 1, CorePrimOp::IsMap),
        // Deep structural comparison
        ("cmp_eq", 2, CorePrimOp::CmpEq),
        ("cmp_ne", 2, CorePrimOp::CmpNe),
        // Control
        ("panic", 1, CorePrimOp::Panic),
        ("now_ms", 0, CorePrimOp::ClockNow),
        ("try", 1, CorePrimOp::Try),
        ("assert_throws", 1, CorePrimOp::AssertThrows),
        ("assert_throws", 2, CorePrimOp::AssertThrows),
        // Math
        ("abs", 1, CorePrimOp::Abs),
        ("min", 2, CorePrimOp::Min),
        ("max", 2, CorePrimOp::Max),
        // Time
        ("time", 0, CorePrimOp::Time),
        // Parsing
        ("parse_int", 1, CorePrimOp::ParseInt),
        ("parse_ints", 1, CorePrimOp::ParseInts),
        ("split_ints", 2, CorePrimOp::SplitInts),
        // List / cons cell
        ("to_list", 1, CorePrimOp::ToList),
        ("to_array", 1, CorePrimOp::ToArray),
        // Polymorphic length
        ("len", 1, CorePrimOp::Len),
    ];
    entries
        .iter()
        .map(|&(n, a, op)| ((n, a), op))
        .collect()
}

/// Run the primop promotion pass on a `CoreProgram`.
pub fn promote_builtins(program: &mut CoreProgram, interner: &Interner) {
    let table = builtin_primop_table();
    let sentinel = CoreExpr::Lit(crate::core::CoreLit::Unit, Default::default());
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        def.expr = promote_expr(e, &table, interner);
    }
}

/// Recursively walk a `CoreExpr`, promoting eligible `App` nodes.
fn promote_expr(
    expr: CoreExpr,
    table: &HashMap<(&str, usize), CorePrimOp>,
    interner: &Interner,
) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            if let CoreExpr::Var { var, .. } = func.as_ref() {
                let is_promotable = var.binder.is_none()
                    && interner
                        .try_resolve(var.name)
                        .and_then(|name_str| table.get(&(name_str, args.len())))
                        .is_some();
                if is_promotable {
                    let name_str = interner.resolve(var.name);
                    let op = table[&(name_str, args.len())];
                    let promoted_args: Vec<CoreExpr> = args
                        .into_iter()
                        .map(|a| promote_expr(a, table, interner))
                        .collect();
                    return CoreExpr::PrimOp {
                        op,
                        args: promoted_args,
                        span,
                    };
                }
            }
            CoreExpr::App {
                func: Box::new(promote_expr(*func, table, interner)),
                args: args
                    .into_iter()
                    .map(|a| promote_expr(a, table, interner))
                    .collect(),
                span,
            }
        }
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(promote_expr(*body, table, interner)),
            span,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(promote_expr(*rhs, table, interner)),
            body: Box::new(promote_expr(*body, table, interner)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(promote_expr(*rhs, table, interner)),
            body: Box::new(promote_expr(*body, table, interner)),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(promote_expr(*scrutinee, table, interner)),
            alts: alts
                .into_iter()
                .map(|alt| crate::core::CoreAlt {
                    rhs: promote_expr(alt.rhs, table, interner),
                    guard: alt.guard.map(|g| promote_expr(g, table, interner)),
                    ..alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| promote_expr(f, table, interner))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| promote_expr(a, table, interner))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(promote_expr(*value, table, interner)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args
                .into_iter()
                .map(|a| promote_expr(a, table, interner))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(promote_expr(*body, table, interner)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|h| crate::core::CoreHandler {
                    body: promote_expr(h.body, table, interner),
                    ..h
                })
                .collect(),
            span,
        },
        // Leaf nodes
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
        // Aether nodes (should not appear before promotion, but handle for safety)
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(promote_expr(*func, table, interner)),
            args: args
                .into_iter()
                .map(|a| promote_expr(a, table, interner))
                .collect(),
            arg_modes,
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(promote_expr(*body, table, interner)),
            span,
        },
        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(promote_expr(*body, table, interner)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields
                .into_iter()
                .map(|f| promote_expr(f, table, interner))
                .collect(),
            field_mask,
            span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(promote_expr(*unique_body, table, interner)),
            shared_body: Box::new(promote_expr(*shared_body, table, interner)),
            span,
        },
        CoreExpr::MemberAccess { object, member, span } => CoreExpr::MemberAccess {
            object: Box::new(promote_expr(*object, table, interner)),
            member,
            span,
        },
        CoreExpr::TupleField { object, index, span } => CoreExpr::TupleField {
            object: Box::new(promote_expr(*object, table, interner)),
            index,
            span,
        },
    }
}
