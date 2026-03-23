use crate::{
    core::{CoreAlt, CoreBinderId, CoreExpr, CoreLit, CorePat, CorePrimOp},
    core_to_llvm::{
        CallConv, GlobalId, LlvmCmpOp, LlvmConst, LlvmInstr, LlvmOperand, LlvmTerminator, LlvmType,
        flux_arith_symbol, flux_prelude_symbol,
    },
    runtime::nanbox::NanTag,
};

use super::function::{CoreToLlvmError, FunctionState};
use super::prelude::FluxNanboxLayout;

pub(super) struct FunctionLowering<'a> {
    state: FunctionState<'a>,
}

impl<'a> FunctionLowering<'a> {
    pub fn new(
        symbol: GlobalId,
        params: &[crate::core::CoreBinder],
        top_level_symbols: &'a std::collections::HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a crate::syntax::interner::Interner>,
    ) -> Self {
        let mut state = FunctionState::new(symbol, params, top_level_symbols, interner);
        for (binder, param_local) in state.param_bindings.clone() {
            let slot = state.new_slot();
            state.emit_entry_alloca(LlvmInstr::Alloca {
                dst: slot.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
            state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(param_local),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
            state.bind_slot(binder.id, slot);
        }
        Self { state }
    }

    pub fn finish_with_return(
        mut self,
        result: LlvmOperand,
    ) -> Result<crate::core_to_llvm::LlvmFunction, CoreToLlvmError> {
        if self.state.current_block_open() {
            self.state.set_terminator(LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: result,
            });
        }
        self.state.finish()
    }

    pub fn lower_expr(&mut self, expr: &CoreExpr) -> Result<LlvmOperand, CoreToLlvmError> {
        match expr {
            CoreExpr::Var { var, .. } => self.lower_var(*var),
            CoreExpr::Lit(lit, _) => self.lower_lit(lit),
            CoreExpr::Lam { .. } => Err(self.unsupported(
                "closure lambda",
                "lambda inside expression requires Phase 4 closure lowering",
            )),
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                self.lower_call(func, args)
            }
            CoreExpr::Let { var, rhs, body, .. } => self.lower_let(*var, rhs, body),
            CoreExpr::LetRec { rhs, .. } if matches!(rhs.as_ref(), CoreExpr::Lam { .. }) => {
                Err(self.unsupported(
                    "local letrec lambda",
                    "local recursive functions require Phase 4 closure lowering",
                ))
            }
            CoreExpr::LetRec { var, rhs, body, .. } => self.lower_let(*var, rhs, body),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => self.lower_case(scrutinee, alts),
            CoreExpr::Con { .. } => {
                Err(self.unsupported("constructors", "ADT construction is deferred to Phase 5"))
            }
            CoreExpr::PrimOp { op, args, .. } => self.lower_primop(op, args),
            CoreExpr::Return { value, .. } => {
                let ret = self.lower_expr(value)?;
                if self.state.current_block_open() {
                    self.state.set_terminator(LlvmTerminator::Ret {
                        ty: LlvmType::i64(),
                        value: ret.clone(),
                    });
                }
                Ok(ret)
            }
            CoreExpr::Perform { .. } => {
                Err(self.unsupported("effects", "Perform requires Phase 6 runtime support"))
            }
            CoreExpr::Handle { .. } => {
                Err(self.unsupported("effects", "Handle requires Phase 6 runtime support"))
            }
            CoreExpr::Dup { body, .. } => self.lower_expr(body),
            CoreExpr::Drop { .. } => {
                Err(self.unsupported("aether drop", "Drop nodes are not lowered in Phase 3"))
            }
            CoreExpr::Reuse { .. } => {
                Err(self.unsupported("reuse", "Reuse requires Phase 7 Aether lowering"))
            }
            CoreExpr::DropSpecialized { .. } => Err(self.unsupported(
                "drop specialization",
                "DropSpecialized requires Phase 7 Aether lowering",
            )),
        }
    }

    fn lower_var(&mut self, var: crate::core::CoreVarRef) -> Result<LlvmOperand, CoreToLlvmError> {
        if let Some(binder) = var.binder {
            if let Some(slot) = self.state.local_slots.get(&binder).cloned() {
                let tmp = self.state.temp_local("load");
                self.state.emit(LlvmInstr::Load {
                    dst: tmp.clone(),
                    ty: LlvmType::i64(),
                    ptr: LlvmOperand::Local(slot),
                    align: Some(8),
                });
                return Ok(LlvmOperand::Local(tmp));
            }

            if self.state.top_level_symbols.contains_key(&binder) {
                return Err(self.unsupported(
                    "function values",
                    "top-level function references are only supported in direct call position in Phase 3",
                ));
            }
        }

        Err(CoreToLlvmError::MissingSymbol {
            message: format!(
                "unresolved local binding for `{}`",
                super::function::display_ident(var.name, self.state.interner)
            ),
        })
    }

    fn lower_lit(&mut self, lit: &CoreLit) -> Result<LlvmOperand, CoreToLlvmError> {
        match lit {
            CoreLit::Int(n) => {
                if !(FluxNanboxLayout::MIN_INLINE_INT..=FluxNanboxLayout::MAX_INLINE_INT)
                    .contains(n)
                {
                    return Err(self.unsupported(
                        "large integer literals",
                        "boxed integer literals are not lowered in Phase 3",
                    ));
                }
                Ok(const_i64(tagged_int_bits(*n)))
            }
            CoreLit::Float(f) => Ok(const_i64(f.to_bits() as i64)),
            CoreLit::Bool(value) => Ok(const_i64(tagged_bool_bits(*value))),
            CoreLit::Unit => Ok(const_i64(tagged_none_bits())),
            CoreLit::String(_) => Err(self.unsupported(
                "string literals",
                "string lowering requires later runtime phases",
            )),
        }
    }

    fn lower_let(
        &mut self,
        binder: crate::core::CoreBinder,
        rhs: &CoreExpr,
        body: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let rhs_value = self.lower_expr(rhs)?;
        let slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: slot.clone(),
            ty: LlvmType::i64(),
            count: None,
            align: Some(8),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i64(),
            value: rhs_value,
            ptr: LlvmOperand::Local(slot.clone()),
            align: Some(8),
        });

        let old = self.state.local_slots.insert(binder.id, slot);
        let result = self.lower_expr(body);
        if let Some(previous) = old {
            self.state.local_slots.insert(binder.id, previous);
        } else {
            self.state.local_slots.remove(&binder.id);
        }
        result
    }

    fn lower_call(
        &mut self,
        func: &CoreExpr,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let callee = self.resolve_call_target(func)?;
        let lowered_args = args
            .iter()
            .map(|arg| self.lower_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.state.temp_local("call");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(callee),
            args: lowered_args
                .into_iter()
                .map(|arg| (LlvmType::i64(), arg))
                .collect(),
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn resolve_call_target(&self, func: &CoreExpr) -> Result<GlobalId, CoreToLlvmError> {
        match func {
            CoreExpr::Var { var, .. } => {
                if let Some(binder) = var.binder {
                    if let Some(symbol) = self.state.top_level_symbols.get(&binder) {
                        return Ok(symbol.clone());
                    }
                    if self.state.local_slots.contains_key(&binder) {
                        return Err(self.unsupported(
                            "indirect local calls",
                            "calling closure-valued locals is deferred to Phase 4",
                        ));
                    }
                }
                Err(self.unsupported(
                    "dynamic calls",
                    "direct named or self calls are the only supported call targets in Phase 3",
                ))
            }
            CoreExpr::Lam { .. } => Err(self.unsupported(
                "lambda calls",
                "calling lambdas directly requires Phase 4 closure lowering",
            )),
            _ => Err(self.unsupported(
                "dynamic calls",
                "direct named or self calls are the only supported call targets in Phase 3",
            )),
        }
    }

    fn lower_case(
        &mut self,
        scrutinee: &CoreExpr,
        alts: &[CoreAlt],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if alts.is_empty() {
            return Err(CoreToLlvmError::Malformed {
                message: "case expression had no alternatives".into(),
            });
        }
        if alts.iter().any(|alt| alt.guard.is_some()) {
            return Err(self.unsupported(
                "case guards",
                "guarded case alternatives are not lowered in Phase 3",
            ));
        }

        let scrutinee = self.lower_expr(scrutinee)?;
        let join_label = self.state.new_block_label("case.join");
        let join_idx = self.state.push_block(join_label.clone());
        let mut incoming = Vec::new();
        let mut active_block = self.state.current_block;

        for (index, alt) in alts.iter().enumerate() {
            self.state.switch_to_block(active_block);
            match &alt.pat {
                CorePat::Wildcard => {
                    let value = self.lower_expr(&alt.rhs)?;
                    if self.state.current_block_open() {
                        let from = self.state.current_block_label();
                        self.state.set_terminator(LlvmTerminator::Br {
                            target: join_label.clone(),
                        });
                        incoming.push((value, from));
                    }
                    break;
                }
                CorePat::Lit(lit) => {
                    let then_label = self.state.new_block_label("case.then");
                    let then_idx = self.state.push_block(then_label.clone());
                    let else_label = self.state.new_block_label("case.else");
                    let else_idx = self.state.push_block(else_label.clone());
                    let cond = self.emit_match_cond(scrutinee.clone(), lit)?;
                    self.state.set_terminator(LlvmTerminator::CondBr {
                        cond_ty: LlvmType::i1(),
                        cond,
                        then_label: then_label.clone(),
                        else_label: else_label.clone(),
                    });

                    self.state.switch_to_block(then_idx);
                    let value = self.lower_expr(&alt.rhs)?;
                    if self.state.current_block_open() {
                        let from = self.state.current_block_label();
                        self.state.set_terminator(LlvmTerminator::Br {
                            target: join_label.clone(),
                        });
                        incoming.push((value, from));
                    }

                    active_block = else_idx;
                    if index == alts.len() - 1 {
                        self.state.switch_to_block(active_block);
                        return Err(self.unsupported(
                            "non-exhaustive literal case",
                            "literal-only case expressions require a wildcard/default arm in Phase 3",
                        ));
                    }
                }
                CorePat::Var(_) => {
                    return Err(self.unsupported(
                        "case binder patterns",
                        "binding patterns are deferred until richer pattern lowering",
                    ));
                }
                CorePat::Con { .. } | CorePat::Tuple(_) | CorePat::EmptyList => {
                    return Err(self.unsupported(
                        "ADT patterns",
                        "general pattern matching is deferred to Phase 5",
                    ));
                }
            }
        }

        self.state.switch_to_block(join_idx);
        if incoming.is_empty() {
            return Err(CoreToLlvmError::Malformed {
                message: "case join had no incoming values".into(),
            });
        }
        let phi = self.state.temp_local("case.result");
        self.state.emit(LlvmInstr::Phi {
            dst: phi.clone(),
            ty: LlvmType::i64(),
            incoming,
        });
        Ok(LlvmOperand::Local(phi))
    }

    fn emit_match_cond(
        &mut self,
        scrutinee: LlvmOperand,
        lit: &CoreLit,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let rhs = match lit {
            CoreLit::Bool(value) => const_i64(tagged_bool_bits(*value)),
            CoreLit::Int(n) => {
                if !(FluxNanboxLayout::MIN_INLINE_INT..=FluxNanboxLayout::MAX_INLINE_INT)
                    .contains(n)
                {
                    return Err(self.unsupported(
                        "large integer literal patterns",
                        "boxed integer patterns are not lowered in Phase 3",
                    ));
                }
                const_i64(tagged_int_bits(*n))
            }
            CoreLit::Unit => const_i64(tagged_none_bits()),
            CoreLit::Float(f) => const_i64(f.to_bits() as i64),
            CoreLit::String(_) => {
                return Err(self.unsupported(
                    "string patterns",
                    "string matching is deferred to later phases",
                ));
            }
        };
        let cond = self.state.temp_local("case.cond");
        self.state.emit(LlvmInstr::Icmp {
            dst: cond.clone(),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::i64(),
            lhs: scrutinee,
            rhs,
        });
        Ok(LlvmOperand::Local(cond))
    }

    fn lower_primop(
        &mut self,
        op: &CorePrimOp,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        match op {
            CorePrimOp::Add => self.lower_helper_call("flux_iadd", args),
            CorePrimOp::Sub => self.lower_helper_call("flux_isub", args),
            CorePrimOp::Mul => self.lower_helper_call("flux_imul", args),
            CorePrimOp::Div => self.lower_helper_call("flux_idiv", args),
            CorePrimOp::IAdd => self.lower_helper_call("flux_iadd", args),
            CorePrimOp::ISub => self.lower_helper_call("flux_isub", args),
            CorePrimOp::IMul => self.lower_helper_call("flux_imul", args),
            CorePrimOp::IDiv => self.lower_helper_call("flux_idiv", args),
            CorePrimOp::FAdd => self.lower_helper_call("flux_fadd", args),
            CorePrimOp::Eq => self.lower_cmp_bool(args, LlvmCmpOp::Eq, false),
            CorePrimOp::NEq => self.lower_cmp_bool(args, LlvmCmpOp::Ne, false),
            CorePrimOp::Lt => self.lower_cmp_bool(args, LlvmCmpOp::Slt, true),
            CorePrimOp::Le => self.lower_cmp_bool(args, LlvmCmpOp::Sle, true),
            CorePrimOp::Gt => self.lower_cmp_bool(args, LlvmCmpOp::Sgt, true),
            CorePrimOp::Ge => self.lower_cmp_bool(args, LlvmCmpOp::Sge, true),
            CorePrimOp::Mod
            | CorePrimOp::IMod
            | CorePrimOp::FSub
            | CorePrimOp::FMul
            | CorePrimOp::FDiv
            | CorePrimOp::Neg
            | CorePrimOp::Not
            | CorePrimOp::And
            | CorePrimOp::Or
            | CorePrimOp::Concat
            | CorePrimOp::Interpolate
            | CorePrimOp::MakeList
            | CorePrimOp::MakeArray
            | CorePrimOp::MakeTuple
            | CorePrimOp::MakeHash
            | CorePrimOp::Index
            | CorePrimOp::MemberAccess(_)
            | CorePrimOp::TupleField(_) => Err(self.unsupported(
                "primop",
                &format!("primop `{op:?}` is not lowered in Phase 3"),
            )),
        }
    }

    fn lower_helper_call(
        &mut self,
        name: &str,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 2 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("helper `{name}` expected 2 args, got {}", args.len()),
            });
        }
        let left = self.lower_expr(&args[0])?;
        let right = self.lower_expr(&args[1])?;
        let dst = self.state.temp_local("primop");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_arith_symbol(name)),
            args: vec![(LlvmType::i64(), left), (LlvmType::i64(), right)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn lower_cmp_bool(
        &mut self,
        args: &[CoreExpr],
        op: LlvmCmpOp,
        untag_inputs: bool,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 2 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("comparison expected 2 args, got {}", args.len()),
            });
        }
        let left = self.lower_expr(&args[0])?;
        let right = self.lower_expr(&args[1])?;
        let (lhs, rhs) = if untag_inputs {
            (
                self.emit_untag_call(left, "cmp.lhs")?,
                self.emit_untag_call(right, "cmp.rhs")?,
            )
        } else {
            (left, right)
        };
        let cond = self.state.temp_local("cmp.cond");
        self.state.emit(LlvmInstr::Icmp {
            dst: cond.clone(),
            op,
            ty: LlvmType::i64(),
            lhs,
            rhs,
        });
        let result = self.state.temp_local("cmp.bool");
        self.state.emit(LlvmInstr::Select {
            dst: result.clone(),
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(cond),
            value_ty: LlvmType::i64(),
            then_value: const_i64(tagged_bool_bits(true)),
            else_value: const_i64(tagged_bool_bits(false)),
        });
        Ok(LlvmOperand::Local(result))
    }

    fn emit_untag_call(
        &mut self,
        value: LlvmOperand,
        prefix: &str,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let dst = self.state.temp_local(prefix);
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_untag_int")),
            args: vec![(LlvmType::i64(), value)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn unsupported(&self, feature: &'static str, context: impl Into<String>) -> CoreToLlvmError {
        CoreToLlvmError::Unsupported {
            feature,
            context: context.into(),
        }
    }
}

fn tagged_int_bits(value: i64) -> i64 {
    ((value as u64) & FluxNanboxLayout::PAYLOAD_MASK_U64 | FluxNanboxLayout::NANBOX_SENTINEL_U64)
        as i64
}

fn tagged_bool_bits(value: bool) -> i64 {
    (FluxNanboxLayout::NANBOX_SENTINEL_U64
        | ((NanTag::Boolean as u64) << FluxNanboxLayout::TAG_SHIFT)
        | u64::from(value)) as i64
}

fn tagged_none_bits() -> i64 {
    (FluxNanboxLayout::NANBOX_SENTINEL_U64 | ((NanTag::None as u64) << FluxNanboxLayout::TAG_SHIFT))
        as i64
}

pub(super) fn const_i64(value: i64) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: value.into(),
    })
}
