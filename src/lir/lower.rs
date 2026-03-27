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

// ── Object layout constants (match runtime/c/flux_rt.h) ──────────────────────

/// ADT header: {i32 ctor_tag, i32 field_count}, then i64 fields[].
const ADT_HEADER_SIZE: i32 = 8;
/// Tuple header: {i32 obj_tag, i32 arity}, then i64 fields[].
const TUPLE_PAYLOAD_OFFSET: i32 = 8;

/// Constructor tag IDs (must match core_to_llvm/codegen/adt.rs and runtime).
const SOME_TAG_ID: i64 = 1;
const LEFT_TAG_ID: i64 = 2;
const RIGHT_TAG_ID: i64 = 3;
const CONS_TAG_ID: i64 = 4;
const FIRST_USER_TAG_ID: i64 = 5;

/// RC runtime object type tags (match runtime/c/rc.c).
const OBJ_TAG_ADT: u8 = 3;
const OBJ_TAG_TUPLE: u8 = 4;
const OBJ_TAG_CLOSURE: u8 = 5;

// ── Public entry point ───────────────────────────────────────────────────────

/// Lower a complete `CoreProgram` to `LirProgram`.
pub fn lower_program(program: &CoreProgram) -> LirProgram {
    let mut lir = LirProgram::new();

    // Collect all top-level binder IDs so cross-function references resolve.
    let top_level_binders: Vec<CoreBinderId> =
        program.defs.iter().map(|d| d.binder.id).collect();

    // Phase 1: Lower all non-main defs first, recording binder → func_idx.
    let mut binder_func_map: HashMap<CoreBinderId, usize> = HashMap::new();
    let num_defs = program.defs.len();
    for (i, def) in program.defs.iter().enumerate() {
        if i == num_defs - 1 {
            break; // skip main (last def) for now
        }
        let func = lower_def(def, &mut lir, &top_level_binders, &HashMap::new());
        let func_idx = lir.functions.len();
        lir.functions.push(func);
        binder_func_map.insert(def.binder.id, func_idx);
    }

    // Phase 2: Lower the main function with knowledge of sibling function indices.
    if let Some(main_def) = program.defs.last() {
        let func = lower_def(main_def, &mut lir, &top_level_binders, &binder_func_map);
        lir.functions.push(func);
    }

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
}

impl<'a> FnLower<'a> {
    fn new(name: String, program: &'a mut LirProgram) -> Self {
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
                } else {
                    // Unresolved external variable — emit as a named constant
                    // placeholder.  Full resolution happens in later phases.
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
                // For letrec, bind the variable first (for recursive references),
                // then lower the RHS.  The RHS is typically a Lam which will be
                // handled as a closure in Phase 4.  For now, use a placeholder.
                let placeholder = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst: placeholder,
                    value: LirConst::None,
                });
                self.bind(var.id, placeholder);
                let rhs_var = self.lower_expr(rhs);
                // Update the binding to point to the actual value.
                self.bind(var.id, rhs_var);
                self.lower_expr(body)
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
                    let mut inner = FnLower::new(func_name, &mut temp_program);

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
                let func_var = self.lower_expr(func);
                let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();

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

            CoreExpr::Return { value, .. } => self.lower_expr(value),

            CoreExpr::MemberAccess { object, member, .. } => {
                // Member access on a module object.  At LIR level this is
                // a runtime field load.  The bytecode/LLVM emitters resolve
                // module members statically; LIR emits a PrimCall placeholder.
                let obj = self.lower_expr(object);
                let dst = self.fresh_var();
                self.emit(LirInstr::Copy { dst, src: obj });
                // TODO: resolve module member at emit time
                let _ = member;
                dst
            }

            CoreExpr::TupleField {
                object, index, ..
            } => {
                // Tuple field access → untag pointer, load at field offset.
                let obj = self.lower_expr(object);
                let ptr = self.fresh_var();
                self.emit(LirInstr::UntagPtr { dst: ptr, val: obj });
                let dst = self.fresh_var();
                // Tuple layout: {i32 obj_tag, i32 arity, i64 fields[]}
                // Fields start at offset 8, each field is 8 bytes.
                let offset = TUPLE_PAYLOAD_OFFSET + (*index as i32) * 8;
                self.emit(LirInstr::Load {
                    dst,
                    ptr,
                    offset,
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
        // Extract the NaN-box tag to determine if it's a pointer or immediate.
        let tag = self.fresh_var();
        self.emit(LirInstr::GetTag { dst: tag, val: scrut });

        // Pre-allocate blocks for all alts and collect (case_tag, block_id) pairs.
        let mut alt_block_indices: Vec<usize> = Vec::new();
        for _alt in alts {
            alt_block_indices.push(self.new_block());
        }

        // Build switch cases based on pattern types.
        let mut cases: Vec<(i64, BlockId)> = Vec::new();
        let mut default_idx: Option<usize> = None;

        for (i, alt) in alts.iter().enumerate() {
            let block_id = BlockId(alt_block_indices[i] as u32);
            match &alt.pat {
                CorePat::EmptyList => cases.push((0x4, block_id)),
                CorePat::Con { tag: core_tag, .. } => match core_tag {
                    CoreTag::None => cases.push((0x2, block_id)),
                    CoreTag::Nil => cases.push((0x4, block_id)),
                    CoreTag::Some => cases.push((-SOME_TAG_ID, block_id)),
                    CoreTag::Left => cases.push((-LEFT_TAG_ID, block_id)),
                    CoreTag::Right => cases.push((-RIGHT_TAG_ID, block_id)),
                    CoreTag::Cons => cases.push((-CONS_TAG_ID, block_id)),
                    CoreTag::Named(_) => cases.push((-FIRST_USER_TAG_ID, block_id)),
                },
                CorePat::Tuple(_) => cases.push((-100, block_id)),
                CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {
                    default_idx = Some(alt_block_indices[i]);
                }
            }
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

        // Emit the switch from the current block.
        self.set_terminator(LirTerminator::Switch {
            scrutinee: tag,
            cases,
            default: default_id,
        });

        // Lower each alt's body in its pre-allocated block.
        for (i, alt) in alts.iter().enumerate() {
            self.switch_to_block(alt_block_indices[i]);
            self.bind_pattern(scrut, &alt.pat);
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
            CorePat::Con { tag, fields, .. } => {
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
                let ptr = self.fresh_var();
                self.emit(LirInstr::UntagPtr {
                    dst: ptr,
                    val: scrut,
                });
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_val = self.fresh_var();
                    let offset = TUPLE_PAYLOAD_OFFSET + (i as i32) * 8;
                    self.emit(LirInstr::Load {
                        dst: field_val,
                        ptr,
                        offset,
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
                let ctor_id = match tag {
                    CoreTag::Some => SOME_TAG_ID,
                    CoreTag::Left => LEFT_TAG_ID,
                    CoreTag::Right => RIGHT_TAG_ID,
                    CoreTag::Cons => CONS_TAG_ID,
                    _ => unreachable!(),
                };
                self.lower_boxed_ctor(ctor_id as i32, &field_vars)
            }
            CoreTag::Named(_) => {
                // User-defined ADT — use FIRST_USER_TAG_ID for now.
                // TODO: ADT registry for stable tag assignment.
                self.lower_boxed_ctor(FIRST_USER_TAG_ID as i32, &field_vars)
            }
        }
    }

    /// Allocate a heap ADT: {i32 ctor_tag, i32 field_count, i64 fields[]}.
    fn lower_boxed_ctor(&mut self, ctor_tag: i32, fields: &[LirVar]) -> LirVar {
        let n_fields = fields.len();
        let size = (ADT_HEADER_SIZE as u32) + (n_fields as u32) * 8;
        let ptr = self.fresh_var();
        self.emit(LirInstr::Alloc {
            dst: ptr,
            size,
            scan_fields: n_fields as u8,
            obj_tag: OBJ_TAG_ADT,
        });

        // Write header: ctor_tag at offset 0, field_count at offset 4.
        let tag_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: tag_val,
            value: LirConst::Tagged(ctor_tag as i64),
        });
        self.emit(LirInstr::Store {
            ptr,
            offset: 0,
            val: tag_val,
        });
        let count_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: count_val,
            value: LirConst::Tagged(n_fields as i64),
        });
        self.emit(LirInstr::Store {
            ptr,
            offset: 4,
            val: count_val,
        });

        // Write fields.
        for (i, field) in fields.iter().enumerate() {
            self.emit(LirInstr::Store {
                ptr,
                offset: ADT_HEADER_SIZE + (i as i32) * 8,
                val: *field,
            });
        }

        // Tag the pointer for NaN-boxing.
        let dst = self.fresh_var();
        self.emit(LirInstr::TagPtr { dst, ptr });
        dst
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
            if let Some(mask) = field_mask {
                if mask & (1 << i) == 0 {
                    continue; // field unchanged, skip write
                }
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

        // Fresh alloc path.
        self.switch_to_block(fresh_idx);
        let fresh_val = self.lower_boxed_ctor(ctor_tag, &field_vars);
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
) -> LirFunction {
    let name = format!("def_{}", def.binder.id.0);
    let mut ctx = FnLower::new(name, program);

    // Pre-register all top-level binders.  If we know the function index
    // (from binder_func_map), emit MakeClosure; otherwise emit None placeholder.
    for &binder_id in top_level_binders {
        if !ctx.env.contains_key(&binder_id) {
            let var = ctx.fresh_var();
            if let Some(&func_idx) = binder_func_map.get(&binder_id) {
                ctx.emit(LirInstr::MakeClosure {
                    dst: var,
                    func_idx,
                    captures: Vec::new(),
                });
            } else {
                ctx.emit(LirInstr::Const {
                    dst: var,
                    value: LirConst::None,
                });
            }
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
        LirInstr::Copy { dst, src } => format!("{dst} = copy {src}"),
        LirInstr::Const { dst, value } => format!("{dst} = const {value:?}"),
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
            if let Some(b) = var.binder {
                if !bound.contains(&b) {
                    free.insert(b);
                }
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
            if let Some(b) = var.binder {
                if !bound.contains(&b) {
                    free.insert(b);
                }
            }
            free_vars_rec(body, bound, free);
        }
        CoreExpr::Reuse { token, fields, .. } => {
            if let Some(b) = token.binder {
                if !bound.contains(&b) {
                    free.insert(b);
                }
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
            if let Some(b) = scrutinee.binder {
                if !bound.contains(&b) {
                    free.insert(b);
                }
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
