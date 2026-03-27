//! Core IR → LIR lowering (Proposal 0132 Phase 2).
//!
//! Translates the functional Core IR into the flat, NaN-box-aware LIR CFG.
//! Phase 2 handles: literals, variables, let/letrec bindings, primop calls,
//! and top-level non-capturing functions.

use std::collections::HashMap;

use crate::core::{
    CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreLit, CorePrimOp, CoreProgram, FluxRep,
};
use crate::lir::*;

// ── Public entry point ───────────────────────────────────────────────────────

/// Lower a complete `CoreProgram` to `LirProgram`.
pub fn lower_program(program: &CoreProgram) -> LirProgram {
    let mut lir = LirProgram::new();

    for def in &program.defs {
        let func = lower_def(def, &mut lir);
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

            // ── Constructs handled in later phases ───────────────────
            CoreExpr::Case { scrutinee, .. } => {
                // Phase 3: pattern matching
                let scrut = self.lower_expr(scrutinee);
                scrut // placeholder — returns scrutinee value
            }

            CoreExpr::Con { fields, .. } => {
                // Phase 3: ADT construction
                if let Some(first) = fields.first() {
                    self.lower_expr(first) // placeholder
                } else {
                    let dst = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst,
                        value: LirConst::None,
                    });
                    dst
                }
            }

            CoreExpr::Return { value, .. } => self.lower_expr(value),

            CoreExpr::MemberAccess { object, .. } => {
                // Phase 3: member access → Load at known offset
                self.lower_expr(object) // placeholder
            }

            CoreExpr::TupleField { object, .. } => {
                // Phase 3: tuple field → Load at known offset
                self.lower_expr(object) // placeholder
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
fn lower_def(def: &CoreDef, program: &mut LirProgram) -> LirFunction {
    let name = format!("def_{}", def.binder.id.0);
    let mut ctx = FnLower::new(name, program);

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
