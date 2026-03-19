use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::cfg::{
    BlockId, IrBinaryOp, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr, IrListTest,
    IrTagTest, IrTerminator, IrVar,
};
use crate::{
    bytecode::op_code::OpCode,
    diagnostics::{Diagnostic, DiagnosticBuilder},
    runtime::{handler_descriptor::HandlerDescriptor, value::Value},
    syntax::symbol::Symbol,
};

use super::{CompileResult, Compiler};

impl Compiler {
    #[allow(dead_code)]
    pub(super) fn collect_tail_call_spans_for_ir_function(
        function: &IrFunction,
    ) -> Vec<crate::diagnostics::position::Span> {
        function
            .blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                IrTerminator::TailCall { metadata, .. } => metadata.span,
                _ => None,
            })
            .collect()
    }

    #[allow(dead_code)]
    pub(super) fn can_compile_ir_cfg_function(function: &IrFunction) -> bool {
        fn supported_expr(expr: &IrExpr) -> bool {
            match expr {
                IrExpr::Const(IrConst::Int(_))
                | IrExpr::Const(IrConst::Float(_))
                | IrExpr::Const(IrConst::Bool(_))
                | IrExpr::Const(IrConst::String(_))
                | IrExpr::Const(IrConst::Unit)
                | IrExpr::Var(_)
                | IrExpr::None
                | IrExpr::TupleFieldAccess { .. }
                | IrExpr::TupleArityTest { .. }
                | IrExpr::TagTest { .. }
                | IrExpr::TagPayload { .. }
                | IrExpr::ListTest { .. }
                | IrExpr::ListHead { .. }
                | IrExpr::ListTail { .. }
                | IrExpr::AdtTagTest { .. }
                | IrExpr::AdtField { .. }
                | IrExpr::MakeClosure(_, _)
                | IrExpr::Perform { .. } => true,
                IrExpr::Binary(op, _, _) => matches!(
                    op,
                    IrBinaryOp::Add
                        | IrBinaryOp::Sub
                        | IrBinaryOp::Mul
                        | IrBinaryOp::Div
                        | IrBinaryOp::Eq
                        | IrBinaryOp::NotEq
                        | IrBinaryOp::Lt
                        | IrBinaryOp::Gt
                        | IrBinaryOp::Ge
                        | IrBinaryOp::Le
                ),
                _ => false,
            }
        }

        let Some(entry_index) = function
            .blocks
            .iter()
            .position(|block| block.id == function.entry)
        else {
            return false;
        };
        if entry_index != 0 {
            return false;
        }
        let block_indices: HashMap<_, _> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.id, index))
            .collect();

        for (index, block) in function.blocks.iter().enumerate() {
            if !block.instrs.iter().all(|instr| match instr {
                IrInstr::Assign { expr, .. } => supported_expr(expr),
                IrInstr::Call { target, .. } => {
                    matches!(
                        target,
                        IrCallTarget::Named(_) | IrCallTarget::Direct(_) | IrCallTarget::Var(_)
                    )
                }
                IrInstr::HandleScope { .. } => true,
                IrInstr::AetherDrop { .. } => true,
            }) {
                return false;
            }

            match &block.terminator {
                IrTerminator::Return(..) | IrTerminator::TailCall { .. } => {}
                IrTerminator::Jump(target, args, _) => {
                    let Some(target_index) = block_indices.get(target).copied() else {
                        return false;
                    };
                    if target_index <= index {
                        return false;
                    }
                    if function.blocks[target_index].params.len() != args.len() {
                        return false;
                    }
                }
                IrTerminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    let Some(then_index) = block_indices.get(then_block).copied() else {
                        return false;
                    };
                    let Some(else_index) = block_indices.get(else_block).copied() else {
                        return false;
                    };
                    if then_index != index + 1 || else_index <= index {
                        return false;
                    }
                    if !function.blocks[then_index].params.is_empty()
                        || !function.blocks[else_index].params.is_empty()
                    {
                        return false;
                    }
                }
                IrTerminator::Unreachable(_) => return false,
            }
        }

        matches!(
            function.blocks.last().map(|block| &block.terminator),
            Some(IrTerminator::Return(..) | IrTerminator::TailCall { .. })
        )
    }

    #[allow(dead_code)]
    pub(super) fn try_compile_ir_cfg_function_body(
        &mut self,
        function: &IrFunction,
        current_name: Symbol,
    ) -> Option<CompileResult<()>> {
        if !Self::can_compile_ir_cfg_function(function) {
            return None;
        }

        let mut bindings: HashMap<IrVar, crate::bytecode::binding::Binding> = HashMap::new();
        for param in &function.params {
            let binding = self.symbol_table.resolve(param.name)?;
            bindings.insert(param.var, binding);
        }
        for block in &function.blocks {
            for param in &block.params {
                bindings
                    .entry(param.var)
                    .or_insert_with(|| self.symbol_table.define_temp());
            }
            for instr in &block.instrs {
                match instr {
                    IrInstr::Assign { dest, .. }
                    | IrInstr::Call { dest, .. }
                    | IrInstr::HandleScope { dest, .. } => {
                        bindings
                            .entry(*dest)
                            .or_insert_with(|| self.symbol_table.define_temp());
                    }
                    IrInstr::AetherDrop { .. } => {}
                }
            }
        }

        // Identify continuation blocks for HandleScope instructions.
        // The cont block is the block whose parameter matches HandleScope's dest.
        let mut end_handle_blocks = HashSet::<BlockId>::new();
        for block in &function.blocks {
            for instr in &block.instrs {
                if let IrInstr::HandleScope { dest, .. } = instr {
                    // Find the block that has dest as a parameter
                    for b in &function.blocks {
                        if b.params.iter().any(|p| p.var == *dest) {
                            end_handle_blocks.insert(b.id);
                        }
                    }
                }
            }
        }

        let block_map: HashMap<_, _> = function
            .blocks
            .iter()
            .map(|block| (block.id, block))
            .collect();

        Some((|| {
            let mut block_offsets = HashMap::<BlockId, usize>::new();
            let mut pending_jumps = Vec::<(usize, BlockId)>::new();
            let mut false_target_blocks = HashSet::<BlockId>::new();

            for block in &function.blocks {
                block_offsets.insert(block.id, self.current_instructions().len());
                if false_target_blocks.remove(&block.id) {
                    self.emit(OpCode::OpPop, &[]);
                }

                // Emit OpEndHandle at the start of a handle-scope continuation block
                if end_handle_blocks.contains(&block.id) {
                    self.emit(OpCode::OpEndHandle, &[]);
                }

                for instr in &block.instrs {
                    self.compile_ir_cfg_instr(instr, &bindings, current_name)?;
                }

                match &block.terminator {
                    IrTerminator::Return(..) | IrTerminator::TailCall { .. } => {
                        self.compile_ir_cfg_terminator(&block.terminator, &bindings, current_name)?;
                    }
                    IrTerminator::Jump(target, args, _) => {
                        let target_block = block_map.get(target).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode jump target block",
                            ))
                        })?;
                        self.compile_ir_cfg_jump_args(target_block, args, &bindings)?;
                        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
                        pending_jumps.push((jump_pos, *target));
                    }
                    IrTerminator::Branch {
                        cond, else_block, ..
                    } => {
                        let cond_binding = bindings.get(cond).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode branch condition binding",
                            ))
                        })?;
                        self.load_symbol(cond_binding);
                        let false_jump = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                        pending_jumps.push((false_jump, *else_block));
                        false_target_blocks.insert(*else_block);
                    }
                    IrTerminator::Unreachable(_) => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "unsupported unreachable CFG bytecode block",
                        )));
                    }
                }
            }

            for (jump_pos, target) in pending_jumps {
                let target_pos = block_offsets.get(&target).copied().ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode block offset"))
                })?;
                self.change_operand(jump_pos, target_pos);
            }

            Ok(())
        })())
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_instr(
        &mut self,
        instr: &IrInstr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        match instr {
            IrInstr::Assign { dest, expr, .. } => {
                self.compile_ir_cfg_expr(expr, bindings)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR var",
                    ))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
            IrInstr::Call {
                dest, target, args, ..
            } => {
                self.compile_ir_cfg_call(target, args, bindings, current_name)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR call dest",
                    ))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
            IrInstr::HandleScope {
                effect, arms, dest, ..
            } => {
                // 1. Emit arm closures (each arm is a MakeClosure)
                for arm in arms {
                    for cap in &arm.capture_vars {
                        self.load_symbol(bindings.get(cap).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode handle capture binding",
                            ))
                        })?);
                    }
                    let fn_symbol = self
                        .lookup_ir_function_symbol_by_raw_id(arm.function_id.0)
                        .ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode handle arm function symbol",
                            ))
                        })?;
                    let fn_binding = self.symbol_table.resolve(fn_symbol).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode handle arm function binding",
                        ))
                    })?;
                    self.emit_closure_index(fn_binding.index, arm.capture_vars.len());
                }

                // 2. Create HandlerDescriptor constant and emit OpHandle
                let ops: Vec<_> = arms.iter().map(|a| a.operation_name).collect();
                let descriptor = HandlerDescriptor {
                    effect: *effect,
                    ops,
                };
                let const_idx = self.add_constant(Value::HandlerDescriptor(Rc::new(descriptor)));
                self.emit(OpCode::OpHandle, &[const_idx]);

                // 3. Body blocks follow inline; OpEndHandle will be emitted at
                //    the start of the continuation block (tracked by end_handle_blocks).
                //    The dest binding is assigned when the cont block's param
                //    receives the body result via jump args.
                let _ = dest; // dest is handled by block param assignment
                Ok(())
            }
            IrInstr::AetherDrop { var, .. } => {
                // Aether early-release: overwrite the local slot with None to
                // decrement the Rc refcount as soon as the value is no longer
                // needed, rather than waiting for the scope to end.
                if let Some(binding) = bindings.get(var) {
                    self.emit(OpCode::OpNone, &[]);
                    self.emit_store_binding(binding);
                }
                Ok(())
            }
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_expr(
        &mut self,
        expr: &IrExpr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
    ) -> CompileResult<()> {
        match expr {
            IrExpr::Const(IrConst::Int(value)) => {
                self.emit_constant_value(Value::Integer(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::Float(value)) => {
                self.emit_constant_value(Value::Float(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::Bool(value)) => {
                self.emit_constant_value(Value::Boolean(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::String(value)) => {
                self.emit_constant_value(Value::String(value.clone().into()));
                Ok(())
            }
            IrExpr::Const(IrConst::Unit) | IrExpr::None => {
                self.emit(OpCode::OpNone, &[]);
                Ok(())
            }
            IrExpr::Var(var) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR var",
                    ))
                })?);
                Ok(())
            }
            IrExpr::TagTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tag-test binding"))
                })?);
                match tag {
                    IrTagTest::None => {
                        self.emit(OpCode::OpNone, &[]);
                        self.emit(OpCode::OpEqual, &[]);
                    }
                    IrTagTest::Some => {
                        self.emit(OpCode::OpIsSome, &[]);
                    }
                    IrTagTest::Left => {
                        self.emit(OpCode::OpIsLeft, &[]);
                    }
                    IrTagTest::Right => {
                        self.emit(OpCode::OpIsRight, &[]);
                    }
                }
                Ok(())
            }
            IrExpr::TupleArityTest { value, .. } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tuple-test binding",
                    ))
                })?);
                self.emit(OpCode::OpIsTuple, &[]);
                Ok(())
            }
            IrExpr::TupleFieldAccess { object, index } => {
                self.load_symbol(bindings.get(object).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tuple-field binding",
                    ))
                })?);
                self.emit(OpCode::OpTupleIndex, &[*index]);
                Ok(())
            }
            IrExpr::TagPayload { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tag-payload binding",
                    ))
                })?);
                match tag {
                    IrTagTest::Some => self.emit(OpCode::OpUnwrapSome, &[]),
                    IrTagTest::Left => self.emit(OpCode::OpUnwrapLeft, &[]),
                    IrTagTest::Right => self.emit(OpCode::OpUnwrapRight, &[]),
                    IrTagTest::None => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "invalid CFG bytecode None payload lowering",
                        )));
                    }
                };
                Ok(())
            }
            IrExpr::ListTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-test binding",
                    ))
                })?);
                match tag {
                    IrListTest::Empty => self.emit(OpCode::OpIsEmptyList, &[]),
                    IrListTest::Cons => self.emit(OpCode::OpIsCons, &[]),
                };
                Ok(())
            }
            IrExpr::ListHead { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-head binding",
                    ))
                })?);
                self.emit(OpCode::OpConsHead, &[]);
                Ok(())
            }
            IrExpr::ListTail { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-tail binding",
                    ))
                })?);
                self.emit(OpCode::OpConsTail, &[]);
                Ok(())
            }
            IrExpr::AdtTagTest { value, constructor } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode adt-tag binding"))
                })?);
                let const_idx =
                    self.add_constant(Value::String(self.sym(*constructor).to_string().into()));
                let jump_to_false = self.emit(OpCode::OpIsAdtJump, &[const_idx, 9999]);
                self.emit(OpCode::OpPop, &[]);
                self.emit(OpCode::OpTrue, &[]);
                let jump_to_end = self.emit(OpCode::OpJump, &[9999]);
                let false_pos = self.current_instructions().len();
                self.replace_instruction(
                    jump_to_false,
                    crate::bytecode::op_code::make(OpCode::OpIsAdtJump, &[const_idx, false_pos]),
                );
                self.emit(OpCode::OpPop, &[]);
                self.emit(OpCode::OpFalse, &[]);
                let end_pos = self.current_instructions().len();
                self.change_operand(jump_to_end, end_pos);
                Ok(())
            }
            IrExpr::AdtField { value, index } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode adt-field binding",
                    ))
                })?);
                self.emit(OpCode::OpAdtField, &[*index]);
                Ok(())
            }
            IrExpr::Binary(op, lhs, rhs) => {
                let lhs_binding = bindings.get(lhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode lhs binding"))
                })?;
                let rhs_binding = bindings.get(rhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode rhs binding"))
                })?;
                if matches!(op, IrBinaryOp::Lt) {
                    self.load_symbol(rhs_binding);
                    self.load_symbol(lhs_binding);
                } else {
                    self.load_symbol(lhs_binding);
                    self.load_symbol(rhs_binding);
                }
                match op {
                    IrBinaryOp::Add => self.emit(OpCode::OpAdd, &[]),
                    IrBinaryOp::Sub => self.emit(OpCode::OpSub, &[]),
                    IrBinaryOp::Mul => self.emit(OpCode::OpMul, &[]),
                    IrBinaryOp::Div => self.emit(OpCode::OpDiv, &[]),
                    IrBinaryOp::Eq => self.emit(OpCode::OpEqual, &[]),
                    IrBinaryOp::NotEq => self.emit(OpCode::OpNotEqual, &[]),
                    IrBinaryOp::Lt => self.emit(OpCode::OpGreaterThan, &[]),
                    IrBinaryOp::Gt => self.emit(OpCode::OpGreaterThan, &[]),
                    IrBinaryOp::Ge => self.emit(OpCode::OpGreaterThanOrEqual, &[]),
                    IrBinaryOp::Le => self.emit(OpCode::OpLessThanOrEqual, &[]),
                    _ => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "unsupported CFG bytecode binary op",
                        )));
                    }
                };
                Ok(())
            }
            IrExpr::MakeClosure(fn_id, captures) => {
                // Load each capture variable onto the stack
                for cap in captures {
                    self.load_symbol(bindings.get(cap).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode closure capture binding",
                        ))
                    })?);
                }
                // Resolve FunctionId → Symbol → Binding to get the constant index
                let fn_symbol = self
                    .lookup_ir_function_symbol_by_raw_id(fn_id.0)
                    .ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode closure function symbol",
                        ))
                    })?;
                let fn_binding = self.symbol_table.resolve(fn_symbol).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode closure function binding",
                    ))
                })?;
                self.emit_closure_index(fn_binding.index, captures.len());
                Ok(())
            }
            IrExpr::Perform {
                effect,
                operation,
                args,
            } => {
                // Push arguments onto the stack
                for arg in args {
                    self.load_symbol(bindings.get(arg).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode perform arg binding",
                        ))
                    })?);
                }
                // Create PerformDescriptor constant
                let effect_name = self.sym(*effect).to_string();
                let op_name = self.sym(*operation).to_string();
                let descriptor = crate::runtime::perform_descriptor::PerformDescriptor {
                    effect: *effect,
                    op: *operation,
                    effect_name: effect_name.into(),
                    op_name: op_name.into(),
                };
                let const_idx =
                    self.add_constant(Value::PerformDescriptor(std::rc::Rc::new(descriptor)));
                self.emit(OpCode::OpPerform, &[const_idx, args.len()]);
                Ok(())
            }
            _ => Err(Self::boxed(Diagnostic::warning(
                "unsupported CFG bytecode expression lowering",
            ))),
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_jump_args(
        &mut self,
        target: &IrBlock,
        args: &[IrVar],
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
    ) -> CompileResult<()> {
        for (param, arg) in target.params.iter().zip(args) {
            self.load_symbol(bindings.get(arg).ok_or_else(|| {
                Self::boxed(Diagnostic::warning("missing CFG bytecode jump arg binding"))
            })?);
            self.emit_store_binding(bindings.get(&param.var).ok_or_else(|| {
                Self::boxed(Diagnostic::warning(
                    "missing CFG bytecode block param binding",
                ))
            })?);
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_terminator(
        &mut self,
        terminator: &IrTerminator,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        match terminator {
            IrTerminator::Return(var, _) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode return binding"))
                })?);
                self.emit(OpCode::OpReturnValue, &[]);
                Ok(())
            }
            IrTerminator::TailCall { callee, args, .. } => {
                // Effect checking for named tail calls.
                if let IrCallTarget::Named(name) = callee {
                    let name_str = self.sym(*name);
                    if let Some(primop) = crate::primop::resolve_primop_call(name_str, args.len()) {
                        let required = match primop.effect_kind() {
                            crate::primop::PrimEffect::Io => Some("IO"),
                            crate::primop::PrimEffect::Time => Some("Time"),
                            _ => None,
                        };
                        if let Some(required_name) = required
                            && !self.is_effect_available_name(required_name)
                        {
                            return Err(Self::boxed(
                                Diagnostic::make_error_dynamic(
                                    "E400",
                                    "MISSING EFFECT",
                                    crate::diagnostics::ErrorType::Compiler,
                                    format!(
                                        "Call to `{}` requires effect `{}` in this function signature.",
                                        name_str, required_name
                                    ),
                                    Some(format!(
                                        "Add `with {}` to the enclosing function.",
                                        required_name
                                    )),
                                    self.file_path.clone(),
                                    crate::diagnostics::position::Span::default(),
                                )
                                .with_display_title("Missing Ambient Effect"),
                            ));
                        }
                    }
                }

                let is_self = matches!(callee, IrCallTarget::Named(name) if *name == current_name);
                if !is_self {
                    match callee {
                        IrCallTarget::Named(name) => {
                            let symbol = self.symbol_table.resolve(*name).ok_or_else(|| {
                                Self::boxed(Diagnostic::warning(
                                    "missing CFG bytecode tail-call target binding",
                                ))
                            })?;
                            self.load_symbol(&symbol);
                        }
                        IrCallTarget::Var(var) => {
                            self.load_symbol(bindings.get(var).ok_or_else(|| {
                                Self::boxed(Diagnostic::warning(
                                    "missing CFG bytecode tail-call callee binding",
                                ))
                            })?);
                        }
                        IrCallTarget::Direct(_) => {
                            let function_id = match callee {
                                IrCallTarget::Direct(id) => id,
                                _ => unreachable!(),
                            };
                            let symbol = self
                                .lookup_ir_function_symbol_by_raw_id(function_id.0)
                                .and_then(|symbol| self.symbol_table.resolve(symbol))
                                .ok_or_else(|| {
                                    Self::boxed(Diagnostic::warning(
                                        "missing direct CFG bytecode tail-call target binding",
                                    ))
                                })?;
                            self.load_symbol(&symbol);
                        }
                    }
                }
                for arg in args {
                    self.load_symbol(bindings.get(arg).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode tail-call arg binding",
                        ))
                    })?);
                }
                if is_self {
                    self.emit(OpCode::OpTailCall, &[args.len()]);
                } else {
                    self.emit(OpCode::OpCall, &[args.len()]);
                    self.emit(OpCode::OpReturnValue, &[]);
                }
                Ok(())
            }
            _ => Err(Self::boxed(Diagnostic::warning(
                "unsupported CFG bytecode terminator lowering",
            ))),
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_call(
        &mut self,
        target: &IrCallTarget,
        args: &[IrVar],
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        // Effect checking for named calls: verify that effectful base
        // functions (e.g. print, read_file) have the required effect
        // available in the surrounding scope.
        if let IrCallTarget::Named(name) = target {
            let name_str = self.sym(*name);
            if let Some(primop) = crate::primop::resolve_primop_call(name_str, args.len()) {
                let required = match primop.effect_kind() {
                    crate::primop::PrimEffect::Io => Some("IO"),
                    crate::primop::PrimEffect::Time => Some("Time"),
                    _ => None,
                };
                if let Some(required_name) = required
                    && !self.is_effect_available_name(required_name)
                {
                    return Err(Self::boxed(
                        Diagnostic::make_error_dynamic(
                            "E400",
                            "MISSING EFFECT",
                            crate::diagnostics::ErrorType::Compiler,
                            format!(
                                "Call to `{}` requires effect `{}` in this function signature.",
                                name_str, required_name
                            ),
                            Some(format!(
                                "Add `with {}` to the enclosing function.",
                                required_name
                            )),
                            self.file_path.clone(),
                            crate::diagnostics::position::Span::default(),
                        )
                        .with_display_title("Missing Ambient Effect"),
                    ));
                }
            }
        }

        let is_self = matches!(target, IrCallTarget::Named(name) if *name == current_name);
        if !is_self {
            match target {
                IrCallTarget::Named(name) => {
                    let symbol = self.symbol_table.resolve(*name).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode call target binding",
                        ))
                    })?;
                    self.load_symbol(&symbol);
                }
                IrCallTarget::Var(var) => {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode call callee binding",
                        ))
                    })?);
                }
                IrCallTarget::Direct(_) => {
                    let function_id = match target {
                        IrCallTarget::Direct(id) => id,
                        _ => unreachable!(),
                    };
                    let symbol = self
                        .lookup_ir_function_symbol_by_raw_id(function_id.0)
                        .and_then(|symbol| self.symbol_table.resolve(symbol))
                        .ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing direct CFG bytecode call target binding",
                            ))
                        })?;
                    self.load_symbol(&symbol);
                }
            }
        }

        for arg in args {
            self.load_symbol(bindings.get(arg).ok_or_else(|| {
                Self::boxed(Diagnostic::warning("missing CFG bytecode call arg binding"))
            })?);
        }

        if is_self {
            self.emit(OpCode::OpCallSelf, &[args.len()]);
        } else {
            self.emit(OpCode::OpCall, &[args.len()]);
        }
        Ok(())
    }
}
