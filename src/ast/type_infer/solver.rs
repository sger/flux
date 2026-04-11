use crate::ast::type_infer::{InferCtx, constraint::Constraint};

impl<'a> InferCtx<'a> {
    /// Solve all deferred constraints in emission order.
    ///
    /// The current engine solves constraints eagerly at the point of emission
    /// (inside `unify_with_context` and `constrain_call_effects`), so this
    /// method is a no-op today — `self.deferred_constraints` is always empty
    /// after inference completes.
    ///
    /// This entry point exists as a structural hook for future work:
    /// switching to deferred constraint solving requires only changing
    /// `record_constraint` to push instead of eagerly solving, then calling
    /// this method after inference.
    pub(super) fn solve_deferred_constraints(&mut self) {
        let constrains = std::mem::take(&mut self.deferred_constraints);
        for constraint in constrains {
            self.solve_one(constraint)
        }
    }

    /// Solve a single constraint by dispatching to the appropriate handler.
    fn solve_one(&mut self, constraint: Constraint) {
        match constraint {
            Constraint::Unify {
                t1,
                t2,
                span,
                context,
            } => {
                self.unify_with_context(&t1, &t2, span, context);
            }
            Constraint::EffectSubset {
                required,
                available,
                span,
            } => {
                self.constrain_call_effects(&required, &available, span);
            }
            Constraint::Class { .. } => {
                // Class constraints are recorded but not solved eagerly.
                // Step 4 (constraint solving) will resolve these at generalization time.
            }
        }
    }

    /// Record a constraint for future deferred solving.
    ///
    /// Currently unused — constraints are solved eagerly. When deferred mode
    /// is enabled, callers will use this instead of direct unification.
    #[allow(dead_code)]
    pub(super) fn defer_constraint(&mut self, constraint: Constraint) {
        self.deferred_constraints.push(constraint);
    }
}
