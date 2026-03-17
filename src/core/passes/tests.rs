use super::*;
use crate::{
    core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp,
        CoreTag,
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
