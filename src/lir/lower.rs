//! Core IR → LIR lowering (Proposal 0132 Phases 2–3).
//!
//! Translates the functional Core IR into the flat, NaN-box-aware LIR CFG.
//! - Phase 2: literals, variables, let/letrec bindings, primop calls, top-level functions.
//! - Phase 3: pattern matching (Case), ADT/cons/tuple construction (Con), tuple field access.

use std::collections::HashMap;

use crate::core::{
    CoreAlt, CoreBinderId, CoreDef, CoreExpr, CoreLit, CorePat, CorePrimOp, CoreProgram, CoreTag,
};
use crate::lir::*;
use crate::syntax::interner::Interner;

// ── Object layout constants (match runtime/c/flux_rt.h) ──────────────────────

/// ADT header: {i32 ctor_tag, i32 field_count}, then i64 fields[].
const ADT_HEADER_SIZE: i32 = 8;
/// Tuple header: {i32 obj_tag, i32 arity}, then i64 fields[].
/// Used by the LLVM emitter for direct memory access; the bytecode emitter
/// uses TupleGet (→ OpTupleIndex) instead.
#[allow(dead_code)]
const TUPLE_PAYLOAD_OFFSET: i32 = 8;

/// Constructor tag IDs (must match core_to_llvm/codegen/adt.rs and runtime).
const SOME_TAG_ID: i64 = 1;
const LEFT_TAG_ID: i64 = 2;
const RIGHT_TAG_ID: i64 = 3;
const CONS_TAG_ID: i64 = 4;
const FIRST_USER_TAG_ID: i64 = 5;

/// RC runtime object type tags (match runtime/c/rc.c).
/// Used by the reuse path (Alloc instructions) and future LLVM emitter.
#[allow(dead_code)]
const OBJ_TAG_ADT: u8 = 3;
#[allow(dead_code)]
const OBJ_TAG_TUPLE: u8 = 4;
#[allow(dead_code)]
const OBJ_TAG_CLOSURE: u8 = 5;

// ── Library function → PrimOp resolution ────────────────────────────────────
//
// Maps unbound function names (from Flow.* prelude modules) to CorePrimOp
// variants that have C runtime implementations.  This allows the LIR to emit
// direct PrimCall instructions instead of GetGlobal + closure Call, which is
// essential for native compilation where the VM globals table is unavailable.

fn resolve_library_primop(name: &str, arity: usize) -> Option<CorePrimOp> {
    // Strip module prefix (e.g. "Flow.List.first" → "first")
    let short = name.rsplit('.').next().unwrap_or(name);
    match (short, arity) {
        // Collection access
        ("first", 1) => Some(CorePrimOp::First),
        ("last", 1) => Some(CorePrimOp::Last),
        ("rest", 1) => Some(CorePrimOp::Rest),
        // Higher-order (C runtime calls closures via flux_call_closure_c)
        ("map", 2) => Some(CorePrimOp::HoMap),
        ("filter", 2) => Some(CorePrimOp::HoFilter),
        ("sort", 1) => Some(CorePrimOp::Sort),
        ("sort_by", 2) => Some(CorePrimOp::SortBy),
        ("any", 2) => Some(CorePrimOp::HoAny),
        ("all", 2) => Some(CorePrimOp::HoAll),
        ("each", 2) => Some(CorePrimOp::HoEach),
        ("find", 2) => Some(CorePrimOp::HoFind),
        ("count", 2) => Some(CorePrimOp::HoCount),
        ("flat_map", 2) => Some(CorePrimOp::HoFlatMap),
        ("zip", 2) => Some(CorePrimOp::Zip),
        ("flatten", 1) => Some(CorePrimOp::Flatten),
        _ => None,
    }
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Lower a complete `CoreProgram` to `LirProgram`.
pub fn lower_program(program: &CoreProgram) -> LirProgram {
    lower_program_with_interner(program, None, None)
}

/// Lower a `CoreProgram` to `LirProgram` with symbol resolution support.
///
/// `globals_map` maps `Symbol` → VM global index for imported/prelude functions.
/// When a `CoreExpr::Var` has no binder (external reference), the lowerer checks
/// this map and emits `LirInstr::GetGlobal` instead of a `None` placeholder.
pub fn lower_program_with_interner(
    program: &CoreProgram,
    interner: Option<&Interner>,
    globals_map: Option<&HashMap<String, usize>>,
) -> LirProgram {
    let mut lir = LirProgram::new();

    // Collect all top-level binder IDs so cross-function references resolve.
    let top_level_binders: Vec<CoreBinderId> =
        program.defs.iter().map(|d| d.binder.id).collect();

    // Find the main def — it could be at any position in defs[].
    let main_idx = if let Some(interner) = interner {
        program
            .defs
            .iter()
            .position(|d| interner.resolve(d.name) == "main")
            .unwrap_or(0)
    } else {
        0
    };

    // Phase 1: Lower all non-main defs, pre-assigning function indices
    // so self-recursive and mutually-recursive functions work.
    let mut binder_func_map: HashMap<CoreBinderId, usize> = HashMap::new();
    let mut func_counter = 0;
    for (i, def) in program.defs.iter().enumerate() {
        if i == main_idx {
            continue;
        }
        binder_func_map.insert(def.binder.id, func_counter);
        func_counter += 1;
    }

    for (i, def) in program.defs.iter().enumerate() {
        if i == main_idx {
            continue;
        }
        let func = lower_def(def, &mut lir, &top_level_binders, &binder_func_map, interner, globals_map);
        lir.functions.push(func);
    }

    // Phase 2: Lower main with knowledge of all sibling function indices.
    // Main is always last in LIR (emit_program expects this).
    let main_def = &program.defs[main_idx];
    let func = lower_def(main_def, &mut lir, &top_level_binders, &binder_func_map, interner, globals_map);
    lir.functions.push(func);

    lir
}

// ── Per-function lowering context ────────────────────────────────────────────

/// Tracks state while lowering a single function body to LIR.
struct FnLower<'a> {
    /// Mapping from Core binder IDs to LIR variables.
    env: HashMap<CoreBinderId, LirVar>,
    /// The function being built.
    func: LirFunction,
    /// Index of the currently active block.
    current_block: usize,
    /// Reference to the program-level string pool.
    program: &'a mut LirProgram,
    /// Optional interner for resolving Symbol → string names.
    interner: Option<&'a Interner>,
    /// Optional mapping from function name → VM global index for imported functions.
    globals_map: Option<&'a HashMap<String, usize>>,
    /// Tracks LIR variables that were produced by GetGlobal, mapping to their
    /// function name.  Used by the App handler to intercept closure calls to
    /// known library functions and emit PrimCall instead.
    global_var_names: HashMap<LirVar, String>,
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        program: &'a mut LirProgram,
        interner: Option<&'a Interner>,
        globals_map: Option<&'a HashMap<String, usize>>,
    ) -> Self {
        let entry_block = LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Unreachable, // placeholder
        };
        Self {
            env: HashMap::new(),
            func: LirFunction {
                name,
                params: Vec::new(),
                blocks: vec![entry_block],
                next_var: 0,
                capture_vars: Vec::new(),
            },
            current_block: 0,
            program,
            interner,
            global_var_names: HashMap::new(),
            globals_map,
        }
    }

    /// Resolve an Identifier (Symbol) to a string name.
    /// Falls back to the numeric symbol ID if no interner is available.
    fn resolve_name(&self, name: crate::syntax::Identifier) -> String {
        if let Some(interner) = self.interner {
            interner.resolve(name).to_string()
        } else {
            format!("ctor_{}", name)
        }
    }

    /// Allocate a fresh LIR variable.
    fn fresh_var(&mut self) -> LirVar {
        self.func.fresh_var()
    }

    /// Emit an instruction into the current block.
    fn emit(&mut self, instr: LirInstr) {
        self.func.blocks[self.current_block].instrs.push(instr);
    }

    /// Set the terminator of the current block.
    fn set_terminator(&mut self, term: LirTerminator) {
        self.func.blocks[self.current_block].terminator = term;
    }

    /// Copy `val` into the first parameter of `target_block` (SSA phi-node bridging).
    fn emit_copy_to_join_param(&mut self, val: LirVar, target_block: BlockId) {
        let target_idx = target_block.0 as usize;
        if let Some(&param) = self.func.blocks[target_idx].params.first() {
            self.emit(LirInstr::Copy { dst: param, src: val });
        }
    }

    /// Create a new block and return its index.
    fn new_block(&mut self) -> usize {
        let id = BlockId(self.func.blocks.len() as u32);
        self.func.blocks.push(LirBlock {
            id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Unreachable,
        });
        self.func.blocks.len() - 1
    }

    /// Switch to emitting into a different block.
    fn switch_to_block(&mut self, block_idx: usize) {
        self.current_block = block_idx;
    }

    /// Bind a Core binder to a LIR variable.
    fn bind(&mut self, binder: CoreBinderId, var: LirVar) {
        self.env.insert(binder, var);
    }

    /// Look up a Core binder, returning its LIR variable.
    fn lookup(&self, binder: CoreBinderId) -> LirVar {
        *self.env.get(&binder).unwrap_or_else(|| {
            panic!("LIR lower: unbound CoreBinderId({})", binder.0)
        })
    }

    // ── Expression lowering ──────────────────────────────────────────

    /// Lower a `CoreExpr` and return the `LirVar` holding the result.
    /// The result is always a NaN-boxed i64 value.
    fn lower_expr(&mut self, expr: &CoreExpr) -> LirVar {
        match expr {
            CoreExpr::Lit(lit, _span) => self.lower_lit(lit),

            CoreExpr::Var { var, .. } => {
                if let Some(binder) = var.binder {
                    self.lookup(binder)
                } else if let Some(globals) = self.globals_map {
                    // External variable — resolve name and check the globals map.
                    let name = self.resolve_name(var.name);
                    if let Some(&global_idx) = globals.get(&name) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, name);
                        dst
                    } else {
                        // Unknown external — emit None placeholder.
                        let dst = self.fresh_var();
                        self.emit(LirInstr::Const {
                            dst,
                            value: LirConst::None,
                        });
                        dst
                    }
                } else {
                    // No globals map available — emit None placeholder.
                    let dst = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst,
                        value: LirConst::None,
                    });
                    dst
                }
            }

            CoreExpr::Let {
                var, rhs, body, ..
            } => {
                let rhs_var = self.lower_expr(rhs);
                self.bind(var.id, rhs_var);
                self.lower_expr(body)
            }

            CoreExpr::LetRec {
                var, rhs, body, ..
            } => {
                // For letrec where the RHS is a lambda (recursive function):
                // 1. Pre-assign a function index in the program
                // 2. Create a MakeClosure that the lambda body can reference
                // 3. The inner lambda sees itself via its own function index
                if let CoreExpr::Lam { params, body: lam_body, .. } = rhs.as_ref() {
                    let free = collect_free_vars(rhs);
                    let outer_captures: Vec<(CoreBinderId, LirVar)> = free
                        .iter()
                        .filter(|id| **id != var.id) // exclude self-reference
                        .filter_map(|id| self.env.get(id).copied().map(|v| (*id, v)))
                        .collect();

                    let mut temp_program = std::mem::replace(
                        &mut *self.program,
                        LirProgram { functions: Vec::new(), string_pool: Vec::new() },
                    );

                    // Pre-assign function index so self-recursion works.
                    // Push a placeholder to reserve the slot so nested letrecs
                    // don't occupy this index.
                    let func_idx = temp_program.functions.len();
                    temp_program.functions.push(LirFunction {
                        name: format!("letrec_{}_placeholder", func_idx),
                        params: Vec::new(),
                        blocks: Vec::new(),
                        next_var: 0,
                        capture_vars: Vec::new(),
                    });

                    let func_name = format!("letrec_{}", func_idx);
                    let mut inner = FnLower::new(func_name, &mut temp_program, self.interner, self.globals_map);

                    // Capture outer variables.
                    for &(binder_id, _outer_var) in &outer_captures {
                        let inner_var = inner.fresh_var();
                        inner.func.capture_vars.push(inner_var);
                        inner.bind(binder_id, inner_var);
                    }

                    // Self-reference: the letrec variable inside the lambda
                    // creates a MakeClosure to itself (same func_idx).
                    let self_var = inner.fresh_var();
                    inner.emit(LirInstr::MakeClosure {
                        dst: self_var,
                        func_idx,
                        captures: (0..outer_captures.len())
                            .map(|i| inner.func.capture_vars[i])
                            .collect(),
                    });
                    inner.bind(var.id, self_var);

                    // Register parameters.
                    for param in params {
                        let pv = inner.fresh_var();
                        inner.bind(param.id, pv);
                        inner.func.params.push(pv);
                    }

                    // Lower the body.
                    let result = inner.lower_expr(lam_body);
                    inner.set_terminator(LirTerminator::Return(result));
                    let inner_func = inner.func;

                    *self.program = temp_program;
                    // Replace the placeholder with the actual function.
                    self.program.functions[func_idx] = inner_func;

                    // Emit MakeClosure in the outer context.
                    let outer_capture_vars: Vec<LirVar> =
                        outer_captures.iter().map(|&(_, v)| v).collect();
                    let dst = self.fresh_var();
                    self.emit(LirInstr::MakeClosure {
                        dst,
                        func_idx,
                        captures: outer_capture_vars,
                    });
                    self.bind(var.id, dst);
                    self.lower_expr(body)
                } else {
                    // Non-lambda letrec (rare): use placeholder approach.
                    let placeholder = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst: placeholder,
                        value: LirConst::None,
                    });
                    self.bind(var.id, placeholder);
                    let rhs_var = self.lower_expr(rhs);
                    self.bind(var.id, rhs_var);
                    self.lower_expr(body)
                }
            }

            CoreExpr::PrimOp { op, args, .. } => self.lower_primop(*op, args),

            CoreExpr::Lam { params, body, .. } => {
                // Always create a nested LirFunction for lambdas.
                // Even non-capturing lambdas need to be callable via OpCall.
                let free = collect_free_vars(expr);
                let outer_captures: Vec<(CoreBinderId, LirVar)> = free
                    .iter()
                    .filter_map(|id| self.env.get(id).copied().map(|v| (*id, v)))
                    .collect();

                {

                    // Temporarily take the program so the inner FnLower can use it
                    // (Rust can't have two &mut borrows of self.program).
                    let mut temp_program = std::mem::replace(
                        &mut *self.program,
                        LirProgram { functions: Vec::new(), string_pool: Vec::new() },
                    );

                    let func_name = format!("closure_{}", temp_program.functions.len());
                    let mut inner = FnLower::new(func_name, &mut temp_program, self.interner, self.globals_map);

                    // Map captured variables: create fresh LirVars inside the inner
                    // function, mark them as capture_vars (→ OpGetFree in emitter).
                    for &(binder_id, _outer_var) in &outer_captures {
                        let inner_var = inner.fresh_var();
                        inner.func.capture_vars.push(inner_var);
                        inner.bind(binder_id, inner_var);
                    }

                    // Register parameters.
                    for param in params {
                        let pv = inner.fresh_var();
                        inner.bind(param.id, pv);
                        inner.func.params.push(pv);
                    }

                    // Lower the body in the inner context.
                    let result = inner.lower_expr(body);
                    inner.set_terminator(LirTerminator::Return(result));

                    let inner_func = inner.func;

                    // Restore the program and add the inner function.
                    *self.program = temp_program;
                    let func_idx = self.program.functions.len();
                    self.program.functions.push(inner_func);

                    // Emit MakeClosure in the outer context.
                    let outer_capture_vars: Vec<LirVar> =
                        outer_captures.iter().map(|&(_, v)| v).collect();
                    let dst = self.fresh_var();
                    self.emit(LirInstr::MakeClosure {
                        dst,
                        func_idx,
                        captures: outer_capture_vars,
                    });
                    dst
                }
            }

            CoreExpr::App { func, args, .. }
            | CoreExpr::AetherCall {
                func, args, ..
            } => {
                // Check if func is an unbound variable that maps to a known
                // library function with a C runtime implementation.  If so,
                // emit PrimCall directly instead of GetGlobal + Call (which
                // would crash in native mode where the globals table is empty).
                // Check for unbound Var or MemberAccess → library primop.
                let resolved_name = match func.as_ref() {
                    CoreExpr::Var { var, .. } if var.binder.is_none() => {
                        Some(self.resolve_name(var.name))
                    }
                    CoreExpr::MemberAccess { member, .. } => {
                        Some(self.resolve_name(*member))
                    }
                    _ => None,
                };
                if let Some(name) = resolved_name {
                    if let Some(op) = resolve_library_primop(&name, args.len()) {
                        let arg_vars: Vec<LirVar> =
                            args.iter().map(|a| self.lower_expr(a)).collect();
                        let dst = self.fresh_var();
                        self.emit(LirInstr::PrimCall {
                            dst: Some(dst),
                            op,
                            args: arg_vars,
                        });
                        return dst;
                    }
                }

                let func_var = self.lower_expr(func);
                let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();

                // If func_var was produced by a GetGlobal for a known library
                // function, emit a direct PrimCall instead of a closure Call.
                if let Some(name) = self.global_var_names.get(&func_var).cloned() {
                    if let Some(op) = resolve_library_primop(&name, arg_vars.len()) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::PrimCall {
                            dst: Some(dst),
                            op,
                            args: arg_vars,
                        });
                        return dst;
                    }
                }

                // Emit a Call.  The continuation block receives the result.
                let cont_idx = self.new_block();
                let cont_id = BlockId(cont_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[cont_idx].params.push(result);

                self.set_terminator(LirTerminator::Call {
                    dst: result,
                    func: func_var,
                    args: arg_vars,
                    cont: cont_id,
                });

                self.switch_to_block(cont_idx);
                result
            }

            // ── Pattern matching (Phase 3) ────────────────────────────
            CoreExpr::Case {
                scrutinee,
                alts,
                ..
            } => self.lower_case(scrutinee, alts),

            // ── ADT / collection construction (Phase 3) ──────────────
            CoreExpr::Con { tag, fields, .. } => self.lower_con(tag, fields),

            CoreExpr::Return { value, .. } => {
                // Early return from function — emit Return terminator.
                let val = self.lower_expr(value);
                self.set_terminator(LirTerminator::Return(val));
                // Create a new unreachable block for any dead code after the return.
                let dead_idx = self.new_block();
                self.switch_to_block(dead_idx);
                val
            }

            CoreExpr::MemberAccess { object, member, .. } => {
                // Qualified member access (e.g. Array.sort).  Try to resolve
                // "<object_name>.<member_name>" in the globals map.
                if let Some(globals) = self.globals_map {
                    let member_str = self.resolve_name(*member);
                    // Try resolving with the object's name as prefix.
                    let qualified = if let CoreExpr::Var { var, .. } = object.as_ref() {
                        let obj_name = self.resolve_name(var.name);
                        Some(format!("{obj_name}.{member_str}"))
                    } else {
                        None
                    };
                    if let Some(ref qname) = qualified
                        && let Some(&global_idx) = globals.get(qname.as_str())
                    {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, member_str.clone());
                        return dst;
                    }
                    // Try just the member name (unqualified).
                    if let Some(&global_idx) = globals.get(&member_str) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, member_str.clone());
                        return dst;
                    }
                }
                // Fallback: lower the object and ignore the member.
                let obj = self.lower_expr(object);
                let dst = self.fresh_var();
                self.emit(LirInstr::Copy { dst, src: obj });
                dst
            }

            CoreExpr::TupleField {
                object, index, ..
            } => {
                let obj = self.lower_expr(object);
                let dst = self.fresh_var();
                self.emit(LirInstr::TupleGet {
                    dst,
                    tuple: obj,
                    index: *index,
                });
                dst
            }

            // ── Effect handlers (Phase 9) ────────────────────────────
            CoreExpr::Perform { .. } | CoreExpr::Handle { .. } => {
                let dst = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst,
                    value: LirConst::None,
                });
                dst
            }

            // ── Aether RC nodes (Phase 5) ─────────────────────────────
            CoreExpr::Dup { var, body, .. } => {
                if let Some(binder) = var.binder {
                    let v = self.lookup(binder);
                    self.emit(LirInstr::Dup { val: v });
                }
                self.lower_expr(body)
            }

            CoreExpr::Drop { var, body, .. } => {
                if let Some(binder) = var.binder {
                    let v = self.lookup(binder);
                    self.emit(LirInstr::Drop { val: v });
                }
                self.lower_expr(body)
            }

            CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                ..
            } => self.lower_reuse(token, tag, fields, *field_mask),

            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } => self.lower_drop_specialized(scrutinee, unique_body, shared_body),
        }
    }

    // ── Literal lowering ─────────────────────────────────────────────

    fn lower_lit(&mut self, lit: &CoreLit) -> LirVar {
        let dst = self.fresh_var();
        let value = match lit {
            CoreLit::Int(n) => LirConst::Int(*n),
            CoreLit::Float(f) => LirConst::Float(*f),
            CoreLit::Bool(b) => LirConst::Bool(*b),
            CoreLit::String(s) => {
                self.program.intern_string(s.clone());
                LirConst::String(s.clone())
            }
            CoreLit::Unit => LirConst::None,
        };
        self.emit(LirInstr::Const { dst, value });
        dst
    }

    // ── PrimOp lowering ──────────────────────────────────────────────

    fn lower_primop(&mut self, op: CorePrimOp, args: &[CoreExpr]) -> LirVar {
        let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();

        match op {
            // Typed integer arithmetic → inline LIR instructions.
            // Untag operands, compute, retag result.
            CorePrimOp::IAdd => self.lower_int_binop(LirIntOp::Add, &arg_vars),
            CorePrimOp::ISub => self.lower_int_binop(LirIntOp::Sub, &arg_vars),
            CorePrimOp::IMul => self.lower_int_binop(LirIntOp::Mul, &arg_vars),
            CorePrimOp::IDiv => self.lower_int_binop(LirIntOp::Div, &arg_vars),
            CorePrimOp::IMod => self.lower_int_binop(LirIntOp::Rem, &arg_vars),

            // Typed integer comparisons → inline ICmp.
            CorePrimOp::ICmpEq => self.lower_int_cmp(CmpOp::Eq, &arg_vars),
            CorePrimOp::ICmpNe => self.lower_int_cmp(CmpOp::Ne, &arg_vars),
            CorePrimOp::ICmpLt => self.lower_int_cmp(CmpOp::Slt, &arg_vars),
            CorePrimOp::ICmpLe => self.lower_int_cmp(CmpOp::Sle, &arg_vars),
            CorePrimOp::ICmpGt => self.lower_int_cmp(CmpOp::Sgt, &arg_vars),
            CorePrimOp::ICmpGe => self.lower_int_cmp(CmpOp::Sge, &arg_vars),

            // Boolean logic → control flow (no VM opcode for And/Or).
            CorePrimOp::And => {
                // a && b → if a then b else false
                let a = arg_vars[0];
                let b = arg_vars[1];
                let then_idx = self.new_block();
                let else_idx = self.new_block();
                let join_idx = self.new_block();
                let then_id = BlockId(then_idx as u32);
                let else_id = BlockId(else_idx as u32);
                let join_id = BlockId(join_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[join_idx].params.push(result);

                // Branch on a.
                let a_bool = self.fresh_var();
                self.emit(LirInstr::UntagBool { dst: a_bool, val: a });
                self.set_terminator(LirTerminator::Branch {
                    cond: a_bool,
                    then_block: then_id,
                    else_block: else_id,
                });

                // Then: result = b
                self.switch_to_block(then_idx);
                self.emit_copy_to_join_param(b, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                // Else: result = false
                self.switch_to_block(else_idx);
                let false_val = self.fresh_var();
                self.emit(LirInstr::Const { dst: false_val, value: LirConst::Bool(false) });
                self.emit_copy_to_join_param(false_val, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                self.switch_to_block(join_idx);
                result
            }
            CorePrimOp::Or => {
                // a || b → if a then true else b
                let a = arg_vars[0];
                let b = arg_vars[1];
                let then_idx = self.new_block();
                let else_idx = self.new_block();
                let join_idx = self.new_block();
                let then_id = BlockId(then_idx as u32);
                let else_id = BlockId(else_idx as u32);
                let join_id = BlockId(join_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[join_idx].params.push(result);

                let a_bool = self.fresh_var();
                self.emit(LirInstr::UntagBool { dst: a_bool, val: a });
                self.set_terminator(LirTerminator::Branch {
                    cond: a_bool,
                    then_block: then_id,
                    else_block: else_id,
                });

                // Then: result = true
                self.switch_to_block(then_idx);
                let true_val = self.fresh_var();
                self.emit(LirInstr::Const { dst: true_val, value: LirConst::Bool(true) });
                self.emit_copy_to_join_param(true_val, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                // Else: result = b
                self.switch_to_block(else_idx);
                self.emit_copy_to_join_param(b, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                self.switch_to_block(join_idx);
                result
            }

            // Collection constructors → dedicated LIR instructions.
            CorePrimOp::MakeArray => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeArray { dst, elements: arg_vars });
                dst
            }
            CorePrimOp::MakeTuple => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeTuple { dst, elements: arg_vars });
                dst
            }
            CorePrimOp::MakeHash => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeHash { dst, pairs: arg_vars });
                dst
            }
            CorePrimOp::MakeList => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeList { dst, elements: arg_vars });
                dst
            }
            CorePrimOp::Interpolate => {
                let dst = self.fresh_var();
                self.emit(LirInstr::Interpolate { dst, parts: arg_vars });
                dst
            }

            // Everything else → C runtime call via PrimCall.
            _ => {
                let dst = self.fresh_var();
                self.emit(LirInstr::PrimCall {
                    dst: Some(dst),
                    op,
                    args: arg_vars,
                });
                dst
            }
        }
    }

    /// Lower typed integer binary op: untag → compute → retag.
    fn lower_int_binop(&mut self, int_op: LirIntOp, args: &[LirVar]) -> LirVar {
        let a_raw = self.fresh_var();
        let b_raw = self.fresh_var();
        self.emit(LirInstr::UntagInt {
            dst: a_raw,
            val: args[0],
        });
        self.emit(LirInstr::UntagInt {
            dst: b_raw,
            val: args[1],
        });

        let result_raw = self.fresh_var();
        let instr = match int_op {
            LirIntOp::Add => LirInstr::IAdd {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Sub => LirInstr::ISub {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Mul => LirInstr::IMul {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Div => LirInstr::IDiv {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Rem => LirInstr::IRem {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
        };
        self.emit(instr);

        let dst = self.fresh_var();
        self.emit(LirInstr::TagInt {
            dst,
            raw: result_raw,
        });
        dst
    }

    /// Lower typed integer comparison: untag → ICmp → retag as bool.
    fn lower_int_cmp(&mut self, cmp_op: CmpOp, args: &[LirVar]) -> LirVar {
        let a_raw = self.fresh_var();
        let b_raw = self.fresh_var();
        self.emit(LirInstr::UntagInt {
            dst: a_raw,
            val: args[0],
        });
        self.emit(LirInstr::UntagInt {
            dst: b_raw,
            val: args[1],
        });

        let cmp_result = self.fresh_var();
        self.emit(LirInstr::ICmp {
            dst: cmp_result,
            op: cmp_op,
            a: a_raw,
            b: b_raw,
        });

        let dst = self.fresh_var();
        self.emit(LirInstr::TagBool {
            dst,
            raw: cmp_result,
        });
        dst
    }

    // ── Phase 3: Pattern matching ────────────────────────────────────

    /// Lower a `Case` expression to LIR blocks with branches/switches.
    fn lower_case(&mut self, scrutinee: &CoreExpr, alts: &[CoreAlt]) -> LirVar {
        let scrut = self.lower_expr(scrutinee);

        // Single wildcard/var alt: no branching needed.
        if alts.len() == 1 {
            return self.lower_single_alt(scrut, &alts[0]);
        }

        // Create a join block where all alt branches merge their results.
        let join_idx = self.new_block();
        let join_id = self.func.blocks[join_idx].id;
        let result_var = self.fresh_var();
        self.func.blocks[join_idx].params.push(result_var);

        // Classify patterns to decide dispatch strategy.
        let has_lit = alts.iter().any(|a| matches!(a.pat, CorePat::Lit(_)));
        let has_con = alts.iter().any(|a| {
            matches!(
                a.pat,
                CorePat::Con { .. } | CorePat::EmptyList | CorePat::Tuple(_)
            )
        });

        if has_lit {
            self.lower_case_lit(scrut, alts, join_id);
        } else if has_con {
            self.lower_case_con(scrut, alts, join_id);
        } else {
            // All wildcards/vars — just take the first alt.
            let val = self.lower_single_alt(scrut, &alts[0]);
            self.emit(LirInstr::Copy { dst: result_var, src: val });
            self.set_terminator(LirTerminator::Jump(join_id));
            self.switch_to_block(join_idx);
            return result_var;
        }

        // Switch to join block for subsequent code.
        self.switch_to_block(join_idx);
        result_var
    }

    /// Lower a single case alternative (bind pattern vars, evaluate body).
    fn lower_single_alt(&mut self, scrut: LirVar, alt: &CoreAlt) -> LirVar {
        self.bind_pattern(scrut, &alt.pat);
        if let Some(guard) = &alt.guard {
            // Guards: evaluate guard, if false fall through.
            // For now, just evaluate guard and ignore it (Phase 3 simplification).
            let _guard_val = self.lower_expr(guard);
        }
        self.lower_expr(&alt.rhs)
    }

    /// Lower a Case on literal patterns — chain of if-else comparisons.
    fn lower_case_lit(
        &mut self,
        scrut: LirVar,
        alts: &[CoreAlt],
        join_block: BlockId,
    ) {
        for alt in alts {
            match &alt.pat {
                CorePat::Lit(lit) => {
                    let lit_var = self.lower_lit(lit);
                    let cmp = self.fresh_var();
                    self.emit(LirInstr::PrimCall {
                        dst: Some(cmp),
                        op: CorePrimOp::CmpEq,
                        args: vec![scrut, lit_var],
                    });
                    let raw_cmp = self.fresh_var();
                    self.emit(LirInstr::UntagBool {
                        dst: raw_cmp,
                        val: cmp,
                    });

                    let then_idx = self.new_block();
                    let else_idx = self.new_block();
                    let then_id = BlockId(then_idx as u32);
                    let else_id = BlockId(else_idx as u32);

                    self.set_terminator(LirTerminator::Branch {
                        cond: raw_cmp,
                        then_block: then_id,
                        else_block: else_id,
                    });

                    // Then: evaluate body, jump to join.
                    self.switch_to_block(then_idx);
                    self.bind_pattern(scrut, &alt.pat);
                    let val = self.lower_expr(&alt.rhs);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));

                    // Else: continue chain.
                    self.switch_to_block(else_idx);
                }
                CorePat::Wildcard | CorePat::Var(_) => {
                    self.bind_pattern(scrut, &alt.pat);
                    let val = self.lower_expr(&alt.rhs);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));
                    return; // default handled, done.
                }
                _ => {
                    let val = self.lower_single_alt(scrut, alt);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));
                }
            }
        }
        // No default — unreachable.
        self.set_terminator(LirTerminator::Unreachable);
    }

    /// Lower a Case on constructor patterns (ADT, cons, None, Some, etc.).
    fn lower_case_con(
        &mut self,
        scrut: LirVar,
        alts: &[CoreAlt],
        join_block: BlockId,
    ) {
        // Pre-allocate blocks for all alts.
        let mut alt_block_indices: Vec<usize> = Vec::new();
        for _alt in alts {
            alt_block_indices.push(self.new_block());
        }

        // Build MatchCtor arms from patterns.
        let mut arms: Vec<CtorArm> = Vec::new();
        let mut default_idx: Option<usize> = None;

        for (i, alt) in alts.iter().enumerate() {
            let block_id = BlockId(alt_block_indices[i] as u32);

            let (ctor_tag, field_pats) = match &alt.pat {
                CorePat::EmptyList => (CtorTag::EmptyList, vec![]),
                CorePat::Con { tag: core_tag, fields, .. } => {
                    let ct = match core_tag {
                        CoreTag::None => CtorTag::None,
                        CoreTag::Nil => CtorTag::EmptyList,
                        CoreTag::Some => CtorTag::Some,
                        CoreTag::Left => CtorTag::Left,
                        CoreTag::Right => CtorTag::Right,
                        CoreTag::Cons => CtorTag::Cons,
                        CoreTag::Named(name) => CtorTag::Named(self.resolve_name(*name)),
                    };
                    (ct, fields.clone())
                }
                CorePat::Tuple(fields) => (CtorTag::Tuple, fields.clone()),
                CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {
                    default_idx = Some(alt_block_indices[i]);
                    continue;
                }
            };

            // Create field binder LirVars for this arm.
            let field_binders: Vec<LirVar> = field_pats.iter().map(|_| self.fresh_var()).collect();

            arms.push(CtorArm {
                tag: ctor_tag,
                field_binders: field_binders.clone(),
                target: block_id,
            });

            // Pre-bind pattern variables in the target block.
            // We'll bind them when we switch to the block below.
        }

        // Default block.
        let default_block_idx = default_idx.unwrap_or_else(|| {
            let idx = self.new_block();
            let save = self.current_block;
            self.switch_to_block(idx);
            self.set_terminator(LirTerminator::Unreachable);
            self.switch_to_block(save);
            idx
        });
        let default_id = BlockId(default_block_idx as u32);

        // Emit the MatchCtor terminator.
        self.set_terminator(LirTerminator::MatchCtor {
            scrutinee: scrut,
            arms: arms.clone(),
            default: default_id,
        });

        // Lower each alt's body, binding field binders from MatchCtor arms.
        let mut arm_idx = 0;
        for (i, alt) in alts.iter().enumerate() {
            self.switch_to_block(alt_block_indices[i]);

            match &alt.pat {
                CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {
                    // Default arm — bind scrutinee to variable if Var pattern.
                    if let CorePat::Var(binder) = &alt.pat {
                        self.bind(binder.id, scrut);
                    }
                }
                CorePat::EmptyList => {
                    // No fields to bind.
                    arm_idx += 1;
                }
                CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
                    // Bind field binders from the MatchCtor arm.
                    if arm_idx < arms.len() {
                        let arm = &arms[arm_idx];
                        for (j, field_pat) in fields.iter().enumerate() {
                            if j < arm.field_binders.len() {
                                self.bind_pattern(arm.field_binders[j], field_pat);
                            }
                        }
                    }
                    arm_idx += 1;
                }
            }

            let val = self.lower_expr(&alt.rhs);
            self.emit_copy_to_join_param(val, join_block);
            self.set_terminator(LirTerminator::Jump(join_block));
        }
    }

    /// Bind pattern variables to LIR vars by extracting fields from scrutinee.
    fn bind_pattern(&mut self, scrut: LirVar, pat: &CorePat) {
        match pat {
            CorePat::Wildcard => {}
            CorePat::Var(binder) => {
                self.bind(binder.id, scrut);
            }
            CorePat::Lit(_) => {}
            CorePat::EmptyList => {}
            CorePat::Con { tag: _, fields, .. } => {
                if fields.is_empty() {
                    return;
                }
                // Untag the pointer to access heap fields.
                let ptr = self.fresh_var();
                self.emit(LirInstr::UntagPtr {
                    dst: ptr,
                    val: scrut,
                });
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_val = self.fresh_var();
                    let offset = ADT_HEADER_SIZE + (i as i32) * 8;
                    self.emit(LirInstr::Load {
                        dst: field_val,
                        ptr,
                        offset,
                    });
                    self.bind_pattern(field_val, field_pat);
                }
            }
            CorePat::Tuple(fields) => {
                if fields.is_empty() {
                    return;
                }
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_val = self.fresh_var();
                    self.emit(LirInstr::TupleGet {
                        dst: field_val,
                        tuple: scrut,
                        index: i,
                    });
                    self.bind_pattern(field_val, field_pat);
                }
            }
        }
    }

    // ── Phase 3: Constructor lowering ────────────────────────────────

    /// Lower a `Con` expression (ADT, cons, some, none, etc.).
    fn lower_con(&mut self, tag: &CoreTag, fields: &[CoreExpr]) -> LirVar {
        let field_vars: Vec<LirVar> = fields.iter().map(|f| self.lower_expr(f)).collect();

        match tag {
            CoreTag::None | CoreTag::Nil => {
                // Immediate values — no heap allocation.
                let dst = self.fresh_var();
                let value = if matches!(tag, CoreTag::Nil) {
                    LirConst::EmptyList
                } else {
                    LirConst::None
                };
                self.emit(LirInstr::Const { dst, value });
                dst
            }
            CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Cons => {
                let ctor_tag = match tag {
                    CoreTag::Some => SOME_TAG_ID as i32,
                    CoreTag::Left => LEFT_TAG_ID as i32,
                    CoreTag::Right => RIGHT_TAG_ID as i32,
                    CoreTag::Cons => CONS_TAG_ID as i32,
                    _ => unreachable!(),
                };
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeCtor {
                    dst,
                    ctor_tag,
                    ctor_name: None, // built-in ctors don't need a name
                    fields: field_vars,
                });
                dst
            }
            CoreTag::Named(name) => {
                let ctor_name = self.resolve_name(*name);
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeCtor {
                    dst,
                    ctor_tag: FIRST_USER_TAG_ID as i32,
                    ctor_name: Some(ctor_name),
                    fields: field_vars,
                });
                dst
            }
        }
    }

    // ── Phase 5: Aether RC ───────────────────────────────────────────

    /// Lower `Reuse { token, tag, fields, field_mask }`.
    ///
    /// Perceus reuse: try to reuse `token`'s heap allocation for a new
    /// constructor.  Emit `DropReuse` to test uniqueness.  If the token
    /// was unique (returned non-null), write fields in-place.  If shared,
    /// fall back to a fresh allocation.
    fn lower_reuse(
        &mut self,
        token: &crate::core::CoreVarRef,
        tag: &CoreTag,
        fields: &[CoreExpr],
        field_mask: Option<u64>,
    ) -> LirVar {
        let field_vars: Vec<LirVar> = fields.iter().map(|f| self.lower_expr(f)).collect();

        let ctor_tag = match tag {
            CoreTag::Some => SOME_TAG_ID as i32,
            CoreTag::Left => LEFT_TAG_ID as i32,
            CoreTag::Right => RIGHT_TAG_ID as i32,
            CoreTag::Cons => CONS_TAG_ID as i32,
            CoreTag::Named(_) => FIRST_USER_TAG_ID as i32,
            CoreTag::None | CoreTag::Nil => {
                // None/Nil are immediates — no allocation to reuse.
                let dst = self.fresh_var();
                let value = if matches!(tag, CoreTag::Nil) {
                    LirConst::EmptyList
                } else {
                    LirConst::None
                };
                self.emit(LirInstr::Const { dst, value });
                return dst;
            }
        };

        // Try to reuse the token's allocation.
        let token_var = if let Some(b) = token.binder {
            self.lookup(b)
        } else {
            let v = self.fresh_var();
            self.emit(LirInstr::Const { dst: v, value: LirConst::None });
            v
        };

        let reuse_ptr = self.fresh_var();
        self.emit(LirInstr::DropReuse {
            dst: reuse_ptr,
            val: token_var,
        });

        // Branch: if reuse_ptr != 0, reuse; else fresh alloc.
        let is_reusable = self.fresh_var();
        let null = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: null,
            value: LirConst::Tagged(0),
        });
        self.emit(LirInstr::ICmp {
            dst: is_reusable,
            op: CmpOp::Ne,
            a: reuse_ptr,
            b: null,
        });

        let reuse_idx = self.new_block();
        let fresh_idx = self.new_block();
        let join_idx = self.new_block();
        let reuse_id = BlockId(reuse_idx as u32);
        let fresh_id = BlockId(fresh_idx as u32);
        let join_id = BlockId(join_idx as u32);

        let result = self.fresh_var();
        self.func.blocks[join_idx].params.push(result);

        self.set_terminator(LirTerminator::Branch {
            cond: is_reusable,
            then_block: reuse_id,
            else_block: fresh_id,
        });

        // Reuse path: write header + fields into existing allocation.
        self.switch_to_block(reuse_idx);
        let tag_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: tag_val,
            value: LirConst::Tagged(ctor_tag as i64),
        });
        self.emit(LirInstr::Store {
            ptr: reuse_ptr,
            offset: 0,
            val: tag_val,
        });
        let count_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: count_val,
            value: LirConst::Tagged(field_vars.len() as i64),
        });
        self.emit(LirInstr::Store {
            ptr: reuse_ptr,
            offset: 4,
            val: count_val,
        });
        for (i, fv) in field_vars.iter().enumerate() {
            // If field_mask is set, skip unchanged fields on the reuse path.
            if let Some(mask) = field_mask && mask & (1 << i) == 0 {
                continue; // field unchanged, skip write
            }
            self.emit(LirInstr::Store {
                ptr: reuse_ptr,
                offset: ADT_HEADER_SIZE + (i as i32) * 8,
                val: *fv,
            });
        }
        let reuse_tagged = self.fresh_var();
        self.emit(LirInstr::TagPtr {
            dst: reuse_tagged,
            ptr: reuse_ptr,
        });
        self.emit_copy_to_join_param(reuse_tagged, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Fresh alloc path — use MakeCtor (high-level).
        self.switch_to_block(fresh_idx);
        let ctor_name = match tag {
            CoreTag::Named(name) => Some(self.resolve_name(*name)),
            _ => None,
        };
        let fresh_val = self.fresh_var();
        self.emit(LirInstr::MakeCtor {
            dst: fresh_val,
            ctor_tag,
            ctor_name,
            fields: field_vars.clone(),
        });
        self.emit_copy_to_join_param(fresh_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Join.
        self.switch_to_block(join_idx);
        result
    }

    /// Lower `DropSpecialized { scrutinee, unique_body, shared_body }`.
    ///
    /// Perceus drop specialization: test if scrutinee is uniquely owned.
    /// - Unique: fields are already owned, only free the shell.
    /// - Shared: dup fields, decrement scrutinee refcount.
    fn lower_drop_specialized(
        &mut self,
        scrutinee: &crate::core::CoreVarRef,
        unique_body: &CoreExpr,
        shared_body: &CoreExpr,
    ) -> LirVar {
        let scrut_var = if let Some(b) = scrutinee.binder {
            self.lookup(b)
        } else {
            let v = self.fresh_var();
            self.emit(LirInstr::Const { dst: v, value: LirConst::None });
            v
        };

        let is_unique = self.fresh_var();
        self.emit(LirInstr::IsUnique {
            dst: is_unique,
            val: scrut_var,
        });

        let unique_idx = self.new_block();
        let shared_idx = self.new_block();
        let join_idx = self.new_block();
        let unique_id = BlockId(unique_idx as u32);
        let shared_id = BlockId(shared_idx as u32);
        let join_id = BlockId(join_idx as u32);

        let result = self.fresh_var();
        self.func.blocks[join_idx].params.push(result);

        self.set_terminator(LirTerminator::Branch {
            cond: is_unique,
            then_block: unique_id,
            else_block: shared_id,
        });

        // Unique path.
        self.switch_to_block(unique_idx);
        let unique_val = self.lower_expr(unique_body);
        self.emit_copy_to_join_param(unique_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Shared path.
        self.switch_to_block(shared_idx);
        let shared_val = self.lower_expr(shared_body);
        self.emit_copy_to_join_param(shared_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Join.
        self.switch_to_block(join_idx);
        result
    }
}

/// Internal enum for typed integer binary operations.
enum LirIntOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

// ── Top-level definition lowering ────────────────────────────────────────────

/// Lower a single `CoreDef` to a `LirFunction`.
///
/// `binder_func_map` maps sibling function binder IDs to their LIR function
/// indices, so cross-function references emit `MakeClosure` instead of
/// `None` placeholders.
fn lower_def(
    def: &CoreDef,
    program: &mut LirProgram,
    top_level_binders: &[CoreBinderId],
    binder_func_map: &HashMap<CoreBinderId, usize>,
    interner: Option<&Interner>,
    globals_map: Option<&HashMap<String, usize>>,
) -> LirFunction {
    let name = format!("def_{}", def.binder.id.0);
    let mut ctx = FnLower::new(name, program, interner, globals_map);

    // Pre-register top-level binders that are callable functions.
    // Only binders in binder_func_map have compiled functions — skip the rest
    // (e.g., main's own binder) to avoid wasting variable slots.
    for &binder_id in top_level_binders {
        if !ctx.env.contains_key(&binder_id) && let Some(&func_idx) = binder_func_map.get(&binder_id) {
            let var = ctx.fresh_var();
            ctx.emit(LirInstr::MakeClosure {
                dst: var,
                func_idx,
                captures: Vec::new(),
            });
            ctx.bind(binder_id, var);
        }
    }

    // If the def is a lambda, register its parameters.
    let body = match &def.expr {
        CoreExpr::Lam { params, body, .. } => {
            for param in params {
                let pv = ctx.fresh_var();
                ctx.bind(param.id, pv);
                ctx.func.params.push(pv);
            }
            body.as_ref()
        }
        other => other,
    };

    let result = ctx.lower_expr(body);
    ctx.set_terminator(LirTerminator::Return(result));

    ctx.func
}

// ── Display ──────────────────────────────────────────────────────────────────

/// Pretty-print a `LirProgram` for `--dump-lir`.
pub fn display_program(program: &LirProgram) -> String {
    let mut out = String::new();
    for func in &program.functions {
        display_function(func, &mut out);
        out.push('\n');
    }
    out
}

fn display_function(func: &LirFunction, out: &mut String) {
    use std::fmt::Write;
    let params: Vec<String> = func.params.iter().map(|v| format!("{v}")).collect();
    if func.capture_vars.is_empty() {
        writeln!(out, "fn {}({}) {{", func.name, params.join(", ")).unwrap();
    } else {
        let caps: Vec<String> = func.capture_vars.iter().map(|v| format!("{v}")).collect();
        writeln!(
            out,
            "fn {}({}) captures [{}] {{",
            func.name,
            params.join(", "),
            caps.join(", ")
        )
        .unwrap();
    }
    for block in &func.blocks {
        display_block(block, out);
    }
    writeln!(out, "}}").unwrap();
}

fn display_block(block: &LirBlock, out: &mut String) {
    use std::fmt::Write;
    let params: Vec<String> = block.params.iter().map(|v| format!("{v}")).collect();
    if params.is_empty() {
        writeln!(out, "  {}:", block.id).unwrap();
    } else {
        writeln!(out, "  {}({}):", block.id, params.join(", ")).unwrap();
    }
    for instr in &block.instrs {
        writeln!(out, "    {}", display_instr(instr)).unwrap();
    }
    writeln!(out, "    {}", display_terminator(&block.terminator)).unwrap();
}

fn display_instr(instr: &LirInstr) -> String {
    match instr {
        LirInstr::Load { dst, ptr, offset } => format!("{dst} = load {ptr}[{offset}]"),
        LirInstr::Store { ptr, offset, val } => format!("store {ptr}[{offset}] = {val}"),
        LirInstr::Alloc { dst, size, scan_fields, obj_tag } => {
            format!("{dst} = alloc({size}, scan={scan_fields}, tag={obj_tag})")
        }
        LirInstr::TagInt { dst, raw } => format!("{dst} = tag_int({raw})"),
        LirInstr::UntagInt { dst, val } => format!("{dst} = untag_int({val})"),
        LirInstr::TagFloat { dst, raw } => format!("{dst} = tag_float({raw})"),
        LirInstr::UntagFloat { dst, val } => format!("{dst} = untag_float({val})"),
        LirInstr::GetTag { dst, val } => format!("{dst} = get_tag({val})"),
        LirInstr::TagPtr { dst, ptr } => format!("{dst} = tag_ptr({ptr})"),
        LirInstr::UntagPtr { dst, val } => format!("{dst} = untag_ptr({val})"),
        LirInstr::TagBool { dst, raw } => format!("{dst} = tag_bool({raw})"),
        LirInstr::UntagBool { dst, val } => format!("{dst} = untag_bool({val})"),
        LirInstr::IAdd { dst, a, b } => format!("{dst} = iadd {a}, {b}"),
        LirInstr::ISub { dst, a, b } => format!("{dst} = isub {a}, {b}"),
        LirInstr::IMul { dst, a, b } => format!("{dst} = imul {a}, {b}"),
        LirInstr::IDiv { dst, a, b } => format!("{dst} = idiv {a}, {b}"),
        LirInstr::IRem { dst, a, b } => format!("{dst} = irem {a}, {b}"),
        LirInstr::ICmp { dst, op, a, b } => format!("{dst} = icmp {op} {a}, {b}"),
        LirInstr::PrimCall { dst, op, args } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            match dst {
                Some(d) => format!("{d} = call {:?}({})", op, args_str.join(", ")),
                None => format!("call {:?}({})", op, args_str.join(", ")),
            }
        }
        LirInstr::Dup { val } => format!("dup {val}"),
        LirInstr::Drop { val } => format!("drop {val}"),
        LirInstr::IsUnique { dst, val } => format!("{dst} = is_unique({val})"),
        LirInstr::DropReuse { dst, val } => format!("{dst} = drop_reuse({val})"),
        LirInstr::MakeClosure {
            dst,
            func_idx,
            captures,
        } => {
            let caps: Vec<String> = captures.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_closure(func={func_idx}, [{}])", caps.join(", "))
        }
        LirInstr::MakeArray { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_array([{}])", es.join(", "))
        }
        LirInstr::MakeTuple { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_tuple([{}])", es.join(", "))
        }
        LirInstr::MakeHash { dst, pairs } => {
            let ps: Vec<String> = pairs.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_hash([{}])", ps.join(", "))
        }
        LirInstr::MakeList { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_list([{}])", es.join(", "))
        }
        LirInstr::Interpolate { dst, parts } => {
            let ps: Vec<String> = parts.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = interpolate([{}])", ps.join(", "))
        }
        LirInstr::MakeCtor { dst, ctor_tag, ctor_name, fields } => {
            let fs: Vec<String> = fields.iter().map(|v| format!("{v}")).collect();
            let name = ctor_name.as_deref().unwrap_or("?");
            format!("{dst} = make_ctor(tag={ctor_tag}, name={name}, [{}])", fs.join(", "))
        }
        LirInstr::Copy { dst, src } => format!("{dst} = copy {src}"),
        LirInstr::Const { dst, value } => format!("{dst} = const {value:?}"),
        LirInstr::TupleGet { dst, tuple, index } => format!("{dst} = tuple_get({tuple}, {index})"),
        LirInstr::GetGlobal { dst, global_idx } => format!("{dst} = get_global({global_idx})"),
    }
}

fn display_terminator(term: &LirTerminator) -> String {
    match term {
        LirTerminator::Return(v) => format!("ret {v}"),
        LirTerminator::Jump(block) => format!("jmp {block}"),
        LirTerminator::Branch { cond, then_block, else_block } => {
            format!("br {cond}, {then_block}, {else_block}")
        }
        LirTerminator::Switch { scrutinee, cases, default } => {
            let cases_str: Vec<String> = cases
                .iter()
                .map(|(val, block)| format!("{val} -> {block}"))
                .collect();
            format!("switch {scrutinee} [{}, default -> {default}]", cases_str.join(", "))
        }
        LirTerminator::TailCall { func, args } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            format!("tailcall {func}({})", args_str.join(", "))
        }
        LirTerminator::Call { dst, func, args, cont } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = call {func}({}) -> {cont}", args_str.join(", "))
        }
        LirTerminator::MatchCtor { scrutinee, arms, default } => {
            let arms_str: Vec<String> = arms
                .iter()
                .map(|arm| {
                    let fs: Vec<String> = arm.field_binders.iter().map(|v| format!("{v}")).collect();
                    format!("{:?}({}) -> {}", arm.tag, fs.join(", "), arm.target)
                })
                .collect();
            format!(
                "match_ctor {scrutinee} [{}, default -> {default}]",
                arms_str.join(", ")
            )
        }
        LirTerminator::Unreachable => "unreachable".to_string(),
    }
}

// ── Free variable collection ─────────────────────────────────────────────────

use std::collections::HashSet;

/// Collect free variable binder IDs in a `CoreExpr`.
fn collect_free_vars(expr: &CoreExpr) -> HashSet<CoreBinderId> {
    let mut free = HashSet::new();
    let mut bound = HashSet::new();
    free_vars_rec(expr, &mut bound, &mut free);
    free
}

fn free_vars_rec(
    expr: &CoreExpr,
    bound: &mut HashSet<CoreBinderId>,
    free: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(b) = var.binder && !bound.contains(&b) {
                free.insert(b);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let added: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .map(|p| p.id)
                .collect();
            free_vars_rec(body, bound, free);
            for id in added {
                bound.remove(&id);
            }
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            free_vars_rec(func, bound, free);
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            free_vars_rec(rhs, bound, free);
            let added = bound.insert(var.id);
            free_vars_rec(body, bound, free);
            if added {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let added = bound.insert(var.id);
            free_vars_rec(rhs, bound, free);
            free_vars_rec(body, bound, free);
            if added {
                bound.remove(&var.id);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            free_vars_rec(scrutinee, bound, free);
            for alt in alts {
                let added = bind_pat(&alt.pat, bound);
                if let Some(g) = &alt.guard {
                    free_vars_rec(g, bound, free);
                }
                free_vars_rec(&alt.rhs, bound, free);
                for id in added {
                    bound.remove(&id);
                }
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::Return { value, .. } => free_vars_rec(value, bound, free),
        CoreExpr::Perform { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            free_vars_rec(body, bound, free);
            for h in handlers {
                let mut added = vec![];
                if bound.insert(h.resume.id) {
                    added.push(h.resume.id);
                }
                for p in &h.params {
                    if bound.insert(p.id) {
                        added.push(p.id);
                    }
                }
                free_vars_rec(&h.body, bound, free);
                for id in added {
                    bound.remove(&id);
                }
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            free_vars_rec(object, bound, free);
        }
        CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
            if let Some(b) = var.binder && !bound.contains(&b) {
                free.insert(b);
            }
            free_vars_rec(body, bound, free);
        }
        CoreExpr::Reuse { token, fields, .. } => {
            if let Some(b) = token.binder && !bound.contains(&b) {
                free.insert(b);
            }
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            if let Some(b) = scrutinee.binder && !bound.contains(&b) {
                free.insert(b);
            }
            free_vars_rec(unique_body, bound, free);
            free_vars_rec(shared_body, bound, free);
        }
    }
}

/// Bind pattern variables into the bound set, returning newly added IDs.
fn bind_pat(pat: &CorePat, bound: &mut HashSet<CoreBinderId>) -> Vec<CoreBinderId> {
    let mut added = vec![];
    match pat {
        CorePat::Var(b) => {
            if bound.insert(b.id) {
                added.push(b.id);
            }
        }
        CorePat::Con { fields, .. } => {
            for f in fields {
                added.extend(bind_pat(f, bound));
            }
        }
        CorePat::Tuple(fields) => {
            for f in fields {
                added.extend(bind_pat(f, bound));
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
    added
}
