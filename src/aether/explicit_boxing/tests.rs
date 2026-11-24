use super::*;
use crate::core::{CoreAlt, CoreDef, CoreTag};

const GLOBAL: u32 = 900;
const FUNCTION: u32 = 901;

fn s() -> Span {
    Span::default()
}

fn binder(id: u32, rep: FluxRep) -> CoreBinder {
    CoreBinder::with_rep(CoreBinderId(id), Symbol::new(id), rep)
}

fn tagged(id: u32) -> CoreBinder {
    binder(id, FluxRep::TaggedRep)
}

fn int_binder(id: u32) -> CoreBinder {
    binder(id, FluxRep::IntRep)
}

fn v(binder: CoreBinder) -> CoreExpr {
    CoreExpr::bound_var(&binder, s())
}

fn int(n: i64) -> CoreExpr {
    CoreExpr::Lit(CoreLit::Int(n), s())
}

fn op(op: CorePrimOp, args: Vec<CoreExpr>) -> CoreExpr {
    CoreExpr::PrimOp { op, args, span: s() }
}

fn let_(var: CoreBinder, rhs: CoreExpr, body: CoreExpr) -> CoreExpr {
    let_in(var, rhs, body, s())
}

/// A call to the global `g`.
fn call(args: Vec<CoreExpr>) -> CoreExpr {
    CoreExpr::apply(v(tagged(GLOBAL)), args, s())
}

fn lam(params: Vec<CoreBinder>, body: CoreExpr) -> CoreExpr {
    CoreExpr::lambda(params, body, s())
}

fn case(scrutinee: CoreExpr, alts: Vec<CoreAlt>) -> CoreExpr {
    CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, join_ty: None, span: s() }
}

fn alt(pat: CorePat, guard: Option<CoreExpr>, rhs: CoreExpr) -> CoreAlt {
    CoreAlt { pat, guard, rhs, span: s() }
}

fn cons(head: CorePat, tail: CorePat) -> CorePat {
    CorePat::Con { tag: CoreTag::Cons, fields: vec![head, tail] }
}

/// Runs the pass on `fn f(params) = body` beside a global `g`, checks the
/// result with the checker and Core lint, checks that a second run changes
/// nothing, and prints `f`.
fn boxed(params: Vec<CoreBinder>, body: CoreExpr) -> String {
    let mut program = CoreProgram {
        defs: vec![
            CoreDef::new(tagged(GLOBAL), int(0), false, s()),
            CoreDef::new(tagged(FUNCTION), lam(params, body), false, s()),
        ],
        top_level_items: Vec::new(),
    };
    explicit_boxing(&mut program);
    if let Err(violations) = check::check_program(&program) {
        panic!("the checker rejects the output: {violations:?}\n{}", show(&program.defs[1].expr));
    }
    if let Err(errors) = crate::core::passes::lint::lint_core_program(&program) {
        panic!("Core lint rejects the output: {errors:?}\n{}", show(&program.defs[1].expr));
    }
    let once = format!("{:?}", program.defs);
    explicit_boxing(&mut program);
    assert_eq!(format!("{:?}", program.defs), once, "a second run changes the program");
    show(&program.defs[1].expr)
}

/// A compact rendering: `vN` is binder N, `:I` marks an IntRep binder, and
/// minted binders are numbered from the program's largest id.
fn show(expr: &CoreExpr) -> String {
    match expr {
        CoreExpr::Var { var, .. } => match var.binder {
            Some(id) if id.0 == GLOBAL => "g".to_string(),
            Some(id) => format!("v{}", id.0),
            None => "?".to_string(),
        },
        CoreExpr::Lit(CoreLit::Int(n), _) => n.to_string(),
        CoreExpr::Lit(lit, _) => format!("{lit:?}"),
        CoreExpr::Lam { params, body, .. } => {
            let params: Vec<_> = params.iter().map(show_binder).collect();
            format!("\\({}) -> {}", params.join(", "), show(body))
        }
        CoreExpr::App { func, args, .. } => format!("{}({})", show(func), show_all(args)),
        CoreExpr::Let { var, rhs, body, .. } => {
            format!("let {} = {}; {}", show_binder(var), show(rhs), show(body))
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            let alts: Vec<_> = alts
                .iter()
                .map(|alt| {
                    let guard =
                        alt.guard.as_ref().map(|g| format!(" if {}", show(g))).unwrap_or_default();
                    format!("{}{guard} -> {}", show_pat(&alt.pat), show(&alt.rhs))
                })
                .collect();
            format!("case {} {{ {} }}", show(scrutinee), alts.join(" | "))
        }
        CoreExpr::Con { tag, fields, .. } => format!("{tag:?}({})", show_all(fields)),
        CoreExpr::PrimOp { op, args, .. } => format!("{op:?}({})", show_all(args)),
        CoreExpr::MemberAccess { object, .. } => format!("{}.member", show(object)),
        CoreExpr::Handle { body, handlers, .. } => {
            let clauses: Vec<_> = handlers
                .iter()
                .map(|h| {
                    let params: Vec<_> = h.params.iter().map(show_binder).collect();
                    format!("op({}) -> {}", params.join(", "), show(&h.body))
                })
                .collect();
            format!("handle {} with {{ {} }}", show(body), clauses.join(" | "))
        }
        other => format!("{other:?}"),
    }
}

fn show_all(exprs: &[CoreExpr]) -> String {
    exprs.iter().map(show).collect::<Vec<_>>().join(", ")
}

fn show_binder(binder: &CoreBinder) -> String {
    match binder.rep {
        FluxRep::IntRep => format!("v{}:I", binder.id.0),
        FluxRep::BoolRep => format!("v{}:B", binder.id.0),
        _ => format!("v{}", binder.id.0),
    }
}

fn show_pat(pat: &CorePat) -> String {
    match pat {
        CorePat::Var(binder) => show_binder(binder),
        CorePat::Con { tag, fields } => {
            format!("{tag:?}({})", fields.iter().map(show_pat).collect::<Vec<_>>().join(", "))
        }
        CorePat::Lit(CoreLit::Int(n)) => n.to_string(),
        CorePat::Wildcard => "_".to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_parameter_used_only_raw_is_consumed_by_its_unbox() {
    // fn f(n: Int) = n + 1
    let n = int_binder(1);
    let body = op(CorePrimOp::IAdd, vec![v(n), int(1)]);
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v903:I = IAdd(v902, 1); BoxInt(v903)"
    );
}

#[test]
fn a_parameter_used_only_uniform_is_not_unboxed() {
    // fn f(n: Int) = g(n)
    let n = int_binder(1);
    assert_eq!(boxed(vec![n], call(vec![v(n)])), "\\(v1) -> g(v1)");
}

#[test]
fn a_parameter_used_both_ways_is_borrowed_by_its_unbox() {
    // fn f(n: Int) = { let m = n + 1; g(n, m) }
    let (n, m) = (int_binder(1), int_binder(2));
    let body = let_(m, op(CorePrimOp::IAdd, vec![v(n), int(1)]), call(vec![v(n), v(m)]));
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxInt(v1); let v2:I = IAdd(v902, 1); \
         let v903 = BoxInt(v2); g(v1, v903)"
    );
}

#[test]
fn an_unused_parameter_is_only_retagged() {
    assert_eq!(boxed(vec![int_binder(1)], int(0)), "\\(v1) -> 0");
}

#[test]
fn a_field_passed_on_unchanged_is_not_reboxed() {
    // match xs { [h | t] -> [h | t] }, with h: Int
    let (xs, h, t) = (tagged(1), int_binder(2), tagged(3));
    let rebuilt = CoreExpr::Con { tag: CoreTag::Cons, fields: vec![v(h), v(t)], span: s() };
    let body = case(v(xs), vec![alt(cons(CorePat::Var(h), CorePat::Var(t)), None, rebuilt)]);
    assert_eq!(boxed(vec![xs], body), "\\(v1) -> case v1 { Cons(v2, v3) -> Cons(v2, v3) }");
}

#[test]
fn a_field_used_in_arithmetic_is_borrowed_by_its_unbox() {
    // match xs { [h | _] -> h + 1 }
    let (xs, h) = (tagged(1), int_binder(2));
    let rhs = op(CorePrimOp::IAdd, vec![v(h), int(1)]);
    let body = case(v(xs), vec![alt(cons(CorePat::Var(h), CorePat::Wildcard), None, rhs)]);
    assert_eq!(
        boxed(vec![xs], body),
        "\\(v1) -> case v1 { Cons(v2, _) -> let v902:I = UnboxInt(v2); \
         let v903:I = IAdd(v902, 1); BoxInt(v903) }"
    );
}

#[test]
fn a_guard_and_its_arm_unbox_a_field_separately() {
    // match xs { [h | _] if h > 0 -> h + 1 }
    let (xs, h, positive) = (tagged(1), int_binder(2), binder(3, FluxRep::BoolRep));
    let guard = let_(positive, op(CorePrimOp::ICmpGt, vec![v(h), int(0)]), v(positive));
    let rhs = op(CorePrimOp::IAdd, vec![v(h), int(1)]);
    let body = case(v(xs), vec![alt(cons(CorePat::Var(h), CorePat::Wildcard), Some(guard), rhs)]);
    assert_eq!(
        boxed(vec![xs], body),
        "\\(v1) -> case v1 { Cons(v2, _) if let v902:I = UnboxInt(v2); \
         let v3:B = ICmpGt(v902, 0); v3 -> let v903:I = UnboxInt(v2); \
         let v904:I = IAdd(v903, 1); BoxInt(v904) }"
    );
}

#[test]
fn a_lambda_capturing_a_raw_int_gets_a_box() {
    // fn f(n: Int) = { let m = n + 1; let k = fn(y) { m + 1 }; k }
    let (n, m, k, y) = (int_binder(1), int_binder(2), tagged(3), tagged(4));
    let inner = lam(vec![y], op(CorePrimOp::IAdd, vec![v(m), int(1)]));
    let body = let_(m, op(CorePrimOp::IAdd, vec![v(n), int(1)]), let_(k, inner, v(k)));
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v2:I = IAdd(v902, 1); \
         let v903 = BoxInt(v2); let v3 = \\(v4) -> let v904:I = UnboxInt(v903); \
         let v905:I = IAdd(v904, 1); BoxInt(v905); v3"
    );
}

#[test]
fn a_handler_clause_capturing_a_raw_int_gets_a_box() {
    // fn f(n: Int) = { let m = n + 1; handle 0 with { op(a: Int) -> m + a } }
    let (n, m, a) = (int_binder(1), int_binder(2), int_binder(3));
    let handler = CoreHandler {
        operation: Symbol::new(50),
        params: vec![a],
        param_types: Vec::new(),
        state: None,
        state_ty: None,
        resume: binder(4, FluxRep::BoxedRep),
        resume_ty: None,
        body: op(CorePrimOp::IAdd, vec![v(m), v(a)]),
        span: s(),
    };
    let handle = CoreExpr::Handle {
        body: Box::new(int(0)),
        effect: Symbol::new(51),
        parameter: None,
        handlers: vec![handler],
        span: s(),
    };
    let body = let_(m, op(CorePrimOp::IAdd, vec![v(n), int(1)]), handle);
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v2:I = IAdd(v902, 1); \
         let v903 = BoxInt(v2); handle 0 with { op(v3) -> let v904:I = UnboxInt(v903); \
         let v905:I = UnboxInt(v3); let v906:I = IAdd(v904, v905); BoxInt(v906) }"
    );
}

#[test]
fn a_call_result_bound_as_an_int_is_consumed_by_its_unbox() {
    // fn f(x) = { let r: Int = g(x); r + 1 }
    let (x, r) = (tagged(1), int_binder(2));
    let body = let_(r, call(vec![v(x)]), op(CorePrimOp::IAdd, vec![v(r), int(1)]));
    assert_eq!(
        boxed(vec![x], body),
        "\\(v1) -> let v2 = g(v1); let v902:I = UnboxIntMove(v2); \
         let v903:I = IAdd(v902, 1); BoxInt(v903)"
    );
}

#[test]
fn a_call_result_bound_as_an_int_and_returned_stays_the_call_result() {
    // fn f(x) = { let r: Int = g(x); r }: unboxing `r` and boxing it again
    // would leave work after the call, so it would not be a tail call.
    let (x, r) = (tagged(1), int_binder(2));
    let body = let_(r, call(vec![v(x)]), v(r));
    assert_eq!(boxed(vec![x], body), "\\(v1) -> let v2 = g(v1); v2");
}

#[test]
fn a_length_bound_as_an_int_and_passed_on_is_not_unboxed() {
    // A runtime length is a uniform word, though ANF types its temp IntRep.
    let (text, len) = (tagged(1), int_binder(2));
    let body = let_(len, op(CorePrimOp::StringLength, vec![v(text)]), call(vec![v(len)]));
    assert_eq!(boxed(vec![text], body), "\\(v1) -> let v2 = StringLength(v1); g(v2)");
}

#[test]
fn a_length_bound_as_an_int_is_borrowed_by_its_unbox() {
    // A length is not a value Aether owns, so its unbox only borrows it.
    let (text, len) = (tagged(1), int_binder(2));
    let body = let_(
        len,
        op(CorePrimOp::StringLength, vec![v(text)]),
        op(CorePrimOp::IAdd, vec![v(len), int(1)]),
    );
    assert_eq!(
        boxed(vec![text], body),
        "\\(v1) -> let v2 = StringLength(v1); let v902:I = UnboxInt(v2); \
         let v903:I = IAdd(v902, 1); BoxInt(v903)"
    );
}

#[test]
fn a_uniform_parameter_read_only_by_int_operations_is_consumed_by_one_unbox() {
    // fn f(n) = n + n, where the binder says Tagged but ANF typed the op.
    let n = tagged(1);
    let body = op(CorePrimOp::IAdd, vec![v(n), v(n)]);
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v903:I = IAdd(v902, v902); BoxInt(v903)"
    );
}

#[test]
fn a_uniform_parameter_keeps_generic_arithmetic_generic() {
    // The binder doesn't say Int, so `Add` on it may be a Float's or a String's.
    let (x, sum) = (tagged(1), tagged(2));
    let body = let_(sum, op(CorePrimOp::Add, vec![v(x), int(1)]), v(sum));
    assert_eq!(boxed(vec![x], body), "\\(v1) -> let v2 = Add(v1, 1); v2");
}

#[test]
fn a_lambda_capturing_a_uniform_parameter_holds_the_parameter() {
    // fn f(n) = { let k = fn(y) { n + 1 }; k }, the op typed by ANF.
    let (n, k, y) = (tagged(1), tagged(2), tagged(3));
    let inner = lam(vec![y], op(CorePrimOp::IAdd, vec![v(n), int(1)]));
    let body = let_(k, inner, v(k));
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v2 = \\(v3) -> let v902:I = UnboxInt(v1); \
         let v903:I = IAdd(v902, 1); BoxInt(v903); v2"
    );
}

#[test]
fn generic_arithmetic_on_ints_becomes_int_arithmetic() {
    // fn f(a: Int, b: Int, c: Int) = (a + b) * c, from a specialised generic function
    let (a, b, c, sum, product) =
        (int_binder(1), int_binder(2), int_binder(3), tagged(4), tagged(5));
    let body = let_(
        sum,
        op(CorePrimOp::Add, vec![v(a), v(b)]),
        let_(product, op(CorePrimOp::Mul, vec![v(sum), v(c)]), v(product)),
    );
    assert_eq!(
        boxed(vec![a, b, c], body),
        "\\(v1, v2, v3) -> let v902:I = UnboxIntMove(v1); let v903:I = UnboxIntMove(v2); \
         let v904:I = UnboxIntMove(v3); let v4:I = IAdd(v902, v903); \
         let v5:I = IMul(v4, v904); BoxInt(v5)"
    );
}

#[test]
fn a_generic_comparison_of_ints_becomes_an_int_comparison() {
    let (a, less) = (int_binder(1), tagged(2));
    let body = let_(less, op(CorePrimOp::Lt, vec![v(a), int(10)]), v(less));
    assert_eq!(
        boxed(vec![a], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v2:B = ICmpLt(v902, 10); v2"
    );
}

#[test]
fn generic_arithmetic_on_an_unknown_value_stays_generic() {
    let (a, x, sum) = (int_binder(1), tagged(2), tagged(3));
    let body = let_(sum, op(CorePrimOp::Add, vec![v(a), v(x)]), v(sum));
    assert_eq!(boxed(vec![a, x], body), "\\(v1, v2) -> let v3 = Add(v1, v2); v3");
}

#[test]
fn a_module_member_callee_is_left_alone() {
    // fn f(n: Int) = M.h(n)
    let n = int_binder(1);
    let callee = CoreExpr::MemberAccess {
        object: Box::new(CoreExpr::external_var(Symbol::new(60), s())),
        member: Symbol::new(61),
        span: s(),
    };
    let body = CoreExpr::apply(callee, vec![v(n)], s());
    assert_eq!(boxed(vec![n], body), "\\(v1) -> ?.member(v1)");
}

#[test]
fn a_parameter_matched_against_literals_is_read_raw() {
    // fn f(n: Int) = match n { 0 -> 0, _ -> g(n - 1) }
    let (n, pred) = (int_binder(1), int_binder(2));
    let recurse = let_(pred, op(CorePrimOp::ISub, vec![v(n), int(1)]), call(vec![v(pred)]));
    let body = case(
        v(n),
        vec![
            alt(CorePat::Lit(CoreLit::Int(0)), None, int(0)),
            alt(CorePat::Wildcard, None, recurse),
        ],
    );
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); case v902 { 0 -> 0 | _ -> \
         let v2:I = ISub(v902, 1); let v903 = BoxInt(v2); g(v903) }"
    );
}

#[test]
fn a_variable_pattern_on_a_raw_scrutinee_is_raw() {
    let (n, m, k) = (int_binder(1), int_binder(2), tagged(3));
    let body = let_(
        m,
        op(CorePrimOp::IAdd, vec![v(n), int(1)]),
        case(v(m), vec![alt(CorePat::Var(k), None, call(vec![v(k)]))]),
    );
    assert_eq!(
        boxed(vec![n], body),
        "\\(v1) -> let v902:I = UnboxIntMove(v1); let v2:I = IAdd(v902, 1); \
         case v2 { v3:I -> let v903 = BoxInt(v3); g(v903) }"
    );
}

#[test]
fn a_dictionary_is_left_alone() {
    let n = int_binder(1);
    let mut dict = CoreDef::new(tagged(FUNCTION), lam(vec![n], v(n)), false, s());
    dict.is_dict_def = true;
    let mut program = CoreProgram { defs: vec![dict], top_level_items: Vec::new() };
    let before = format!("{:?}", program.defs);
    explicit_boxing(&mut program);
    assert_eq!(format!("{:?}", program.defs), before);
}
