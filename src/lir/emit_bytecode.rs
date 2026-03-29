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
    emit_program_with_base_constants(program, Vec::new())
}

/// Emit bytecode with a pre-populated constants pool base.
///
/// `base_constants` contains constants from previously compiled modules
/// (e.g., prelude functions compiled via the CFG pipeline). The LIR-compiled
/// constants are appended after these, so CFG-compiled closures can find
/// their sub-closures at the expected indices.
pub fn emit_program_with_base_constants(
    program: &LirProgram,
    base_constants: Vec<Value>,
) -> Bytecode {
    let main_func = program.functions.last().expect("empty LIR program");
    let num_funcs = program.functions.len();

    // Shared constants pool — starts with base constants from CFG compilation.
    let mut shared_constants = base_constants;
    let mut func_const_indices: HashMap<LirFuncId, usize> = HashMap::new();

    // Pre-reserve constant pool slots for all nested functions so that
    // self-recursive and mutually-recursive closures can reference the
    // correct constant index during compilation.
    for func in program.functions.iter().take(num_funcs - 1) {
        let slot = shared_constants.len();
        shared_constants.push(Value::None); // placeholder
        func_const_indices.insert(func.id, slot);
    }

    // Compile nested functions, then backfill their constant pool slots.
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
        shared_constants[func_const_indices[&func.id]] = Value::Function(Rc::new(compiled));
    }

    // Compile main, sharing the same constants pool.
    let mut emitter = FnEmitter::new(main_func, program);
    emitter.is_main = true;
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
    func_const_indices: &HashMap<LirFuncId, usize>,
) -> CompiledFunction {
    let mut emitter = FnEmitter::new(func, program);
    // Share the constants pool.
    emitter.constants = std::mem::take(shared_constants);
    emitter.func_const_indices = func_const_indices.clone();
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
    /// True if this is the top-level main function (uses OpPop+OpJump for return).
    is_main: bool,
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
    /// LirFuncId → constants pool index (for MakeClosure).
    func_const_indices: HashMap<LirFuncId, usize>,
    /// LirVar → free variable index (for OpGetFree in nested functions).
    free_var_indices: HashMap<LirVar, usize>,
    /// Tracks how each LirVar was produced, for peephole optimizations.
    /// Only populated for instructions relevant to fused compare-and-jump.
    var_producer: HashMap<LirVar, VarProducer>,
}

/// Tracks how a LirVar was produced, for peephole optimization.
#[derive(Clone)]
enum VarProducer {
    /// PrimCall comparison: Ge, Gt, Le, Lt, Eq, Ne
    Comparison { op: crate::core::CorePrimOp, args: Vec<LirVar> },
    /// CmpEq(var, true) — boolean truthiness check
    CmpEqTrue { inner: LirVar },
    /// UntagBool(var) — unwrap boolean
    UntagBool { inner: LirVar },
}

impl<'a> FnEmitter<'a> {
    fn new(func: &'a LirFunction, program: &'a LirProgram) -> Self {
        Self {
            func,
            program,
            is_main: false,
            instructions: Vec::new(),
            constants: Vec::new(),
            locals: HashMap::new(),
            next_local: 0,
            block_offsets: HashMap::new(),
            jump_patches: Vec::new(),
            return_patches: Vec::new(),
            func_const_indices: HashMap::new(),
            free_var_indices: HashMap::new(),
            var_producer: HashMap::new(),
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

    /// Try to produce a fused compare-and-jump for a Branch condition.
    /// Returns (fused_opcode, [arg_a, arg_b]) if the condition matches
    /// the pattern: PrimCall(cmp) → CmpEq(result, true) → UntagBool.
    fn try_fused_cmp_branch(&self, cond: LirVar) -> Option<(OpCode, Vec<LirVar>)> {
        use crate::core::CorePrimOp;

        // Walk the producer chain: cond ← UntagBool ← CmpEqTrue ← Comparison
        let inner1 = match self.var_producer.get(&cond)? {
            VarProducer::UntagBool { inner } => *inner,
            // Also handle direct comparison (no CmpEq wrapper).
            VarProducer::CmpEqTrue { inner } => *inner,
            VarProducer::Comparison { op, args } => {
                let fused = Self::comparison_to_fused_opcode(op)?;
                return Some((fused, args.clone()));
            }
        };

        let inner2 = match self.var_producer.get(&inner1)? {
            VarProducer::CmpEqTrue { inner } => *inner,
            VarProducer::Comparison { op, args } => {
                let fused = Self::comparison_to_fused_opcode(op)?;
                return Some((fused, args.clone()));
            }
            _ => return None,
        };

        match self.var_producer.get(&inner2)? {
            VarProducer::Comparison { op, args } => {
                let fused = Self::comparison_to_fused_opcode(op)?;
                Some((fused, args.clone()))
            }
            _ => None,
        }
    }

    fn comparison_to_fused_opcode(op: &crate::core::CorePrimOp) -> Option<OpCode> {
        use crate::core::CorePrimOp;
        match op {
            CorePrimOp::Eq => Some(OpCode::OpCmpEqJumpNotTruthy),
            CorePrimOp::NEq => Some(OpCode::OpCmpNeJumpNotTruthy),
            CorePrimOp::Gt => Some(OpCode::OpCmpGtJumpNotTruthy),
            CorePrimOp::Ge => Some(OpCode::OpCmpGeJumpNotTruthy),
            CorePrimOp::Le => Some(OpCode::OpCmpLeJumpNotTruthy),
            // No fused Lt opcode exists — skip.
            _ => None,
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

            LirInstr::GetGlobal { dst, global_idx } => {
                self.emit_op(OpCode::OpGetGlobal, &[*global_idx]);
                self.pop_into(*dst);
            }

            LirInstr::TupleGet { dst, tuple, index } => {
                self.push_var(*tuple);
                self.emit_op(OpCode::OpTupleIndex, &[*index]);
                self.pop_into(*dst);
            }

            LirInstr::PrimCall { dst, op, args } => {
                // Record comparison producers for fused compare-and-jump.
                use crate::core::CorePrimOp;
                if let Some(dst_var) = dst {
                    match op {
                        CorePrimOp::Ge | CorePrimOp::Gt | CorePrimOp::Le
                        | CorePrimOp::Lt | CorePrimOp::Eq | CorePrimOp::NEq => {
                            self.var_producer.insert(*dst_var, VarProducer::Comparison {
                                op: *op,
                                args: args.clone(),
                            });
                        }
                        CorePrimOp::CmpEq if args.len() == 2 => {
                            if let Some(VarProducer::Comparison { .. }) = self.var_producer.get(&args[0]) {
                                self.var_producer.insert(*dst_var, VarProducer::CmpEqTrue { inner: args[0] });
                            }
                        }
                        _ => {}
                    }
                }

                // Push all arguments onto the stack.
                for &arg in args {
                    self.push_var(arg);
                }
                // Polymorphic ops use dedicated VM stack opcodes, not OpPrimOp
                // (the VM rejects them via OpPrimOp dispatch).
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
                    CorePrimOp::Neg => {
                        // Unary negation
                        self.emit_op(OpCode::OpMinus, &[]);
                    }
                    CorePrimOp::Index => {
                        // collection[key] — OpIndex pops (collection, key)
                        self.emit_op(OpCode::OpIndex, &[]);
                    }
                    CorePrimOp::Concat => {
                        // String/array concatenation — polymorphic add
                        self.emit_op(OpCode::OpAdd, &[]);
                    }
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

            // ── NaN-boxing (no-ops for the VM since Values are already
            //    tagged — these exist for the LLVM path) ────────
            LirInstr::UntagBool { dst, val } => {
                // Record for fused compare-and-jump peephole.
                if let Some(producer) = self.var_producer.get(val).cloned() {
                    if matches!(producer, VarProducer::CmpEqTrue { .. } | VarProducer::Comparison { .. }) {
                        self.var_producer.insert(*dst, VarProducer::UntagBool { inner: *val });
                    }
                }
                // Alias dst to val's slot (identity op).
                let slot = self.local_for(*val);
                self.locals.insert(*dst, slot);
            }
            LirInstr::TagInt { dst, raw }
            | LirInstr::UntagInt { dst, val: raw }
            | LirInstr::TagFloat { dst, raw }
            | LirInstr::UntagFloat { dst, val: raw }
            | LirInstr::TagBool { dst, raw }
            | LirInstr::TagPtr { dst, ptr: raw }
            | LirInstr::UntagPtr { dst, val: raw } => {
                // VM values are already NaN-boxed — tag/untag is identity.
                // Alias dst to raw's slot so no bytecode is emitted.
                let slot = self.local_for(*raw);
                self.locals.insert(*dst, slot);
            }

            LirInstr::GetTag { dst, val } => {
                // VM doesn't need explicit tag extraction — pattern matching
                // uses OpIsCons/OpIsEmptyList/etc.  Emit a placeholder.
                self.push_var(*val);
                self.pop_into(*dst);
            }

            // ── Collection construction ──────────────────────────────
            LirInstr::MakeArray { dst, elements } => {
                for &elem in elements {
                    self.push_var(elem);
                }
                self.emit_op(OpCode::OpArray, &[elements.len()]);
                self.pop_into(*dst);
            }
            LirInstr::MakeTuple { dst, elements } => {
                for &elem in elements {
                    self.push_var(elem);
                }
                self.emit_op(OpCode::OpTuple, &[elements.len()]);
                self.pop_into(*dst);
            }
            LirInstr::MakeHash { dst, pairs } => {
                // pairs are interleaved: [key0, val0, key1, val1, ...]
                for &p in pairs {
                    self.push_var(p);
                }
                self.emit_op(OpCode::OpHash, &[pairs.len()]);
                self.pop_into(*dst);
            }
            LirInstr::MakeList { dst, elements } => {
                // Build cons list: push all elements forward, push EmptyList, then
                // OpCons N times (matching CFG compiler's MakeList emission).
                // OpCons pops (head=below, tail=TOS) → Cons(head, tail).
                //   [e1, e2, e3, EmptyList] → Cons(e3, []) → Cons(e2, Cons(e3, [])) → ...
                for &elem in elements {
                    self.push_var(elem);
                }
                let empty_idx = self.add_constant(Value::EmptyList);
                self.emit_op(OpCode::OpConstant, &[empty_idx]);
                for _ in 0..elements.len() {
                    self.emit_op(OpCode::OpCons, &[]);
                }
                self.pop_into(*dst);
            }
            LirInstr::Interpolate { dst, parts } => {
                // String interpolation: convert each part to string, then concatenate.
                // Uses OpToString (interpolation-safe, no added quotes) not
                // OpPrimOp(ToString) (which wraps strings in quotes via format_value).
                if parts.is_empty() {
                    let idx = self.add_constant(Value::String(Rc::new(String::new())));
                    self.emit_op(OpCode::OpConstant, &[idx]);
                    self.pop_into(*dst);
                } else {
                    self.push_var(parts[0]);
                    self.emit_op(OpCode::OpToString, &[]);
                    for &part in &parts[1..] {
                        self.push_var(part);
                        self.emit_op(OpCode::OpToString, &[]);
                        self.emit_op(OpCode::OpAdd, &[]);
                    }
                    self.pop_into(*dst);
                }
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
            LirInstr::DropReuse { dst, val, size: _ } => {
                // The reuse path (Store/TagPtr) cannot work with the VM's
                // Rust-heap values — it's designed for the LLVM backend's C-heap
                // memory model.  Always return "not unique" (Tagged(0) = null
                // pointer) so the MakeCtor fallback path is taken.
                let _ = val;
                let idx = self.add_constant(Value::Integer(0));
                self.emit_op(OpCode::OpConstant, &[idx]);
                self.pop_into(*dst);
            }

            LirInstr::MakeClosure {
                dst,
                func_id,
                captures,
            } => {
                // Push captured values onto the stack.
                for &cap in captures {
                    self.push_var(cap);
                }
                // OpClosure pops the captures and creates a Closure from
                // the CompiledFunction stored in the constants pool.
                let const_idx = self.func_const_indices[func_id];
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
                if self.is_main {
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
                // Peephole: fused compare-and-jump.
                // Pattern: PrimCall(Ge/Gt/...) → CmpEq(result, true) → UntagBool → Branch
                // Emit: push a, push b, OpCmpXxJumpNotTruthy
                let fused = self.try_fused_cmp_branch(*cond);
                if let Some((cmp_opcode, args)) = fused {
                    for &arg in &args {
                        self.push_var(arg);
                    }
                    let patch_target = self.pos() + 1;
                    self.emit_op(cmp_opcode, &[0xFFFF]);
                    // Fused cmp-jump: falls through on truthy → then_block.
                    let patch_then = self.pos() + 1;
                    self.emit_op(OpCode::OpJump, &[0xFFFF]);
                    self.jump_patches.push((patch_then, *then_block));
                    // Jump target for not-truthy → else_block.
                    let target = self.pos();
                    let patch_else = self.pos() + 1;
                    self.emit_op(OpCode::OpJump, &[0xFFFF]);
                    self.jump_patches.push((patch_else, *else_block));
                    self.instructions[patch_target] = (target >> 8) as u8;
                    self.instructions[patch_target + 1] = target as u8;
                } else {
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
                    self.instructions[patch_not_truthy] = (trampoline >> 8) as u8;
                    self.instructions[patch_not_truthy + 1] = trampoline as u8;
                }
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
                kind,
            } => {
                // For direct calls, use OpCallDirect — no closure on stack.
                if let CallKind::Direct { func_id } = kind
                    && let Some(&const_idx) = self.func_const_indices.get(func_id)
                {
                    for &arg in args {
                        self.push_var(arg);
                    }
                    self.emit_op(OpCode::OpCallDirect, &[const_idx, args.len()]);
                } else {
                    self.push_var(*func);
                    for &arg in args {
                        self.push_var(arg);
                    }
                    self.emit_op(OpCode::OpCall, &[args.len()]);
                }
                self.pop_into(*dst);
                // Fall through to continuation block — emit jump if needed.
                let patch_pos = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((patch_pos, *cont));
            }

            LirTerminator::TailCall { func, args, kind } => {
                if let CallKind::Direct { func_id } = kind
                    && let Some(&const_idx) = self.func_const_indices.get(func_id)
                {
                    for &arg in args {
                        self.push_var(arg);
                    }
                    self.emit_op(OpCode::OpTailCallDirect, &[const_idx, args.len()]);
                } else {
                    self.push_var(*func);
                    for &arg in args {
                        self.push_var(arg);
                    }
                    self.emit_op(OpCode::OpTailCall, &[args.len()]);
                }
            }

            LirTerminator::MatchCtor {
                scrutinee,
                arms,
                default,
            } => {
                // Emit constructor pattern matching using VM-specific opcodes.
                // For each arm: push scrutinee, test constructor, jump on match.
                let mut match_patches: Vec<(usize, BlockId, Vec<LirVar>, CtorTag)> = Vec::new();

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
                        CtorTag::Some => {
                            self.emit_op(OpCode::OpIsSome, &[]);
                        }
                        CtorTag::Left => {
                            self.emit_op(OpCode::OpIsLeft, &[]);
                        }
                        CtorTag::Right => {
                            self.emit_op(OpCode::OpIsRight, &[]);
                        }
                        CtorTag::Named(name) => {
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
                    match_patches.push((patch, arm.target, arm.field_binders.clone(), arm.tag.clone()));
                }

                // Default: jump unconditionally.
                let default_patch = self.pos() + 1;
                self.emit_op(OpCode::OpJump, &[0xFFFF]);
                self.jump_patches.push((default_patch, *default));

                // Emit trampolines for each matched arm: pop bool, extract fields, jump.
                for (patch, target, field_binders, ctor_tag) in match_patches {
                    let trampoline = self.pos();
                    // Pop the leftover boolean from JumpTruthy.
                    self.emit_op(OpCode::OpPop, &[]);
                    // Extract fields from scrutinee into binder slots.
                    if !field_binders.is_empty() {
                        match &ctor_tag {
                            // Some/Left/Right are single-field wrappers with dedicated unwrap opcodes.
                            CtorTag::Some if field_binders.len() == 1 => {
                                self.push_var(*scrutinee);
                                self.emit_op(OpCode::OpUnwrapSome, &[]);
                                self.pop_into(field_binders[0]);
                            }
                            CtorTag::Left if field_binders.len() == 1 => {
                                self.push_var(*scrutinee);
                                self.emit_op(OpCode::OpUnwrapLeft, &[]);
                                self.pop_into(field_binders[0]);
                            }
                            CtorTag::Right if field_binders.len() == 1 => {
                                self.push_var(*scrutinee);
                                self.emit_op(OpCode::OpUnwrapRight, &[]);
                                self.pop_into(field_binders[0]);
                            }
                            // Cons: extract head and tail via OpConsHead/OpConsTail.
                            // These opcodes push the raw value directly (not Some-wrapped).
                            CtorTag::Cons if field_binders.len() == 2 => {
                                self.push_var(*scrutinee);
                                self.emit_op(OpCode::OpConsHead, &[]);
                                self.pop_into(field_binders[0]);
                                self.push_var(*scrutinee);
                                self.emit_op(OpCode::OpConsTail, &[]);
                                self.pop_into(field_binders[1]);
                            }
                            // Tuple: use OpTupleIndex for each field.
                            CtorTag::Tuple => {
                                for (i, &binder) in field_binders.iter().enumerate() {
                                    self.push_var(*scrutinee);
                                    self.emit_op(OpCode::OpTupleIndex, &[i]);
                                    self.pop_into(binder);
                                }
                            }
                            // General ADT: use OpAdtField for each field.
                            _ => {
                                self.push_var(*scrutinee);
                                if field_binders.len() == 2 {
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
