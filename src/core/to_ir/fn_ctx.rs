use std::collections::HashMap;

use crate::{
    backend_ir::{
        BlockId, FunctionId, IrBlock, IrCallTarget, IrExpr, IrFunction, IrFunctionOrigin, IrInstr,
        IrMetadata, IrParam, IrTerminator, IrType, IrVar,
    },
    core::{CoreBinderId, CoreExpr, CoreTag, CoreType},
    diagnostics::position::Span,
    syntax::{Identifier, effect_expr::EffectExpr, type_expr::TypeExpr},
};

use super::ToIrCtx;
use super::primop::lower_lit;

// ── Per-function context ───────────────────────────────────────────────────────

pub(super) struct FnCtx<'a> {
    pub(super) ctx: &'a mut ToIrCtx,
    pub(super) id: FunctionId,
    pub(super) origin: IrFunctionOrigin,
    pub(super) name: Option<Identifier>,
    pub(super) params: Vec<IrParam>,
    pub(super) parameter_types: Vec<Option<TypeExpr>>,
    pub(super) return_type_annotation: Option<TypeExpr>,
    pub(super) effects: Vec<EffectExpr>,
    pub(super) blocks: Vec<IrBlock>,
    pub(super) current_block: usize,
    pub(super) env: HashMap<CoreBinderId, IrVar>,
    pub(super) binder_names: HashMap<CoreBinderId, Identifier>,
    pub(super) last_value: Option<IrVar>,
    pub(super) inferred_param_types: Vec<Option<CoreType>>,
    pub(super) inferred_return_type: Option<CoreType>,
}

impl<'a> FnCtx<'a> {
    pub(super) fn new(
        ctx: &'a mut ToIrCtx,
        id: FunctionId,
        origin: IrFunctionOrigin,
        entry: BlockId,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type_annotation: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
    ) -> Self {
        Self {
            ctx,
            id,
            name: None,
            origin,
            params: Vec::new(),
            parameter_types,
            return_type_annotation,
            effects,
            blocks: vec![IrBlock {
                id: entry,
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
        }
    }

    pub(super) fn emit(&mut self, instr: IrInstr) {
        self.blocks[self.current_block].instrs.push(instr);
    }

    pub(super) fn bound_var(&self, binder: CoreBinderId, name: Identifier) -> IrVar {
        *self.env.get(&binder).unwrap_or_else(|| {
            panic!(
                "Core binder resolution invariant failed during Core→IR lowering: missing env entry for {}#{}",
                name.as_u32(),
                binder.0
            )
        })
    }

    pub(super) fn new_block(&mut self) -> usize {
        let id = self.ctx.alloc_block();
        self.blocks.push(IrBlock {
            id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.blocks.len() - 1
    }

    pub(super) fn with_bound_var<T>(
        &mut self,
        binder_id: CoreBinderId,
        name: Identifier,
        ir_var: IrVar,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let old_var = self.env.insert(binder_id, ir_var);
        let old_name = self.binder_names.insert(binder_id, name);
        let result = f(self);
        match old_var {
            Some(var) => {
                self.env.insert(binder_id, var);
            }
            None => {
                self.env.remove(&binder_id);
            }
        }
        match old_name {
            Some(existing_name) => {
                self.binder_names.insert(binder_id, existing_name);
            }
            None => {
                self.binder_names.remove(&binder_id);
            }
        }
        result
    }

    pub(super) fn set_terminator(&mut self, t: IrTerminator) {
        self.blocks[self.current_block].terminator = t;
    }

    pub(super) fn current_block_is_open(&self) -> bool {
        matches!(
            self.blocks[self.current_block].terminator,
            IrTerminator::Unreachable(_)
        )
    }

    pub(super) fn finish_return(self, ret: IrVar, span: Span) {
        let mut s = self;
        if matches!(
            s.blocks[s.current_block].terminator,
            IrTerminator::Unreachable(_)
        ) {
            s.blocks[s.current_block].terminator =
                IrTerminator::Return(ret, IrMetadata::from_span(span));
        }
        let entry = s.blocks[0].id;
        s.ctx.functions.push(IrFunction {
            id: s.id,
            name: s.name,
            params: s.params,
            parameter_types: s.parameter_types,
            return_type_annotation: s.return_type_annotation,
            effects: s.effects,
            captures: Vec::new(),
            body_span: span,
            ret_type: IrType::Any,
            blocks: s.blocks,
            entry,
            origin: s.origin,
            metadata: IrMetadata::empty(),
            inferred_param_types: s.inferred_param_types,
            inferred_return_type: s.inferred_return_type,
        });
    }

    /// Lower a `CoreExpr`, returning the `IrVar` that holds the result.
    pub(super) fn lower_expr(&mut self, expr: &CoreExpr) -> IrVar {
        match expr {
            CoreExpr::Var { var, span } => {
                if let Some(binder) = var.binder {
                    return self.bound_var(binder, var.name);
                }
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::LoadName(var.name),
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::Lit(lit, span) => {
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Const(lower_lit(lit)),
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::Lam { .. } => self.lower_lam_as_closure(None, None, expr),

            CoreExpr::App { func, args, span } => {
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dest = self.ctx.alloc_var();
                let meta = IrMetadata::from_span(*span);

                match func.as_ref() {
                    CoreExpr::Var { var, .. } => {
                        if let Some(binder) = var.binder {
                            let fv = self.bound_var(binder, var.name);
                            self.emit(IrInstr::Call {
                                dest,
                                target: IrCallTarget::Var(fv),
                                args: arg_vars,
                                metadata: meta,
                            });
                            return dest;
                        }
                        self.emit(IrInstr::Call {
                            dest,
                            target: IrCallTarget::Named(var.name),
                            args: arg_vars,
                            metadata: meta,
                        });
                    }
                    other => {
                        let fv = self.lower_expr(other);
                        self.emit(IrInstr::Call {
                            dest,
                            target: IrCallTarget::Var(fv),
                            args: arg_vars,
                            metadata: meta,
                        });
                    }
                }
                dest
            }

            CoreExpr::Let { var, rhs, body, .. } => {
                let rhs_var = self.lower_expr(rhs);
                self.with_bound_var(var.id, var.name, rhs_var, |this| this.lower_expr(body))
            }

            CoreExpr::LetRec { var, rhs, body, .. } => {
                let rhs_var = match rhs.as_ref() {
                    CoreExpr::Lam { .. } => {
                        let placeholder = self.ctx.alloc_var();
                        self.emit(IrInstr::Assign {
                            dest: placeholder,
                            expr: IrExpr::LoadName(var.name),
                            metadata: IrMetadata::empty(),
                        });
                        self.with_bound_var(var.id, var.name, placeholder, |this| {
                            this.lower_lam_as_closure(Some(var.name), Some(var.id), rhs.as_ref())
                        })
                    }
                    _ => self.lower_expr(rhs),
                };
                self.with_bound_var(var.id, var.name, rhs_var, |this| this.lower_expr(body))
            }

            CoreExpr::Case {
                scrutinee,
                alts,
                span,
            } => self.lower_case(scrutinee, alts, *span),

            CoreExpr::Con { tag, fields, span } => {
                let field_vars: Vec<IrVar> = fields.iter().map(|f| self.lower_expr(f)).collect();
                let dest = self.ctx.alloc_var();
                let ir_expr = match tag {
                    CoreTag::None => IrExpr::None,
                    CoreTag::Some => IrExpr::Some(*field_vars.first().expect("Some needs 1 field")),
                    CoreTag::Left => IrExpr::Left(*field_vars.first().expect("Left needs 1 field")),
                    CoreTag::Right => {
                        IrExpr::Right(*field_vars.first().expect("Right needs 1 field"))
                    }
                    CoreTag::Nil => IrExpr::EmptyList,
                    CoreTag::Cons => IrExpr::Cons {
                        head: field_vars[0],
                        tail: field_vars[1],
                    },
                    CoreTag::Named(name) => IrExpr::MakeAdt(*name, field_vars),
                };
                self.emit(IrInstr::Assign {
                    dest,
                    expr: ir_expr,
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::PrimOp { op, args, span } => self.lower_primop(op, args, *span),

            CoreExpr::Return { value, span } => {
                let ret = self.lower_expr(value);
                if self.current_block_is_open() {
                    self.set_terminator(IrTerminator::Return(ret, IrMetadata::from_span(*span)));
                }
                ret
            }

            CoreExpr::Perform {
                effect,
                operation,
                args,
                span,
            } => {
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Perform {
                        effect: *effect,
                        operation: *operation,
                        args: arg_vars,
                    },
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::Handle {
                body,
                effect,
                handlers,
                span,
            } => {
                // Compile each handler arm as a separate closure function.
                let mut scope_arms = Vec::new();
                for h in handlers {
                    let (fn_id, capture_vars) = self.lower_handler_arm(h);
                    scope_arms.push(crate::backend_ir::HandleScopeArm {
                        operation_name: h.operation,
                        function_id: fn_id,
                        capture_vars,
                    });
                }

                // Create a body block, lower the body into it, then jump to
                // a continuation block carrying the result.
                let body_block_idx = self.new_block();
                let body_block_id = self.blocks[body_block_idx].id;

                let cont_block_idx = self.new_block();
                let cont_block_id = self.blocks[cont_block_idx].id;
                let dest = self.ctx.alloc_var();
                self.blocks[cont_block_idx]
                    .params
                    .push(crate::backend_ir::IrBlockParam {
                        var: dest,
                        ty: IrType::Any,
                    });

                // Emit HandleScope in the current block, then terminate with
                // a jump into the body entry block.
                let meta = IrMetadata::from_span(*span);
                self.emit(IrInstr::HandleScope {
                    effect: *effect,
                    arms: scope_arms,
                    body_entry: body_block_id,
                    body_result: dest, // will be set below via jump arg
                    dest,
                    metadata: meta.clone(),
                });
                self.set_terminator(IrTerminator::Jump(body_block_id, Vec::new(), meta.clone()));

                // Switch to body block, lower the body, jump to continuation.
                self.current_block = body_block_idx;
                let body_var = self.lower_expr(body);
                self.set_terminator(IrTerminator::Jump(cont_block_id, vec![body_var], meta));

                // Continue in the continuation block.
                self.current_block = cont_block_idx;
                dest
            }
        }
    }
}
