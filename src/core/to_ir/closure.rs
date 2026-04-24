use std::collections::{HashMap, HashSet};

use crate::{
    aether::{AetherExpr, AetherHandler},
    cfg::{
        FunctionId, IrBlock, IrExpr, IrFunctionOrigin, IrInstr, IrMetadata, IrParam, IrTerminator,
        IrType, IrVar,
    },
    core::{CoreBinder, CoreBinderId, CoreExpr, CoreHandler},
    syntax::Identifier,
};

use super::free_vars::{collect_free_vars_core, free_vars_rec};

fn handler_resume_result_type(handler: &CoreHandler) -> Option<crate::core::CoreType> {
    match handler.resume_ty.as_ref() {
        Some(crate::core::CoreType::Function(_, ret_ty)) => Some((**ret_ty).clone()),
        Some(other) => Some(other.clone()),
        None => None,
    }
}

fn aether_handler_resume_result_type(handler: &AetherHandler) -> Option<crate::core::CoreType> {
    match handler.resume_ty.as_ref() {
        Some(crate::core::CoreType::Function(_, ret_ty)) => Some((**ret_ty).clone()),
        Some(other) => Some(other.clone()),
        None => None,
    }
}

fn collect_used_candidate_binders(
    expr: &CoreExpr,
    bound: &mut HashSet<CoreBinderId>,
    candidates: &HashSet<CoreBinderId>,
    used: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(binder) = var.binder
                && candidates.contains(&binder)
                && !bound.contains(&binder)
            {
                used.insert(binder);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .cloned()
                .collect();
            collect_used_candidate_binders(body, bound, candidates, used);
            for p in new_params {
                bound.remove(&p.id);
            }
        }
        CoreExpr::App { func, args, .. } => {
            collect_used_candidate_binders(func, bound, candidates, used);
            for arg in args {
                collect_used_candidate_binders(arg, bound, candidates, used);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            collect_used_candidate_binders(rhs, bound, candidates, used);
            let is_new = bound.insert(var.id);
            collect_used_candidate_binders(body, bound, candidates, used);
            if is_new {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let is_new = bound.insert(var.id);
            collect_used_candidate_binders(rhs, bound, candidates, used);
            collect_used_candidate_binders(body, bound, candidates, used);
            if is_new {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            let new_binders: Vec<_> = bindings
                .iter()
                .filter(|(var, _)| bound.insert(var.id))
                .map(|(var, _)| var.id)
                .collect();
            for (_, rhs) in bindings {
                collect_used_candidate_binders(rhs, bound, candidates, used);
            }
            collect_used_candidate_binders(body, bound, candidates, used);
            for id in new_binders {
                bound.remove(&id);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_used_candidate_binders(scrutinee, bound, candidates, used);
            for alt in alts {
                let mut alt_bound = HashSet::new();
                super::free_vars::collect_pat_binders(&alt.pat, &mut alt_bound);
                let new_binders: Vec<_> = alt_bound
                    .iter()
                    .filter(|binder| bound.insert(**binder))
                    .cloned()
                    .collect();
                if let Some(guard) = &alt.guard {
                    collect_used_candidate_binders(guard, bound, candidates, used);
                }
                collect_used_candidate_binders(&alt.rhs, bound, candidates, used);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        CoreExpr::Con { fields, .. } => {
            for field in fields {
                collect_used_candidate_binders(field, bound, candidates, used);
            }
        }
        CoreExpr::Return { value, .. } => {
            collect_used_candidate_binders(value, bound, candidates, used);
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_used_candidate_binders(arg, bound, candidates, used);
            }
        }
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                collect_used_candidate_binders(parameter, bound, candidates, used);
            }
            collect_used_candidate_binders(body, bound, candidates, used);
            for handler in handlers {
                let mut new_binders = Vec::new();
                if bound.insert(handler.resume.id) {
                    new_binders.push(handler.resume.id);
                }
                for param in &handler.params {
                    if bound.insert(param.id) {
                        new_binders.push(param.id);
                    }
                }
                if let Some(state) = handler.state
                    && bound.insert(state.id)
                {
                    new_binders.push(state.id);
                }
                collect_used_candidate_binders(&handler.body, bound, candidates, used);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_used_candidate_binders(object, bound, candidates, used);
        }
    }
}

fn used_outer_binders(
    expr: &CoreExpr,
    initially_bound: impl IntoIterator<Item = CoreBinderId>,
    candidates: &HashSet<CoreBinderId>,
) -> HashSet<CoreBinderId> {
    let mut bound: HashSet<_> = initially_bound.into_iter().collect();
    let mut used = HashSet::new();
    collect_used_candidate_binders(expr, &mut bound, candidates, &mut used);
    used
}

impl<'a> super::fn_ctx::FnCtx<'a> {
    pub(super) fn lower_handler_arm_aether(
        &mut self,
        handler: &AetherHandler,
    ) -> (FunctionId, Vec<IrVar>) {
        let free = crate::aether::free_vars::collect_free_vars_aether(&handler.body);
        let used = used_outer_binders_aether(
            &handler.body,
            std::iter::once(handler.resume.id)
                .chain(handler.params.iter().map(|p| p.id))
                .chain(handler.state.as_ref().map(|s| s.id)),
            &free,
        );
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| used.contains(binder))
            .filter_map(|binder| {
                self.env
                    .get(&binder)
                    .map(|_| CoreBinder::new(binder, self.binder_names[&binder]))
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();
        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = super::fn_ctx::FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: None,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            };

            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Tagged,
                });
                sub.inferred_param_types.push(None);
            }

            let resume_var = sub.ctx.alloc_var();
            sub.env.insert(handler.resume.id, resume_var);
            sub.binder_names
                .insert(handler.resume.id, handler.resume.name);
            sub.params.push(IrParam {
                name: handler.resume.name,
                var: resume_var,
                ty: handler
                    .resume_ty
                    .as_ref()
                    .map(super::core_type_to_ir_type)
                    .unwrap_or(IrType::Tagged),
            });
            sub.inferred_param_types.push(handler.resume_ty.clone());
            for (i, p) in handler.params.iter().enumerate() {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: handler
                        .param_types
                        .get(i)
                        .and_then(|ty| ty.as_ref())
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types
                    .push(handler.param_types.get(i).cloned().unwrap_or(None));
            }
            if let Some(state) = &handler.state {
                let v = sub.ctx.alloc_var();
                sub.env.insert(state.id, v);
                sub.binder_names.insert(state.id, state.name);
                sub.params.push(IrParam {
                    name: state.name,
                    var: v,
                    ty: handler
                        .state_ty
                        .as_ref()
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types.push(handler.state_ty.clone());
            }
            sub.inferred_return_type = aether_handler_resume_result_type(handler);

            let ret = sub.lower_expr_aether(&handler.body);
            sub.finish_return(ret, handler.span);
        }

        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        (fn_id, capture_env.into_iter().map(|(_, var)| var).collect())
    }

    /// Lower a handler arm as a separate closure function.
    /// Parameters: [resume, param0, param1, ...] -- matches the VM calling convention.
    pub(super) fn lower_handler_arm(&mut self, handler: &CoreHandler) -> (FunctionId, Vec<IrVar>) {
        // Collect free variables in the arm body that are bound in the enclosing scope.
        let mut free = HashSet::new();
        free_vars_rec(&handler.body, &mut HashSet::new(), &mut free);
        let used = used_outer_binders(
            &handler.body,
            std::iter::once(handler.resume.id)
                .chain(handler.params.iter().map(|p| p.id))
                .chain(handler.state.as_ref().map(|s| s.id)),
            &free,
        );
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| used.contains(binder))
            .filter_map(|binder| {
                self.env
                    .get(&binder)
                    .map(|_| CoreBinder::new(binder, self.binder_names[&binder]))
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = super::fn_ctx::FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: None,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            };

            // Captures first (matching VM convention for closures).
            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Tagged,
                });
                sub.inferred_param_types.push(None);
            }
            // Resume param first, then operation params.
            let resume_var = sub.ctx.alloc_var();
            sub.env.insert(handler.resume.id, resume_var);
            sub.binder_names
                .insert(handler.resume.id, handler.resume.name);
            sub.params.push(IrParam {
                name: handler.resume.name,
                var: resume_var,
                ty: handler
                    .resume_ty
                    .as_ref()
                    .map(super::core_type_to_ir_type)
                    .unwrap_or(IrType::Tagged),
            });
            sub.inferred_param_types.push(handler.resume_ty.clone());
            for (i, p) in handler.params.iter().enumerate() {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: handler
                        .param_types
                        .get(i)
                        .and_then(|ty| ty.as_ref())
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types
                    .push(handler.param_types.get(i).cloned().unwrap_or(None));
            }
            if let Some(state) = &handler.state {
                let v = sub.ctx.alloc_var();
                sub.env.insert(state.id, v);
                sub.binder_names.insert(state.id, state.name);
                sub.params.push(IrParam {
                    name: state.name,
                    var: v,
                    ty: handler
                        .state_ty
                        .as_ref()
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types.push(handler.state_ty.clone());
            }
            sub.inferred_return_type = handler_resume_result_type(handler);

            let ret = sub.lower_expr(&handler.body);
            sub.finish_return(ret, handler.span);
        }

        // Record captures on the generated function.
        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        (fn_id, capture_env.into_iter().map(|(_, var)| var).collect())
    }

    /// Lower a `Lam` node appearing inside an expression as a closure.
    pub(super) fn lower_lam_as_closure(
        &mut self,
        forced_name: Option<Identifier>,
        recursive_binder: Option<CoreBinderId>,
        expr: &CoreExpr,
    ) -> IrVar {
        let CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } = expr
        else {
            panic!("lower_lam_as_closure: not a Lam");
        };

        // Compute free variables that need to be captured.
        let free = collect_free_vars_core(expr);
        let used = used_outer_binders(
            body,
            params.iter().map(|param| param.id).chain(recursive_binder),
            &free,
        );
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| Some(*binder) != recursive_binder)
            .filter(|binder| used.contains(binder))
            .filter_map(|binder| {
                self.env
                    .get(&binder)
                    .map(|_| CoreBinder::new(binder, self.binder_names[&binder]))
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = super::fn_ctx::FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: forced_name,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            };

            // Captures are the first params (matching how the VM expects them).
            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Tagged,
                });
                sub.inferred_param_types.push(None);
            }
            if let (Some(name), Some(binder_id)) = (forced_name, recursive_binder) {
                let self_capture_vars: Vec<IrVar> = captures
                    .iter()
                    .filter_map(|binder| sub.env.get(&binder.id).cloned())
                    .collect();
                let self_var = sub.ctx.alloc_var();
                sub.emit(IrInstr::Assign {
                    dest: self_var,
                    expr: IrExpr::MakeClosure(fn_id, self_capture_vars),
                    metadata: IrMetadata::from_span(*span),
                });
                sub.env.insert(binder_id, self_var);
                sub.binder_names.insert(binder_id, name);
            }
            for p in params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                let semantic_idx = sub
                    .inferred_param_types
                    .len()
                    .saturating_sub(capture_env.len());
                let inferred_ty = param_types.get(semantic_idx).cloned().unwrap_or(None);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: inferred_ty
                        .as_ref()
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types.push(inferred_ty);
            }
            sub.inferred_return_type = result_ty.clone();

            let ret = sub.lower_expr(body);
            sub.finish_return(ret, *span);
        }

        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        let capture_vars: Vec<IrVar> = capture_env
            .iter()
            .filter_map(|(b, _)| self.env.get(&b.id).cloned())
            .collect();

        let dest = self.ctx.alloc_var();
        self.emit(IrInstr::Assign {
            dest,
            expr: IrExpr::MakeClosure(fn_id, capture_vars),
            metadata: IrMetadata::from_span(*span),
        });
        dest
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_lam_as_closure_aether(
        &mut self,
        forced_name: Option<Identifier>,
        recursive_binder: Option<CoreBinderId>,
        params: &[CoreBinder],
        param_types: &[Option<crate::core::CoreType>],
        result_ty: Option<&crate::core::CoreType>,
        body: &AetherExpr,
        span: crate::diagnostics::position::Span,
        expr: &AetherExpr,
    ) -> IrVar {
        let free = crate::aether::free_vars::collect_free_vars_aether(expr);
        let used = used_outer_binders_aether(
            body,
            params.iter().map(|param| param.id).chain(recursive_binder),
            &free,
        );
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| Some(*binder) != recursive_binder)
            .filter(|binder| used.contains(binder))
            .filter_map(|binder| {
                self.env
                    .get(&binder)
                    .map(|_| CoreBinder::new(binder, self.binder_names[&binder]))
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();
        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = super::fn_ctx::FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: forced_name,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            };

            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Tagged,
                });
                sub.inferred_param_types.push(None);
            }
            if let (Some(name), Some(binder_id)) = (forced_name, recursive_binder) {
                let self_capture_vars: Vec<IrVar> = captures
                    .iter()
                    .filter_map(|binder| sub.env.get(&binder.id).cloned())
                    .collect();
                let self_var = sub.ctx.alloc_var();
                sub.emit(IrInstr::Assign {
                    dest: self_var,
                    expr: IrExpr::MakeClosure(fn_id, self_capture_vars),
                    metadata: IrMetadata::from_span(span),
                });
                sub.env.insert(binder_id, self_var);
                sub.binder_names.insert(binder_id, name);
            }
            for p in params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                let semantic_idx = sub
                    .inferred_param_types
                    .len()
                    .saturating_sub(capture_env.len());
                let inferred_ty = param_types.get(semantic_idx).cloned().unwrap_or(None);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: inferred_ty
                        .as_ref()
                        .map(super::core_type_to_ir_type)
                        .unwrap_or(IrType::Tagged),
                });
                sub.inferred_param_types.push(inferred_ty);
            }
            sub.inferred_return_type = result_ty.cloned();

            let ret = sub.lower_expr_aether(body);
            sub.finish_return(ret, span);
        }

        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        let capture_vars: Vec<IrVar> = capture_env
            .iter()
            .filter_map(|(b, _)| self.env.get(&b.id).cloned())
            .collect();

        let dest = self.ctx.alloc_var();
        self.emit(IrInstr::Assign {
            dest,
            expr: IrExpr::MakeClosure(fn_id, capture_vars),
            metadata: IrMetadata::from_span(span),
        });
        dest
    }
}

fn collect_used_candidate_binders_aether(
    expr: &AetherExpr,
    bound: &mut HashSet<CoreBinderId>,
    candidates: &HashSet<CoreBinderId>,
    used: &mut HashSet<CoreBinderId>,
) {
    match expr {
        AetherExpr::Var { var, .. } => {
            if let Some(binder) = var.binder
                && candidates.contains(&binder)
                && !bound.contains(&binder)
            {
                used.insert(binder);
            }
        }
        AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .cloned()
                .collect();
            collect_used_candidate_binders_aether(body, bound, candidates, used);
            for p in new_params {
                bound.remove(&p.id);
            }
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            collect_used_candidate_binders_aether(func, bound, candidates, used);
            for arg in args {
                collect_used_candidate_binders_aether(arg, bound, candidates, used);
            }
        }
        AetherExpr::Let { var, rhs, body, .. } => {
            collect_used_candidate_binders_aether(rhs, bound, candidates, used);
            let is_new = bound.insert(var.id);
            collect_used_candidate_binders_aether(body, bound, candidates, used);
            if is_new {
                bound.remove(&var.id);
            }
        }
        AetherExpr::LetRec { var, rhs, body, .. } => {
            let is_new = bound.insert(var.id);
            collect_used_candidate_binders_aether(rhs, bound, candidates, used);
            collect_used_candidate_binders_aether(body, bound, candidates, used);
            if is_new {
                bound.remove(&var.id);
            }
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            let new_binders: Vec<_> = bindings
                .iter()
                .filter(|(var, _)| bound.insert(var.id))
                .map(|(var, _)| var.id)
                .collect();
            for (_, rhs) in bindings {
                collect_used_candidate_binders_aether(rhs, bound, candidates, used);
            }
            collect_used_candidate_binders_aether(body, bound, candidates, used);
            for id in new_binders {
                bound.remove(&id);
            }
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_used_candidate_binders_aether(scrutinee, bound, candidates, used);
            for alt in alts {
                let mut alt_bound = HashSet::new();
                crate::aether::free_vars::collect_pat_binders(&alt.pat, &mut alt_bound);
                let new_binders: Vec<_> = alt_bound
                    .iter()
                    .filter(|binder| bound.insert(**binder))
                    .cloned()
                    .collect();
                if let Some(guard) = &alt.guard {
                    collect_used_candidate_binders_aether(guard, bound, candidates, used);
                }
                collect_used_candidate_binders_aether(&alt.rhs, bound, candidates, used);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        AetherExpr::Con { fields, .. }
        | AetherExpr::PrimOp { args: fields, .. }
        | AetherExpr::Perform { args: fields, .. } => {
            for field in fields {
                collect_used_candidate_binders_aether(field, bound, candidates, used);
            }
        }
        AetherExpr::Return { value, .. } => {
            collect_used_candidate_binders_aether(value, bound, candidates, used);
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                collect_used_candidate_binders_aether(parameter, bound, candidates, used);
            }
            collect_used_candidate_binders_aether(body, bound, candidates, used);
            for handler in handlers {
                let mut new_binders = Vec::new();
                if bound.insert(handler.resume.id) {
                    new_binders.push(handler.resume.id);
                }
                for param in &handler.params {
                    if bound.insert(param.id) {
                        new_binders.push(param.id);
                    }
                }
                if let Some(state) = &handler.state
                    && bound.insert(state.id)
                {
                    new_binders.push(state.id);
                }
                collect_used_candidate_binders_aether(&handler.body, bound, candidates, used);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            collect_used_candidate_binders_aether(object, bound, candidates, used);
        }
        AetherExpr::Dup { var, body, .. } | AetherExpr::Drop { var, body, .. } => {
            if let Some(binder) = var.binder
                && candidates.contains(&binder)
                && !bound.contains(&binder)
            {
                used.insert(binder);
            }
            collect_used_candidate_binders_aether(body, bound, candidates, used);
        }
        AetherExpr::Reuse { token, fields, .. } => {
            if let Some(binder) = token.binder
                && candidates.contains(&binder)
                && !bound.contains(&binder)
            {
                used.insert(binder);
            }
            for field in fields {
                collect_used_candidate_binders_aether(field, bound, candidates, used);
            }
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            if let Some(binder) = scrutinee.binder
                && candidates.contains(&binder)
                && !bound.contains(&binder)
            {
                used.insert(binder);
            }
            collect_used_candidate_binders_aether(unique_body, bound, candidates, used);
            collect_used_candidate_binders_aether(shared_body, bound, candidates, used);
        }
    }
}

fn used_outer_binders_aether(
    expr: &AetherExpr,
    initially_bound: impl IntoIterator<Item = CoreBinderId>,
    candidates: &HashSet<CoreBinderId>,
) -> HashSet<CoreBinderId> {
    let mut bound: HashSet<_> = initially_bound.into_iter().collect();
    let mut used = HashSet::new();
    collect_used_candidate_binders_aether(expr, &mut bound, candidates, &mut used);
    used
}
