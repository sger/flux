//! Dictionary arguments at call sites, from evidence (0.0.8 plan, step 1e).
//!
//! A call takes its dictionaries from the solver's answer at the occurrence
//! that raised them, and nothing else:
//!
//! - a class-method call raised its predicate at the call. A dictionary
//!   parameter is projected for the method's slot; an instance's dictionary
//!   becomes a direct call to the instance's method, applied to the
//!   dictionaries the instance's context needs;
//! - a call to a constrained function raised its predicates at the callee,
//!   and passes their dictionaries ahead of its own arguments.
//!
//! A site with no usable evidence is an internal error naming it.
//!
//! Left as they are:
//!
//! - a bare reference to a constrained function, passed as a value rather
//!   than called, passes no dictionaries (KI-090, Phase 3a);
//! - infix operators stay `PrimOp`s. `desugar_operators` runs before
//!   inference, so a constrained body's operators are already method calls.

use crate::{
    ast::type_infer::constraint::WantedClassConstraintOrigin,
    core::CoreExpr,
    diagnostics::{Diagnostic, DiagnosticBuilder},
    source::position::Span,
    syntax::{
        Identifier,
        expression::{ExprId, Expression},
    },
    types::{evidence::EvidenceSite, translate::DictArg},
};

use super::AstLowerer;

impl AstLowerer<'_> {
    /// Lower the call `call_id` at `span` with its dictionaries from
    /// evidence.
    pub(super) fn lower_call_from_evidence(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        call_id: ExprId,
        span: Span,
    ) -> CoreExpr {
        let (func, mut args) = if self.raised_method_call(call_id) {
            self.method_call_from_evidence(function, call_id, span)
        } else {
            if let Some(method) = self.unraised_method_call(function, call_id) {
                let method = self
                    .interner
                    .and_then(|interner| interner.try_resolve(method))
                    .unwrap_or("<unnamed>");
                self.site_error(
                    span,
                    &format!("the call to the class method `{method}` raised no predicate"),
                );
            }
            self.constrained_call_from_evidence(function, arguments)
        };
        args.extend(arguments.iter().map(|argument| self.lower_expr(argument)));
        CoreExpr::App {
            func: Box::new(func),
            args,
            span,
        }
    }

    /// The method a call inference classified as a class-method call names,
    /// when the call raised no predicate for it.
    pub(super) fn unraised_method_call(
        &self,
        function: &Expression,
        call_id: ExprId,
    ) -> Option<Identifier> {
        let evidence = self.evidence?;
        if !evidence.is_method_call(call_id) || self.raised_method_call(call_id) {
            return None;
        }
        match function {
            Expression::Identifier { name, .. } => Some(*name),
            Expression::MemberAccess { member, .. } => Some(*member),
            _ => None,
        }
    }

    /// Whether the call `call_id` raised its method's predicate.
    pub(super) fn raised_method_call(&self, call_id: ExprId) -> bool {
        self.evidence
            .and_then(|evidence| evidence.predicate(&EvidenceSite::new(call_id, 0)))
            .is_some_and(|predicate| predicate.origin == WantedClassConstraintOrigin::MethodCall)
    }

    /// The function a class-method call applies, and the dictionaries it
    /// passes ahead of the call's own arguments.
    fn method_call_from_evidence(
        &mut self,
        function: &Expression,
        call_id: ExprId,
        span: Span,
    ) -> (CoreExpr, Vec<CoreExpr>) {
        let fallback = |this: &mut Self| (this.lower_expr(function), Vec::new());
        let method = match function {
            Expression::Identifier { name, .. } => *name,
            Expression::MemberAccess { member, .. } => *member,
            _ => {
                self.site_error(span, "a method call whose callee is not a name");
                return fallback(self);
            }
        };
        let Some(dictionaries) = self.site_dict_args(call_id, span) else {
            return fallback(self);
        };
        let (Some(evidence), Some(class_env), Some(interner)) =
            (self.evidence, self.class_env, self.interner)
        else {
            return fallback(self);
        };
        let Some(predicate) = evidence.predicate(&EvidenceSite::new(call_id, 0)) else {
            return fallback(self);
        };
        match dictionaries.as_slice() {
            // A marker class: no dictionary to read the method from.
            [] => fallback(self),
            [dictionary @ DictArg::Param { .. }] => {
                let Some(slot) = class_env.method_slot(predicate.class_id, method) else {
                    self.site_error(span, "the method has no slot in its class's dictionary");
                    return fallback(self);
                };
                let Some(object) = self.render_dict_arg(dictionary, span) else {
                    self.site_error(span, "the dictionary parameter it names has no binder");
                    return fallback(self);
                };
                let func = CoreExpr::TupleField {
                    object: Box::new(object),
                    index: slot,
                    span,
                };
                (func, Vec::new())
            }
            [DictArg::Global { instance }] | [DictArg::Applied { instance, .. }] => {
                let mangled = crate::types::class_env::mangled_method_name(
                    instance.class_id,
                    &instance.dict_type_key,
                    interner.resolve(method),
                    interner,
                );
                let Some(mangled) = interner.lookup(&mangled) else {
                    self.site_error(
                        span,
                        &format!("no instance method `{mangled}` was generated"),
                    );
                    return fallback(self);
                };
                let context = match &dictionaries[0] {
                    DictArg::Applied { context, .. } => context.as_slice(),
                    _ => &[],
                };
                let Some(context) = context
                    .iter()
                    .map(|arg| self.render_dict_arg(arg, span))
                    .collect::<Option<Vec<_>>>()
                else {
                    self.site_error(
                        span,
                        "a dictionary its instance's context needs is unbuildable",
                    );
                    return fallback(self);
                };
                (CoreExpr::external_var(mangled, span), context)
            }
            _ => {
                self.site_error(span, "a method call answered with more than one dictionary");
                fallback(self)
            }
        }
    }

    /// The callee of a call that is not a method call, and the dictionaries
    /// its predicates ask for.
    ///
    /// A call that already passes `__dict_*` variables first is a forwarder:
    /// generated code handing on its own dictionaries. Its predicates must be
    /// answered by exactly those, and nothing is added.
    fn constrained_call_from_evidence(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> (CoreExpr, Vec<CoreExpr>) {
        let callee = function.expr_id();
        let callee_span = function.span();
        let raised = self.evidence.map_or(0, |evidence| evidence.raised(callee));
        let func = self.lower_expr(function);
        // A dictionary clone stands for the dictionaries this call would pass.
        if let CoreExpr::Var { var, .. } = &func
            && self.dictless_clones.contains(&var.name)
        {
            return (func, Vec::new());
        }
        if raised == 0 {
            // A call to an enclosing definition that takes dictionaries: its
            // own recursive call, with no declared signature to raise them.
            if let Expression::Identifier {
                name: callee_name, ..
            } = function
                && self.constrained_bodies.contains(callee_name)
            {
                let callee_name = *callee_name;
                let callee_name = self
                    .interner
                    .and_then(|interner| interner.try_resolve(callee_name))
                    .unwrap_or("<unnamed>");
                self.site_error(
                    callee_span,
                    &format!(
                        "it calls `{callee_name}`, which takes dictionaries, but raised no \
                         predicate there (KI-101)"
                    ),
                );
            }
            return (func, Vec::new());
        }
        let Some(dictionaries) = self.site_dict_args(callee, callee_span) else {
            return (func, Vec::new());
        };
        let Some(rendered) = dictionaries
            .iter()
            .map(|arg| self.render_dict_arg(arg, callee_span))
            .collect::<Option<Vec<_>>>()
        else {
            self.site_error(
                callee_span,
                "a dictionary its evidence names is unbuildable",
            );
            return (func, Vec::new());
        };

        let forwarded = self.forwarded_dictionaries(arguments);
        if forwarded.is_empty() {
            return (func, rendered);
        }
        let rendered_names = rendered
            .iter()
            .map(|dictionary| match dictionary {
                CoreExpr::Var { var, .. } => Some(var.name),
                _ => None,
            })
            .collect::<Vec<_>>();
        if rendered_names != forwarded.iter().copied().map(Some).collect::<Vec<_>>() {
            self.site_error(
                callee_span,
                "a forwarder passes dictionaries other than the ones evidence names",
            );
        }
        (func, Vec::new())
    }

    /// The leading arguments that are dictionary variables.
    fn forwarded_dictionaries(&self, arguments: &[Expression]) -> Vec<Identifier> {
        let Some(interner) = self.interner else {
            return Vec::new();
        };
        arguments
            .iter()
            .map_while(|argument| match argument {
                Expression::Identifier { name, .. }
                    if crate::types::class_env::is_dictionary_name(interner.resolve(*name)) =>
                {
                    Some(*name)
                }
                _ => None,
            })
            .collect()
    }

    /// The dictionaries the occurrence `id` at `span` passes, or `None` after
    /// reporting why it has none: its predicates were raised by another
    /// expression, or one has no answer that can be built.
    fn site_dict_args(&mut self, id: ExprId, span: Span) -> Option<Vec<DictArg>> {
        let evidence = self.evidence?;
        if let Some(predicate) = evidence.predicate(&EvidenceSite::new(id, 0))
            && predicate.span.start != span.start
        {
            let raised_at = predicate.span.start;
            self.site_error(
                span,
                &format!(
                    "its evidence was raised at {}:{}, by another expression",
                    raised_at.line, raised_at.column
                ),
            );
            return None;
        }
        let mut args = evidence.dict_args_at(id);
        // Inside a dictionary clone, the function's own parameters are the
        // known dictionaries the clone stands for (step 2a-3b).
        if let (Some(found), Some((owner, dicts))) = (args.as_mut(), self.clone_dicts.as_ref()) {
            for arg in found.iter_mut() {
                *arg = super::specialize::resolve_dict_arg(arg, *owner, dicts, self.class_env);
            }
            if found
                .iter()
                .any(|arg| super::specialize::names_param_of(arg, *owner))
            {
                self.site_error(
                    span,
                    "a dictionary clone reads a dictionary its known ones do not resolve",
                );
                return None;
            }
        }
        if args.is_none() {
            let classes = (0..evidence.raised(id))
                .filter_map(|index| evidence.predicate(&EvidenceSite::new(id, index)))
                .filter_map(|predicate| {
                    self.interner
                        .and_then(|interner| interner.try_resolve(predicate.class_name))
                })
                .collect::<Vec<_>>()
                .join(", ");
            self.site_error(
                span,
                &format!("a predicate it raised ({classes}) has no answer that can be built"),
            );
        }
        args
    }

    /// Record that the site at `span` cannot be lowered from its evidence.
    pub(super) fn site_error(&mut self, span: Span, problem: &str) {
        let error: Diagnostic = crate::ice!(format!(
            "lowering the site at {}:{} from its evidence: {problem}",
            span.start.line, span.start.column
        ))
        .with_span(span);
        self.errors.push(error);
    }
}
