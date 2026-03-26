//! Aether (Perceus RC) lowering helpers for core_to_llvm.
//!
//! Provides methods on `FunctionLowering` to emit LLVM IR for Core IR
//! Aether annotations: `Dup`, `Drop`, `Reuse`, `DropSpecialized`, and
//! `AetherCall` with per-argument borrow modes.

use crate::core::{CoreExpr, CoreTag, CoreVarRef};
use crate::core_to_llvm::{CallConv, LlvmInstr, LlvmOperand, LlvmTerminator, LlvmType};

use super::function::CoreToLlvmError;
use super::prelude::flux_prelude_symbol;

use super::expr::FunctionLowering;

impl<'a, 'p> FunctionLowering<'a, 'p> {
    /// Lower a `Dup { var, body }` node: emit `flux_dup(%var)` then lower body.
    ///
    /// Phase 5 (Proposal 0119): skip dup for unboxed values (IntRep, FloatRep,
    /// BoolRep) — they are immediate scalars that don't need reference counting.
    pub(super) fn lower_dup(
        &mut self,
        var: CoreVarRef,
        body: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if !self.var_is_unboxed(var)
            && let Ok(val) = self.load_var_value(var)
        {
            self.emit_dup_call(val);
        }
        self.lower_expr(body)
    }

    /// Lower a `Drop { var, body }` node: emit `flux_drop(%var)` then lower body.
    ///
    /// Phase 5 (Proposal 0119): skip drop for unboxed values (IntRep, FloatRep,
    /// BoolRep) — they are immediate scalars that don't need reference counting.
    pub(super) fn lower_drop(
        &mut self,
        var: CoreVarRef,
        body: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if !self.var_is_unboxed(var)
            && let Ok(val) = self.load_var_value(var)
        {
            self.emit_drop_call(val);
        }
        self.lower_expr(body)
    }

    /// Lower a `Reuse { token, tag, fields, field_mask }` node.
    ///
    /// If the token's RC == 1 (unique), reuse its allocation in-place.
    /// Otherwise, allocate fresh memory and construct normally.
    pub(super) fn lower_reuse(
        &mut self,
        token: CoreVarRef,
        tag: &CoreTag,
        fields: &[CoreExpr],
        field_mask: Option<u64>,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        // For value-type constructors (None, Nil), reuse doesn't apply.
        match tag {
            CoreTag::None | CoreTag::Nil => return self.lower_constructor(tag, fields),
            _ => {}
        }

        let ctor_tag = self.program.adt_metadata.tag_for(tag).ok_or_else(|| {
            CoreToLlvmError::MissingSymbol {
                message: format!("missing ADT tag for reuse constructor {:?}", tag),
            }
        })?;

        // Lower all field values first.
        // Phase 4: tag raw int fields for ADT storage (NaN-boxed context).
        let field_values: Vec<LlvmOperand> = {
            let mut vals = Vec::with_capacity(fields.len());
            for f in fields {
                let val = self.lower_expr_not_tail(f)?;
                vals.push(self.ensure_tagged_if_raw(val, f)?);
            }
            vals
        };

        // Load the token value.
        let token_val = self.load_var_value(token)?;

        // Call flux_drop_reuse to get a ptr (reused or fresh).
        // Size: 8 (tag) + 4 (field_count) + 4 (padding) + 8 * nfields
        let alloc_size = 8 + 8 * field_values.len() as i32;
        let mem_ptr = self.state.temp_local("reuse.mem");
        self.state.emit(LlvmInstr::Call {
            dst: Some(mem_ptr.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_drop_reuse")),
            args: vec![
                (LlvmType::i64(), token_val),
                (LlvmType::i32(), const_i32(alloc_size)),
            ],
            attrs: vec![],
        });

        // Write the tag.
        let tag_ptr = self.state.temp_local("reuse.tag_ptr");
        self.state.emit(LlvmInstr::Cast {
            dst: tag_ptr.clone(),
            op: crate::core_to_llvm::LlvmValueKind::Bitcast,
            from_ty: LlvmType::ptr(),
            operand: LlvmOperand::Local(mem_ptr.clone()),
            to_ty: LlvmType::ptr(),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i32(),
            value: const_i32(ctor_tag),
            ptr: LlvmOperand::Local(tag_ptr),
            align: Some(4),
        });

        // Write field count.
        let nf_ptr = self.state.temp_local("reuse.nf_ptr");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: nf_ptr.clone(),
            inbounds: true,
            element_ty: LlvmType::i32(),
            base: LlvmOperand::Local(mem_ptr.clone()),
            indices: vec![(LlvmType::i32(), const_i32(1))],
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i32(),
            value: const_i32(field_values.len() as i32),
            ptr: LlvmOperand::Local(nf_ptr),
            align: Some(4),
        });

        // Write fields (respecting field_mask if present).
        let fields_base = self.state.temp_local("reuse.fields_base");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: fields_base.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(mem_ptr.clone()),
            indices: vec![(LlvmType::i32(), const_i32(1))], // skip 8-byte header (tag+nfields)
        });

        for (i, val) in field_values.iter().enumerate() {
            // NOTE: The Aether reuse pass provides a field_mask indicating which
            // fields changed.  However, flux_drop_reuse may allocate *fresh*
            // (zeroed) memory when the RC > 1, so we must always write all
            // fields to avoid reading stale zeros.
            let _ = field_mask;
            let field_ptr = self.state.temp_local(&format!("reuse.field.{i}"));
            self.state.emit(LlvmInstr::GetElementPtr {
                dst: field_ptr.clone(),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(fields_base.clone()),
                indices: vec![(LlvmType::i32(), const_i32(i as i32))],
            });
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: val.clone(),
                ptr: LlvmOperand::Local(field_ptr),
                align: Some(8),
            });
        }

        // Tag the pointer as a NaN-boxed value.
        let result = self.state.temp_local("reuse.tagged");
        self.state.emit(LlvmInstr::Call {
            dst: Some(result.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(super::closure::flux_closure_symbol("flux_tag_boxed_ptr")),
            args: vec![(LlvmType::ptr(), LlvmOperand::Local(mem_ptr))],
            attrs: vec![],
        });

        Ok(LlvmOperand::Local(result))
    }

    /// Lower a `DropSpecialized { scrutinee, unique_body, shared_body }` node.
    ///
    /// Emits a runtime branch on whether the scrutinee's RC == 1:
    /// - Unique path: fields are already owned, just lower unique_body
    /// - Shared path: dup extracted fields, decrement scrutinee, lower shared_body
    pub(super) fn lower_drop_specialized(
        &mut self,
        scrutinee: CoreVarRef,
        unique_body: &CoreExpr,
        shared_body: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let scrut_val = self.load_var_value(scrutinee)?;

        // Check uniqueness.
        let is_unique = self.state.temp_local("ds.unique");
        self.state.emit(LlvmInstr::Call {
            dst: Some(is_unique.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i1(),
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_rc_is_unique")),
            args: vec![(LlvmType::i64(), scrut_val)],
            attrs: vec![],
        });

        // Create basic blocks for unique / shared / join.
        let unique_label = self.state.new_block_label("ds.unique");
        let shared_label = self.state.new_block_label("ds.shared");
        let join_label = self.state.new_block_label("ds.join");

        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(is_unique),
            then_label: unique_label.clone(),
            else_label: shared_label.clone(),
        });

        // --- Unique body ---
        let unique_idx = self.state.push_block(unique_label.clone());
        self.state.switch_to_block(unique_idx);
        let unique_result = self.lower_expr(unique_body)?;
        let mut incoming = Vec::new();
        if self.state.current_block_open() {
            let unique_from = self.state.current_block_label();
            self.state.set_terminator(LlvmTerminator::Br {
                target: join_label.clone(),
            });
            incoming.push((unique_result, unique_from));
        }

        // --- Shared body ---
        let shared_idx = self.state.push_block(shared_label.clone());
        self.state.switch_to_block(shared_idx);
        let shared_result = self.lower_expr(shared_body)?;
        if self.state.current_block_open() {
            let shared_from = self.state.current_block_label();
            self.state.set_terminator(LlvmTerminator::Br {
                target: join_label.clone(),
            });
            incoming.push((shared_result, shared_from));
        }

        // --- Join block with phi ---
        let join_idx = self.state.push_block(join_label);
        self.state.switch_to_block(join_idx);
        if incoming.is_empty() {
            // Both arms terminated (e.g., both tail-called back to TCO loop).
            self.state.set_terminator(LlvmTerminator::Unreachable);
            return Ok(LlvmOperand::Const(crate::core_to_llvm::LlvmConst::Int {
                bits: 64,
                value: 0,
            }));
        }
        let result = self.state.temp_local("ds.result");
        self.state.emit(LlvmInstr::Phi {
            dst: result.clone(),
            ty: LlvmType::i64(),
            incoming,
        });

        Ok(LlvmOperand::Local(result))
    }

    /// Emit a call to `flux_dup(i64 %val)`.
    fn emit_dup_call(&mut self, val: LlvmOperand) {
        self.state.emit(LlvmInstr::Call {
            dst: None,
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::Void,
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_dup")),
            args: vec![(LlvmType::i64(), val)],
            attrs: vec![],
        });
    }

    /// Emit a call to `flux_drop(i64 %val)`.
    fn emit_drop_call(&mut self, val: LlvmOperand) {
        self.state.emit(LlvmInstr::Call {
            dst: None,
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::Void,
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_drop")),
            args: vec![(LlvmType::i64(), val)],
            attrs: vec![],
        });
    }

    /// Check if a variable has an unboxed representation (IntRep, FloatRep,
    /// BoolRep).  Unboxed values are immediate scalars — no heap allocation,
    /// no RC.  Dup/drop on them is a no-op.
    fn var_is_unboxed(&self, var: CoreVarRef) -> bool {
        var.binder
            .and_then(|b| self.local_reps.get(&b).copied())
            .is_some_and(|rep| rep.is_unboxed())
    }

    /// Load a variable's current value from its local slot.
    /// Returns the loaded operand, or an error if the variable is unbound.
    fn load_var_value(&mut self, var: CoreVarRef) -> Result<LlvmOperand, CoreToLlvmError> {
        if let Some(binder) = var.binder
            && let Some(slot) = self.state.local_slots.get(&binder).cloned()
        {
            return self.load_slot_value(slot, "aether.load");
        }
        // If the variable is not in scope (e.g., a top-level reference or
        // already dropped), silently skip the RC operation.  The Aether pass
        // guarantees that dup/drop targets are resolved binders, but during
        // lowering some bindings may not yet be visible.
        Err(CoreToLlvmError::MissingSymbol {
            message: format!("Aether var {:?} not in scope", var.name),
        })
    }
}

fn const_i32(val: i32) -> LlvmOperand {
    LlvmOperand::Const(crate::core_to_llvm::LlvmConst::Int {
        bits: 32,
        value: val as i128,
    })
}
