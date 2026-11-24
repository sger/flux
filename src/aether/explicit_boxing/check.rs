//! Checks a program after `explicit_boxing`: every raw Int reaches only a
//! position that takes one, and every uniform position gets a uniform word.
//! `--verify-each` runs it after the pass.

use std::collections::{HashMap, HashSet};

use super::reps::{
    Node, Rep, free_locals, gives_raw, matches_structure, takes_raw, unboxes, walk, walk_pat,
};
use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CoreLit, CorePat, CoreProgram, FluxRep};
use crate::syntax::Identifier;

/// A word in the wrong representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxingViolation {
    pub def: Identifier,
    pub message: String,
}

/// Checks every definition but dictionaries, which the pass leaves alone.
pub(crate) fn check_program(program: &CoreProgram) -> Result<(), Vec<BoxingViolation>> {
    let globals: HashSet<CoreBinderId> = program.defs.iter().map(|def| def.binder.id).collect();
    let mut violations = Vec::new();
    for def in program.defs.iter().filter(|def| !def.is_dict_def) {
        let mut reps = HashMap::new();
        walk(&def.expr, &mut |node| {
            if let Node::Bind(binder) = node {
                reps.insert(binder.id, rep_of(&binder));
            }
        });
        let mut checker = Checker { reps, globals: &globals, messages: Vec::new() };
        checker.expr(&def.expr, Want::Is(Rep::Uniform));
        violations.extend(
            checker.messages.into_iter().map(|message| BoxingViolation { def: def.name, message }),
        );
    }
    if violations.is_empty() { Ok(()) } else { Err(violations) }
}

fn rep_of(binder: &CoreBinder) -> Rep {
    if binder.rep == FluxRep::IntRep { Rep::Raw } else { Rep::Uniform }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Is(Rep),
    /// A scrutinee matched only by literals and variables takes either.
    Either,
}

struct Checker<'a> {
    reps: HashMap<CoreBinderId, Rep>,
    globals: &'a HashSet<CoreBinderId>,
    messages: Vec<String>,
}

impl Checker<'_> {
    /// The rep of a variable: a global is a uniform word even when its
    /// binder is `IntRep`, since globals are stored uniform.
    fn var_rep(&self, id: Option<CoreBinderId>) -> Rep {
        match id {
            Some(id) if !self.globals.contains(&id) => {
                self.reps.get(&id).copied().unwrap_or(Rep::Uniform)
            }
            _ => Rep::Uniform,
        }
    }

    fn report(&mut self, message: String) {
        self.messages.push(message);
    }

    fn gives(&mut self, have: Rep, want: Want, what: &str) {
        if let Want::Is(want) = want
            && have != want
        {
            self.report(format!("{what} gives a {have:?} word where a {want:?} one is wanted"));
        }
    }

    fn expr(&mut self, expr: &CoreExpr, want: Want) {
        match expr {
            CoreExpr::Var { var, .. } => {
                let have = self.var_rep(var.binder);
                self.gives(have, want, &format!("variable {:?}", var.binder));
            }
            CoreExpr::Lit(CoreLit::Int(_), _) => {}
            CoreExpr::Lit(..) => self.gives(Rep::Uniform, want, "a literal"),
            CoreExpr::Lam { params, body, .. } => {
                self.closure(params, &[], expr, "lambda");
                self.expr(body, Want::Is(Rep::Uniform));
                self.gives(Rep::Uniform, want, "a lambda");
            }
            CoreExpr::App { func, args, .. } => {
                if let CoreExpr::Var { var, .. } = func.as_ref()
                    && self.var_rep(var.binder) == Rep::Raw
                {
                    self.report(format!("a raw word {:?} is called", var.binder));
                }
                for arg in args {
                    self.expr(arg, Want::Is(Rep::Uniform));
                }
                self.gives(Rep::Uniform, want, "a call");
            }
            CoreExpr::Let { var, rhs, body, .. } => {
                self.expr(rhs, Want::Is(rep_of(var)));
                self.expr(body, want);
            }
            CoreExpr::LetRec { var, rhs, body, .. } => {
                self.uniform_binder(var, "a recursive binder");
                self.expr(rhs, Want::Is(Rep::Uniform));
                self.expr(body, want);
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                for (binder, rhs) in bindings {
                    self.uniform_binder(binder, "a recursive binder");
                    self.expr(rhs, Want::Is(Rep::Uniform));
                }
                self.expr(body, want);
            }
            CoreExpr::Case { scrutinee, alts, .. } => {
                let scrutinee_want =
                    if matches_structure(alts) { Want::Is(Rep::Uniform) } else { Want::Either };
                self.expr(scrutinee, scrutinee_want);
                let scrutinee_rep = match scrutinee.as_ref() {
                    CoreExpr::Var { var, .. } => Some(self.var_rep(var.binder)),
                    _ => None,
                };
                for alt in alts {
                    match &alt.pat {
                        CorePat::Var(binder) => {
                            if let Some(rep) = scrutinee_rep
                                && rep_of(binder) != rep
                            {
                                self.report(format!(
                                    "pattern binder {:?} is {:?} but its scrutinee is {rep:?}",
                                    binder.id,
                                    rep_of(binder)
                                ));
                            }
                        }
                        nested => {
                            let mut fields = Vec::new();
                            walk_pat(nested, &mut |node| {
                                if let Node::Bind(binder) = node {
                                    fields.push(binder);
                                }
                            });
                            for field in fields {
                                self.uniform_binder(&field, "a field binder");
                            }
                        }
                    }
                    if let Some(guard) = &alt.guard {
                        self.expr(guard, Want::Is(Rep::Uniform));
                    }
                    self.expr(&alt.rhs, want);
                }
            }
            CoreExpr::Con { fields, .. } => {
                for field in fields {
                    self.expr(field, Want::Is(Rep::Uniform));
                }
                self.gives(Rep::Uniform, want, "a constructor");
            }
            CoreExpr::PrimOp { op, args, .. } => {
                for arg in args {
                    if unboxes(*op) {
                        let uniform_var = matches!(
                            arg,
                            CoreExpr::Var { var, .. } if self.var_rep(var.binder) == Rep::Uniform
                        );
                        if !uniform_var {
                            self.report(format!("{op:?} takes {arg:?}, not a uniform variable"));
                        }
                    } else {
                        let rep = if takes_raw(*op) { Rep::Raw } else { Rep::Uniform };
                        self.expr(arg, Want::Is(rep));
                    }
                }
                let have = if gives_raw(*op) { Rep::Raw } else { Rep::Uniform };
                self.gives(have, want, &format!("{op:?}"));
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                self.expr(object, Want::Is(Rep::Uniform));
                self.gives(Rep::Uniform, want, "a field read");
            }
            // A function's result is uniform, whatever the context around the
            // `return` wants, and control does not come back.
            CoreExpr::Return { value, .. } => self.expr(value, Want::Is(Rep::Uniform)),
            CoreExpr::Perform { args, .. } => {
                for arg in args {
                    self.expr(arg, Want::Is(Rep::Uniform));
                }
                self.gives(Rep::Uniform, want, "a perform");
            }
            CoreExpr::Handle { body, parameter, handlers, .. } => {
                if let Some(parameter) = parameter {
                    self.expr(parameter, Want::Is(Rep::Uniform));
                }
                self.expr(body, Want::Is(Rep::Uniform));
                for handler in handlers {
                    let bound: Vec<_> = handler
                        .params
                        .iter()
                        .chain(handler.state.iter())
                        .chain(std::iter::once(&handler.resume))
                        .map(|binder| binder.id)
                        .collect();
                    self.closure(&handler.params, &bound, &handler.body, "handler clause");
                    if let Some(state) = &handler.state {
                        self.uniform_binder(state, "a handler state");
                    }
                    self.expr(&handler.body, Want::Is(Rep::Uniform));
                }
                self.gives(Rep::Uniform, want, "a handle");
            }
        }
    }

    /// A closure's parameters and captures are uniform words: callers and the
    /// closure's environment hold nothing else.
    fn closure(
        &mut self,
        params: &[CoreBinder],
        bound: &[CoreBinderId],
        body: &CoreExpr,
        what: &str,
    ) {
        for param in params {
            self.uniform_binder(param, &format!("a {what} parameter"));
        }
        let mut captured: Vec<_> = free_locals(body)
            .into_iter()
            .filter(|id| {
                !bound.contains(id)
                    && !self.globals.contains(id)
                    && self.reps.get(id) == Some(&Rep::Raw)
            })
            .collect();
        captured.sort();
        for id in captured {
            self.report(format!("a {what} captures the raw word {id:?}"));
        }
    }

    fn uniform_binder(&mut self, binder: &CoreBinder, what: &str) {
        if rep_of(binder) == Rep::Raw {
            self.report(format!("{what} {:?} is IntRep", binder.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CoreAlt, CoreDef, CoreHandler, CorePrimOp, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::symbol::Symbol;

    const GLOBAL: u32 = 900;

    fn s() -> Span {
        Span::default()
    }

    fn binder(id: u32, rep: FluxRep) -> CoreBinder {
        CoreBinder::with_rep(CoreBinderId(id), Symbol::new(id), rep)
    }

    fn tagged(id: u32) -> CoreBinder {
        binder(id, FluxRep::TaggedRep)
    }

    fn raw(id: u32) -> CoreBinder {
        binder(id, FluxRep::IntRep)
    }

    fn v(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(&binder, s())
    }

    fn int(n: i64) -> CoreExpr {
        CoreExpr::Lit(CoreLit::Int(n), s())
    }

    fn prim(op: CorePrimOp, args: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::PrimOp { op, args, span: s() }
    }

    fn let_in(var: CoreBinder, rhs: CoreExpr, body: CoreExpr) -> CoreExpr {
        CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span: s() }
    }

    fn call(args: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::apply(v(tagged(GLOBAL)), args, s())
    }

    /// `fn f(params) = body`, beside a global `g` (binder `GLOBAL`), which
    /// `call` calls.
    fn check(params: Vec<CoreBinder>, body: CoreExpr) -> Vec<String> {
        check_with_global(tagged(GLOBAL), params, body)
    }

    fn check_with_global(
        global: CoreBinder,
        params: Vec<CoreBinder>,
        body: CoreExpr,
    ) -> Vec<String> {
        let program = CoreProgram {
            defs: vec![
                CoreDef::new(global, int(0), false, s()),
                CoreDef::new(tagged(901), CoreExpr::lambda(params, body, s()), false, s()),
            ],
            top_level_items: Vec::new(),
        };
        match check_program(&program) {
            Ok(()) => Vec::new(),
            Err(violations) => violations.into_iter().map(|v| v.message).collect(),
        }
    }

    fn rejected(messages: Vec<String>, needle: &str) {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected a violation mentioning `{needle}`, got {messages:?}"
        );
    }

    #[test]
    fn an_explicit_program_is_accepted() {
        // fn f(p) { let x = UnboxIntMove(p); let y = IAdd(x, 1); BoxInt(y) }
        let (p, x, y) = (tagged(1), raw(2), raw(3));
        let body = let_in(
            x,
            prim(CorePrimOp::UnboxIntMove, vec![v(p)]),
            let_in(
                y,
                prim(CorePrimOp::IAdd, vec![v(x), int(1)]),
                prim(CorePrimOp::BoxInt, vec![v(y)]),
            ),
        );
        assert_eq!(check(vec![p], body), Vec::<String>::new());
    }

    #[test]
    fn a_raw_word_passed_to_a_call_is_rejected() {
        let (p, x) = (tagged(1), raw(2));
        let body = let_in(x, prim(CorePrimOp::UnboxInt, vec![v(p)]), call(vec![v(x)]));
        rejected(check(vec![p], body), "Raw word where a Uniform one");
    }

    #[test]
    fn a_tagged_word_in_int_arithmetic_is_rejected() {
        let p = tagged(1);
        let body = let_in(raw(2), prim(CorePrimOp::IAdd, vec![v(p), int(1)]), int(0));
        rejected(check(vec![p], body), "Uniform word where a Raw one");
    }

    #[test]
    fn a_raw_binder_bound_to_a_call_result_is_rejected() {
        let p = tagged(1);
        let body = let_in(raw(2), call(vec![v(p)]), int(0));
        rejected(check(vec![p], body), "a call gives a Uniform word");
    }

    #[test]
    fn a_function_returning_a_raw_word_is_rejected() {
        let (p, x) = (tagged(1), raw(2));
        let body = let_in(
            x,
            prim(CorePrimOp::UnboxInt, vec![v(p)]),
            prim(CorePrimOp::IAdd, vec![v(x), int(1)]),
        );
        rejected(check(vec![p], body), "IAdd gives a Raw word");
    }

    #[test]
    fn an_unbox_takes_only_a_uniform_variable() {
        let x = raw(2);
        let body = let_in(
            x,
            prim(CorePrimOp::UnboxInt, vec![int(1)]),
            let_in(raw(3), prim(CorePrimOp::UnboxInt, vec![v(x)]), int(0)),
        );
        let messages = check(Vec::new(), body);
        rejected(messages.clone(), "UnboxInt takes Lit");
        rejected(messages, "UnboxInt takes Var");
    }

    #[test]
    fn an_int_parameter_is_rejected() {
        rejected(check(vec![raw(1)], int(0)), "a lambda parameter");
    }

    #[test]
    fn an_int_field_binder_is_rejected() {
        let (p, head, tail) = (tagged(1), raw(2), tagged(3));
        let alt = CoreAlt {
            pat: CorePat::Con {
                tag: CoreTag::Cons,
                fields: vec![CorePat::Var(head), CorePat::Var(tail)],
            },
            guard: None,
            rhs: int(0),
            span: s(),
        };
        let body =
            CoreExpr::Case { scrutinee: Box::new(v(p)), alts: vec![alt], join_ty: None, span: s() };
        rejected(check(vec![p], body), "a field binder");
    }

    #[test]
    fn a_variable_pattern_takes_its_scrutinees_rep() {
        let (p, x) = (tagged(1), raw(2));
        let alt = CoreAlt { pat: CorePat::Var(tagged(3)), guard: None, rhs: int(0), span: s() };
        let case =
            CoreExpr::Case { scrutinee: Box::new(v(x)), alts: vec![alt], join_ty: None, span: s() };
        let body = let_in(x, prim(CorePrimOp::UnboxInt, vec![v(p)]), case);
        rejected(check(vec![p], body), "but its scrutinee is Raw");
    }

    #[test]
    fn a_lambda_capturing_a_raw_word_is_rejected() {
        let (p, x) = (tagged(1), raw(2));
        let lambda = CoreExpr::lambda(vec![tagged(4)], prim(CorePrimOp::BoxInt, vec![v(x)]), s());
        let body =
            let_in(x, prim(CorePrimOp::UnboxInt, vec![v(p)]), let_in(tagged(3), lambda, int(0)));
        rejected(check(vec![p], body), "a lambda captures the raw word");
    }

    #[test]
    fn a_handler_clause_takes_uniform_parameters_and_captures() {
        let (p, x, arg, resume) = (tagged(1), raw(2), raw(3), binder(4, FluxRep::BoxedRep));
        let handler = CoreHandler {
            operation: Symbol::new(50),
            params: vec![arg],
            param_types: Vec::new(),
            state: None,
            state_ty: None,
            resume,
            resume_ty: None,
            body: prim(CorePrimOp::BoxInt, vec![v(x)]),
            span: s(),
        };
        let handle = CoreExpr::Handle {
            body: Box::new(int(0)),
            effect: Symbol::new(51),
            parameter: None,
            handlers: vec![handler],
            span: s(),
        };
        let body = let_in(x, prim(CorePrimOp::UnboxInt, vec![v(p)]), handle);
        let messages = check(vec![p], body);
        rejected(messages.clone(), "a handler clause parameter");
        rejected(messages, "a handler clause captures the raw word");
    }

    #[test]
    fn a_recursive_binder_is_uniform() {
        let f = raw(2);
        let body = CoreExpr::LetRec {
            var: f,
            rhs: Box::new(CoreExpr::lambda(Vec::new(), int(0), s())),
            body: Box::new(int(0)),
            span: s(),
        };
        rejected(check(Vec::new(), body), "a recursive binder");
    }

    #[test]
    fn a_global_is_uniform_even_when_its_binder_is_int() {
        // A top-level `let g = 0` can have an `IntRep` binder; it is stored
        // uniform, so passing it to a call needs no box.
        let body = call(vec![v(raw(GLOBAL))]);
        assert_eq!(check_with_global(raw(GLOBAL), Vec::new(), body), Vec::<String>::new());
    }
}
