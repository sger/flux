use std::collections::{HashMap, HashSet};

use crate::{
    cfg::{
        FunctionId, IrBlock, IrExpr, IrFunctionOrigin, IrInstr, IrMetadata, IrParam, IrTerminator,
        IrType, IrVar,
    },
    core::{CoreBinder, CoreBinderId, CoreExpr, CoreHandler},
    syntax::Identifier,
};

use super::free_vars::{collect_free_vars_core, free_vars_rec};

impl<'a> super::fn_ctx::FnCtx<'a> {
    /// Lower a handler arm as a separate closure function.
    /// Parameters: [resume, param0, param1, ...] -- matches the VM calling convention.
    pub(super) fn lower_handler_arm(&mut self, handler: &CoreHandler) -> (FunctionId, Vec<IrVar>) {
        // Collect free variables in the arm body that are bound in the enclosing scope.
        let mut free = HashSet::new();
        free_vars_rec(&handler.body, &mut HashSet::new(), &mut free);
        // Remove the arm's own parameters from the free set.
        free.remove(&handler.resume.id);
        for p in &handler.params {
            free.remove(&p.id);
        }
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter_map(|binder| {
                self.env.get(&binder).map(|_| CoreBinder {
                    id: binder,
                    name: self.binder_names[&binder],
                })
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
                    ty: IrType::Any,
                });
            }
            // Resume param first, then operation params.
            let resume_var = sub.ctx.alloc_var();
            sub.env.insert(handler.resume.id, resume_var);
            sub.binder_names
                .insert(handler.resume.id, handler.resume.name);
            sub.params.push(IrParam {
                name: handler.resume.name,
                var: resume_var,
                ty: IrType::Any,
            });
            for &p in &handler.params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: IrType::Any,
                });
            }

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
        let CoreExpr::Lam { params, body, span } = expr else {
            panic!("lower_lam_as_closure: not a Lam");
        };

        // Compute free variables that need to be captured.
        let free = collect_free_vars_core(expr);
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| Some(*binder) != recursive_binder)
            .filter_map(|binder| {
                self.env.get(&binder).map(|_| CoreBinder {
                    id: binder,
                    name: self.binder_names[&binder],
                })
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
                    ty: IrType::Any,
                });
            }
            if let (Some(name), Some(binder_id)) = (forced_name, recursive_binder) {
                let self_capture_vars: Vec<IrVar> = captures
                    .iter()
                    .filter_map(|binder| sub.env.get(&binder.id).copied())
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
            for &p in params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: IrType::Any,
                });
            }

            let ret = sub.lower_expr(body);
            sub.finish_return(ret, *span);
        }

        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        let capture_vars: Vec<IrVar> = capture_env
            .iter()
            .filter_map(|(b, _)| self.env.get(&b.id).copied())
            .collect();

        let dest = self.ctx.alloc_var();
        self.emit(IrInstr::Assign {
            dest,
            expr: IrExpr::MakeClosure(fn_id, capture_vars),
            metadata: IrMetadata::from_span(*span),
        });
        dest
    }
}
