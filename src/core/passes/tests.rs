use super::*;
use crate::{
    core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat,
        CorePrimOp, CoreProgram, CoreTag, CoreTopLevelItem, CoreVarRef,
    },
    diagnostics::position::Span,
    syntax::interner::Interner,
};

fn s() -> Span {
    Span::default()
}

fn binder(raw: u32, name: crate::syntax::Identifier) -> CoreBinder {
    CoreBinder::new(CoreBinderId(raw), name)
}

fn var_ref(binder: CoreBinder) -> CoreExpr {
    CoreExpr::bound_var(binder, s())
}

// ── case_of_known_constructor ─────────────────────────────────────────────

#[test]
fn cokc_reduces_some_constructor() {
    // Case(Con(Some, [Lit(42)]), [Con(Some, [Var(x)]) → Var(x), Wildcard → Lit(0)])
    //   → Lit(42)
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Case {
        scrutinee: Box::new(CoreExpr::Con {
            tag: CoreTag::Some,
            fields: vec![CoreExpr::Lit(CoreLit::Int(42), s())],
            span: s(),
        }),
        alts: vec![
            CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![CorePat::Var(x_binder)],
                },
                guard: None,
                rhs: var_ref(x_binder),
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                span: s(),
            },
        ],
        span: s(),
    };

    let result = case_of_known_constructor(expr);
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(42), _)),
        "expected Lit(42), got {result:?}"
    );
}

#[test]
fn cokc_reduces_bool_literal() {
    // Case(Lit(true), [Lit(true) → Lit(1), Wildcard → Lit(0)])  →  Lit(1)
    let expr = CoreExpr::Case {
        scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), s())),
        alts: vec![
            CoreAlt {
                pat: CorePat::Lit(CoreLit::Bool(true)),
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(1), s()),
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                span: s(),
            },
        ],
        span: s(),
    };

    let result = case_of_known_constructor(expr);
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(1), _)),
        "expected Lit(1), got {result:?}"
    );
}

#[test]
fn cokc_skips_guarded_arm() {
    // A guarded arm must not be selected even if the tag matches.
    // The wildcard fallthrough should be chosen instead.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let guard = CoreExpr::Lit(CoreLit::Bool(false), s()); // always-false guard
    let expr = CoreExpr::Case {
        scrutinee: Box::new(CoreExpr::Con {
            tag: CoreTag::Some,
            fields: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
            span: s(),
        }),
        alts: vec![
            CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![CorePat::Var(x_binder)],
                },
                guard: Some(guard),
                rhs: CoreExpr::Lit(CoreLit::Int(99), s()),
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                span: s(),
            },
        ],
        span: s(),
    };

    let result = case_of_known_constructor(expr);
    // Guarded arm is skipped → wildcard arm is selected → Lit(0)
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(0), _)),
        "expected Lit(0) (guarded arm skipped), got {result:?}"
    );
}

#[test]
fn cokc_leaves_unknown_scrutinee_alone() {
    // Case(Var(x), [...]) — scrutinee not statically known; must not be reduced.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Case {
        scrutinee: Box::new(var_ref(x_binder)),
        alts: vec![CoreAlt {
            pat: CorePat::Wildcard,
            guard: None,
            rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
            span: s(),
        }],
        span: s(),
    };

    let result = case_of_known_constructor(expr);
    assert!(
        matches!(result, CoreExpr::Case { .. }),
        "should remain a Case"
    );
}

// ── inline_trivial_lets ───────────────────────────────────────────────────

#[test]
fn inline_trivial_substitutes_literal() {
    // let x = 5; x  →  5
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(5), s())),
        body: Box::new(var_ref(x_binder)),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(5), _)),
        "expected Lit(5), got {result:?}"
    );
}

#[test]
fn inline_trivial_copy_propagation() {
    // let x = y; x  →  y
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let y = interner.intern("y");
    let x_binder = binder(0, x);
    let y_binder = binder(1, y);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(var_ref(y_binder)),
        body: Box::new(var_ref(x_binder)),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    assert!(
        matches!(result, CoreExpr::Var { var, .. } if var.binder == Some(y_binder.id)),
        "expected Var(y), got {result:?}"
    );
}

#[test]
fn inline_trivial_multiple_uses() {
    // let x = 3; PrimOp(Add, [x, x])  →  PrimOp(Add, [3, 3])
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(3), s())),
        body: Box::new(CoreExpr::PrimOp {
            op: CorePrimOp::IAdd,
            args: vec![var_ref(x_binder), var_ref(x_binder)],
            span: s(),
        }),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    match result {
        CoreExpr::PrimOp { args, .. } => {
            assert!(matches!(args[0], CoreExpr::Lit(CoreLit::Int(3), _)));
            assert!(matches!(args[1], CoreExpr::Lit(CoreLit::Int(3), _)));
        }
        other => panic!("expected PrimOp, got {other:?}"),
    }
}

#[test]
fn run_core_passes_reports_aether_contract_stage() {
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let malformed = CoreExpr::Drop {
        var: crate::core::CoreVarRef::resolved(x_binder),
        body: Box::new(var_ref(x_binder)),
        span: s(),
    };
    let mut program = CoreProgram {
        defs: vec![CoreDef {
            name: x,
            binder: x_binder,
            expr: malformed,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span: s(),
        }],
        top_level_items: Vec::new(),
    };

    let err =
        run_core_passes_with_interner(&mut program, &interner, false).expect_err("should fail");
    let message = err.message.clone().unwrap_or_default();
    assert!(
        message.contains("after `beta_reduce`"),
        "expected stage label in contract error, got: {}",
        message
    );
}

#[test]
fn primop_promote_keeps_bare_delete_bound_to_flow_list_delete() {
    let mut interner = Interner::new();
    let flow = interner.intern("Flow");
    let list = interner.intern("List");
    let delete = interner.intern("delete");
    let main = interner.intern("main");
    let xs = interner.intern("xs");
    let x = interner.intern("x");

    let delete_binder = binder(0, delete);
    let xs_binder = binder(1, xs);
    let x_binder = binder(2, x);
    let main_binder = binder(3, main);

    let delete_def = CoreDef {
        name: delete,
        binder: delete_binder,
        expr: CoreExpr::Lam {
            params: vec![xs_binder, x_binder],
            body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
            span: s(),
        },
        borrow_signature: None,
        result_ty: None,
        is_anonymous: false,
        is_recursive: false,
        fip: None,
        span: s(),
    };

    let main_def = CoreDef {
        name: main,
        binder: main_binder,
        expr: CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef {
                    name: delete,
                    binder: Some(delete_binder.id),
                },
                span: s(),
            }),
            args: vec![
                CoreExpr::Lit(CoreLit::Int(1), s()),
                CoreExpr::Lit(CoreLit::Int(2), s()),
            ],
            span: s(),
        },
        borrow_signature: None,
        result_ty: None,
        is_anonymous: false,
        is_recursive: false,
        fip: None,
        span: s(),
    };

    let mut program = CoreProgram {
        defs: vec![delete_def, main_def],
        top_level_items: vec![
            CoreTopLevelItem::Module {
                name: flow,
                body: vec![CoreTopLevelItem::Module {
                    name: list,
                    body: vec![CoreTopLevelItem::Function {
                        is_public: true,
                        name: delete,
                        type_params: Vec::new(),
                        parameters: vec![xs, x],
                        parameter_types: Vec::new(),
                        return_type: None,
                        effects: Vec::new(),
                        span: s(),
                    }],
                    span: s(),
                }],
                span: s(),
            },
            CoreTopLevelItem::Function {
                is_public: true,
                name: main,
                type_params: Vec::new(),
                parameters: Vec::new(),
                parameter_types: Vec::new(),
                return_type: None,
                effects: Vec::new(),
                span: s(),
            },
        ],
    };

    promote_builtins(&mut program, &interner);

    match &program.defs[1].expr {
        CoreExpr::App { .. } => {}
        other => panic!("expected bare delete to remain a stdlib call, got {other:?}"),
    }
}

#[test]
fn primop_promote_promotes_explicit_map_delete_name() {
    let mut interner = Interner::new();
    let map_delete = interner.intern("map_delete");
    let main = interner.intern("main");
    let main_binder = binder(0, main);

    let main_def = CoreDef {
        name: main,
        binder: main_binder,
        expr: CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef {
                    name: map_delete,
                    binder: None,
                },
                span: s(),
            }),
            args: vec![
                CoreExpr::Lit(CoreLit::Int(1), s()),
                CoreExpr::Lit(CoreLit::Int(2), s()),
            ],
            span: s(),
        },
        borrow_signature: None,
        result_ty: None,
        is_anonymous: false,
        is_recursive: false,
        fip: None,
        span: s(),
    };

    let mut program = CoreProgram {
        defs: vec![main_def],
        top_level_items: vec![CoreTopLevelItem::Function {
            is_public: true,
            name: main,
            type_params: Vec::new(),
            parameters: Vec::new(),
            parameter_types: Vec::new(),
            return_type: None,
            effects: Vec::new(),
            span: s(),
        }],
    };

    promote_builtins(&mut program, &interner);

    match &program.defs[0].expr {
        CoreExpr::PrimOp { op, .. } => assert_eq!(*op, CorePrimOp::HamtDelete),
        other => panic!("expected explicit map_delete promotion, got {other:?}"),
    }
}

#[test]
fn inline_trivial_does_not_inline_non_trivial() {
    // let x = PrimOp(Add, ...); x  — must keep the let.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let a = interner.intern("a");
    let b = interner.intern("b");
    let x_binder = binder(0, x);
    let a_binder = binder(1, a);
    let b_binder = binder(2, b);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::PrimOp {
            op: CorePrimOp::IAdd,
            args: vec![var_ref(a_binder), var_ref(b_binder)],
            span: s(),
        }),
        body: Box::new(var_ref(x_binder)),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    assert!(
        matches!(result, CoreExpr::Let { .. }),
        "non-trivial rhs must keep the Let"
    );
}

#[test]
fn inline_trivial_substitutes_inside_handler_body() {
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let resume = interner.intern("resume");
    let op = interner.intern("print");
    let effect = interner.intern("Console");
    let x_binder = binder(0, x);
    let resume_binder = binder(1, resume);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(5), s())),
        body: Box::new(CoreExpr::Handle {
            body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
            effect,
            handlers: vec![CoreHandler {
                operation: op,
                params: vec![],
                resume: resume_binder,
                body: var_ref(x_binder),
                span: s(),
            }],
            span: s(),
        }),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    match result {
        CoreExpr::Handle { handlers, .. } => {
            assert!(matches!(
                handlers[0].body,
                CoreExpr::Lit(CoreLit::Int(5), _)
            ));
        }
        other => panic!("expected Handle, got {other:?}"),
    }
}

#[test]
fn inline_trivial_respects_handler_shadowing() {
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let resume = interner.intern("resume");
    let op = interner.intern("print");
    let effect = interner.intern("Console");
    let outer_x = binder(0, x);
    let handler_x = binder(1, x);
    let resume_binder = binder(2, resume);

    let expr = CoreExpr::Let {
        var: outer_x,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(5), s())),
        body: Box::new(CoreExpr::Handle {
            body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
            effect,
            handlers: vec![CoreHandler {
                operation: op,
                params: vec![handler_x],
                resume: resume_binder,
                body: var_ref(handler_x),
                span: s(),
            }],
            span: s(),
        }),
        span: s(),
    };

    let result = inline_trivial_lets(expr);
    match result {
        CoreExpr::Handle { handlers, .. } => {
            assert!(matches!(
                handlers[0].body,
                CoreExpr::Var { ref var, .. } if var.binder == Some(handler_x.id)
            ));
        }
        other => panic!("expected Handle, got {other:?}"),
    }
}

// ── combined: COKC + inline_trivial ──────────────────────────────────────

#[test]
fn cokc_then_inline_collapses_field_binding() {
    // COKC creates `let x = Lit(7)` from a field binding; inline_trivial then
    // substitutes it so the final result is just Lit(7).
    //
    // Case(Con(Some, [Lit(7)]), [Con(Some, [Var(x)]) → x])
    //   --COKC→  let x = Lit(7); Var(x)   (field binding)
    //   --inline→  Lit(7)
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Case {
        scrutinee: Box::new(CoreExpr::Con {
            tag: CoreTag::Some,
            fields: vec![CoreExpr::Lit(CoreLit::Int(7), s())],
            span: s(),
        }),
        alts: vec![CoreAlt {
            pat: CorePat::Con {
                tag: CoreTag::Some,
                fields: vec![CorePat::Var(x_binder)],
            },
            guard: None,
            rhs: var_ref(x_binder),
            span: s(),
        }],
        span: s(),
    };

    // COKC substitutes x := Lit(7) directly (no intermediate let needed
    // since the field count is one and the pattern is Var).
    let result = case_of_known_constructor(expr);
    let result = inline_trivial_lets(result);
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(7), _)),
        "expected Lit(7), got {result:?}"
    );
}

// ── case_of_case ─────────────────────────────────────────────────────────

#[test]
fn case_of_case_pushes_outer_into_inner_arms() {
    // case (case x of { True -> Some(1); False -> None }) of
    //   { Some(y) -> y; None -> 0 }
    // →
    // case x of { True -> case Some(1) of { Some(y) -> y; None -> 0 };
    //             False -> case None of { Some(y) -> y; None -> 0 } }
    //
    // After COKC the inner cases reduce:
    //   True arm → 1,  False arm → 0
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let y = interner.intern("y");
    let x_binder = binder(0, x);
    let y_binder = binder(1, y);

    let inner_case = CoreExpr::Case {
        scrutinee: Box::new(var_ref(x_binder)),
        alts: vec![
            CoreAlt {
                pat: CorePat::Lit(CoreLit::Bool(true)),
                guard: None,
                rhs: CoreExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
                    span: s(),
                },
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Lit(CoreLit::Bool(false)),
                guard: None,
                rhs: CoreExpr::Con {
                    tag: CoreTag::None,
                    fields: vec![],
                    span: s(),
                },
                span: s(),
            },
        ],
        span: s(),
    };

    let outer_case = CoreExpr::Case {
        scrutinee: Box::new(inner_case),
        alts: vec![
            CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![CorePat::Var(y_binder)],
                },
                guard: None,
                rhs: var_ref(y_binder),
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::None,
                    fields: vec![],
                },
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                span: s(),
            },
        ],
        span: s(),
    };

    let result = case_of_case(outer_case);

    // The result should be: case x of { True -> ...; False -> ... }
    // where the inner cases have been pushed down.
    match &result {
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            // Scrutinee should be Var(x), not another Case.
            assert!(
                matches!(**scrutinee, CoreExpr::Var { .. }),
                "scrutinee should be Var(x) after case-of-case, got {scrutinee:?}"
            );
            assert_eq!(alts.len(), 2, "expected 2 alternatives");
            // Each arm's RHS should now be a nested Case (outer pushed in).
            for alt in alts {
                assert!(
                    matches!(alt.rhs, CoreExpr::Case { .. }),
                    "inner arm RHS should be a Case, got {:?}",
                    alt.rhs
                );
            }
        }
        other => panic!("expected Case, got {other:?}"),
    }

    // After COKC, the pushed-down cases should reduce fully.
    let result = case_of_known_constructor(result);
    match &result {
        CoreExpr::Case { alts, .. } => {
            // True arm: case Some(1) of { Some(y) -> y; ... } → 1
            assert!(
                matches!(alts[0].rhs, CoreExpr::Lit(CoreLit::Int(1), _)),
                "True arm should reduce to Lit(1), got {:?}",
                alts[0].rhs
            );
            // False arm: case None of { ...; None -> 0 } → 0
            assert!(
                matches!(alts[1].rhs, CoreExpr::Lit(CoreLit::Int(0), _)),
                "False arm should reduce to Lit(0), got {:?}",
                alts[1].rhs
            );
        }
        other => panic!("expected Case after COKC, got {other:?}"),
    }
}

#[test]
fn case_of_case_leaves_non_case_scrutinee_alone() {
    // case Var(x) of { ... } — scrutinee is not a Case, no transformation.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Case {
        scrutinee: Box::new(var_ref(x_binder)),
        alts: vec![CoreAlt {
            pat: CorePat::Wildcard,
            guard: None,
            rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
            span: s(),
        }],
        span: s(),
    };

    let result = case_of_case(expr);
    match &result {
        CoreExpr::Case { scrutinee, .. } => {
            assert!(
                matches!(**scrutinee, CoreExpr::Var { .. }),
                "scrutinee should remain Var"
            );
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn case_of_case_preserves_inner_guards() {
    // case (case x of { True if g -> A; _ -> B }) of { ... }
    // Guards on inner arms must be preserved after transformation.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let g = interner.intern("g");
    let x_binder = binder(0, x);
    let g_binder = binder(1, g);

    let inner = CoreExpr::Case {
        scrutinee: Box::new(var_ref(x_binder)),
        alts: vec![
            CoreAlt {
                pat: CorePat::Lit(CoreLit::Bool(true)),
                guard: Some(var_ref(g_binder)),
                rhs: CoreExpr::Lit(CoreLit::Int(1), s()),
                span: s(),
            },
            CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(2), s()),
                span: s(),
            },
        ],
        span: s(),
    };

    let outer = CoreExpr::Case {
        scrutinee: Box::new(inner),
        alts: vec![CoreAlt {
            pat: CorePat::Wildcard,
            guard: None,
            rhs: CoreExpr::Lit(CoreLit::Int(99), s()),
            span: s(),
        }],
        span: s(),
    };

    let result = case_of_case(outer);
    match &result {
        CoreExpr::Case { alts, .. } => {
            // First inner alt should still have its guard.
            assert!(
                alts[0].guard.is_some(),
                "guard on first inner alt must be preserved"
            );
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

// ── inline_lets (occurrence-based inliner) ────────────────────────────────

#[test]
fn inliner_eliminates_dead_binding() {
    // let x = 5; 42  →  42  (x is unused)
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(5), s())),
        body: Box::new(CoreExpr::Lit(CoreLit::Int(42), s())),
        span: s(),
    };

    let result = inline_lets(expr);
    assert!(
        matches!(result, CoreExpr::Lit(CoreLit::Int(42), _)),
        "dead binding should be eliminated, got {result:?}"
    );
}

#[test]
fn inliner_inlines_single_use() {
    // let x = PrimOp(IAdd, [1, 2]); PrimOp(IMul, [x, 3])
    //   → PrimOp(IMul, [PrimOp(IAdd, [1, 2]), 3])
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let rhs = CoreExpr::PrimOp {
        op: CorePrimOp::IAdd,
        args: vec![
            CoreExpr::Lit(CoreLit::Int(1), s()),
            CoreExpr::Lit(CoreLit::Int(2), s()),
        ],
        span: s(),
    };
    let body = CoreExpr::PrimOp {
        op: CorePrimOp::IMul,
        args: vec![var_ref(x_binder), CoreExpr::Lit(CoreLit::Int(3), s())],
        span: s(),
    };
    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(rhs),
        body: Box::new(body),
        span: s(),
    };

    let result = inline_lets(expr);
    // Should be a PrimOp(IMul, ...) not a Let.
    assert!(
        matches!(
            result,
            CoreExpr::PrimOp {
                op: CorePrimOp::IMul,
                ..
            }
        ),
        "single-use binding should be inlined, got {result:?}"
    );
}

#[test]
fn inliner_inlines_small_multi_use() {
    // let x = 3; PrimOp(IAdd, [x, x])  →  PrimOp(IAdd, [3, 3])
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let expr = CoreExpr::Let {
        var: x_binder,
        rhs: Box::new(CoreExpr::Lit(CoreLit::Int(3), s())),
        body: Box::new(CoreExpr::PrimOp {
            op: CorePrimOp::IAdd,
            args: vec![var_ref(x_binder), var_ref(x_binder)],
            span: s(),
        }),
        span: s(),
    };

    let result = inline_lets(expr);
    match result {
        CoreExpr::PrimOp { args, .. } => {
            assert!(matches!(args[0], CoreExpr::Lit(CoreLit::Int(3), _)));
            assert!(matches!(args[1], CoreExpr::Lit(CoreLit::Int(3), _)));
        }
        other => panic!("expected PrimOp, got {other:?}"),
    }
}

#[test]
fn inliner_preserves_letrec() {
    // letrec f = Lam(...); f(1)  — recursive bindings must NOT be inlined.
    let mut interner = Interner::new();
    let f = interner.intern("f");
    let f_binder = binder(0, f);

    let expr = CoreExpr::LetRec {
        var: f_binder,
        rhs: Box::new(CoreExpr::Lam {
            params: vec![binder(1, interner.intern("n"))],
            body: Box::new(CoreExpr::Lit(CoreLit::Int(0), s())),
            span: s(),
        }),
        body: Box::new(CoreExpr::App {
            func: Box::new(var_ref(f_binder)),
            args: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
            span: s(),
        }),
        span: s(),
    };

    let result = inline_lets(expr);
    assert!(
        matches!(result, CoreExpr::LetRec { .. }),
        "letrec must be preserved, got {result:?}"
    );
}

// ── anf_normalize ─────────────────────────────────────────────────────────

#[test]
fn anf_flattens_nested_primop_args() {
    // PrimOp(IAdd, [PrimOp(IMul, [2, 3]), Lit(1)])
    //   → Let(t0, PrimOp(IMul, [2, 3]),
    //       PrimOp(IAdd, [Var(t0), 1]))
    let nested = CoreExpr::PrimOp {
        op: CorePrimOp::IAdd,
        args: vec![
            CoreExpr::PrimOp {
                op: CorePrimOp::IMul,
                args: vec![
                    CoreExpr::Lit(CoreLit::Int(2), s()),
                    CoreExpr::Lit(CoreLit::Int(3), s()),
                ],
                span: s(),
            },
            CoreExpr::Lit(CoreLit::Int(1), s()),
        ],
        span: s(),
    };

    let mut next_id = 100;
    let result = anf_normalize(nested, &mut next_id);

    // Should be Let(_, PrimOp(IMul, ...), PrimOp(IAdd, [Var, Lit]))
    match &result {
        CoreExpr::Let { rhs, body, .. } => {
            assert!(
                matches!(
                    **rhs,
                    CoreExpr::PrimOp {
                        op: CorePrimOp::IMul,
                        ..
                    }
                ),
                "rhs should be IMul, got {rhs:?}"
            );
            assert!(
                matches!(
                    **body,
                    CoreExpr::PrimOp {
                        op: CorePrimOp::IAdd,
                        ..
                    }
                ),
                "body should be IAdd, got {body:?}"
            );
            // The IAdd args should be trivial (Var, Lit).
            if let CoreExpr::PrimOp { args, .. } = &**body {
                assert!(
                    matches!(args[0], CoreExpr::Var { .. }),
                    "first arg should be Var, got {:?}",
                    args[0]
                );
                assert!(
                    matches!(args[1], CoreExpr::Lit(CoreLit::Int(1), _)),
                    "second arg should be Lit(1), got {:?}",
                    args[1]
                );
            }
        }
        other => panic!("expected Let, got {other:?}"),
    }
}

#[test]
fn anf_leaves_trivial_expressions_alone() {
    // Var and Lit are already trivial — no let-binding needed.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    let x_binder = binder(0, x);

    let var_expr = var_ref(x_binder);
    let lit_expr = CoreExpr::Lit(CoreLit::Int(42), s());

    let mut next_id = 100;
    let r1 = anf_normalize(var_expr.clone(), &mut next_id);
    let r2 = anf_normalize(lit_expr.clone(), &mut next_id);

    assert!(matches!(r1, CoreExpr::Var { .. }));
    assert!(matches!(r2, CoreExpr::Lit(CoreLit::Int(42), _)));
    // No fresh binders should have been allocated.
    assert_eq!(next_id, 100);
}

#[test]
fn anf_normalizes_app_func_and_args() {
    // App(PrimOp(IAdd, [1, 2]), [PrimOp(IMul, [3, 4])])
    // Both func and arg should be let-bound.
    let expr = CoreExpr::App {
        func: Box::new(CoreExpr::PrimOp {
            op: CorePrimOp::IAdd,
            args: vec![
                CoreExpr::Lit(CoreLit::Int(1), s()),
                CoreExpr::Lit(CoreLit::Int(2), s()),
            ],
            span: s(),
        }),
        args: vec![CoreExpr::PrimOp {
            op: CorePrimOp::IMul,
            args: vec![
                CoreExpr::Lit(CoreLit::Int(3), s()),
                CoreExpr::Lit(CoreLit::Int(4), s()),
            ],
            span: s(),
        }],
        span: s(),
    };

    let mut next_id = 100;
    let result = anf_normalize(expr, &mut next_id);

    // Should produce two Let bindings wrapping an App of trivial operands.
    // Let(t0, PrimOp(IAdd,...), Let(t1, PrimOp(IMul,...), App(Var(t0), [Var(t1)])))
    match &result {
        CoreExpr::Let { body, .. } => match &**body {
            CoreExpr::Let { body: inner, .. } => {
                assert!(
                    matches!(**inner, CoreExpr::App { .. }),
                    "innermost should be App, got {inner:?}"
                );
            }
            other => panic!("expected inner Let, got {other:?}"),
        },
        other => panic!("expected outer Let, got {other:?}"),
    }
    assert_eq!(next_id, 102, "should have allocated 2 fresh binders");
}

#[test]
fn run_core_passes_rejects_malformed_aether_before_lowering() {
    let mut interner = Interner::new();
    let main_name = interner.intern("main");
    let xs = binder(1, interner.intern("xs"));

    let mut program = crate::core::CoreProgram {
        defs: vec![crate::core::CoreDef {
            name: main_name,
            binder: binder(0, main_name),
            expr: CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(xs),
                body: Box::new(var_ref(xs)),
                span: s(),
            },
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span: s(),
        }],
        top_level_items: Vec::new(),
    };

    let err = run_core_passes(&mut program).expect_err("expected malformed Aether to fail");
    assert!(
        err.message()
            .is_some_and(|message| message.contains("malformed Aether")),
        "unexpected diagnostic: {err:?}"
    );
}
