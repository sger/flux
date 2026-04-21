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
/// Functions NOT promoted here (map, filter, fold, sort, bare list-facing
/// collection helpers, assert_*, etc.) will be rewritten in Flux
/// (`lib/Flow/*.flx`) using explicit builtin names where needed.
use std::collections::HashMap;

use crate::core::{CoreBinderId, CoreExpr, CorePrimOp, CoreProgram, CoreTopLevelItem};
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
        ("upper", 1, CorePrimOp::Upper),
        ("lower", 1, CorePrimOp::Lower),
        ("replace", 3, CorePrimOp::Replace),
        ("chars", 1, CorePrimOp::Chars),
        ("str_contains", 2, CorePrimOp::StrContains),
        // Array memory operations
        ("array_push", 2, CorePrimOp::ArrayPush),
        ("array_concat", 2, CorePrimOp::ArrayConcat),
        ("array_slice", 3, CorePrimOp::ArraySlice),
        // HAMT operations
        ("map_set", 3, CorePrimOp::HamtSet),
        ("map_get", 2, CorePrimOp::HamtGet),
        ("map_has", 2, CorePrimOp::HamtContains),
        ("map_delete", 2, CorePrimOp::HamtDelete),
        ("map_merge", 2, CorePrimOp::HamtMerge),
        ("map_keys", 1, CorePrimOp::HamtKeys),
        ("map_values", 1, CorePrimOp::HamtValues),
        ("map_size", 1, CorePrimOp::HamtSize),
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
        ("sqrt", 1, CorePrimOp::FSqrt),
        ("sin", 1, CorePrimOp::FSin),
        ("fsqrt", 1, CorePrimOp::FSqrt),
        ("fsin", 1, CorePrimOp::FSin),
        ("cos", 1, CorePrimOp::FCos),
        ("fcos", 1, CorePrimOp::FCos),
        ("exp", 1, CorePrimOp::FExp),
        ("fexp", 1, CorePrimOp::FExp),
        ("log", 1, CorePrimOp::FLog),
        ("flog", 1, CorePrimOp::FLog),
        ("floor", 1, CorePrimOp::FFloor),
        ("ffloor", 1, CorePrimOp::FFloor),
        ("ceil", 1, CorePrimOp::FCeil),
        ("fceil", 1, CorePrimOp::FCeil),
        ("round", 1, CorePrimOp::FRound),
        ("fround", 1, CorePrimOp::FRound),
        ("bit_and", 2, CorePrimOp::BitAnd),
        ("bit_or", 2, CorePrimOp::BitOr),
        ("bit_xor", 2, CorePrimOp::BitXor),
        ("bit_shl", 2, CorePrimOp::BitShl),
        ("bit_shr", 2, CorePrimOp::BitShr),
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
        // Collection helpers (C runtime implementations).
        // map/filter/sort/sort_by are NOT promoted — they take closures
        // which the VM dispatch can't call (needs prelude closure path).
        // stdlib-facing `reverse`/`contains` stay ordinary bindings; only the
        // array-specific builtin names promote to primops.
        ("array_reverse", 1, CorePrimOp::ArrayReverse),
        ("array_contains", 2, CorePrimOp::ArrayContains),
    ];
    entries.iter().map(|&(n, a, op)| ((n, a), op)).collect()
}

/// Run the primop promotion pass on a `CoreProgram`.
pub fn promote_builtins(program: &mut CoreProgram, interner: &Interner) {
    let table = builtin_primop_table();
    // Build a map from binder ID → def arity so we can detect arity
    // mismatches (e.g., call to `concat(a, b)` with binder pointing to
    // `Flow.List.concat` which has arity 1). In such cases the call can't
    // be to that def, so primop promotion is safe.
    let def_arities: HashMap<crate::core::CoreBinderId, usize> = program
        .defs
        .iter()
        .filter_map(|def| {
            if let CoreExpr::Lam { params, .. } = &def.expr {
                Some((def.binder.id, params.len()))
            } else {
                None
            }
        })
        .collect();
    let binder_qualified_names = build_qualified_names(program, interner);
    let sentinel = CoreExpr::Lit(crate::core::CoreLit::Unit, Default::default());
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        def.expr = promote_expr(e, &table, interner, &def_arities, &binder_qualified_names);
    }
}

fn collect_module_paths(
    item: &CoreTopLevelItem,
    prefix: &[String],
    out: &mut Vec<(crate::syntax::Identifier, String)>,
    interner: &Interner,
) {
    match item {
        CoreTopLevelItem::Let { .. } => {}
        CoreTopLevelItem::Function { name, .. } => {
            let func_name = interner.resolve(*name).to_string();
            let qualified = if prefix.is_empty() {
                func_name
            } else {
                let mut parts = prefix.to_vec();
                parts.push(func_name);
                parts.join("_")
            };
            out.push((*name, qualified.replace('.', "_")));
        }
        CoreTopLevelItem::Module { name, body, .. } => {
            let mut new_prefix = prefix.to_vec();
            new_prefix.push(interner.resolve(*name).replace('.', "_"));
            for child in body {
                collect_module_paths(child, &new_prefix, out, interner);
            }
        }
        _ => {}
    }
}

fn build_qualified_names(
    program: &CoreProgram,
    interner: &Interner,
) -> HashMap<CoreBinderId, String> {
    let mut name_qualified_pairs = Vec::new();
    for item in &program.top_level_items {
        collect_module_paths(item, &[], &mut name_qualified_pairs, interner);
    }

    let mut result = HashMap::new();
    let mut claimed = std::collections::HashSet::new();
    for (bare_name, qualified_name) in name_qualified_pairs {
        if let Some(def) = program
            .defs
            .iter()
            .find(|def| def.name == bare_name && !claimed.contains(&def.binder.id))
        {
            claimed.insert(def.binder.id);
            result.insert(def.binder.id, qualified_name);
        }
    }
    result
}

fn is_conflicting_prelude_binding(
    _name: &str,
    _arity: usize,
    _qualified_name: Option<&str>,
) -> bool {
    false
}

/// Recursively walk a `CoreExpr`, promoting eligible `App` nodes.
fn promote_expr(
    expr: CoreExpr,
    table: &HashMap<(&str, usize), CorePrimOp>,
    interner: &Interner,
    def_arities: &HashMap<crate::core::CoreBinderId, usize>,
    binder_qualified_names: &HashMap<CoreBinderId, String>,
) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            if let CoreExpr::Var { var, .. } = func.as_ref() {
                let name_str = interner.try_resolve(var.name);
                if name_str == Some("list") && var.binder.is_none() {
                    let promoted_args: Vec<CoreExpr> = args
                        .into_iter()
                        .map(|a| {
                            promote_expr(a, table, interner, def_arities, binder_qualified_names)
                        })
                        .collect();
                    return CoreExpr::PrimOp {
                        op: CorePrimOp::MakeList,
                        args: promoted_args,
                        span,
                    };
                }
                let name_matches_primop = name_str
                    .and_then(|resolved| table.get(&(resolved, args.len())))
                    .is_some();
                // Promote if:
                // 1. No binder (truly unbound — classic case), OR
                // 2. Binder exists but points to a top-level def with a
                //    *different* arity than the call site. This happens in
                //    merged programs (--native) where `except` prevents
                //    unqualified access but binder resolution still assigns
                //    the module function's binder. E.g., `concat(a, b)`
                //    with arity 2 gets binder for `Flow.List.concat` (arity 1).
                // 3. Binder exists but is a known conflicting prelude symbol.
                //    `delete/2` is the current same-arity case: merged native
                //    programs may bind bare `delete` to `Flow.List.delete`,
                //    but the surface builtin should still promote to HamtDelete.
                let is_promotable = name_matches_primop
                    && (var.binder.is_none()
                        || var.binder.is_some_and(|bid| {
                            def_arities
                                .get(&bid)
                                .is_some_and(|&def_arity| def_arity != args.len())
                                || is_conflicting_prelude_binding(
                                    name_str.unwrap_or_default(),
                                    args.len(),
                                    binder_qualified_names.get(&bid).map(String::as_str),
                                )
                        }));
                if is_promotable {
                    let name_str = interner.resolve(var.name);
                    let op = table[&(name_str, args.len())];
                    let promoted_args: Vec<CoreExpr> = args
                        .into_iter()
                        .map(|a| {
                            promote_expr(a, table, interner, def_arities, binder_qualified_names)
                        })
                        .collect();
                    return CoreExpr::PrimOp {
                        op,
                        args: promoted_args,
                        span,
                    };
                }
            }
            CoreExpr::App {
                func: Box::new(promote_expr(
                    *func,
                    table,
                    interner,
                    def_arities,
                    binder_qualified_names,
                )),
                args: args
                    .into_iter()
                    .map(|a| promote_expr(a, table, interner, def_arities, binder_qualified_names))
                    .collect(),
                span,
            }
        }
        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body: Box::new(promote_expr(
                *body,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            span,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(promote_expr(
                *rhs,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            body: Box::new(promote_expr(
                *body,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(promote_expr(
                *rhs,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            body: Box::new(promote_expr(
                *body,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            span,
        },
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(b, rhs)| {
                    (
                        b,
                        Box::new(promote_expr(
                            *rhs,
                            table,
                            interner,
                            def_arities,
                            binder_qualified_names,
                        )),
                    )
                })
                .collect(),
            body: Box::new(promote_expr(
                *body,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(promote_expr(
                *scrutinee,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            alts: alts
                .into_iter()
                .map(|alt| crate::core::CoreAlt {
                    rhs: promote_expr(
                        alt.rhs,
                        table,
                        interner,
                        def_arities,
                        binder_qualified_names,
                    ),
                    guard: alt.guard.map(|g| {
                        promote_expr(g, table, interner, def_arities, binder_qualified_names)
                    }),
                    ..alt
                })
                .collect(),
            join_ty,
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| promote_expr(f, table, interner, def_arities, binder_qualified_names))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| promote_expr(a, table, interner, def_arities, binder_qualified_names))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(promote_expr(
                *value,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
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
                .map(|a| promote_expr(a, table, interner, def_arities, binder_qualified_names))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(promote_expr(
                *body,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            effect,
            handlers: handlers
                .into_iter()
                .map(|h| crate::core::CoreHandler {
                    body: promote_expr(
                        h.body,
                        table,
                        interner,
                        def_arities,
                        binder_qualified_names,
                    ),
                    ..h
                })
                .collect(),
            span,
        },
        // Leaf nodes
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(promote_expr(
                *object,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(promote_expr(
                *object,
                table,
                interner,
                def_arities,
                binder_qualified_names,
            )),
            index,
            span,
        },
    }
}
