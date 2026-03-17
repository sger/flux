use crate::{
    backend_ir::{
        IrBinaryOp, IrBlockParam, IrConst, IrExpr, IrInstr, IrListTest, IrMetadata, IrTagTest,
        IrTerminator, IrType, IrVar,
    },
    core::{CoreAlt, CoreExpr, CorePat, CoreTag},
    diagnostics::position::Span,
};

use super::primop::lower_lit;

impl<'a> super::fn_ctx::FnCtx<'a> {
    /// Lower a `Case` expression into branching IR blocks.
    pub(super) fn lower_case(
        &mut self,
        scrutinee: &CoreExpr,
        alts: &[CoreAlt],
        span: Span,
    ) -> IrVar {
        let scrut_var = self.lower_expr(scrutinee);
        let saved_env = self.env.clone();
        let saved_binder_names = self.binder_names.clone();

        // Allocate a "join" block where all arms converge via a block param.
        let join_idx = self.new_block();
        let result_var = self.ctx.alloc_var();
        self.blocks[join_idx].params.push(IrBlockParam {
            var: result_var,
            ty: IrType::Any,
        });
        let join_block_id = self.blocks[join_idx].id;

        for (i, alt) in alts.iter().enumerate() {
            let is_last = i == alts.len() - 1;
            self.env = saved_env.clone();
            self.binder_names = saved_binder_names.clone();

            // Track the fail block index so that the next alt starts from it.
            let mut next_block_idx = None;

            if !is_irrefutable(&alt.pat) {
                let test_var = self.emit_pattern_test(scrut_var, &alt.pat);
                let arm_block_idx = self.new_block();
                let arm_block_id = self.blocks[arm_block_idx].id;

                let fail_block_idx = if is_last {
                    // Last alt failing is unreachable (exhaustive match).
                    self.new_block()
                } else {
                    // Will be filled by the next iteration.
                    self.new_block()
                };
                let fail_block_id = self.blocks[fail_block_idx].id;

                self.set_terminator(IrTerminator::Branch {
                    cond: test_var,
                    then_block: arm_block_id,
                    else_block: fail_block_id,
                    metadata: IrMetadata::from_span(span),
                });
                self.current_block = arm_block_idx;
                next_block_idx = Some(fail_block_idx);
            }

            // Bind pattern variables.
            self.bind_pattern(scrut_var, &alt.pat);

            // Evaluate optional guard.
            if let Some(guard) = &alt.guard {
                let guard_var = self.lower_expr(guard);
                let body_idx = self.new_block();
                let body_block_id = self.blocks[body_idx].id;
                let fail_idx = next_block_idx.unwrap_or_else(|| self.new_block());
                let fail_block_id = self.blocks[fail_idx].id;
                next_block_idx = Some(fail_idx);
                self.set_terminator(IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: fail_block_id,
                    metadata: IrMetadata::from_span(span),
                });
                self.current_block = body_idx;
            }

            // Lower the arm body and jump to the join block.
            let arm_result = self.lower_expr(&alt.rhs);
            if self.current_block_is_open() {
                self.set_terminator(IrTerminator::Jump(
                    join_block_id,
                    vec![arm_result],
                    IrMetadata::from_span(span),
                ));
            }

            // Switch to the fail block so the next alt starts from it.
            if let Some(idx) = next_block_idx {
                self.current_block = idx;
            }
        }

        self.current_block = join_idx;
        self.env = saved_env;
        self.binder_names = saved_binder_names;
        result_var
    }

    /// Emit instructions that test whether `var` matches `pat`.
    /// Returns an `IrVar` holding a bool.
    pub(super) fn emit_pattern_test(&mut self, var: IrVar, pat: &CorePat) -> IrVar {
        match pat {
            CorePat::Wildcard | CorePat::Var(_) => {
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Const(IrConst::Bool(true)),
                    metadata: IrMetadata::empty(),
                });
                dest
            }
            CorePat::Lit(lit) => {
                let lit_var = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest: lit_var,
                    expr: IrExpr::Const(lower_lit(lit)),
                    metadata: IrMetadata::empty(),
                });
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Binary(IrBinaryOp::Eq, var, lit_var),
                    metadata: IrMetadata::empty(),
                });
                dest
            }
            CorePat::Con { tag, fields } => {
                let dest = self.ctx.alloc_var();
                let test_expr = match tag {
                    CoreTag::None => IrExpr::TagTest {
                        value: var,
                        tag: IrTagTest::None,
                    },
                    CoreTag::Some => IrExpr::TagTest {
                        value: var,
                        tag: IrTagTest::Some,
                    },
                    CoreTag::Left => IrExpr::TagTest {
                        value: var,
                        tag: IrTagTest::Left,
                    },
                    CoreTag::Right => IrExpr::TagTest {
                        value: var,
                        tag: IrTagTest::Right,
                    },
                    CoreTag::Nil => IrExpr::ListTest {
                        value: var,
                        tag: IrListTest::Empty,
                    },
                    CoreTag::Cons => IrExpr::ListTest {
                        value: var,
                        tag: IrListTest::Cons,
                    },
                    CoreTag::Named(name) => IrExpr::AdtTagTest {
                        value: var,
                        constructor: *name,
                    },
                };
                self.emit(IrInstr::Assign {
                    dest,
                    expr: test_expr,
                    metadata: IrMetadata::empty(),
                });
                let mut combined = dest;
                for (i, field_pat) in fields.iter().enumerate() {
                    if matches!(field_pat, CorePat::Wildcard | CorePat::Var(_)) {
                        continue;
                    }
                    let field_var = self.ctx.alloc_var();
                    let field_expr = match tag {
                        CoreTag::Some => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Some,
                        },
                        CoreTag::Left => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Left,
                        },
                        CoreTag::Right => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Right,
                        },
                        CoreTag::Cons if i == 0 => IrExpr::ListHead { value: var },
                        CoreTag::Cons => IrExpr::ListTail { value: var },
                        CoreTag::Named(_) => IrExpr::AdtField {
                            value: var,
                            index: i,
                        },
                        CoreTag::None | CoreTag::Nil => continue,
                    };
                    self.emit(IrInstr::Assign {
                        dest: field_var,
                        expr: field_expr,
                        metadata: IrMetadata::empty(),
                    });
                    let nested = self.emit_pattern_test(field_var, field_pat);
                    let both = self.ctx.alloc_var();
                    self.emit(IrInstr::Assign {
                        dest: both,
                        expr: IrExpr::Binary(IrBinaryOp::And, combined, nested),
                        metadata: IrMetadata::empty(),
                    });
                    combined = both;
                }
                combined
            }
            CorePat::Tuple(fields) => {
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::TupleArityTest {
                        value: var,
                        arity: fields.len(),
                    },
                    metadata: IrMetadata::empty(),
                });
                let mut combined = dest;
                for (i, field_pat) in fields.iter().enumerate() {
                    if matches!(field_pat, CorePat::Wildcard | CorePat::Var(_)) {
                        continue;
                    }
                    let field_var = self.ctx.alloc_var();
                    self.emit(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::TupleFieldAccess {
                            object: var,
                            index: i,
                        },
                        metadata: IrMetadata::empty(),
                    });
                    let nested = self.emit_pattern_test(field_var, field_pat);
                    let both = self.ctx.alloc_var();
                    self.emit(IrInstr::Assign {
                        dest: both,
                        expr: IrExpr::Binary(IrBinaryOp::And, combined, nested),
                        metadata: IrMetadata::empty(),
                    });
                    combined = both;
                }
                combined
            }
            CorePat::EmptyList => {
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::ListTest {
                        value: var,
                        tag: IrListTest::Empty,
                    },
                    metadata: IrMetadata::empty(),
                });
                dest
            }
        }
    }

    /// Bind pattern variables from `var` into `self.env`.
    pub(super) fn bind_pattern(&mut self, var: IrVar, pat: &CorePat) {
        match pat {
            CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
            CorePat::Var(binder) => {
                self.env.insert(binder.id, var);
                self.binder_names.insert(binder.id, binder.name);
            }
            CorePat::Con { tag, fields } => {
                if fields.is_empty() {
                    return;
                }
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_var = self.ctx.alloc_var();
                    let field_expr = match tag {
                        CoreTag::Some => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Some,
                        },
                        CoreTag::Left => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Left,
                        },
                        CoreTag::Right => IrExpr::TagPayload {
                            value: var,
                            tag: IrTagTest::Right,
                        },
                        CoreTag::Cons if i == 0 => IrExpr::ListHead { value: var },
                        CoreTag::Cons => IrExpr::ListTail { value: var },
                        CoreTag::Named(_) => IrExpr::AdtField {
                            value: var,
                            index: i,
                        },
                        CoreTag::None | CoreTag::Nil => return,
                    };
                    self.emit(IrInstr::Assign {
                        dest: field_var,
                        expr: field_expr,
                        metadata: IrMetadata::empty(),
                    });
                    self.bind_pattern(field_var, field_pat);
                }
            }
            CorePat::Tuple(fields) => {
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_var = self.ctx.alloc_var();
                    self.emit(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::TupleFieldAccess {
                            object: var,
                            index: i,
                        },
                        metadata: IrMetadata::empty(),
                    });
                    self.bind_pattern(field_var, field_pat);
                }
            }
        }
    }
}

fn is_irrefutable(pat: &CorePat) -> bool {
    matches!(pat, CorePat::Wildcard | CorePat::Var(_))
}
