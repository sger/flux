use std::collections::HashMap;

use crate::{
    core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreLit, CorePat, CorePrimOp, CoreTag,
        CoreVarRef,
    },
    core_to_llvm::{
        CallConv, GlobalId, LabelId, LlvmCmpOp, LlvmConst, LlvmInstr, LlvmLocal, LlvmOperand,
        LlvmTerminator, LlvmType, flux_adt_symbol, flux_arith_symbol, flux_closure_symbol,
        flux_prelude_symbol,
    },
    runtime::nanbox::NanTag,
};

use super::{
    adt::tagged_empty_list_bits,
    closure::{analyze_lambda_captures, common_closure_load_instrs, const_i32_operand, local},
    function::{
        CoreToLlvmError, FunctionState, ProgramState, closure_entry_function,
        emit_closure_param_unpack,
    },
    prelude::FluxNanboxLayout,
};

/// Recorded continuation block for the unwind switch (Phase 3 CPS).
pub(super) struct CpsContBlock {
    /// Integer tag for this continuation in the unwind switch.
    pub tag: u8,
    /// Label of the continuation block (already generated during body lowering).
    pub label: LabelId,
}

/// State for Phase 3 CPS driver loop.
pub(super) struct CpsState {
    /// Label of the main eval loop.
    pub loop_header: LabelId,
    /// Label of the unwind loop (continuation application).
    pub unwind_header: LabelId,
    /// Alloca slot for the continuation stack head pointer (ptr, initially null).
    pub head_slot: LlvmLocal,
    /// Alloca slots for the current function arguments (one per param).
    pub arg_slots: Vec<LlvmLocal>,
    /// Alloca slot for the current result value.
    pub result_slot: LlvmLocal,
    /// Alloca slot for the most recently popped frame pointer (set by unwind, read by cont blocks).
    pub frame_slot: LlvmLocal,
    /// Continuation blocks (populated during body lowering, used to generate the unwind switch).
    pub cont_blocks: Vec<CpsContBlock>,
    /// Counter for continuation tags.
    pub next_cont_tag: u8,
    /// The function's own binder ID (for detecting self-calls).
    pub self_binder: CoreBinderId,
}

pub(super) struct FunctionLowering<'a, 'p> {
    pub state: FunctionState<'a>,
    pub program: &'p mut ProgramState<'a>,
    /// Whether the current expression is in tail position (its result is the
    /// function's return value).  Used by TCO to decide whether a self-recursive
    /// call can be converted into a branch back to the loop header.
    is_tail_position: bool,
    /// If this function belongs to a mutual tail-recursion group (Phase 2),
    /// stores (this function's binder, the group).
    pub mutual_group: Option<(
        CoreBinderId,
        std::sync::Arc<super::function::MutualRecGroup>,
    )>,
    /// Phase 3 CPS state — present when the function has non-tail self-recursion.
    pub cps_state: Option<CpsState>,
}

struct BindingRestore {
    binder: CoreBinderId,
    old_slot: Option<LlvmLocal>,
    old_name: Option<crate::syntax::Identifier>,
}

impl<'a, 'p> FunctionLowering<'a, 'p> {
    pub fn new_top_level(
        symbol: GlobalId,
        params: &[CoreBinder],
        program: &'p mut ProgramState<'a>,
    ) -> Self {
        let symbols = top_level_symbols(program);
        let mut state = FunctionState::new_top_level(symbol, params, symbols, program.interner);
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
            state.bind_local(binder, slot);
        }
        Self {
            state,
            program,
            is_tail_position: true,
            mutual_group: None,
            cps_state: None,
        }
    }

    fn new_closure_entry(
        symbol: GlobalId,
        params: &[CoreBinder],
        captures: &[CoreBinder],
        recursive_binder: Option<CoreBinder>,
        program: &'p mut ProgramState<'a>,
    ) -> Result<Self, CoreToLlvmError> {
        let symbols = top_level_symbols(program);
        let mut state = closure_entry_function(symbol, symbols, program.interner);
        state.blocks[0]
            .instrs
            .extend(common_closure_load_instrs(local("closure_raw")));

        for (index, binder) in captures.iter().enumerate() {
            let slot = state.new_slot();
            state.emit_entry_alloca(LlvmInstr::Alloca {
                dst: slot.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
            state.emit(LlvmInstr::GetElementPtr {
                dst: LlvmLocal(format!("capture.src.{index}")),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(LlvmLocal("payload".into())),
                indices: vec![(LlvmType::i32(), const_i32_operand(index as i32))],
            });
            state.emit(LlvmInstr::Load {
                dst: LlvmLocal(format!("capture.val.{index}")),
                ty: LlvmType::i64(),
                ptr: LlvmOperand::Local(LlvmLocal(format!("capture.src.{index}"))),
                align: Some(8),
            });
            state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(LlvmLocal(format!("capture.val.{index}"))),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
            state.bind_local(*binder, slot);
        }

        if let Some(binder) = recursive_binder {
            // closure_raw is already a NaN-boxed i64 — store it directly.
            let slot = state.new_slot();
            state.emit_entry_alloca(LlvmInstr::Alloca {
                dst: slot.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
            state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: local("closure_raw"),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
            state.bind_local(binder, slot);
        }

        let param_unpack = emit_closure_param_unpack(&mut state, params.len(), captures.len());
        state.blocks[0].instrs.extend(param_unpack);
        for (index, binder) in params.iter().enumerate() {
            let slot = state.new_slot();
            state.emit_entry_alloca(LlvmInstr::Alloca {
                dst: slot.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
            state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(LlvmLocal(format!("param.{index}"))),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
            state.bind_local(*binder, slot);
        }

        Ok(Self {
            state,
            program,
            is_tail_position: false,
            mutual_group: None,
            cps_state: None,
        })
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

    /// Set up the TCO loop structure for a self-recursive function.
    ///
    /// Creates a `tco.loop` header block and branches from entry to it.
    /// The function body will be lowered starting from the loop header.
    /// Tail self-calls will store new argument values into the parameter
    /// alloca slots and branch back to the loop header.
    pub fn setup_tco_loop(&mut self) {
        let loop_header = self.state.new_block_label("tco.loop");
        let loop_idx = self.state.push_block(loop_header.clone());

        // Terminate entry block with branch to loop header.
        self.state.set_terminator(LlvmTerminator::Br {
            target: loop_header.clone(),
        });

        // Collect parameter alloca slots in order.
        let param_slots: Vec<LlvmLocal> = self
            .state
            .param_bindings
            .clone()
            .iter()
            .map(|(binder, _)| self.state.local_slots[&binder.id].clone())
            .collect();

        // Switch to loop header block — body lowering continues from here.
        self.state.switch_to_block(loop_idx);

        self.state.tco_loop = Some(super::function::TcoLoopState {
            loop_header,
            param_slots,
        });

        self.is_tail_position = true;
    }

    /// Set up the Phase 3 CPS driver loop for a function with non-tail
    /// self-recursion.  Creates the loop/unwind structure and alloca slots
    /// for the continuation stack head, arguments, and result.
    pub fn setup_cps_driver(&mut self, self_binder: CoreBinderId) {
        let loop_header = self.state.new_block_label("cps.loop");
        let loop_idx = self.state.push_block(loop_header.clone());
        let unwind_header = self.state.new_block_label("cps.unwind");

        // Alloca for continuation stack head pointer (initially null).
        let head_slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: head_slot.clone(),
            ty: LlvmType::ptr(),
            count: None,
            align: Some(8),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::ptr(),
            value: LlvmOperand::Const(LlvmConst::Null),
            ptr: LlvmOperand::Local(head_slot.clone()),
            align: Some(8),
        });

        // Alloca for result value (used during unwind).
        let result_slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: result_slot.clone(),
            ty: LlvmType::i64(),
            count: None,
            align: Some(8),
        });

        // Alloca for popped frame pointer (set by unwind pop, read by cont blocks).
        let frame_slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: frame_slot.clone(),
            ty: LlvmType::ptr(),
            count: None,
            align: Some(8),
        });

        // Collect parameter alloca slots (same as Phase 1 TCO).
        let arg_slots: Vec<LlvmLocal> = self
            .state
            .param_bindings
            .clone()
            .iter()
            .map(|(binder, _)| self.state.local_slots[&binder.id].clone())
            .collect();

        // Branch entry → loop.
        self.state.set_terminator(LlvmTerminator::Br {
            target: loop_header.clone(),
        });
        self.state.switch_to_block(loop_idx);

        // Also set up Phase 1 TCO loop state so tail self-calls still work
        // as `store args + br loop`.
        self.state.tco_loop = Some(super::function::TcoLoopState {
            loop_header: loop_header.clone(),
            param_slots: arg_slots.clone(),
        });

        self.cps_state = Some(CpsState {
            loop_header,
            unwind_header,
            head_slot,
            arg_slots,
            result_slot,
            frame_slot,
            cont_blocks: Vec::new(),
            next_cont_tag: 0,
            self_binder,
        });

        self.is_tail_position = true;
    }

    /// Finish CPS lowering: store the body result, branch to unwind, then
    /// generate the unwind switch from collected continuation blocks.
    pub fn finalize_cps(
        &mut self,
        body_result: LlvmOperand,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let cps = self
            .cps_state
            .as_ref()
            .ok_or_else(|| CoreToLlvmError::Malformed {
                message: "finalize_cps called without CPS state".into(),
            })?;
        let unwind_header = cps.unwind_header.clone();
        let result_slot = cps.result_slot.clone();
        let head_slot = cps.head_slot.clone();

        // Store result and branch to unwind.
        if self.state.current_block_open() {
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: body_result,
                ptr: LlvmOperand::Local(result_slot.clone()),
                align: Some(8),
            });
            self.state.set_terminator(LlvmTerminator::Br {
                target: unwind_header.clone(),
            });
        }

        // Generate unwind block: check if stack empty, pop frame, switch on tag.
        let unwind_idx = self.state.push_block(unwind_header.clone());
        self.state.switch_to_block(unwind_idx);

        // Load result.
        let result_val = self.state.temp_local("cps.result");
        self.state.emit(LlvmInstr::Load {
            dst: result_val.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(result_slot.clone()),
            align: Some(8),
        });

        // Load stack head.
        let head_val = self.state.temp_local("cps.head");
        self.state.emit(LlvmInstr::Load {
            dst: head_val.clone(),
            ty: LlvmType::ptr(),
            ptr: LlvmOperand::Local(head_slot),
            align: Some(8),
        });

        // Check if null (stack empty).
        let is_empty = self.state.temp_local("cps.empty");
        self.state.emit(LlvmInstr::Icmp {
            dst: is_empty.clone(),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::ptr(),
            lhs: LlvmOperand::Local(head_val.clone()),
            rhs: LlvmOperand::Const(LlvmConst::Null),
        });

        let done_label = self.state.new_block_label("cps.done");
        let pop_label = self.state.new_block_label("cps.pop");

        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(is_empty),
            then_label: done_label.clone(),
            else_label: pop_label.clone(),
        });

        // Pop block: unlink head, load tag, switch to cont blocks.
        let pop_idx = self.state.push_block(pop_label);
        self.state.switch_to_block(pop_idx);

        // Load next pointer (offset 0).
        let next_ptr = self.state.temp_local("cps.next");
        self.state.emit(LlvmInstr::Load {
            dst: next_ptr.clone(),
            ty: LlvmType::ptr(),
            ptr: LlvmOperand::Local(head_val.clone()),
            align: Some(8),
        });

        // Store next as new head.
        let cps = self.cps_state.as_ref().unwrap();
        let head_slot2 = cps.head_slot.clone();
        let frame_slot2 = cps.frame_slot.clone();
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::ptr(),
            value: LlvmOperand::Local(next_ptr),
            ptr: LlvmOperand::Local(head_slot2),
            align: Some(8),
        });

        // Store the popped frame pointer so continuation blocks can access it.
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::ptr(),
            value: LlvmOperand::Local(head_val.clone()),
            ptr: LlvmOperand::Local(frame_slot2),
            align: Some(8),
        });

        // Load tag (i8 at offset 8, after the ptr).
        let tag_ptr = self.state.temp_local("cps.tag_ptr");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: tag_ptr.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(head_val),
            indices: vec![(LlvmType::i32(), const_i32(1))],
        });
        let tag_val = self.state.temp_local("cps.tag");
        self.state.emit(LlvmInstr::Load {
            dst: tag_val.clone(),
            ty: LlvmType::i8(),
            ptr: LlvmOperand::Local(tag_ptr),
            align: Some(1),
        });

        // Build switch cases from collected cont_blocks.
        let cps = self.cps_state.as_ref().unwrap();
        let switch_cases: Vec<(LlvmConst, LabelId)> = cps
            .cont_blocks
            .iter()
            .map(|cb| {
                (
                    LlvmConst::Int {
                        bits: 8,
                        value: cb.tag as i128,
                    },
                    cb.label.clone(),
                )
            })
            .collect();

        let unreachable_label = self.state.new_block_label("cps.unreachable");
        self.state.set_terminator(LlvmTerminator::Switch {
            ty: LlvmType::i8(),
            scrutinee: LlvmOperand::Local(tag_val),
            default: unreachable_label.clone(),
            cases: switch_cases,
        });

        // Unreachable block.
        let unr_idx = self.state.push_block(unreachable_label);
        self.state.switch_to_block(unr_idx);
        self.state.set_terminator(LlvmTerminator::Unreachable);

        // Done block: return result.
        let done_idx = self.state.push_block(done_label);
        self.state.switch_to_block(done_idx);

        Ok(LlvmOperand::Local(result_val))
    }

    /// Check if a Let RHS is a non-tail self-recursive call that should be
    /// intercepted by the CPS driver (Phase 3).
    /// Returns `true` if the call was handled (frame pushed, branched to loop,
    /// continuation block generated).
    fn try_lower_cps_let(
        &mut self,
        binder: CoreBinder,
        rhs: &CoreExpr,
        body: &CoreExpr,
    ) -> Result<Option<LlvmOperand>, CoreToLlvmError> {
        // Only active when CPS state exists and we're NOT in tail position.
        if self.is_tail_position || self.cps_state.is_none() {
            return Ok(None);
        }

        // Check if RHS is a direct self-recursive call.
        let (func_expr, call_args) = match rhs {
            CoreExpr::App { func, args, .. } => (func.as_ref(), args.as_slice()),
            CoreExpr::AetherCall { func, args, .. } => (func.as_ref(), args.as_slice()),
            _ => return Ok(None),
        };

        let callee_binder = match func_expr {
            CoreExpr::Var { var, .. } => var.binder,
            _ => return Ok(None),
        };

        let cps = self.cps_state.as_ref().unwrap();
        if callee_binder != Some(cps.self_binder) {
            return Ok(None);
        }

        // This IS a non-tail self-recursive call. Intercept it.
        let tag = cps.next_cont_tag;
        let loop_header = cps.loop_header.clone();
        let unwind_header = cps.unwind_header.clone();
        let head_slot = cps.head_slot.clone();
        let arg_slots = cps.arg_slots.clone();
        let result_slot = cps.result_slot.clone();

        // Increment tag counter.
        self.cps_state.as_mut().unwrap().next_cont_tag = tag + 1;

        // Evaluate the call arguments.
        let lowered_args: Vec<LlvmOperand> = call_args
            .iter()
            .map(|a| self.lower_expr_not_tail(a))
            .collect::<Result<Vec<_>, _>>()?;

        // Find live variables that the continuation body needs.
        let free = crate::core::to_ir::free_vars::collect_free_vars_core(body);
        let live_vars: Vec<(CoreBinder, LlvmLocal)> = self
            .state
            .local_slots
            .iter()
            .filter(|(id, _)| free.contains(id) && **id != binder.id)
            .map(|(id, slot)| {
                let name = self
                    .state
                    .binder_names
                    .get(id)
                    .copied()
                    .unwrap_or(binder.name);
                (CoreBinder::new(*id, name), slot.clone())
            })
            .collect();
        let nfields = live_vars.len() as i32;

        // Allocate continuation frame: {ptr next, i8 tag, pad[7], i64 fields[]}
        // Size: 8 (next) + 8 (tag+pad) + 8*nfields
        let alloc_size = 16 + 8 * nfields;
        let node = self.state.temp_local("cps.frame");
        self.state.emit(LlvmInstr::Call {
            dst: Some(node.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(GlobalId("flux_gc_alloc".into())),
            args: vec![(LlvmType::i32(), const_i32(alloc_size))],
            attrs: vec![],
        });

        // Write next pointer = current head.
        let cur_head = self.state.temp_local("cps.cur_head");
        self.state.emit(LlvmInstr::Load {
            dst: cur_head.clone(),
            ty: LlvmType::ptr(),
            ptr: LlvmOperand::Local(head_slot.clone()),
            align: Some(8),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::ptr(),
            value: LlvmOperand::Local(cur_head),
            ptr: LlvmOperand::Local(node.clone()),
            align: Some(8),
        });

        // Write tag (i8 at offset 8 — we treat this as i64 offset 1 for the tag byte).
        let tag_ptr = self.state.temp_local("cps.frame.tag_ptr");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: tag_ptr.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(node.clone()),
            indices: vec![(LlvmType::i32(), const_i32(1))],
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i8(),
            value: LlvmOperand::Const(LlvmConst::Int {
                bits: 8,
                value: tag as i128,
            }),
            ptr: LlvmOperand::Local(tag_ptr),
            align: Some(1),
        });

        // Write captured fields (i64[] starting at offset 16, i.e., i64 offset 2).
        let fields_base = self.state.temp_local("cps.frame.fields");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: fields_base.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(node.clone()),
            indices: vec![(LlvmType::i32(), const_i32(2))],
        });
        for (i, (_binder, slot)) in live_vars.iter().enumerate() {
            let val = self.state.temp_local(&format!("cps.cap.{i}"));
            self.state.emit(LlvmInstr::Load {
                dst: val.clone(),
                ty: LlvmType::i64(),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
            // Dup the value since it's now captured in the frame.
            self.state.emit(LlvmInstr::Call {
                dst: None,
                tail: false,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::Void,
                callee: LlvmOperand::Global(flux_prelude_symbol("flux_dup")),
                args: vec![(LlvmType::i64(), LlvmOperand::Local(val.clone()))],
                attrs: vec![],
            });
            let field_ptr = self.state.temp_local(&format!("cps.field.{i}"));
            self.state.emit(LlvmInstr::GetElementPtr {
                dst: field_ptr.clone(),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(fields_base.clone()),
                indices: vec![(LlvmType::i32(), const_i32(i as i32))],
            });
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(val),
                ptr: LlvmOperand::Local(field_ptr),
                align: Some(8),
            });
        }

        // Update head = node.
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::ptr(),
            value: LlvmOperand::Local(node),
            ptr: LlvmOperand::Local(head_slot),
            align: Some(8),
        });

        // Store the recursive call's arguments into arg_slots.
        for (slot, arg_val) in arg_slots.iter().zip(lowered_args.iter()) {
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: arg_val.clone(),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
        }

        // Branch to loop header.
        self.state.set_terminator(LlvmTerminator::Br {
            target: loop_header,
        });

        // === Generate the continuation block ===
        // This block is entered from the unwind switch when this frame is popped.
        let cont_label = self.state.new_block_label(&format!("cps.cont.{tag}"));
        let cont_idx = self.state.push_block(cont_label.clone());
        self.state.switch_to_block(cont_idx);

        // The unwind block has already popped the frame into `cps.head` local.
        // We need to reload the frame pointer from what was the head at pop time.
        // Actually, the unwind block loads the frame ptr — we need to pass it here.
        // For simplicity, re-read head_slot to get the popped frame.
        // Wait — the unwind block already updated head_slot to next. The popped
        // node pointer was in the `cps.head` local of the pop block. We can't
        // access that from here.
        //
        // Solution: store the popped frame ptr in result_slot (reuse as temp).
        // Actually, let's use a separate alloca for the popped frame pointer.
        //
        // Better: the unwind block stores the popped frame ptr into a well-known slot.
        // Let me add a `frame_slot` to CpsState.

        // For now, we'll load fields by re-accessing the frame. The unwind block
        // stores the popped frame pointer into a known alloca. Let me add that.
        // Actually, the simplest approach: the continuation block receives the
        // result value and the frame fields through allocas.
        //
        // The unwind pop block will store the popped frame ptr into a frame_slot
        // alloca, and the result is in result_slot. Each cont block loads from those.

        // Load result value (the return value from the recursive call).
        let result_val = self.state.temp_local("cps.cont.result");
        self.state.emit(LlvmInstr::Load {
            dst: result_val.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(result_slot.clone()),
            align: Some(8),
        });

        // Bind the result to the Let variable.
        let result_slot_var = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: result_slot_var.clone(),
            ty: LlvmType::i64(),
            count: None,
            align: Some(8),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i64(),
            value: LlvmOperand::Local(result_val),
            ptr: LlvmOperand::Local(result_slot_var.clone()),
            align: Some(8),
        });
        self.state.bind_local(binder, result_slot_var);

        // Load captured variables from the popped frame.
        {
            let cps = self.cps_state.as_ref().unwrap();
            let frame_slot_ref = cps.frame_slot.clone();
            let frame_ptr = self.state.temp_local("cps.cont.frame");
            self.state.emit(LlvmInstr::Load {
                dst: frame_ptr.clone(),
                ty: LlvmType::ptr(),
                ptr: LlvmOperand::Local(frame_slot_ref),
                align: Some(8),
            });

            // Fields start at byte offset 16 (= i64 index 2 from base).
            let cont_fields_base = self.state.temp_local("cps.cont.fields");
            self.state.emit(LlvmInstr::GetElementPtr {
                dst: cont_fields_base.clone(),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(frame_ptr),
                indices: vec![(LlvmType::i32(), const_i32(2))],
            });

            for (i, (cap_binder, _original_slot)) in live_vars.iter().enumerate() {
                let field_ptr = self.state.temp_local(&format!("cps.cont.fp.{i}"));
                self.state.emit(LlvmInstr::GetElementPtr {
                    dst: field_ptr.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(cont_fields_base.clone()),
                    indices: vec![(LlvmType::i32(), const_i32(i as i32))],
                });
                let field_val = self.state.temp_local(&format!("cps.cont.fv.{i}"));
                self.state.emit(LlvmInstr::Load {
                    dst: field_val.clone(),
                    ty: LlvmType::i64(),
                    ptr: LlvmOperand::Local(field_ptr),
                    align: Some(8),
                });
                // Re-bind the captured variable to a fresh alloca with the restored value.
                let restored_slot = self.state.new_slot();
                self.state.emit_entry_alloca(LlvmInstr::Alloca {
                    dst: restored_slot.clone(),
                    ty: LlvmType::i64(),
                    count: None,
                    align: Some(8),
                });
                self.state.emit(LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: LlvmOperand::Local(field_val),
                    ptr: LlvmOperand::Local(restored_slot.clone()),
                    align: Some(8),
                });
                self.state.bind_local(*cap_binder, restored_slot);
            }
        }

        // Lower the continuation body.
        let cont_result = self.lower_expr(body)?;

        // Store result and branch to unwind.
        if self.state.current_block_open() {
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: cont_result,
                ptr: LlvmOperand::Local(result_slot),
                align: Some(8),
            });
            self.state.set_terminator(LlvmTerminator::Br {
                target: unwind_header,
            });
        }

        // Record this continuation block.
        self.cps_state
            .as_mut()
            .unwrap()
            .cont_blocks
            .push(CpsContBlock {
                tag,
                label: cont_label,
            });

        // Return a dummy — the original block was already terminated by the br to loop.
        Ok(Some(const_i64(0)))
    }

    /// Lower an expression in a non-tail context (its result is used by
    /// subsequent computation, so a self-recursive call here cannot be
    /// converted to a loop branch).
    pub(super) fn lower_expr_not_tail(
        &mut self,
        expr: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let saved = self.is_tail_position;
        self.is_tail_position = false;
        let result = self.lower_expr(expr);
        self.is_tail_position = saved;
        result
    }

    /// Try to lower a direct self-recursive call as a TCO loop branch.
    /// Returns `Some(dummy_operand)` if the call was converted, `None` otherwise.
    fn try_lower_tco_self_call(
        &mut self,
        callee: &GlobalId,
        lowered_args: &[LlvmOperand],
    ) -> Option<LlvmOperand> {
        if !self.is_tail_position {
            return None;
        }
        let tco = self.state.tco_loop.as_ref()?;
        if callee.0 != self.state.symbol.0 {
            return None;
        }

        let loop_header = tco.loop_header.clone();
        let param_slots = tco.param_slots.clone();

        // Store new argument values into the parameter alloca slots.
        // All args are already evaluated into SSA operands, so stores
        // don't interfere with reads.
        for (slot, arg_val) in param_slots.iter().zip(lowered_args.iter()) {
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: arg_val.clone(),
                ptr: LlvmOperand::Local(slot.clone()),
                align: Some(8),
            });
        }

        // Branch back to loop header.
        self.state.set_terminator(LlvmTerminator::Br {
            target: loop_header,
        });

        // Return a dummy operand — this value is dead (the branch already
        // transferred control).
        Some(const_i64(0))
    }

    /// Try to lower a cross-function tail call within a mutual recursion group
    /// as a thunk return (Phase 2 trampoline TCO).
    /// Returns `Some(Ok(dummy))` if converted, `None` if not applicable.
    fn try_lower_mutual_tco_call(
        &mut self,
        callee_binder: CoreBinderId,
        lowered_args: &[LlvmOperand],
    ) -> Option<Result<LlvmOperand, CoreToLlvmError>> {
        if !self.is_tail_position {
            return None;
        }
        let (my_binder, group) = self.mutual_group.as_ref()?;
        // Only apply to cross-function calls within the group (not self-calls).
        if callee_binder == *my_binder {
            return None;
        }
        let fn_index = *group.member_index.get(&callee_binder)?;

        // Pack args into a stack-allocated array.
        let args_ptr = match self.emit_operand_array(lowered_args, "thunk.args") {
            Ok(ptr) => ptr,
            Err(e) => return Some(Err(e)),
        };

        // Allocate thunk: flux_gc_alloc(8 + nargs * 8)
        let nargs = lowered_args.len() as i32;
        let alloc_size = 8 + nargs * 8;
        let mem = self.state.temp_local("thunk.mem");
        self.state.emit(LlvmInstr::Call {
            dst: Some(mem.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(GlobalId("flux_gc_alloc".into())),
            args: vec![(LlvmType::i32(), const_i32(alloc_size))],
            attrs: vec![],
        });

        // Write fn_index (i8 at offset 0).
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i8(),
            value: LlvmOperand::Const(crate::core_to_llvm::LlvmConst::Int {
                bits: 8,
                value: fn_index as i128,
            }),
            ptr: LlvmOperand::Local(mem.clone()),
            align: Some(1),
        });

        // Write nargs (i32 at offset 4).
        let nargs_ptr = self.state.temp_local("thunk.nargs_ptr");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: nargs_ptr.clone(),
            inbounds: true,
            element_ty: LlvmType::i32(),
            base: LlvmOperand::Local(mem.clone()),
            indices: vec![(LlvmType::i32(), const_i32(1))], // offset 4 bytes
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i32(),
            value: const_i32(nargs),
            ptr: LlvmOperand::Local(nargs_ptr),
            align: Some(4),
        });

        // Copy args (i64[] at offset 8).
        let payload_ptr = self.state.temp_local("thunk.payload");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: payload_ptr.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(mem.clone()),
            indices: vec![(LlvmType::i32(), const_i32(1))], // offset 8 bytes
        });
        self.state.emit(LlvmInstr::Call {
            dst: None,
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::Void,
            callee: LlvmOperand::Global(GlobalId("flux_copy_i64s".into())),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Local(payload_ptr)),
                (LlvmType::ptr(), LlvmOperand::Local(args_ptr)),
                (LlvmType::i32(), const_i32(nargs)),
            ],
            attrs: vec![],
        });

        // Tag with thunk NaN-box tag and return.
        let tagged = self.state.temp_local("thunk.tagged");
        self.state.emit(LlvmInstr::Call {
            dst: Some(tagged.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_tag_thunk")),
            args: vec![(LlvmType::ptr(), LlvmOperand::Local(mem))],
            attrs: vec![],
        });

        // Return the thunk — the trampoline will evaluate it.
        self.state.set_terminator(LlvmTerminator::Ret {
            ty: LlvmType::i64(),
            value: LlvmOperand::Local(tagged),
        });

        Some(Ok(const_i64(0)))
    }

    pub fn lower_expr(&mut self, expr: &CoreExpr) -> Result<LlvmOperand, CoreToLlvmError> {
        match expr {
            CoreExpr::Var { var, .. } => self.lower_var(*var),
            CoreExpr::Lit(lit, _) => self.lower_lit(lit),
            CoreExpr::Lam { params, body, .. } => self.lower_lambda_value(params, body, None),
            CoreExpr::App { func, args, .. } => self.lower_call(func, args),
            CoreExpr::AetherCall {
                func,
                args,
                arg_modes,
                ..
            } => self.lower_aether_call(func, args, arg_modes),
            CoreExpr::Let { var, rhs, body, .. } => self.lower_let(*var, rhs, body),
            CoreExpr::LetRec { rhs, .. } if matches!(rhs.as_ref(), CoreExpr::Lam { .. }) => {
                self.lower_letrec_lambda(expr)
            }
            CoreExpr::LetRec { var, rhs, body, .. } => self.lower_let(*var, rhs, body),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => self.lower_case(scrutinee, alts),
            CoreExpr::Con { tag, fields, .. } => self.lower_constructor(tag, fields),
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
            CoreExpr::Dup { var, body, .. } => self.lower_dup(*var, body),
            CoreExpr::Drop { var, body, .. } => self.lower_drop(*var, body),
            CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                ..
            } => self.lower_reuse(*token, tag, fields, *field_mask),
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } => self.lower_drop_specialized(*scrutinee, unique_body, shared_body),
        }
    }

    fn lower_var(&mut self, var: CoreVarRef) -> Result<LlvmOperand, CoreToLlvmError> {
        if let Some(binder) = var.binder {
            if let Some(slot) = self.state.local_slots.get(&binder).cloned() {
                return self.load_slot_value(slot, "load");
            }

            if let Some(info) = self.program.top_level_info(binder).cloned() {
                let wrapper = self.program.ensure_top_level_wrapper(binder)?;
                return self.emit_make_closure_value(wrapper, info.arity as i32, vec![], vec![]);
            }
        }

        if var.binder.is_none()
            && let Some(arity) = self.program.adt_metadata.arity_for_constructor(var.name)
        {
            if arity == 0 {
                return self.lower_constructor(&CoreTag::Named(var.name), &[]);
            }
            return Err(self.unsupported(
                "first-class constructors",
                format!(
                    "constructor `{}` requires {} argument(s) and cannot be used as a bare value in Phase 5",
                    super::function::display_ident(var.name, self.state.interner),
                    arity
                ),
            ));
        }

        // Check if it's a known base function — return an error indicating it
        // should be called directly, not used as a first-class value (yet).
        let name_str = super::function::display_ident(var.name, self.state.interner);
        if super::builtins::find_builtin(&name_str).is_some() {
            return Err(self.unsupported(
                "first-class base functions",
                format!(
                    "base function `{name_str}` cannot be used as a value yet; call it directly"
                ),
            ));
        }

        Err(CoreToLlvmError::MissingSymbol {
            message: format!("unresolved local binding for `{name_str}`"),
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
                        "boxed integer literals are not lowered in Phase 4",
                    ));
                }
                Ok(const_i64(tagged_int_bits(*n)))
            }
            CoreLit::Float(f) => Ok(const_i64(f.to_bits() as i64)),
            CoreLit::Bool(value) => Ok(const_i64(tagged_bool_bits(*value))),
            CoreLit::Unit => Ok(const_i64(tagged_none_bits())),
            CoreLit::String(s) => self.lower_string_literal(s),
        }
    }

    /// Lower a string literal to a `flux_string_new(ptr, len)` call.
    fn lower_string_literal(&mut self, s: &str) -> Result<LlvmOperand, CoreToLlvmError> {
        // flux_string_new declaration is handled by compile_program_with_interner
        // when generated_string_globals is non-empty (special signature: ptr, i32 → i64).

        // Create a global constant for the string bytes.
        // Use program-wide counter to avoid name collisions across functions.
        let str_idx = self.program.generated_string_globals.len();
        let global_name = GlobalId(format!("flux.str.{str_idx}"));

        // Emit a global string constant: @flux.str.N = private constant [N x i8] c"..."
        self.program
            .generated_string_globals
            .push((global_name.clone(), s.to_string()));

        // Get pointer to the string data.
        let ptr_local = self.state.temp_local("str.ptr");
        self.state.emit(LlvmInstr::GetElementPtr {
            dst: ptr_local.clone(),
            inbounds: true,
            element_ty: LlvmType::Array {
                len: s.len() as u64,
                element: Box::new(LlvmType::i8()),
            },
            base: LlvmOperand::Global(global_name),
            indices: vec![
                (LlvmType::i32(), const_i32(0)),
                (LlvmType::i32(), const_i32(0)),
            ],
        });

        // Call flux_string_new(ptr, len) → i64.
        let dst = self.state.temp_local("str.val");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(GlobalId("flux_string_new".into())),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Local(ptr_local)),
                (LlvmType::i32(), const_i32(s.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    pub(super) fn lower_constructor(
        &mut self,
        tag: &CoreTag,
        fields: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        match tag {
            CoreTag::None => {
                if !fields.is_empty() {
                    return Err(CoreToLlvmError::Malformed {
                        message: "None constructor cannot carry fields".into(),
                    });
                }
                Ok(const_i64(tagged_none_bits()))
            }
            CoreTag::Nil => {
                if !fields.is_empty() {
                    return Err(CoreToLlvmError::Malformed {
                        message: "Nil constructor cannot carry fields".into(),
                    });
                }
                Ok(const_i64(tagged_empty_list_bits()))
            }
            CoreTag::Cons => {
                if fields.len() != 2 {
                    return Err(CoreToLlvmError::Malformed {
                        message: format!("Cons expects 2 fields, got {}", fields.len()),
                    });
                }
                let head = self.lower_expr_not_tail(&fields[0])?;
                let tail = self.lower_expr_not_tail(&fields[1])?;
                self.emit_make_cons_value(head, tail)
            }
            CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Named(_) => {
                let expected = self.program.adt_metadata.arity_for(tag).ok_or_else(|| {
                    CoreToLlvmError::MissingSymbol {
                        message: format!("missing ADT arity for constructor {:?}", tag),
                    }
                })?;
                if expected != fields.len() {
                    return Err(CoreToLlvmError::Malformed {
                        message: format!(
                            "constructor {:?} expects {} fields, got {}",
                            tag,
                            expected,
                            fields.len()
                        ),
                    });
                }
                let ctor_tag = self.program.adt_metadata.tag_for(tag).ok_or_else(|| {
                    CoreToLlvmError::MissingSymbol {
                        message: format!("missing ADT tag for constructor {:?}", tag),
                    }
                })?;
                let lowered = fields
                    .iter()
                    .map(|field| self.lower_expr_not_tail(field))
                    .collect::<Result<Vec<_>, _>>()?;
                self.emit_make_adt_value(ctor_tag, lowered)
            }
        }
    }

    fn lower_let(
        &mut self,
        binder: CoreBinder,
        rhs: &CoreExpr,
        body: &CoreExpr,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        // Phase 3 CPS: intercept non-tail self-recursive calls.
        if let Some(result) = self.try_lower_cps_let(binder, rhs, body)? {
            return Ok(result);
        }
        let rhs_value = self.lower_expr_not_tail(rhs)?;
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

        let old_slot = self.state.local_slots.insert(binder.id, slot);
        let old_name = self.state.binder_names.insert(binder.id, binder.name);
        let result = self.lower_expr(body);
        restore_local_binding(&mut self.state, binder.id, old_slot, old_name);
        result
    }

    fn lower_letrec_lambda(&mut self, expr: &CoreExpr) -> Result<LlvmOperand, CoreToLlvmError> {
        let CoreExpr::LetRec { var, rhs, body, .. } = expr else {
            unreachable!();
        };
        let CoreExpr::Lam {
            params,
            body: rhs_body,
            ..
        } = rhs.as_ref()
        else {
            return Err(self.unsupported(
                "local letrec",
                "only lambda letrec bindings are supported in Phase 4",
            ));
        };
        let slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: slot.clone(),
            ty: LlvmType::i64(),
            count: None,
            align: Some(8),
        });
        let old_slot = self.state.local_slots.insert(var.id, slot.clone());
        let old_name = self.state.binder_names.insert(var.id, var.name);
        let rhs_value = self.lower_lambda_value(params, rhs_body, Some(*var))?;
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i64(),
            value: rhs_value,
            ptr: LlvmOperand::Local(slot),
            align: Some(8),
        });
        let result = self.lower_expr(body);
        restore_local_binding(&mut self.state, var.id, old_slot, old_name);
        result
    }

    fn lower_lambda_value(
        &mut self,
        params: &[CoreBinder],
        body: &CoreExpr,
        recursive_binder: Option<CoreBinder>,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let lam = CoreExpr::Lam {
            params: params.to_vec(),
            body: Box::new(body.clone()),
            span: body.span(),
        };
        let available = self
            .state
            .local_slots
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let capture_ids = analyze_lambda_captures(
            &lam,
            body,
            params,
            recursive_binder.map(|binder| binder.id),
            &available,
        );
        let captures = capture_ids
            .into_iter()
            .map(|binder| {
                let name = self
                    .state
                    .binder_names
                    .get(&binder)
                    .copied()
                    .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                        message: format!("missing capture name for binder {:?}", binder),
                    })?;
                Ok(CoreBinder { id: binder, name })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let hint = self.state.symbol.0.clone();
        let symbol = self.program.fresh_lambda_symbol(&hint);
        let mut lowering = FunctionLowering::new_closure_entry(
            symbol.clone(),
            params,
            &captures,
            recursive_binder,
            self.program,
        )?;
        let result = lowering.lower_expr(body)?;
        let function = lowering.finish_with_return(result)?;
        self.program.push_generated_function(function);

        let capture_values = captures
            .iter()
            .map(|binder| {
                let slot = self
                    .state
                    .local_slots
                    .get(&binder.id)
                    .cloned()
                    .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                        message: format!("missing capture slot for binder {:?}", binder.id),
                    })?;
                self.load_slot_value(slot, "capture.load")
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.emit_make_closure_value(symbol, params.len() as i32, capture_values, vec![])
    }

    fn lower_call(
        &mut self,
        func: &CoreExpr,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if let Some(result) = self.try_lower_builtin_call(func, args)? {
            return Ok(result);
        }
        if let Some(tag) = self.resolve_constructor_call(func) {
            return self.lower_constructor(&tag, args);
        }

        if let Some((callee, arity, callee_binder)) = self.resolve_direct_call_target(func)
            && args.len() == arity
        {
            let lowered_args = args
                .iter()
                .map(|arg| self.lower_expr_not_tail(arg))
                .collect::<Result<Vec<_>, _>>()?;
            // Phase 1 TCO: convert tail self-calls into loop branches.
            if let Some(dummy) = self.try_lower_tco_self_call(&callee, &lowered_args) {
                return Ok(dummy);
            }
            // Phase 2 TCO: convert mutual tail calls into thunk returns.
            if let Some(result) = self.try_lower_mutual_tco_call(callee_binder, &lowered_args) {
                return result;
            }
            let is_self_recursive = callee.0 == self.state.symbol.0;
            let dst = self.state.temp_local("call");
            self.state.emit(LlvmInstr::Call {
                dst: Some(dst.clone()),
                tail: is_self_recursive,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::i64(),
                callee: LlvmOperand::Global(callee),
                args: lowered_args
                    .into_iter()
                    .map(|arg| (LlvmType::i64(), arg))
                    .collect(),
                attrs: vec![],
            });
            return Ok(LlvmOperand::Local(dst));
        }

        let callee = self.lower_expr_not_tail(func)?;
        let lowered_args = args
            .iter()
            .map(|arg| self.lower_expr_not_tail(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let args_ptr = self.emit_operand_array(&lowered_args, "call.args")?;
        let dst = self.state.temp_local("closure.call");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_closure_symbol("flux_call_closure")),
            args: vec![
                (LlvmType::i64(), callee),
                (LlvmType::ptr(), LlvmOperand::Local(args_ptr)),
                (LlvmType::i32(), const_i32(args.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    /// Lower an `AetherCall`: like a regular call, but with per-argument
    /// borrow modes.  For `Borrowed` args we skip the dup (the callee only
    /// reads them); for `Owned` args we emit `flux_dup` before passing.
    fn lower_aether_call(
        &mut self,
        func: &CoreExpr,
        args: &[CoreExpr],
        arg_modes: &[crate::aether::borrow_infer::BorrowMode],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        use crate::aether::borrow_infer::BorrowMode;

        // Check if it's a direct base function call first.
        if let Some(result) = self.try_lower_builtin_call(func, args)? {
            return Ok(result);
        }

        // For direct/constructor calls, fall through to the normal path —
        // the borrow annotations are an optimization hint that affects
        // whether the caller duplicates values, not how the call is made.
        if let Some(tag) = self.resolve_constructor_call(func) {
            return self.lower_constructor(&tag, args);
        }

        if let Some((callee, arity, callee_binder)) = self.resolve_direct_call_target(func)
            && args.len() == arity
        {
            let lowered_args = args
                .iter()
                .map(|arg| self.lower_expr_not_tail(arg))
                .collect::<Result<Vec<_>, _>>()?;
            // Emit dup for Owned args that need it.
            for (i, val) in lowered_args.iter().enumerate() {
                if i < arg_modes.len() && arg_modes[i] == BorrowMode::Owned {
                    self.state.emit(LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(flux_prelude_symbol("flux_dup")),
                        args: vec![(LlvmType::i64(), val.clone())],
                        attrs: vec![],
                    });
                }
            }
            // Phase 1 TCO: convert tail self-calls into loop branches.
            if let Some(dummy) = self.try_lower_tco_self_call(&callee, &lowered_args) {
                return Ok(dummy);
            }
            // Phase 2 TCO: convert mutual tail calls into thunk returns.
            if let Some(result) = self.try_lower_mutual_tco_call(callee_binder, &lowered_args) {
                return result;
            }
            let is_self_recursive = callee.0 == self.state.symbol.0;
            let dst = self.state.temp_local("acall");
            self.state.emit(LlvmInstr::Call {
                dst: Some(dst.clone()),
                tail: is_self_recursive,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::i64(),
                callee: LlvmOperand::Global(callee),
                args: lowered_args
                    .into_iter()
                    .map(|arg| (LlvmType::i64(), arg))
                    .collect(),
                attrs: vec![],
            });
            return Ok(LlvmOperand::Local(dst));
        }

        // Closure call path.
        let callee = self.lower_expr_not_tail(func)?;
        let lowered_args = args
            .iter()
            .map(|arg| self.lower_expr_not_tail(arg))
            .collect::<Result<Vec<_>, _>>()?;
        // Emit dup for Owned args.
        for (i, val) in lowered_args.iter().enumerate() {
            if i < arg_modes.len() && arg_modes[i] == BorrowMode::Owned {
                self.state.emit(LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_prelude_symbol("flux_dup")),
                    args: vec![(LlvmType::i64(), val.clone())],
                    attrs: vec![],
                });
            }
        }
        let args_ptr = self.emit_operand_array(&lowered_args, "acall.args")?;
        let dst = self.state.temp_local("acall.closure");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_closure_symbol("flux_call_closure")),
            args: vec![
                (LlvmType::i64(), callee),
                (LlvmType::ptr(), LlvmOperand::Local(args_ptr)),
                (LlvmType::i32(), const_i32(args.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    /// Try to resolve a call as a base function (e.g., `print`, `println`).
    /// Returns the lowered result if successful, None if not a base function.
    fn try_lower_builtin_call(
        &mut self,
        func: &CoreExpr,
        args: &[CoreExpr],
    ) -> Result<Option<LlvmOperand>, CoreToLlvmError> {
        let CoreExpr::Var { var, .. } = func else {
            return Ok(None);
        };
        // Base functions have binder = None (not user-defined).
        if var.binder.is_some() {
            return Ok(None);
        }
        let name_str = super::function::display_ident(var.name, self.state.interner);
        let Some(mapping) = super::builtins::find_builtin(&name_str) else {
            return Ok(None);
        };

        // Register the builtin for declaration emission.
        self.program.register_builtin(mapping);

        // Lower all arguments.
        let lowered_args: Vec<LlvmOperand> = args
            .iter()
            .map(|arg| self.lower_expr_not_tail(arg))
            .collect::<Result<Vec<_>, _>>()?;

        if mapping.returns_value {
            let dst = self.state.temp_local("builtin");
            self.state.emit(LlvmInstr::Call {
                dst: Some(dst.clone()),
                tail: false,
                call_conv: Some(CallConv::Ccc),
                ret_ty: LlvmType::i64(),
                callee: LlvmOperand::Global(GlobalId(mapping.c_name.into())),
                args: lowered_args
                    .into_iter()
                    .map(|a| (LlvmType::i64(), a))
                    .collect(),
                attrs: vec![],
            });
            Ok(Some(LlvmOperand::Local(dst)))
        } else {
            // Void function (e.g., print, println) — call once per argument
            // to match VM semantics where print(a, b) prints both values.
            let is_print_fn = mapping.c_name == "flux_print" || mapping.c_name == "flux_println";
            if is_print_fn && lowered_args.len() > 1 {
                // Multi-arg print: space-separated on one line, matching VM semantics.
                // Use flux_print_space for all but the last, flux_print for the last.
                self.program
                    .ensure_c_decl("flux_print_space", &[LlvmType::i64()], LlvmType::Void);
                let last_idx = lowered_args.len() - 1;
                for (i, arg) in lowered_args.into_iter().enumerate() {
                    let callee_name = if i < last_idx {
                        "flux_print_space"
                    } else {
                        mapping.c_name
                    };
                    self.state.emit(LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(GlobalId(callee_name.into())),
                        args: vec![(LlvmType::i64(), arg)],
                        attrs: vec![],
                    });
                }
            } else {
                self.state.emit(LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(GlobalId(mapping.c_name.into())),
                    args: lowered_args
                        .into_iter()
                        .map(|a| (LlvmType::i64(), a))
                        .collect(),
                    attrs: vec![],
                });
            }
            // Return None (unit) value.
            Ok(Some(const_i64(tagged_none_bits())))
        }
    }

    fn resolve_direct_call_target(
        &self,
        func: &CoreExpr,
    ) -> Option<(GlobalId, usize, CoreBinderId)> {
        match func {
            CoreExpr::Var { var, .. } => {
                let binder = var.binder?;
                if self.state.local_slots.contains_key(&binder) {
                    return None;
                }
                self.program
                    .top_level_info(binder)
                    .map(|info| (info.symbol.clone(), info.arity, binder))
            }
            _ => None,
        }
    }

    fn resolve_constructor_call(&self, func: &CoreExpr) -> Option<CoreTag> {
        let CoreExpr::Var { var, .. } = func else {
            return None;
        };
        if var.binder.is_some() {
            return None;
        }
        self.program
            .adt_metadata
            .arity_for_constructor(var.name)
            .map(|_| CoreTag::Named(var.name))
    }

    fn emit_make_closure_value(
        &mut self,
        fn_symbol: GlobalId,
        remaining_arity: i32,
        capture_values: Vec<LlvmOperand>,
        applied_values: Vec<LlvmOperand>,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let capture_ptr = self.emit_operand_array(&capture_values, "capture.array")?;
        let applied_ptr = self.emit_operand_array(&applied_values, "applied.array")?;
        let dst = self.state.temp_local("closure.make");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_closure_symbol("flux_make_closure")),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Global(fn_symbol)),
                (LlvmType::i32(), const_i32(remaining_arity)),
                (LlvmType::ptr(), LlvmOperand::Local(capture_ptr)),
                (LlvmType::i32(), const_i32(capture_values.len() as i32)),
                (LlvmType::ptr(), LlvmOperand::Local(applied_ptr)),
                (LlvmType::i32(), const_i32(applied_values.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn emit_make_adt_value(
        &mut self,
        ctor_tag: i32,
        field_values: Vec<LlvmOperand>,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let fields_ptr = self.emit_operand_array(&field_values, "adt.fields")?;
        let dst = self.state.temp_local("adt.make");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_make_adt")),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Local(fields_ptr)),
                (LlvmType::i32(), const_i32(field_values.len() as i32)),
                (LlvmType::i32(), const_i32(ctor_tag)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn emit_make_cons_value(
        &mut self,
        head: LlvmOperand,
        tail: LlvmOperand,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let dst = self.state.temp_local("cons.make");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_make_cons")),
            args: vec![(LlvmType::i64(), head), (LlvmType::i64(), tail)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn emit_make_tuple_value(
        &mut self,
        field_values: Vec<LlvmOperand>,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let fields_ptr = self.emit_operand_array(&field_values, "tuple.fields")?;
        let dst = self.state.temp_local("tuple.make");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_make_tuple")),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Local(fields_ptr)),
                (LlvmType::i32(), const_i32(field_values.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn emit_operand_array(
        &mut self,
        values: &[LlvmOperand],
        prefix: &str,
    ) -> Result<LlvmLocal, CoreToLlvmError> {
        let ptr = self.state.temp_local(prefix);
        let count = values.len().max(1) as i32;
        self.state.emit(LlvmInstr::Alloca {
            dst: ptr.clone(),
            ty: LlvmType::i64(),
            count: Some((LlvmType::i32(), const_i32(count))),
            align: Some(8),
        });
        for (index, value) in values.iter().enumerate() {
            let slot = self.state.temp_local(&format!("{prefix}.slot"));
            self.state.emit(LlvmInstr::GetElementPtr {
                dst: slot.clone(),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(ptr.clone()),
                indices: vec![(LlvmType::i32(), const_i32(index as i32))],
            });
            self.state.emit(LlvmInstr::Store {
                ty: LlvmType::i64(),
                value: value.clone(),
                ptr: LlvmOperand::Local(slot),
                align: Some(8),
            });
        }
        Ok(ptr)
    }

    pub(super) fn load_slot_value(
        &mut self,
        slot: LlvmLocal,
        prefix: &str,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let tmp = self.state.temp_local(prefix);
        self.state.emit(LlvmInstr::Load {
            dst: tmp.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(slot),
            align: Some(8),
        });
        Ok(LlvmOperand::Local(tmp))
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
                "guarded case alternatives are not lowered in Phase 4",
            ));
        }

        // Try to emit a switch on ADT constructor tags when all arms are
        // boxed constructor patterns (with an optional wildcard/var default).
        if let Some(result) = self.try_lower_case_switch(scrutinee, alts)? {
            return Ok(result);
        }

        let scrutinee = self.lower_expr_not_tail(scrutinee)?;
        self.lower_case_chain(scrutinee, alts)
    }

    /// Emit a `switch i32 %tag` when all case arms match on boxed ADT
    /// constructor patterns (Some/Left/Right/Cons/Named), optionally
    /// followed by a single wildcard or variable default.  Returns `None`
    /// if the pattern mix is not suitable for a switch.
    fn try_lower_case_switch(
        &mut self,
        scrutinee: &CoreExpr,
        alts: &[CoreAlt],
    ) -> Result<Option<LlvmOperand>, CoreToLlvmError> {
        // Classify each alt as a boxed‐constructor arm or a default arm.
        struct SwitchArm<'a> {
            tag_id: i32,
            fields: &'a [CorePat],
            rhs: &'a CoreExpr,
        }
        let mut ctor_arms: Vec<SwitchArm<'_>> = Vec::new();
        let mut default_alt: Option<&CoreAlt> = None;

        for alt in alts {
            match &alt.pat {
                CorePat::Con { tag, fields } => {
                    let tag_id = match tag {
                        CoreTag::Some
                        | CoreTag::Left
                        | CoreTag::Right
                        | CoreTag::Cons
                        | CoreTag::Named(_) => self.program.adt_metadata.tag_for(tag),
                        // None/Nil are immediate values, not boxed — can't switch on ADT tag
                        CoreTag::None | CoreTag::Nil => return Ok(None),
                    };
                    let Some(id) = tag_id else {
                        return Ok(None);
                    };
                    ctor_arms.push(SwitchArm {
                        tag_id: id,
                        fields,
                        rhs: &alt.rhs,
                    });
                }
                CorePat::Wildcard | CorePat::Var(_) => {
                    if default_alt.is_some() {
                        // Multiple defaults — fall back to chain
                        return Ok(None);
                    }
                    default_alt = Some(alt);
                }
                // Lit, EmptyList, Tuple patterns mixed with constructors — fall back
                _ => return Ok(None),
            }
        }

        // Need at least 2 constructor arms for a switch to be worthwhile
        if ctor_arms.len() < 2 {
            return Ok(None);
        }

        let scrutinee_val = self.lower_expr_not_tail(scrutinee)?;

        // Check is_ptr, then extract tag
        let is_ptr = self.emit_is_ptr_call(scrutinee_val.clone(), "switch.is_ptr")?;
        let tag_block_label = self.state.new_block_label("switch.tag");
        let tag_block_idx = self.state.push_block(tag_block_label.clone());
        let default_label = self.state.new_block_label("switch.default");
        let default_idx = self.state.push_block(default_label.clone());
        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: is_ptr,
            then_label: tag_block_label,
            else_label: default_label.clone(),
        });

        // Extract tag
        self.state.switch_to_block(tag_block_idx);
        let tag_local = self.state.temp_local("switch.adt.tag");
        self.state.emit(LlvmInstr::Call {
            dst: Some(tag_local.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i32(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_adt_tag")),
            args: vec![(LlvmType::i64(), scrutinee_val.clone())],
            attrs: vec![],
        });

        // Build case labels and blocks for each constructor arm
        let join_label = self.state.new_block_label("case.join");
        let join_idx = self.state.push_block(join_label.clone());
        let mut switch_cases: Vec<(LlvmConst, LabelId)> = Vec::new();
        let mut arm_blocks: Vec<(usize, &SwitchArm<'_>)> = Vec::new();

        for arm in &ctor_arms {
            let arm_label = self.state.new_block_label("switch.arm");
            let arm_idx = self.state.push_block(arm_label.clone());
            switch_cases.push((
                LlvmConst::Int {
                    bits: 32,
                    value: arm.tag_id.into(),
                },
                arm_label,
            ));
            arm_blocks.push((arm_idx, arm));
        }

        // Emit the switch terminator
        self.state.set_terminator(LlvmTerminator::Switch {
            ty: LlvmType::i32(),
            scrutinee: LlvmOperand::Local(tag_local),
            default: default_label.clone(),
            cases: switch_cases,
        });

        // Lower each constructor arm: extract fields, lower RHS
        let mut incoming = Vec::new();
        for (arm_idx, arm) in arm_blocks {
            self.state.switch_to_block(arm_idx);
            let mut restores = Vec::new();

            // Extract and bind fields
            if !arm.fields.is_empty() {
                let field_success = self.state.new_block_label("switch.fields");
                let field_idx = self.state.push_block(field_success.clone());
                self.emit_field_pattern_chain(
                    scrutinee_val.clone(),
                    arm.fields,
                    field_success.clone(),
                    default_label.clone(),
                    &mut restores,
                    |this, base, index| this.load_adt_field(base, index),
                )?;
                self.state.switch_to_block(field_idx);
            }

            let value = self.lower_expr(arm.rhs)?;
            if self.state.current_block_open() {
                let from = self.state.current_block_label();
                self.state.set_terminator(LlvmTerminator::Br {
                    target: join_label.clone(),
                });
                incoming.push((value, from));
            }
            self.restore_bindings(restores);
        }

        // Lower the default arm
        self.state.switch_to_block(default_idx);
        if let Some(alt) = default_alt {
            let mut restores = Vec::new();
            if let CorePat::Var(binder) = &alt.pat {
                self.bind_pattern_var(*binder, scrutinee_val.clone(), &mut restores);
            }
            let value = self.lower_expr(&alt.rhs)?;
            if self.state.current_block_open() {
                let from = self.state.current_block_label();
                self.state.set_terminator(LlvmTerminator::Br {
                    target: join_label.clone(),
                });
                incoming.push((value, from));
            }
            self.restore_bindings(restores);
        } else {
            self.state.set_terminator(LlvmTerminator::Unreachable);
        }

        // Join
        self.state.switch_to_block(join_idx);
        if incoming.is_empty() {
            // All arms terminated (e.g., all tail-called back to the TCO loop).
            self.state.set_terminator(LlvmTerminator::Unreachable);
            return Ok(Some(const_i64(0)));
        }
        let phi = self.state.temp_local("case.result");
        self.state.emit(LlvmInstr::Phi {
            dst: phi.clone(),
            ty: LlvmType::i64(),
            incoming,
        });
        Ok(Some(LlvmOperand::Local(phi)))
    }

    fn lower_case_chain(
        &mut self,
        scrutinee: LlvmOperand,
        alts: &[CoreAlt],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let join_label = self.state.new_block_label("case.join");
        let join_idx = self.state.push_block(join_label.clone());
        let mut incoming = Vec::new();
        let mut active_block = self.state.current_block;

        for alt in alts {
            self.state.switch_to_block(active_block);
            let then_label = self.state.new_block_label("case.then");
            let then_idx = self.state.push_block(then_label.clone());
            let else_label = self.state.new_block_label("case.else");
            let else_idx = self.state.push_block(else_label.clone());
            let mut restores = Vec::new();
            self.emit_pattern_branch(
                scrutinee.clone(),
                &alt.pat,
                then_label.clone(),
                else_label.clone(),
                &mut restores,
            )?;

            self.state.switch_to_block(then_idx);
            let value = self.lower_expr(&alt.rhs)?;
            if self.state.current_block_open() {
                let from = self.state.current_block_label();
                self.state.set_terminator(LlvmTerminator::Br {
                    target: join_label.clone(),
                });
                incoming.push((value, from));
            }
            self.restore_bindings(restores);
            active_block = else_idx;
        }

        self.state.switch_to_block(active_block);
        if self.state.current_block_open() {
            self.state.set_terminator(LlvmTerminator::Unreachable);
        }
        self.state.switch_to_block(join_idx);
        if incoming.is_empty() {
            // All arms terminated (e.g., all tail-called back to the TCO loop).
            // The join block is unreachable dead code.
            self.state.set_terminator(LlvmTerminator::Unreachable);
            return Ok(const_i64(0));
        }
        let phi = self.state.temp_local("case.result");
        self.state.emit(LlvmInstr::Phi {
            dst: phi.clone(),
            ty: LlvmType::i64(),
            incoming,
        });
        Ok(LlvmOperand::Local(phi))
    }

    fn emit_pattern_branch(
        &mut self,
        value: LlvmOperand,
        pat: &CorePat,
        success_label: crate::core_to_llvm::LabelId,
        fail_label: crate::core_to_llvm::LabelId,
        restores: &mut Vec<BindingRestore>,
    ) -> Result<(), CoreToLlvmError> {
        match pat {
            CorePat::Wildcard => {
                self.state.set_terminator(LlvmTerminator::Br {
                    target: success_label,
                });
            }
            CorePat::Var(binder) => {
                self.bind_pattern_var(*binder, value, restores);
                self.state.set_terminator(LlvmTerminator::Br {
                    target: success_label,
                });
            }
            CorePat::Lit(lit) => {
                let cond = self.emit_literal_match_cond(value, lit)?;
                self.state.set_terminator(LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond,
                    then_label: success_label,
                    else_label: fail_label,
                });
            }
            CorePat::EmptyList => {
                let cond = self.emit_immediate_eq(value, tagged_empty_list_bits(), "case.empty");
                self.state.set_terminator(LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond,
                    then_label: success_label,
                    else_label: fail_label,
                });
            }
            CorePat::Con { tag, fields } => match tag {
                CoreTag::None => {
                    if !fields.is_empty() {
                        return Err(CoreToLlvmError::Malformed {
                            message: "None pattern cannot bind fields".into(),
                        });
                    }
                    let cond = self.emit_immediate_eq(value, tagged_none_bits(), "case.none");
                    self.state.set_terminator(LlvmTerminator::CondBr {
                        cond_ty: LlvmType::i1(),
                        cond,
                        then_label: success_label,
                        else_label: fail_label,
                    });
                }
                CoreTag::Nil => {
                    if !fields.is_empty() {
                        return Err(CoreToLlvmError::Malformed {
                            message: "Nil pattern cannot bind fields".into(),
                        });
                    }
                    let cond = self.emit_immediate_eq(value, tagged_empty_list_bits(), "case.nil");
                    self.state.set_terminator(LlvmTerminator::CondBr {
                        cond_ty: LlvmType::i1(),
                        cond,
                        then_label: success_label,
                        else_label: fail_label,
                    });
                }
                CoreTag::Some
                | CoreTag::Left
                | CoreTag::Right
                | CoreTag::Cons
                | CoreTag::Named(_) => {
                    let expected_arity =
                        self.program.adt_metadata.arity_for(tag).ok_or_else(|| {
                            CoreToLlvmError::MissingSymbol {
                                message: format!("missing constructor arity for pattern {:?}", tag),
                            }
                        })?;
                    if expected_arity != fields.len() {
                        return Err(CoreToLlvmError::Malformed {
                            message: format!(
                                "pattern for {:?} expects {} fields, got {}",
                                tag,
                                expected_arity,
                                fields.len()
                            ),
                        });
                    }
                    let expected_tag = self.program.adt_metadata.tag_for(tag).ok_or_else(|| {
                        CoreToLlvmError::MissingSymbol {
                            message: format!("missing constructor tag for pattern {:?}", tag),
                        }
                    })?;
                    let check_label = self.state.new_block_label("case.ctor");
                    let check_idx = self.state.push_block(check_label.clone());
                    self.emit_boxed_tag_branch(
                        value.clone(),
                        expected_tag,
                        check_label,
                        fail_label.clone(),
                        "case.adt",
                    )?;
                    self.state.switch_to_block(check_idx);
                    self.emit_field_pattern_chain(
                        value,
                        fields,
                        success_label,
                        fail_label,
                        restores,
                        |this, base, index| this.load_adt_field(base, index),
                    )?;
                }
            },
            CorePat::Tuple(fields) => {
                let check_label = self.state.new_block_label("case.tuple");
                let check_idx = self.state.push_block(check_label.clone());
                self.emit_tuple_arity_branch(
                    value.clone(),
                    fields.len() as i32,
                    check_label,
                    fail_label.clone(),
                )?;
                self.state.switch_to_block(check_idx);
                self.emit_field_pattern_chain(
                    value,
                    fields,
                    success_label,
                    fail_label,
                    restores,
                    |this, base, index| this.load_tuple_field(base, index),
                )?;
            }
        }
        Ok(())
    }

    fn emit_field_pattern_chain<F>(
        &mut self,
        base: LlvmOperand,
        fields: &[CorePat],
        success_label: crate::core_to_llvm::LabelId,
        fail_label: crate::core_to_llvm::LabelId,
        restores: &mut Vec<BindingRestore>,
        mut load_field: F,
    ) -> Result<(), CoreToLlvmError>
    where
        F: FnMut(&mut Self, LlvmOperand, usize) -> Result<LlvmOperand, CoreToLlvmError>,
    {
        if fields.is_empty() {
            self.state.set_terminator(LlvmTerminator::Br {
                target: success_label,
            });
            return Ok(());
        }

        for (index, field_pat) in fields.iter().enumerate() {
            let field_value = load_field(self, base.clone(), index)?;
            let is_last = index + 1 == fields.len();
            let next_label = if is_last {
                success_label.clone()
            } else {
                self.state.new_block_label("case.field")
            };
            let next_idx = if is_last {
                None
            } else {
                Some(self.state.push_block(next_label.clone()))
            };
            self.emit_pattern_branch(
                field_value,
                field_pat,
                next_label,
                fail_label.clone(),
                restores,
            )?;
            if let Some(idx) = next_idx {
                self.state.switch_to_block(idx);
            }
        }
        Ok(())
    }

    fn emit_literal_match_cond(
        &mut self,
        value: LlvmOperand,
        lit: &CoreLit,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let rhs = match lit {
            CoreLit::Bool(flag) => const_i64(tagged_bool_bits(*flag)),
            CoreLit::Int(n) => {
                if !(FluxNanboxLayout::MIN_INLINE_INT..=FluxNanboxLayout::MAX_INLINE_INT)
                    .contains(n)
                {
                    return Err(self.unsupported(
                        "large integer literal patterns",
                        "boxed integer patterns are not lowered in Phase 5",
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
        Ok(self.emit_icmp_eq(value, rhs, "case.lit"))
    }

    fn emit_immediate_eq(&mut self, lhs: LlvmOperand, rhs_bits: i64, prefix: &str) -> LlvmOperand {
        self.emit_icmp_eq(lhs, const_i64(rhs_bits), prefix)
    }

    fn emit_icmp_eq(&mut self, lhs: LlvmOperand, rhs: LlvmOperand, prefix: &str) -> LlvmOperand {
        let cond = self.state.temp_local(prefix);
        self.state.emit(LlvmInstr::Icmp {
            dst: cond.clone(),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::i64(),
            lhs,
            rhs,
        });
        LlvmOperand::Local(cond)
    }

    fn emit_boxed_tag_branch(
        &mut self,
        value: LlvmOperand,
        expected_tag: i32,
        success_label: crate::core_to_llvm::LabelId,
        fail_label: crate::core_to_llvm::LabelId,
        prefix: &str,
    ) -> Result<(), CoreToLlvmError> {
        let is_ptr = self.emit_is_ptr_call(value.clone(), &format!("{prefix}.is_ptr"))?;
        let tag_block_label = self.state.new_block_label(&format!("{prefix}.check"));
        let tag_block_idx = self.state.push_block(tag_block_label.clone());
        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: is_ptr,
            then_label: tag_block_label,
            else_label: fail_label.clone(),
        });
        self.state.switch_to_block(tag_block_idx);
        let tag = self.state.temp_local(&format!("{prefix}.tag"));
        self.state.emit(LlvmInstr::Call {
            dst: Some(tag.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i32(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_adt_tag")),
            args: vec![(LlvmType::i64(), value)],
            attrs: vec![],
        });
        let matches = self.state.temp_local(&format!("{prefix}.matches"));
        self.state.emit(LlvmInstr::Icmp {
            dst: matches.clone(),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::i32(),
            lhs: LlvmOperand::Local(tag),
            rhs: const_i32(expected_tag),
        });
        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(matches),
            then_label: success_label,
            else_label: fail_label,
        });
        Ok(())
    }

    fn emit_tuple_arity_branch(
        &mut self,
        value: LlvmOperand,
        expected_arity: i32,
        success_label: crate::core_to_llvm::LabelId,
        fail_label: crate::core_to_llvm::LabelId,
    ) -> Result<(), CoreToLlvmError> {
        let is_ptr = self.emit_is_ptr_call(value.clone(), "case.tuple.is_ptr")?;
        let len_block_label = self.state.new_block_label("case.tuple.len");
        let len_block_idx = self.state.push_block(len_block_label.clone());
        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: is_ptr,
            then_label: len_block_label,
            else_label: fail_label.clone(),
        });
        self.state.switch_to_block(len_block_idx);
        let len = self.state.temp_local("case.tuple.arity");
        self.state.emit(LlvmInstr::Call {
            dst: Some(len.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i32(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_tuple_len")),
            args: vec![(LlvmType::i64(), value)],
            attrs: vec![],
        });
        let matches = self.state.temp_local("case.tuple.matches");
        self.state.emit(LlvmInstr::Icmp {
            dst: matches.clone(),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::i32(),
            lhs: LlvmOperand::Local(len),
            rhs: const_i32(expected_arity),
        });
        self.state.set_terminator(LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(matches),
            then_label: success_label,
            else_label: fail_label,
        });
        Ok(())
    }

    fn emit_is_ptr_call(
        &mut self,
        value: LlvmOperand,
        prefix: &str,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let dst = self.state.temp_local(prefix);
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i1(),
            callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
            args: vec![(LlvmType::i64(), value)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn load_adt_field(
        &mut self,
        base: LlvmOperand,
        index: usize,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let ptr = self.state.temp_local("case.adt.field.ptr");
        self.state.emit(LlvmInstr::Call {
            dst: Some(ptr.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_adt_field_ptr")),
            args: vec![
                (LlvmType::i64(), base),
                (LlvmType::i32(), const_i32(index as i32)),
            ],
            attrs: vec![],
        });
        let val = self.state.temp_local("case.adt.field");
        self.state.emit(LlvmInstr::Load {
            dst: val.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(ptr),
            align: Some(8),
        });
        Ok(LlvmOperand::Local(val))
    }

    fn load_tuple_field(
        &mut self,
        base: LlvmOperand,
        index: usize,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let ptr = self.state.temp_local("case.tuple.field.ptr");
        self.state.emit(LlvmInstr::Call {
            dst: Some(ptr.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(flux_adt_symbol("flux_tuple_field_ptr")),
            args: vec![
                (LlvmType::i64(), base),
                (LlvmType::i32(), const_i32(index as i32)),
            ],
            attrs: vec![],
        });
        let val = self.state.temp_local("case.tuple.field");
        self.state.emit(LlvmInstr::Load {
            dst: val.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(ptr),
            align: Some(8),
        });
        Ok(LlvmOperand::Local(val))
    }

    /// Lower `Concat` (string `++`) to a C runtime call.
    fn lower_concat_primop(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        self.lower_c_runtime_call("flux_string_concat", args, true)
    }

    /// Lower `Interpolate` to sequential string concatenation via `flux_to_string` + `flux_string_concat`.
    fn lower_interpolate_primop(
        &mut self,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.is_empty() {
            return self.lower_string_literal("");
        }
        // Convert first arg to string.
        let first = self.lower_expr(&args[0])?;
        let mut result = self.emit_c_call("flux_to_string", &[first], true)?;
        // Concatenate remaining args.
        for arg in &args[1..] {
            let val = self.lower_expr(arg)?;
            let s = self.emit_c_call("flux_to_string", &[val], true)?;
            result = self.emit_c_call("flux_string_concat", &[result, s], true)?;
        }
        Ok(result)
    }

    /// Lower `MakeArray` to `flux_array_new(ptr, len)`.
    fn lower_make_array(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        let lowered: Vec<LlvmOperand> = args
            .iter()
            .map(|a| self.lower_expr(a))
            .collect::<Result<Vec<_>, _>>()?;
        let arr_ptr = self.emit_operand_array(&lowered, "arr.elems")?;
        let dst = self.state.temp_local("arr.new");
        self.ensure_c_decl(
            "flux_array_new",
            &[LlvmType::ptr(), LlvmType::i32()],
            LlvmType::i64(),
        );
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(GlobalId("flux_array_new".into())),
            args: vec![
                (LlvmType::ptr(), LlvmOperand::Local(arr_ptr)),
                (LlvmType::i32(), const_i32(lowered.len() as i32)),
            ],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    /// Lower `MakeHash` to sequential `flux_hamt_set` calls.
    fn lower_make_hash(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        // MakeHash args come in key-value pairs.
        let mut map = self.emit_c_call_no_args("flux_hamt_empty", true)?;
        let mut i = 0;
        while i + 1 < args.len() {
            let key = self.lower_expr(&args[i])?;
            let val = self.lower_expr(&args[i + 1])?;
            map = self.emit_c_call("flux_hamt_set", &[map, key, val], true)?;
            i += 2;
        }
        Ok(map)
    }

    /// Lower `Index` (e.g., `arr[i]` or `map[k]`) to a C runtime call.
    fn lower_index_primop(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 2 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("Index expects 2 args, got {}", args.len()),
            });
        }
        let collection = self.lower_expr(&args[0])?;
        let index = self.lower_expr(&args[1])?;
        // Runtime dispatch: checks collection type (array, HAMT, tuple, string).
        self.emit_c_call("flux_rt_index", &[collection, index], true)
    }

    /// Emit a call to a C runtime function, ensuring it's declared.
    fn lower_c_runtime_call(
        &mut self,
        name: &str,
        args: &[CoreExpr],
        returns_value: bool,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let lowered: Vec<LlvmOperand> = args
            .iter()
            .map(|a| self.lower_expr(a))
            .collect::<Result<Vec<_>, _>>()?;
        self.emit_c_call(name, &lowered, returns_value)
    }

    /// Emit a C call with already-lowered operands.
    fn emit_c_call(
        &mut self,
        name: &str,
        args: &[LlvmOperand],
        returns_value: bool,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        let params: Vec<LlvmType> = args.iter().map(|_| LlvmType::i64()).collect();
        let ret = if returns_value {
            LlvmType::i64()
        } else {
            LlvmType::Void
        };
        self.ensure_c_decl(name, &params, ret.clone());

        if returns_value {
            let dst = self.state.temp_local("rt");
            self.state.emit(LlvmInstr::Call {
                dst: Some(dst.clone()),
                tail: false,
                call_conv: Some(CallConv::Ccc),
                ret_ty: LlvmType::i64(),
                callee: LlvmOperand::Global(GlobalId(name.into())),
                args: args.iter().map(|a| (LlvmType::i64(), a.clone())).collect(),
                attrs: vec![],
            });
            Ok(LlvmOperand::Local(dst))
        } else {
            self.state.emit(LlvmInstr::Call {
                dst: None,
                tail: false,
                call_conv: Some(CallConv::Ccc),
                ret_ty: LlvmType::Void,
                callee: LlvmOperand::Global(GlobalId(name.into())),
                args: args.iter().map(|a| (LlvmType::i64(), a.clone())).collect(),
                attrs: vec![],
            });
            Ok(const_i64(tagged_none_bits()))
        }
    }

    /// Emit a C call with no arguments.
    fn emit_c_call_no_args(
        &mut self,
        name: &str,
        returns_value: bool,
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        self.emit_c_call(name, &[], returns_value)
    }

    /// Ensure a C function is declared in the module.
    fn ensure_c_decl(&mut self, name: &str, params: &[LlvmType], ret: LlvmType) {
        // Track as a custom declaration request.
        self.program.ensure_c_decl(name, params, ret);
    }

    fn bind_pattern_var(
        &mut self,
        binder: CoreBinder,
        value: LlvmOperand,
        restores: &mut Vec<BindingRestore>,
    ) {
        let slot = self.state.new_slot();
        self.state.emit_entry_alloca(LlvmInstr::Alloca {
            dst: slot.clone(),
            ty: LlvmType::i64(),
            count: None,
            align: Some(8),
        });
        self.state.emit(LlvmInstr::Store {
            ty: LlvmType::i64(),
            value,
            ptr: LlvmOperand::Local(slot.clone()),
            align: Some(8),
        });
        restores.push(BindingRestore {
            binder: binder.id,
            old_slot: self.state.local_slots.insert(binder.id, slot),
            old_name: self.state.binder_names.insert(binder.id, binder.name),
        });
    }

    fn restore_bindings(&mut self, restores: Vec<BindingRestore>) {
        for restore in restores.into_iter().rev() {
            restore_local_binding(
                &mut self.state,
                restore.binder,
                restore.old_slot,
                restore.old_name,
            );
        }
    }

    fn lower_primop(
        &mut self,
        op: &CorePrimOp,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        match op {
            CorePrimOp::Add => self.lower_rt_call("flux_rt_add", args),
            CorePrimOp::Sub => self.lower_rt_call("flux_rt_sub", args),
            CorePrimOp::Mul => self.lower_rt_call("flux_rt_mul", args),
            CorePrimOp::Div => self.lower_rt_call("flux_rt_div", args),
            CorePrimOp::IAdd => self.lower_rt_call("flux_rt_add", args),
            CorePrimOp::ISub => self.lower_rt_call("flux_rt_sub", args),
            CorePrimOp::IMul => self.lower_rt_call("flux_rt_mul", args),
            CorePrimOp::IDiv => self.lower_rt_call("flux_rt_div", args),
            CorePrimOp::FAdd => self.lower_helper_call("flux_fadd", args),
            CorePrimOp::FSub => self.lower_helper_call("flux_fsub", args),
            CorePrimOp::FMul => self.lower_helper_call("flux_fmul", args),
            CorePrimOp::FDiv => self.lower_helper_call("flux_fdiv", args),
            CorePrimOp::Mod => self.lower_rt_call("flux_rt_mod", args),
            CorePrimOp::IMod => self.lower_rt_call("flux_rt_mod", args),
            CorePrimOp::Neg => self.lower_rt_unary_call("flux_rt_neg", args),
            CorePrimOp::Not => self.lower_unary_helper_call("flux_not", args),
            CorePrimOp::And => self.lower_helper_call("flux_and", args),
            CorePrimOp::Or => self.lower_helper_call("flux_or", args),
            CorePrimOp::Eq => self.lower_rt_call("flux_rt_eq", args),
            CorePrimOp::NEq => self.lower_rt_call("flux_rt_neq", args),
            CorePrimOp::Lt => self.lower_rt_call("flux_rt_lt", args),
            CorePrimOp::Le => self.lower_rt_call("flux_rt_le", args),
            CorePrimOp::Gt => self.lower_rt_call("flux_rt_gt", args),
            CorePrimOp::Ge => self.lower_rt_call("flux_rt_ge", args),
            CorePrimOp::MakeList => self.lower_make_list(args),
            CorePrimOp::MakeTuple => self.lower_make_tuple(args),
            CorePrimOp::TupleField(index) => self.lower_tuple_field(*index, args),
            CorePrimOp::Concat => self.lower_concat_primop(args),
            CorePrimOp::Interpolate => self.lower_interpolate_primop(args),
            CorePrimOp::MakeArray => self.lower_make_array(args),
            CorePrimOp::MakeHash => self.lower_make_hash(args),
            CorePrimOp::Index => self.lower_index_primop(args),
            CorePrimOp::MemberAccess(member) => {
                // Module member access: resolve to the function by name.
                // Must use ensure_top_level_wrapper to get a closure entry
                // with the correct (i64, ptr, i32) calling convention.
                if let Some((binder, info)) = self.program.top_level_by_name_with_binder(*member) {
                    let wrapper = self.program.ensure_top_level_wrapper(binder)?;
                    let arity = info.arity as i32;
                    self.emit_make_closure_value(wrapper, arity, vec![], vec![])
                } else {
                    // Try as a builtin.
                    if let Some(result) = self.try_lower_builtin_call(
                        &CoreExpr::Var {
                            var: crate::core::CoreVarRef {
                                name: *member,
                                binder: None,
                            },
                            span: crate::diagnostics::position::Span::default(),
                        },
                        &[],
                    )? {
                        return Ok(result);
                    }
                    let member_name = super::function::display_ident(*member, self.state.interner);
                    Err(self.unsupported(
                        "primop",
                        format!("MemberAccess `{member_name}` not found in compiled defs"),
                    ))
                }
            }
        }
    }

    fn lower_make_list(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        let mut list = const_i64(tagged_empty_list_bits());
        for arg in args.iter().rev() {
            let head = self.lower_expr(arg)?;
            list = self.emit_make_cons_value(head, list)?;
        }
        Ok(list)
    }

    fn lower_make_tuple(&mut self, args: &[CoreExpr]) -> Result<LlvmOperand, CoreToLlvmError> {
        let fields = args
            .iter()
            .map(|arg| self.lower_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        self.emit_make_tuple_value(fields)
    }

    fn lower_tuple_field(
        &mut self,
        index: usize,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 1 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("TupleField expects 1 arg, got {}", args.len()),
            });
        }
        let tuple = self.lower_expr(&args[0])?;
        self.load_tuple_field(tuple, index)
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
        let left = self.lower_expr_not_tail(&args[0])?;
        let right = self.lower_expr_not_tail(&args[1])?;
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

    fn lower_unary_helper_call(
        &mut self,
        name: &str,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 1 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("helper `{name}` expected 1 arg, got {}", args.len()),
            });
        }
        let operand = self.lower_expr_not_tail(&args[0])?;
        let dst = self.state.temp_local("primop");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(flux_arith_symbol(name)),
            args: vec![(LlvmType::i64(), operand)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    /// Call a C runtime dispatch function (Ccc calling convention).
    fn lower_rt_call(
        &mut self,
        name: &str,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 2 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("rt `{name}` expected 2 args, got {}", args.len()),
            });
        }
        let left = self.lower_expr_not_tail(&args[0])?;
        let right = self.lower_expr_not_tail(&args[1])?;
        self.program
            .ensure_c_decl(name, &[LlvmType::i64(), LlvmType::i64()], LlvmType::i64());
        let dst = self.state.temp_local("primop");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(GlobalId(name.into())),
            args: vec![(LlvmType::i64(), left), (LlvmType::i64(), right)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    fn lower_rt_unary_call(
        &mut self,
        name: &str,
        args: &[CoreExpr],
    ) -> Result<LlvmOperand, CoreToLlvmError> {
        if args.len() != 1 {
            return Err(CoreToLlvmError::Malformed {
                message: format!("rt `{name}` expected 1 arg, got {}", args.len()),
            });
        }
        let operand = self.lower_expr_not_tail(&args[0])?;
        self.program
            .ensure_c_decl(name, &[LlvmType::i64()], LlvmType::i64());
        let dst = self.state.temp_local("primop");
        self.state.emit(LlvmInstr::Call {
            dst: Some(dst.clone()),
            tail: false,
            call_conv: Some(CallConv::Ccc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(GlobalId(name.into())),
            args: vec![(LlvmType::i64(), operand)],
            attrs: vec![],
        });
        Ok(LlvmOperand::Local(dst))
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

fn top_level_symbols(program: &ProgramState<'_>) -> HashMap<CoreBinderId, GlobalId> {
    program
        .top_level
        .iter()
        .map(|(binder, info)| (*binder, info.symbol.clone()))
        .collect()
}

fn restore_local_binding(
    state: &mut FunctionState<'_>,
    binder: CoreBinderId,
    old_slot: Option<LlvmLocal>,
    old_name: Option<crate::syntax::Identifier>,
) {
    if let Some(previous) = old_slot {
        state.local_slots.insert(binder, previous);
    } else {
        state.local_slots.remove(&binder);
    }
    if let Some(previous) = old_name {
        state.binder_names.insert(binder, previous);
    } else {
        state.binder_names.remove(&binder);
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

fn const_i32(value: i32) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 32,
        value: value.into(),
    })
}

pub(super) fn const_i64(value: i64) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: value.into(),
    })
}
