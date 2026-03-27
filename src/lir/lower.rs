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

// ── Public entry point ───────────────────────────────────────────────────────

/// Lower a complete `CoreProgram` to `LirProgram`.
pub fn lower_program(program: &CoreProgram) -> LirProgram {
    let mut lir = LirProgram::new();

    // Collect all top-level binder IDs so cross-function references resolve.
    let top_level_binders: Vec<CoreBinderId> =
        program.defs.iter().map(|d| d.binder.id).collect();

    for def in &program.defs {
        let func = lower_def(def, &mut lir, &top_level_binders);
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
                // Phase 2: only handles the body inline (top-level functions
                // are lowered via lower_def).  Closures with captures are
                // Phase 4.  For now, lower the body directly.
                for param in params {
                    let pv = self.fresh_var();
                    self.bind(param.id, pv);
                    self.func.params.push(pv);
                }
                self.lower_expr(body)
            }

            CoreExpr::App { func, args, .. }
            | CoreExpr::AetherCall {
                func, args, ..
            } => {
                // Phase 4 will handle full call lowering.  For now, emit
                // a PrimCall placeholder for known function applications.
                let func_var = self.lower_expr(func);
                let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dst = self.fresh_var();
                self.emit(LirInstr::PrimCall {
                    dst: Some(dst),
                    op: CorePrimOp::Len, // placeholder — real call lowering in Phase 4
                    args: arg_vars,
                });
                // TODO(Phase 4): Emit LirTerminator::Call with func_var
                let _ = func_var;
                dst
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

            // ── Aether nodes (Phase 5) ───────────────────────────────
            CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
                self.lower_expr(body)
            }

            CoreExpr::Reuse { fields, .. } => {
                if let Some(first) = fields.first() {
                    self.lower_expr(first)
                } else {
                    let dst = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst,
                        value: LirConst::None,
                    });
                    dst
                }
            }

            CoreExpr::DropSpecialized {
                unique_body, ..
            } => self.lower_expr(unique_body),
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
            self.set_terminator(LirTerminator::Jump(join_id));
            // Patch: the jump needs to pass val as a block arg.
            // For simplicity, emit a Copy in the join block.
            self.switch_to_block(join_idx);
            self.emit(LirInstr::Copy {
                dst: result_var,
                src: val,
            });
            self.set_terminator(LirTerminator::Unreachable); // placeholder
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
                    let _val = self.lower_expr(&alt.rhs);
                    self.set_terminator(LirTerminator::Jump(join_block));

                    // Else: continue chain.
                    self.switch_to_block(else_idx);
                }
                CorePat::Wildcard | CorePat::Var(_) => {
                    self.bind_pattern(scrut, &alt.pat);
                    let _val = self.lower_expr(&alt.rhs);
                    self.set_terminator(LirTerminator::Jump(join_block));
                    return; // default handled, done.
                }
                _ => {
                    let _val = self.lower_single_alt(scrut, alt);
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
            let _ = val; // result flows to join block
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
fn lower_def(
    def: &CoreDef,
    program: &mut LirProgram,
    top_level_binders: &[CoreBinderId],
) -> LirFunction {
    let name = format!("def_{}", def.binder.id.0);
    let mut ctx = FnLower::new(name, program);

    // Pre-register all top-level binders as placeholders so cross-function
    // references resolve.  Real call lowering happens in Phase 4.
    for &binder_id in top_level_binders {
        if !ctx.env.contains_key(&binder_id) {
            let placeholder = ctx.fresh_var();
            ctx.emit(LirInstr::Const {
                dst: placeholder,
                value: LirConst::None, // placeholder for function reference
            });
            ctx.bind(binder_id, placeholder);
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
    writeln!(out, "fn {}({}) {{", func.name, params.join(", ")).unwrap();
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
