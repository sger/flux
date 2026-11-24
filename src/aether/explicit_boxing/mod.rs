//! Makes every conversion between a raw Int and a uniform word explicit,
//! before borrow inference and Dup/Drop insertion, so that Aether sees every
//! Int box (Lean's ExplicitBoxing, run before its RC pass).
//!
//! A binder with `FluxRep::IntRep` holds a raw Int. Every other position
//! takes the uniform word: call arguments and results, fields, captures,
//! and the arguments of every primop but Int arithmetic. Until the native
//! backend makes Ints raw, `BoxInt` and the unboxes are copies on both
//! backends.
//!
//! A parameter, pattern field, handler parameter or capture that holds an
//! Int stays a uniform word (a *source*). Its raw copy is made once, at the
//! top of the scope, and only when something needs it raw; uniform uses
//! read the source itself, so a field passed on unchanged is not reboxed.

pub(crate) mod check;
pub(crate) mod reps;

use std::collections::{HashMap, HashSet};

use reps::{Rep, free_locals, gives_raw, int_op, matches_structure, takes_raw};

use crate::core::{
    CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreProgram,
    CoreVarRef, FluxRep, passes::primop_result_rep,
};
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticPhase, compiler_errors::IR_VERIFY_FAILURE,
    diagnostic_for, position::Span,
};
use crate::syntax::{Identifier, interner::Interner, symbol::Symbol};

/// Names of the binders this pass makes, after ANF's 4_000_000 and
/// evidence's 6_000_000 ranges.
const SYNTHETIC_NAMES: u32 = 8_000_000;

/// Runs the pass. Under `--verify-each`, Core lint and the boxing checker
/// then check its output, and a failure stops compilation with E998 or E997.
#[allow(clippy::result_large_err)]
pub(crate) fn run(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
) -> Result<(), Diagnostic> {
    explicit_boxing(program);
    if !crate::shared::verify_each::is_active() {
        return Ok(());
    }
    crate::core::passes::core_lint_stage(program, PASS, interner)?;
    check::check_program(program).map_err(|violations| {
        let detail = violations
            .iter()
            .map(|violation| {
                let def = interner.and_then(|i| i.try_resolve(violation.def)).unwrap_or("?");
                format!("  in `{def}`: {}", violation.message)
            })
            .collect::<Vec<_>>()
            .join("\n");
        diagnostic_for(&IR_VERIFY_FAILURE)
            .with_message(format!("The Core IR is invalid after `{PASS}`:\n{detail}"))
            .with_phase(DiagnosticPhase::Validation)
    })
}

const PASS: &str = "explicit_boxing";

/// Makes every Int box and unbox in `program` explicit. Dictionaries are left
/// alone: later passes match their shape.
pub(crate) fn explicit_boxing(program: &mut CoreProgram) {
    let globals: HashSet<CoreBinderId> = program.defs.iter().map(|def| def.binder.id).collect();
    let mut boxer = Boxer {
        next_id: reps::next_binder_id(program),
        globals,
        env: HashMap::new(),
        sources: Vec::new(),
    };
    for def in program.defs.iter_mut().filter(|def| !def.is_dict_def) {
        boxer.env.clear();
        boxer.sources.clear();
        let span = def.expr.span();
        let expr = std::mem::replace(&mut def.expr, CoreExpr::Lit(CoreLit::Unit, span));
        def.expr = boxer.def_body(expr);
    }
}

/// How a local variable is held.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Raw(Identifier),
    Uniform,
    /// A uniform word holding an Int, with a raw copy made on demand.
    Source(usize),
}

struct Source {
    tagged: CoreBinder,
    raw: Option<CoreBinder>,
    /// Whether anything reads the uniform word itself, which rules out
    /// consuming it to make the raw copy.
    read_uniform: bool,
    /// A function parameter: its raw copy may consume it, which makes the
    /// parameter owned, so a caller's fresh box moves in.
    movable: bool,
    /// Whether its type says Int. A uniform parameter of another type is a
    /// source only for the raw reads Int operations already make of it.
    int: bool,
}

type Lets = Vec<(CoreBinder, CoreExpr)>;

struct Boxer {
    next_id: u32,
    globals: HashSet<CoreBinderId>,
    env: HashMap<CoreBinderId, Slot>,
    sources: Vec<Source>,
}

impl Boxer {
    fn def_body(&mut self, expr: CoreExpr) -> CoreExpr {
        match expr {
            CoreExpr::Lam { params, param_types, result_ty, body, span } => {
                let mut lets = Vec::new();
                let lam = self.lam(params, param_types, result_ty, *body, span, &mut lets);
                wrap(lets, lam)
            }
            other => self.expr(other, Rep::Uniform),
        }
    }

    fn mint(&mut self, rep: FluxRep, name: Option<Identifier>) -> CoreBinder {
        let id = self.next_id;
        self.next_id += 1;
        let name = name.unwrap_or_else(|| Symbol::new(SYNTHETIC_NAMES + id));
        let binder = CoreBinder::with_rep(CoreBinderId(id), name, rep);
        self.bind(&binder);
        binder
    }

    fn slot(&self, var: &CoreVarRef) -> Slot {
        match var.binder {
            Some(id) if !self.globals.contains(&id) => {
                self.env.get(&id).copied().unwrap_or(Slot::Uniform)
            }
            _ => Slot::Uniform,
        }
    }

    fn bind(&mut self, binder: &CoreBinder) {
        let slot =
            if binder.rep == FluxRep::IntRep { Slot::Raw(binder.name) } else { Slot::Uniform };
        self.env.insert(binder.id, slot);
    }

    /// Retags an `IntRep` source binder as a uniform word, and returns the
    /// index of its source.
    fn source(&mut self, binder: &mut CoreBinder, movable: bool) -> Option<usize> {
        if binder.rep != FluxRep::IntRep {
            self.env.insert(binder.id, Slot::Uniform);
            return None;
        }
        binder.rep = FluxRep::TaggedRep;
        let index = self.new_source(*binder, movable);
        self.env.insert(binder.id, Slot::Source(index));
        Some(index)
    }

    /// A function parameter. A uniform one that `I*` operations read is an
    /// Int too, though its binder doesn't say so: its raw reads share one
    /// unbox at entry, as an `IntRep` parameter's do, so a parameter read
    /// only raw is owned and a self tail call passing a fresh box stays one.
    fn param_source(&mut self, binder: &mut CoreBinder) -> Option<usize> {
        if binder.rep != FluxRep::TaggedRep {
            return self.source(binder, true);
        }
        self.sources.push(Source {
            tagged: *binder,
            raw: None,
            read_uniform: false,
            movable: true,
            int: false,
        });
        let index = self.sources.len() - 1;
        self.env.insert(binder.id, Slot::Source(index));
        Some(index)
    }

    fn new_source(&mut self, tagged: CoreBinder, movable: bool) -> usize {
        self.sources.push(Source { tagged, raw: None, read_uniform: false, movable, int: true });
        self.sources.len() - 1
    }

    fn raw_copy(&mut self, index: usize) -> CoreBinder {
        if let Some(raw) = self.sources[index].raw {
            return raw;
        }
        // The raw copy takes the source's name: when the copy consumed the
        // source, it is the only binder a debugger can show the value by.
        let name = self.sources[index].tagged.name;
        let raw = self.mint(FluxRep::IntRep, Some(name));
        self.sources[index].raw = Some(raw);
        raw
    }

    fn uniform_read(&mut self, index: usize) -> CoreBinder {
        self.sources[index].read_uniform = true;
        self.sources[index].tagged
    }

    /// The variable `var` as `want`, binding a conversion in `lets` when it
    /// needs one.
    fn var_as(&mut self, var: CoreVarRef, span: Span, want: Rep, lets: &mut Lets) -> CoreExpr {
        match (self.slot(&var), want) {
            (Slot::Raw(_), Rep::Raw) | (Slot::Uniform, Rep::Uniform) => CoreExpr::Var { var, span },
            (Slot::Raw(_), Rep::Uniform) => {
                let boxed = self.mint(FluxRep::TaggedRep, None);
                lets.push((boxed, prim(CorePrimOp::BoxInt, CoreExpr::Var { var, span }, span)));
                CoreExpr::bound_var(&boxed, span)
            }
            (Slot::Uniform, Rep::Raw) => {
                let raw = self.mint(FluxRep::IntRep, None);
                lets.push((raw, prim(CorePrimOp::UnboxInt, CoreExpr::Var { var, span }, span)));
                CoreExpr::bound_var(&raw, span)
            }
            (Slot::Source(index), Rep::Raw) => {
                let raw = self.raw_copy(index);
                CoreExpr::bound_var(&raw, span)
            }
            (Slot::Source(index), Rep::Uniform) => {
                let tagged = self.uniform_read(index);
                CoreExpr::bound_var(&tagged, span)
            }
        }
    }

    /// An argument: after ANF, a variable or a literal.
    fn atom(&mut self, expr: CoreExpr, want: Rep, lets: &mut Lets) -> CoreExpr {
        match expr {
            CoreExpr::Var { var, span } => self.var_as(var, span, want, lets),
            CoreExpr::Lit(..) => expr,
            other => self.expr(other, want),
        }
    }

    fn atoms(&mut self, args: Vec<CoreExpr>, want: Rep, lets: &mut Lets) -> Vec<CoreExpr> {
        args.into_iter().map(|arg| self.atom(arg, want, lets)).collect()
    }

    /// Whether a generic operator's arguments are all Ints, at least one of
    /// them a variable, so the Int operation can take its place.
    fn int_args(&self, args: &[CoreExpr]) -> bool {
        let mut any_var = false;
        let all_int = args.len() == 2
            && args.iter().all(|arg| match arg {
                CoreExpr::Lit(CoreLit::Int(_), _) => true,
                CoreExpr::Var { var, .. } => {
                    any_var = true;
                    match self.slot(var) {
                        Slot::Raw(_) => true,
                        Slot::Source(index) => self.sources[index].int,
                        Slot::Uniform => false,
                    }
                }
                _ => false,
            });
        all_int && any_var
    }

    /// The Int operation a generic operator becomes, if it does.
    fn typed_op(&self, op: CorePrimOp, args: &[CoreExpr]) -> CorePrimOp {
        match int_op(op) {
            Some(int) if self.int_args(args) => int,
            _ => op,
        }
    }

    /// `expr`, whose value is `have`, converted to `want`. `fresh` says
    /// whether Aether owns the value, so an unbox can consume it.
    fn convert(&mut self, expr: CoreExpr, have: Rep, want: Rep, fresh: bool) -> CoreExpr {
        let span = expr.span();
        match (have, want) {
            (Rep::Raw, Rep::Raw) | (Rep::Uniform, Rep::Uniform) => expr,
            (Rep::Raw, Rep::Uniform) => {
                let raw = self.mint(FluxRep::IntRep, None);
                let boxed = prim(CorePrimOp::BoxInt, CoreExpr::bound_var(&raw, span), span);
                let_in(raw, expr, boxed, span)
            }
            (Rep::Uniform, Rep::Raw) => {
                let tagged = self.mint(FluxRep::TaggedRep, None);
                let op = if fresh { CorePrimOp::UnboxIntMove } else { CorePrimOp::UnboxInt };
                let raw = prim(op, CoreExpr::bound_var(&tagged, span), span);
                let_in(tagged, expr, raw, span)
            }
        }
    }

    fn expr(&mut self, expr: CoreExpr, want: Rep) -> CoreExpr {
        match expr {
            CoreExpr::Var { var, span } => match (self.slot(&var), want) {
                (Slot::Raw(_), Rep::Uniform) => {
                    prim(CorePrimOp::BoxInt, CoreExpr::Var { var, span }, span)
                }
                (Slot::Uniform, Rep::Raw) => {
                    prim(CorePrimOp::UnboxInt, CoreExpr::Var { var, span }, span)
                }
                _ => self.var_as(var, span, want, &mut Vec::new()),
            },
            CoreExpr::Lit(..) => expr,
            CoreExpr::Lam { params, param_types, result_ty, body, span } => {
                let mut lets = Vec::new();
                let lam = self.lam(params, param_types, result_ty, *body, span, &mut lets);
                wrap(lets, self.convert(lam, Rep::Uniform, want, true))
            }
            CoreExpr::App { func, args, span } => {
                let mut lets = Vec::new();
                let args = self.atoms(args, Rep::Uniform, &mut lets);
                let fresh =
                    matches!(func.as_ref(), CoreExpr::Var { var, .. } if var.binder.is_some());
                let call = CoreExpr::App { func, args, span };
                wrap(lets, self.convert(call, Rep::Uniform, want, fresh))
            }
            CoreExpr::Con { tag, fields, span } => {
                let mut lets = Vec::new();
                let fields = self.atoms(fields, Rep::Uniform, &mut lets);
                wrap(
                    lets,
                    self.convert(CoreExpr::Con { tag, fields, span }, Rep::Uniform, want, true),
                )
            }
            CoreExpr::PrimOp { op, args, span } => {
                let op = self.typed_op(op, &args);
                let arg_want = if takes_raw(op) { Rep::Raw } else { Rep::Uniform };
                let mut lets = Vec::new();
                let args = self.atoms(args, arg_want, &mut lets);
                let have = if gives_raw(op) { Rep::Raw } else { Rep::Uniform };
                let fresh = op == CorePrimOp::BoxInt;
                wrap(lets, self.convert(CoreExpr::PrimOp { op, args, span }, have, want, fresh))
            }
            CoreExpr::MemberAccess { object, member, span } => {
                let mut lets = Vec::new();
                let object = Box::new(self.atom(*object, Rep::Uniform, &mut lets));
                let read = CoreExpr::MemberAccess { object, member, span };
                wrap(lets, self.convert(read, Rep::Uniform, want, false))
            }
            CoreExpr::TupleField { object, index, span } => {
                let mut lets = Vec::new();
                let object = Box::new(self.atom(*object, Rep::Uniform, &mut lets));
                let read = CoreExpr::TupleField { object, index, span };
                wrap(lets, self.convert(read, Rep::Uniform, want, false))
            }
            // A function's result is uniform, whatever the context of the
            // `return` wants, and control does not come back.
            CoreExpr::Return { value, span } => {
                CoreExpr::Return { value: Box::new(self.expr(*value, Rep::Uniform)), span }
            }
            CoreExpr::Perform { effect, operation, args, span } => {
                let mut lets = Vec::new();
                let args = self.atoms(args, Rep::Uniform, &mut lets);
                let perform = CoreExpr::Perform { effect, operation, args, span };
                wrap(lets, self.convert(perform, Rep::Uniform, want, false))
            }
            CoreExpr::Let { mut var, rhs, body, span } => {
                if var.rep == FluxRep::TaggedRep
                    && let CoreExpr::PrimOp { op, args, .. } = rhs.as_ref()
                    && int_op(*op).is_some()
                    && self.int_args(args)
                {
                    var.rep = primop_result_rep(&self.typed_op(*op, args));
                }
                // An Int a call or a read hands back stays the uniform word it
                // came as, and is unboxed only for its raw uses: a binder
                // that only goes back out (`let r = f(x); r`) must leave the
                // call last, or it is no longer a tail call.
                if var.rep == FluxRep::IntRep
                    && let Some(fresh) = self.uniform_producer(&rhs)
                {
                    let mut lets = Vec::new();
                    let rhs = self.rhs(*rhs, Rep::Uniform, &mut lets);
                    let source = self.source(&mut var, fresh);
                    let body = self.expr(*body, want);
                    let body = self.with_raw_copies(source.as_slice(), body);
                    return wrap(
                        lets,
                        CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span },
                    );
                }
                let rhs_want = if var.rep == FluxRep::IntRep { Rep::Raw } else { Rep::Uniform };
                let mut lets = Vec::new();
                let rhs = self.rhs(*rhs, rhs_want, &mut lets);
                self.bind(&var);
                let body = self.expr(*body, want);
                wrap(lets, CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span })
            }
            CoreExpr::LetRec { mut var, rhs, body, span } => {
                var.rep = uniform(var.rep);
                self.bind(&var);
                let mut lets = Vec::new();
                let rhs = self.rhs(*rhs, Rep::Uniform, &mut lets);
                let body = self.expr(*body, want);
                wrap(lets, CoreExpr::LetRec { var, rhs: Box::new(rhs), body: Box::new(body), span })
            }
            CoreExpr::LetRecGroup { bindings, body, span } => {
                let bindings: Vec<_> = bindings
                    .into_iter()
                    .map(|(mut binder, rhs)| {
                        binder.rep = uniform(binder.rep);
                        self.bind(&binder);
                        (binder, rhs)
                    })
                    .collect();
                let mut lets = Vec::new();
                let bindings = bindings
                    .into_iter()
                    .map(|(binder, rhs)| {
                        (binder, Box::new(self.rhs(*rhs, Rep::Uniform, &mut lets)))
                    })
                    .collect();
                let body = Box::new(self.expr(*body, want));
                wrap(lets, CoreExpr::LetRecGroup { bindings, body, span })
            }
            CoreExpr::Case { scrutinee, alts, join_ty, span } => {
                self.case(*scrutinee, alts, join_ty, span, want)
            }
            CoreExpr::Handle { body, effect, parameter, handlers, span } => {
                let mut lets = Vec::new();
                let parameter = parameter.map(|p| Box::new(self.expr(*p, Rep::Uniform)));
                let body = Box::new(self.expr(*body, Rep::Uniform));
                let handlers =
                    handlers.into_iter().map(|handler| self.clause(handler, &mut lets)).collect();
                let handle = CoreExpr::Handle { body, effect, parameter, handlers, span };
                wrap(lets, self.convert(handle, Rep::Uniform, want, false))
            }
        }
    }

    /// Whether `expr` gives a uniform word, and if so whether Aether owns it,
    /// so an unbox may consume it. A `let` chain gives what its tail gives.
    fn uniform_producer(&self, expr: &CoreExpr) -> Option<bool> {
        match expr {
            CoreExpr::App { func, .. } => {
                Some(matches!(func.as_ref(), CoreExpr::Var { var, .. } if var.binder.is_some()))
            }
            CoreExpr::Con { .. } => Some(true),
            CoreExpr::PrimOp { op, args, .. } => {
                let op = self.typed_op(*op, args);
                (!gives_raw(op)).then_some(op == CorePrimOp::BoxInt)
            }
            CoreExpr::Perform { .. }
            | CoreExpr::Handle { .. }
            | CoreExpr::MemberAccess { .. }
            | CoreExpr::TupleField { .. } => Some(false),
            CoreExpr::Let { body, .. }
            | CoreExpr::LetRec { body, .. }
            | CoreExpr::LetRecGroup { body, .. } => self.uniform_producer(body),
            _ => None,
        }
    }

    /// A right-hand side. A lambda's capture boxes go in `lets`, before the
    /// binding, so a recursive binding keeps a lambda as its right-hand side.
    fn rhs(&mut self, rhs: CoreExpr, want: Rep, lets: &mut Lets) -> CoreExpr {
        match rhs {
            CoreExpr::Lam { params, param_types, result_ty, body, span } => {
                let lam = self.lam(params, param_types, result_ty, *body, span, lets);
                self.convert(lam, Rep::Uniform, want, true)
            }
            other => self.expr(other, want),
        }
    }

    fn case(
        &mut self,
        scrutinee: CoreExpr,
        alts: Vec<crate::core::CoreAlt>,
        join_ty: Option<crate::core::CoreType>,
        span: Span,
        want: Rep,
    ) -> CoreExpr {
        let mut lets = Vec::new();
        let scrutinee = if matches_structure(&alts) {
            self.atom(scrutinee, Rep::Uniform, &mut lets)
        } else {
            match scrutinee {
                // An Int matched against literals is read raw, so a parameter
                // matched and then counted down needs no uniform read.
                CoreExpr::Var { var, span } => match self.slot(&var) {
                    Slot::Source(index) if matches_int_literals(&alts) => {
                        CoreExpr::bound_var(&self.raw_copy(index), span)
                    }
                    Slot::Source(index) => CoreExpr::bound_var(&self.uniform_read(index), span),
                    _ => CoreExpr::Var { var, span },
                },
                other => self.expr(other, Rep::Uniform),
            }
        };
        let scrutinee_raw = matches!(
            &scrutinee,
            CoreExpr::Var { var, .. } if matches!(self.slot(var), Slot::Raw(_))
        );
        let alts = alts
            .into_iter()
            .map(|mut alt| {
                let mut sources = Vec::new();
                match &mut alt.pat {
                    CorePat::Var(binder) if scrutinee_raw => {
                        binder.rep = FluxRep::IntRep;
                        self.bind(binder);
                    }
                    pat => self.pattern(pat, &mut sources),
                }
                alt.guard = alt.guard.map(|guard| {
                    let guard = self.expr(guard, Rep::Uniform);
                    self.with_raw_copies(&sources, guard)
                });
                let rhs = self.expr(alt.rhs, want);
                alt.rhs = self.with_raw_copies(&sources, rhs);
                alt
            })
            .collect();
        wrap(lets, CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, join_ty, span })
    }

    /// Binds a pattern's variables. Each one holds a uniform word: a field,
    /// or the scrutinee itself when it is uniform.
    fn pattern(&mut self, pat: &mut CorePat, sources: &mut Vec<usize>) {
        match pat {
            CorePat::Var(binder) => sources.extend(self.source(binder, false)),
            CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
                for field in fields {
                    self.pattern(field, sources);
                }
            }
            CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
        }
    }

    fn lam(
        &mut self,
        mut params: Vec<CoreBinder>,
        param_types: Vec<Option<crate::core::CoreType>>,
        result_ty: Option<crate::core::CoreType>,
        body: CoreExpr,
        span: Span,
        lets: &mut Lets,
    ) -> CoreExpr {
        let bound: Vec<_> = params.iter().map(|param| param.id).collect();
        let (restore, mut sources) = self.capture(&body, &bound, span, lets);
        for param in &mut params {
            sources.extend(self.param_source(param));
        }
        let body = self.expr(body, Rep::Uniform);
        let body = self.with_raw_copies(&sources, body);
        self.env.extend(restore);
        CoreExpr::Lam { params, param_types, result_ty, body: Box::new(body), span }
    }

    fn clause(&mut self, mut handler: CoreHandler, lets: &mut Lets) -> CoreHandler {
        let bound: Vec<_> = handler
            .params
            .iter()
            .chain(handler.state.iter())
            .chain(std::iter::once(&handler.resume))
            .map(|binder| binder.id)
            .collect();
        let (restore, mut sources) = self.capture(&handler.body, &bound, handler.span, lets);
        // The clause's parameters are lent to it, so their raw copies borrow.
        for param in &mut handler.params {
            sources.extend(self.source(param, false));
        }
        if let Some(state) = &mut handler.state {
            sources.extend(self.source(state, false));
        }
        self.bind(&handler.resume);
        let body = std::mem::replace(&mut handler.body, CoreExpr::Lit(CoreLit::Unit, handler.span));
        let body = self.expr(body, Rep::Uniform);
        handler.body = self.with_raw_copies(&sources, body);
        self.env.extend(restore);
        handler
    }

    /// Makes the Ints a closure body reads from outside into sources of its
    /// own. A raw Int is boxed before the closure (in `lets`), since the
    /// closure's environment holds uniform words. Returns the slots to
    /// restore after the body, and the new sources.
    fn capture(
        &mut self,
        body: &CoreExpr,
        bound: &[CoreBinderId],
        span: Span,
        lets: &mut Lets,
    ) -> (Vec<(CoreBinderId, Slot)>, Vec<usize>) {
        let mut free: Vec<_> = free_locals(body)
            .into_iter()
            .filter(|id| !bound.contains(id) && !self.globals.contains(id))
            .collect();
        free.sort();
        let mut restore = Vec::new();
        let mut sources = Vec::new();
        for id in free {
            let Some(&slot) = self.env.get(&id) else {
                continue;
            };
            let tagged = match slot {
                Slot::Uniform => continue,
                // The closure holds the parameter itself, as any uniform word.
                Slot::Source(outer) if !self.sources[outer].int => {
                    self.uniform_read(outer);
                    restore.push((id, slot));
                    self.env.insert(id, Slot::Uniform);
                    continue;
                }
                Slot::Raw(name) => {
                    let boxed = self.mint(FluxRep::TaggedRep, Some(name));
                    let raw = CoreExpr::Var { var: CoreVarRef { name, binder: Some(id) }, span };
                    lets.push((boxed, prim(CorePrimOp::BoxInt, raw, span)));
                    boxed
                }
                Slot::Source(outer) => self.uniform_read(outer),
            };
            let index = self.new_source(tagged, false);
            restore.push((id, slot));
            self.env.insert(id, Slot::Source(index));
            sources.push(index);
        }
        (restore, sources)
    }

    /// `body` after the raw copies its sources needed. A copy consumes its
    /// source when nothing else reads it and the source may be moved.
    fn with_raw_copies(&mut self, sources: &[usize], body: CoreExpr) -> CoreExpr {
        let span = body.span();
        let mut body = body;
        for &index in sources.iter().rev() {
            let source = &mut self.sources[index];
            let Some(raw) = source.raw.take() else {
                continue;
            };
            let op = if source.movable && !source.read_uniform {
                CorePrimOp::UnboxIntMove
            } else {
                CorePrimOp::UnboxInt
            };
            let tagged = CoreExpr::bound_var(&source.tagged, span);
            body = let_in(raw, prim(op, tagged, span), body, span);
        }
        body
    }
}

fn matches_int_literals(alts: &[crate::core::CoreAlt]) -> bool {
    alts.iter().any(|alt| matches!(alt.pat, CorePat::Lit(CoreLit::Int(_))))
}

fn uniform(rep: FluxRep) -> FluxRep {
    if rep == FluxRep::IntRep { FluxRep::TaggedRep } else { rep }
}

fn prim(op: CorePrimOp, arg: CoreExpr, span: Span) -> CoreExpr {
    CoreExpr::PrimOp { op, args: vec![arg], span }
}

fn let_in(var: CoreBinder, rhs: CoreExpr, body: CoreExpr, span: Span) -> CoreExpr {
    CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span }
}

fn wrap(lets: Lets, body: CoreExpr) -> CoreExpr {
    let span = body.span();
    lets.into_iter().rev().fold(body, |body, (var, rhs)| let_in(var, rhs, body, span))
}

#[cfg(test)]
mod tests;
