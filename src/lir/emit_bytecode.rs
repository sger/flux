//! LIR → Bytecode emitter (Proposal 0132 Phase 6).
//!
//! Walks a `LirProgram` and emits VM bytecode.  Each `LirVar` is assigned a
//! local slot; instructions load/store from these slots via `OpGetLocal` /
//! `OpSetLocal`.
//!
//! This emitter is activated by `FLUX_USE_LIR=1` and must produce output
//! identical to the existing CFG-based compiler for parity testing.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::bytecode::Bytecode;
use crate::bytecode::op_code::{make, OpCode, Instructions};
use crate::lir::*;
use crate::runtime::compiled_function::CompiledFunction;
use crate::runtime::value::Value;

// ── Public entry point ───────────────────────────────────────────────────────

/// Emit bytecode for a `LirProgram`.
///
/// The last function in `program.functions` is the top-level main entry.
/// All other functions are nested (closures) — they are compiled into
/// `CompiledFunction` objects stored in the shared constants pool,
/// referenced by `MakeClosure` instructions via `OpClosure`.
pub fn emit_program(program: &LirProgram) -> Bytecode {
    let main_func = program.functions.last().expect("empty LIR program");
    let num_funcs = program.functions.len();

    // Shared constants pool — nested functions and main all reference this.
    let mut shared_constants: Vec<Value> = Vec::new();
    let mut func_const_indices = vec![0usize; num_funcs];

    // Compile nested functions first, each adding constants to the shared pool.
    for (i, func) in program.functions.iter().enumerate() {
        if i == num_funcs - 1 {
            break; // skip main
        }
        let compiled = compile_nested_function(
            func,
            program,
            &mut shared_constants,
            &func_const_indices,
        );
        let const_idx = shared_constants.len();
        shared_constants.push(Value::Function(Rc::new(compiled)));
        func_const_indices[i] = const_idx;
    }

    // Compile main, sharing the same constants pool.
    let mut emitter = FnEmitter::new(main_func, program);
    emitter.constants = shared_constants;
    emitter.func_const_indices = func_const_indices;
    emitter.emit_function();

    Bytecode {
        instructions: emitter.instructions,
        constants: emitter.constants,
        debug_info: None,
    }
}

/// Compile a nested LIR function into a `CompiledFunction`.
///
/// Constants are added to `shared_constants` so all functions
/// reference the same pool (the VM uses a single global pool).
fn compile_nested_function(
    func: &LirFunction,
    program: &LirProgram,
    shared_constants: &mut Vec<Value>,
    func_const_indices: &[usize],
) -> CompiledFunction {
    let mut emitter = FnEmitter::new(func, program);
    // Share the constants pool.
    emitter.constants = std::mem::take(shared_constants);
    emitter.func_const_indices = func_const_indices.to_vec();
    emitter.emit_function();
    // Return the constants pool to the caller.
    *shared_constants = emitter.constants;
    let num_params = func.params.len();
    let extra_locals = emitter.next_local.saturating_sub(num_params);
    CompiledFunction::new(
        emitter.instructions,
        extra_locals,          // num_locals (extra, beyond params)
        num_params,            // num_parameters
        None,                  // debug_info
    )
}

// ── Per-function emitter ─────────────────────────────────────────────────────

struct FnEmitter<'a> {
    func: &'a LirFunction,
    #[allow(dead_code)] // used by future LLVM emitter + string pool lookups
    program: &'a LirProgram,
    /// Output instruction stream.
    instructions: Instructions,
    /// Constants pool.
    constants: Vec<Value>,
    /// LirVar → local slot index.
    locals: HashMap<LirVar, usize>,
    /// Next free local slot.
    next_local: usize,
    /// Block ID → bytecode offset (filled during emission).
    block_offsets: HashMap<BlockId, usize>,
    /// Pending jump patches: (instruction_offset, target_block).
    jump_patches: Vec<(usize, BlockId)>,
    /// Positions of OpJump operands that should be patched to the end of bytecode.
    return_patches: Vec<usize>,
    /// LIR function index → constants pool index (for MakeClosure).
    func_const_indices: Vec<usize>,
    /// LirVar → free variable index (for OpGetFree in nested functions).
    free_var_indices: HashMap<LirVar, usize>,
}

impl<'a> FnEmitter<'a> {
    fn new(func: &'a LirFunction, program: &'a LirProgram) -> Self {
        Self {
            func,
            program,
            instructions: Vec::new(),
            constants: Vec::new(),
            locals: HashMap::new(),
            next_local: 0,
            block_offsets: HashMap::new(),
            jump_patches: Vec::new(),
            return_patches: Vec::new(),
            func_const_indices: Vec::new(),
            free_var_indices: HashMap::new(),
        }
    }

    /// Assign a local slot to a LirVar (if not already assigned).
    fn local_for(&mut self, var: LirVar) -> usize {
        if let Some(&slot) = self.locals.get(&var) {
            slot
        } else {
            let slot = self.next_local;
            self.next_local += 1;
            self.locals.insert(var, slot);
            slot
        }
    }

    /// Add a constant to the pool, returning its index.
    fn add_constant(&mut self, val: Value) -> usize {
        let idx = self.constants.len();
        self.constants.push(val);
        idx
    }

    /// Emit raw instruction bytes.
    fn emit_raw(&mut self, bytes: &[u8]) {
        self.instructions.extend_from_slice(bytes);
    }

    /// Emit an opcode with operands.
    fn emit_op(&mut self, op: OpCode, operands: &[usize]) {
        let bytes = make(op, operands);
        self.emit_raw(&bytes);
    }

    /// Current position in the instruction stream.
    fn pos(&self) -> usize {
        self.instructions.len()
    }

    /// Push a LirVar's value onto the stack.
    /// Uses `OpGetFree` for captured variables, `OpGetLocal` otherwise.
    fn push_var(&mut self, var: LirVar) {
        if let Some(&free_idx) = self.free_var_indices.get(&var) {
            self.emit_op(OpCode::OpGetFree, &[free_idx]);
        } else {
            let slot = self.local_for(var);
            self.emit_op(OpCode::OpGetLocal, &[slot]);
        }
    }

    /// Pop TOS into a LirVar's local slot.
    fn pop_into(&mut self, var: LirVar) {
        let slot = self.local_for(var);
        self.emit_op(OpCode::OpSetLocal, &[slot]);
    }

    // ── Function emission ────────────────────────────────────────────

    fn emit_function(&mut self) {
        // Register captured variables — these use OpGetFree, not OpGetLocal.
        for (i, &var) in self.func.capture_vars.iter().enumerate() {
            self.free_var_indices.insert(var, i);
        }

        // Assign parameter vars to slots 0..N-1 (already on the stack
        // when OpCall sets up the frame — do NOT emit OpNone for these).
        for &param in &self.func.params {
            let slot = self.next_local;
            self.next_local += 1;
            self.locals.insert(param, slot);
        }

        // Pre-allocate local slots for remaining LIR variables (not params,
        // not captured).  For the top-level function, emit OpNone to reserve
        // stack space.  For nested functions, the VM's `sp += num_locals`
        // already allocates the space — just assign slot numbers.
        let is_top_level = self.func.params.is_empty() && self.func.capture_vars.is_empty();
        let total_vars = self.func.next_var as usize;
        for i in 0..total_vars {
            let var = LirVar(i as u32);
            if self.free_var_indices.contains_key(&var) || self.locals.contains_key(&var) {
                continue; // free var or already assigned (param)
            }
            self.locals.insert(var, self.next_local);
            if is_top_level {
                // Top-level: no frame setup, must push placeholders.
                self.emit_op(OpCode::OpNone, &[]);
            }
            self.next_local += 1;
        }

        // Emit blocks in order.  The entry block (bb0) comes first.
        for block in &self.func.blocks {
            self.block_offsets.insert(block.id, self.pos());
            self.emit_block(block);
        }

        // Patch jump targets.
        self.patch_jumps();

        // Patch return jumps to point past the end of bytecode.
        let end = self.pos();
        for patch_pos in &self.return_patches {
            self.instructions[*patch_pos] = (end >> 8) as u8;
            self.instructions[*patch_pos + 1] = end as u8;
        }
    }

    fn emit_block(&mut self, block: &LirBlock) {
        for instr in &block.instrs {
            self.emit_instr(instr);
        }
        self.emit_terminator(&block.terminator);
    }

    // ── Instruction emission ─────────────────────────────────────────

    fn emit_instr(&mut self, instr: &LirInstr) {
        match instr {
            LirInstr::Const { dst, value } => {
                self.emit_const(value);
                self.pop_into(*dst);
            }

            LirInstr::Copy { dst, src } => {
                self.push_var(*src);
                self.pop_into(*dst);
            }

            LirInstr::PrimCall { dst, op, args } => {
                // Push all arguments onto the stack.
                for &arg in args {
                    self.push_var(arg);
                }
                // Polymorphic ops use dedicated VM stack opcodes, not OpPrimOp
                // (the VM rejects them via OpPrimOp dispatch).
                use crate::core::CorePrimOp;
                match op {
                    CorePrimOp::Add => self.emit_op(OpCode::OpAdd, &[]),
                    CorePrimOp::Sub => self.emit_op(OpCode::OpSub, &[]),
                    CorePrimOp::Mul => self.emit_op(OpCode::OpMul, &[]),
                    CorePrimOp::Div => self.emit_op(OpCode::OpDiv, &[]),
                    CorePrimOp::Mod => self.emit_op(OpCode::OpMod, &[]),
                    CorePrimOp::Eq => self.emit_op(OpCode::OpEqual, &[]),
                    CorePrimOp::NEq => self.emit_op(OpCode::OpNotEqual, &[]),
                    CorePrimOp::Lt => {
                        // a < b = !(a >= b)
                        self.emit_op(OpCode::OpGreaterThanOrEqual, &[]);
                        self.emit_op(OpCode::OpBang, &[]);
                    }
                    CorePrimOp::Le => self.emit_op(OpCode::OpLessThanOrEqual, &[]),
                    CorePrimOp::Gt => self.emit_op(OpCode::OpGreaterThan, &[]),
                    CorePrimOp::Ge => self.emit_op(OpCode::OpGreaterThanOrEqual, &[]),
                    CorePrimOp::Not => self.emit_op(OpCode::OpBang, &[]),
                    _ => {
                        self.emit_op(OpCode::OpPrimOp, &[op.id() as usize, args.len()]);
                    }
                }
                if let Some(d) = dst {
                    self.pop_into(*d);
                } else {
                    self.emit_op(OpCode::OpPop, &[]);
                }
            }

            // ── Inline arithmetic (stack-based: push operands, emit op) ──
            LirInstr::IAdd { dst, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                self.emit_op(OpCode::OpAdd, &[]);
                self.pop_into(*dst);
            }
            LirInstr::ISub { dst, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                self.emit_op(OpCode::OpSub, &[]);
                self.pop_into(*dst);
            }
            LirInstr::IMul { dst, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                self.emit_op(OpCode::OpMul, &[]);
                self.pop_into(*dst);
            }
            LirInstr::IDiv { dst, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                self.emit_op(OpCode::OpDiv, &[]);
                self.pop_into(*dst);
            }
            LirInstr::IRem { dst, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                self.emit_op(OpCode::OpMod, &[]);
                self.pop_into(*dst);
            }

            LirInstr::ICmp { dst, op, a, b } => {
                self.push_var(*a);
                self.push_var(*b);
                let opcode = match op {
                    CmpOp::Eq => OpCode::OpEqual,
                    CmpOp::Ne => OpCode::OpNotEqual,
                    CmpOp::Sgt => OpCode::OpGreaterThan,
                    CmpOp::Sle => OpCode::OpLessThanOrEqual,
                    CmpOp::Sge => OpCode::OpGreaterThanOrEqual,
                    CmpOp::Slt => {
                        // No OpLessThan — use !(a >= b)
                        self.emit_op(OpCode::OpGreaterThanOrEqual, &[]);
                        self.emit_op(OpCode::OpBang, &[]);
                        self.pop_into(*dst);
                        return;
                    }
                };
                self.emit_op(opcode, &[]);
                self.pop_into(*dst);
            }

            // ── NaN-boxing (mostly no-ops for the VM since Values are
            //    already tagged — these are for the LLVM path) ────────
            LirInstr::TagInt { dst, raw }
            | LirInstr::UntagInt { dst, val: raw }
            | LirInstr::TagFloat { dst, raw }
            | LirInstr::UntagFloat { dst, val: raw }
            | LirInstr::TagBool { dst, raw }
            | LirInstr::UntagBool { dst, val: raw }
            | LirInstr::TagPtr { dst, ptr: raw }
            | LirInstr::UntagPtr { dst, val: raw } => {
                // VM values are already NaN-boxed — tag/untag is identity.
                self.push_var(*raw);
                self.pop_into(*dst);
            }

            LirInstr::GetTag { dst, val } => {
                // VM doesn't need explicit tag extraction — pattern matching
                // uses OpIsCons/OpIsEmptyList/etc.  Emit a placeholder.
                self.push_var(*val);
                self.pop_into(*dst);
            }

            // ── Constructor creation ──────────────────────────────────
            LirInstr::MakeCtor {
                dst,
                ctor_tag,
                ctor_name,
                fields,
            } => {
                // Push all fields onto the stack first.
                for &field in fields {
                    self.push_var(field);
                }
                match (*ctor_tag, fields.len()) {
                    // Built-in constructors with dedicated opcodes.
                    (1, 1) => {
                        // Some(val): OpSome wraps TOS
                        self.emit_op(OpCode::OpSome, &[]);
                    }
                    (2, 1) => {
                        // Left(val): OpLeft wraps TOS
                        self.emit_op(OpCode::OpLeft, &[]);
                    }
                    (3, 1) => {
                        // Right(val): OpRight wraps TOS
                        self.emit_op(OpCode::OpRight, &[]);
                    }
                    (4, 2) => {
                        // Cons(head, tail): OpCons pops 2
                        self.emit_op(OpCode::OpCons, &[]);
                    }
                    _ => {
                        // User-defined ADT: OpMakeAdt(const_idx, arity)
                        // OpMakeAdt needs the constructor name as a string constant.
                        let name = ctor_name.as_deref().unwrap_or("?");
                        let const_idx =
                            self.add_constant(Value::String(Rc::new(name.to_string())));
                        self.emit_op(
                            OpCode::OpMakeAdt,
                            &[const_idx, fields.len()],
                        );
                    }
                }
                self.pop_into(*dst);
            }

            // ── Memory (raw Alloc/Store/Load — used by reuse path, ──
            // ── not by normal constructor emission) ─────────────────
            LirInstr::Alloc { dst, .. } => {
                // Reuse path fallback — should not appear in normal code
                // after MakeCtor is used. Emit None as placeholder.
                self.emit_op(OpCode::OpNone, &[]);
                self.pop_into(*dst);
            }
            LirInstr::Load { dst, .. } => {
                self.emit_op(OpCode::OpNone, &[]);
                self.pop_into(*dst);
            }
            LirInstr::Store { .. } => {
                // No-op for VM — reuse path writes are handled by OpReuseAdt.
            }

            // ── Aether RC ────────────────────────────────────────────
            LirInstr::Dup { val } => {
                // VM Dup = clone the value (Rc::clone for heap values).
                // The existing VM doesn't have an explicit dup opcode —
                // Rc cloning happens implicitly.  No-op here.
                let _ = val;
            }
            LirInstr::Drop { val } => {
                let slot = self.local_for(*val);
                self.emit_op(OpCode::OpAetherDropLocal, &[slot]);
            }
            LirInstr::IsUnique { dst, val } => {
                self.push_var(*val);
                self.emit_op(OpCode::OpIsUnique, &[]);
                self.pop_into(*dst);
            }
            LirInstr::DropReuse { dst, val } => {
                self.push_var(*val);
                self.emit_op(OpCode::OpDropReuse, &[]);
                self.pop_into(*dst);
            }

            LirInstr::MakeClosure {
                dst,
                func_idx,
                captures,
            } => {
                // Push captured values onto the stack.
                for &cap in captures {
                    self.push_var(cap);
                }
                // OpClosure pops the captures and creates a Closure from
                // the CompiledFunction stored in the constants pool.
                let const_idx = self.func_const_indices[*func_idx];
                self.emit_op(OpCode::OpClosure, &[const_idx, captures.len()]);
                self.pop_into(*dst);
            }
        }
    }

    /// Emit a constant value onto the stack.
    fn emit_const(&mut self, value: &LirConst) {
        match value {
            LirConst::Int(n) => {
                let idx = self.add_constant(Value::Integer(*n));
                self.emit_op(OpCode::OpConstant, &[idx]);
            }
            LirConst::Float(f) => {
                let idx = self.add_constant(Value::Float(*f));
                self.emit_op(OpCode::OpConstant, &[idx]);
            }
            LirConst::Bool(true) => self.emit_op(OpCode::OpTrue, &[]),
            LirConst::Bool(false) => self.emit_op(OpCode::OpFalse, &[]),
            LirConst::String(s) => {
                let idx = self.add_constant(Value::String(s.clone().into()));
                self.emit_op(OpCode::OpConstant, &[idx]);
            }
            LirConst::None => self.emit_op(OpCode::OpNone, &[]),
            LirConst::EmptyList => {
                let idx = self.add_constant(Value::EmptyList);
                self.emit_op(OpCode::OpConstant, &[idx]);
            }
            LirConst::Tagged(n) => {
                let idx = self.add_constant(Value::Integer(*n));
                self.emit_op(OpCode::OpConstant, &[idx]);
            }
        }
    }

    // ── Terminator emission ──────────────────────────────────────────

    fn emit_terminator(&mut self, term: &LirTerminator) {
        match term {
            LirTerminator::Return(val) => {
                self.push_var(*val);
                if self.func.capture_vars.is_empty() && self.func.params.is_empty() {
                    // Top-level main: no call frame to return to.
                    // OpPop stores the value in last_popped, then jump past end.
                    self.emit_op(OpCode::OpPop, &[]);
                    let patch_pos = self.pos() + 1;
                    self.emit_op(OpCode::OpJump, &[0xFFFF]);
                    self.return_patches.push(patch_pos);
                } else {
                    // Nested function: return to caller via OpReturnValue.
                    self.emit_op(OpCode::OpReturnValue, &[]);
                }
            }

            LirTerminator::Jump(target) => {
                // Emit OpJump with placeholder offset, patch later.
                let patch_pos = self.pos() + 1; // offset of the u16 operand
                self.emit_op(OpCode::OpJump, &[0xFFFF]); // placeholder
                self.jump_patches.push((patch_pos, *target));
            }

            LirTerminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                self.push_var(*cond);
                // OpJumpNotTruthy: truthy → pops, falls through; not truthy → peeks (no pop), jumps.
                // We jump to a trampoline that pops the leftover value before going to else_block.
                let patch_not_truthy = self.pos() + 1;
                self.emit_op(OpCode::OpJumpNotTruthy, &[0xFFFF]);
                // Truthy: condition was popped. Jump to then_block.
                let patch_then = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((patch_then, *then_block));
                // Not-truthy trampoline: pop leftover condition, jump to else_block.
                let trampoline = self.pos();
                self.emit_op(OpCode::OpPop, &[]);
                let patch_else = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((patch_else, *else_block));
                // Patch OpJumpNotTruthy to point to trampoline (direct offset).
                self.instructions[patch_not_truthy] = (trampoline >> 8) as u8;
                self.instructions[patch_not_truthy + 1] = trampoline as u8;
            }

            LirTerminator::Switch {
                scrutinee,
                cases,
                default,
            } => {
                // Emit as a chain of compare-and-jump.
                // OpJumpTruthy peeks (no pop) when jumping, so each match
                // needs a trampoline to pop the leftover boolean.
                let mut trampolines = Vec::new();
                for &(case_val, target) in cases {
                    self.push_var(*scrutinee);
                    let idx = self.add_constant(Value::Integer(case_val));
                    self.emit_op(OpCode::OpConstant, &[idx]);
                    self.emit_op(OpCode::OpEqual, &[]);
                    let patch_truthy = self.pos() + 1;
                    self.emit_op(OpCode::OpJumpTruthy, &[0xFFFF]);
                    trampolines.push((patch_truthy, target));
                }
                // Default: unconditional jump.
                let patch_pos = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((patch_pos, *default));
                // Emit trampolines: pop leftover boolean, jump to target.
                for (patch_truthy, target) in trampolines {
                    let trampoline = self.pos();
                    self.emit_op(OpCode::OpPop, &[]);
                    let patch_target = self.pos() + 1;
                    self.emit_op(OpCode::OpJump, &[0xFFFF]);
                    self.jump_patches.push((patch_target, target));
                    self.instructions[patch_truthy] = (trampoline >> 8) as u8;
                    self.instructions[patch_truthy + 1] = trampoline as u8;
                }
            }

            LirTerminator::Call {
                dst,
                func,
                args,
                cont,
            } => {
                // Push function, then args, then OpCall.
                self.push_var(*func);
                for &arg in args {
                    self.push_var(arg);
                }
                self.emit_op(OpCode::OpCall, &[args.len()]);
                self.pop_into(*dst);
                // Fall through to continuation block — emit jump if needed.
                let patch_pos = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((patch_pos, *cont));
            }

            LirTerminator::TailCall { func, args } => {
                self.push_var(*func);
                for &arg in args {
                    self.push_var(arg);
                }
                self.emit_op(OpCode::OpTailCall, &[args.len()]);
            }

            LirTerminator::MatchCtor {
                scrutinee,
                arms,
                default,
            } => {
                // Emit constructor pattern matching using VM-specific opcodes.
                // For each arm: push scrutinee, test constructor, jump on match.
                let mut match_patches: Vec<(usize, BlockId, Vec<LirVar>)> = Vec::new();

                for arm in arms {
                    self.push_var(*scrutinee);
                    match &arm.tag {
                        CtorTag::EmptyList => {
                            // OpIsEmptyList: TOS → bool
                            self.emit_op(OpCode::OpIsEmptyList, &[]);
                        }
                        CtorTag::None => {
                            // Compare with None constant.
                            let none_idx = self.add_constant(Value::None);
                            self.emit_op(OpCode::OpConstant, &[none_idx]);
                            self.emit_op(OpCode::OpEqual, &[]);
                        }
                        CtorTag::Cons => {
                            // OpIsCons: TOS → bool
                            self.emit_op(OpCode::OpIsCons, &[]);
                        }
                        CtorTag::Some | CtorTag::Left | CtorTag::Right | CtorTag::Named(_) => {
                            // OpIsAdt: compare constructor name.
                            let name = match &arm.tag {
                                CtorTag::Some => "Some",
                                CtorTag::Left => "Left",
                                CtorTag::Right => "Right",
                                CtorTag::Named(n) => n.as_str(),
                                _ => unreachable!(),
                            };
                            let const_idx =
                                self.add_constant(Value::String(Rc::new(name.to_string())));
                            self.emit_op(OpCode::OpIsAdt, &[const_idx]);
                        }
                        CtorTag::Tuple => {
                            // OpIsTuple: TOS → bool
                            self.emit_op(OpCode::OpIsTuple, &[]);
                        }
                    }
                    // JumpTruthy → trampoline that extracts fields + jumps to block
                    let patch = self.pos() + 1;
                    self.emit_op(OpCode::OpJumpTruthy, &[0xFFFF]);
                    match_patches.push((patch, arm.target, arm.field_binders.clone()));
                }

                // Default: jump unconditionally.
                let default_patch = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((default_patch, *default));

                // Emit trampolines for each matched arm: pop bool, extract fields, jump.
                for (patch, target, field_binders) in match_patches {
                    let trampoline = self.pos();
                    // Pop the leftover boolean from JumpTruthy.
                    self.emit_op(OpCode::OpPop, &[]);
                    // Extract fields from scrutinee into binder slots.
                    if !field_binders.is_empty() {
                        self.push_var(*scrutinee);
                        if field_binders.len() == 2 {
                            // OpAdtFields2: pop ADT, push field0 then field1.
                            self.emit_op(OpCode::OpAdtFields2, &[]);
                            self.pop_into(field_binders[0]);
                            self.pop_into(field_binders[1]);
                        } else {
                            for (i, &binder) in field_binders.iter().enumerate() {
                                if i > 0 {
                                    self.push_var(*scrutinee);
                                }
                                self.emit_op(OpCode::OpAdtField, &[i]);
                                self.pop_into(binder);
                            }
                        }
                    }
                    // Jump to the arm's body block.
                    let target_patch = self.pos() + 1;
                    self.emit_op(OpCode::OpJump, &[0xFFFF]);
                    self.jump_patches.push((target_patch, target));
                    // Patch the JumpTruthy to point to this trampoline.
                    self.instructions[patch] = (trampoline >> 8) as u8;
                    self.instructions[patch + 1] = trampoline as u8;
                }
            }

            LirTerminator::Unreachable => {
                // Emit a halt — shouldn't be reached.
                // Push None and return to avoid VM crash.
                self.emit_op(OpCode::OpNone, &[]);
                self.emit_op(OpCode::OpReturnValue, &[]);
            }
        }
    }

    // ── Jump patching ────────────────────────────────────────────────

    fn patch_jumps(&mut self) {
        for &(patch_pos, target_block) in &self.jump_patches {
            let target_offset = self
                .block_offsets
                .get(&target_block)
                .copied()
                .unwrap_or(0);
            // Encode as big-endian u16.
            self.instructions[patch_pos] = (target_offset >> 8) as u8;
            self.instructions[patch_pos + 1] = target_offset as u8;
        }
    }
}
