/// Core IR → backend IR lowering.
///
/// Translates `CoreProgram`/`CoreExpr` into the `IrFunction`/`IrBlock`
/// representation consumed by the VM bytecode compiler and Cranelift JIT.
///
/// Key design decisions:
/// - **Uncurrying**: Top-level `Lam` chains become multi-param `IrFunction`s.
/// - **Closures**: `Lam` inside expressions → `IrExpr::MakeClosure` + free-var capture.
/// - **Case compilation**: Patterns become sequences of tag/literal tests + jumps.
use std::collections::{HashMap, HashSet};

use crate::{
    cfg::{
        BlockId, FunctionId, IrBinaryOp, IrBlock, IrBlockParam, IrCallTarget, IrConst, IrExpr,
        IrFunction, IrFunctionOrigin, IrInstr, IrListTest, IrMetadata, IrParam, IrProgram,
        IrStringPart, IrTagTest, IrTerminator, IrTopLevelItem, IrType, IrVar,
    },
    diagnostics::position::Span,
    nary::{CoreAlt, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreProgram, CoreTag},
    syntax::Identifier,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a `CoreProgram` into an `IrProgram` ready for backend code generation.
pub fn lower_core_to_ir(core: &CoreProgram) -> IrProgram {
    let mut ctx = ToIrCtx::new();
    ctx.lower_program(core);
    ctx.finish()
}

// ── Global context ────────────────────────────────────────────────────────────

struct ToIrCtx {
    next_var_id: u32,
    next_block_id: u32,
    next_function_id: u32,
    functions: Vec<IrFunction>,
    top_level_items: Vec<IrTopLevelItem>,
    globals: Vec<Identifier>,
}

impl ToIrCtx {
    fn new() -> Self {
        Self {
            next_var_id: 0,
            next_block_id: 0,
            next_function_id: 0,
            functions: Vec::new(),
            top_level_items: Vec::new(),
            globals: Vec::new(),
        }
    }

    fn alloc_var(&mut self) -> IrVar {
        let id = self.next_var_id;
        self.next_var_id += 1;
        IrVar(id)
    }

    fn alloc_block(&mut self) -> BlockId {
        let id = self.next_block_id;
        self.next_block_id += 1;
        BlockId(id)
    }

    fn alloc_function(&mut self) -> FunctionId {
        let id = self.next_function_id;
        self.next_function_id += 1;
        FunctionId(id)
    }

    fn lower_program(&mut self, core: &CoreProgram) {
        // Create the module-level entry function.
        let entry_id = self.alloc_function();
        let entry_block = self.alloc_block();
        let mut entry_fn = FnCtx::new(
            self,
            entry_id,
            IrFunctionOrigin::ModuleTopLevel,
            entry_block,
        );

        for def in &core.defs {
            if let CoreExpr::Lam { params, body, .. } = &def.expr {
                // This def is a function — emit as a named IrFunction.
                let fn_id = entry_fn.ctx.alloc_function();
                let fn_block = entry_fn.ctx.alloc_block();
                {
                    let mut fn_ctx = FnCtx::new(
                        entry_fn.ctx,
                        fn_id,
                        IrFunctionOrigin::NamedFunction,
                        fn_block,
                    );
                    for &p in params {
                        let v = fn_ctx.ctx.alloc_var();
                        fn_ctx.env.insert(p, v);
                        fn_ctx.params.push(IrParam {
                            name: p,
                            var: v,
                            ty: IrType::Any,
                        });
                    }
                    let ret = fn_ctx.lower_expr(body);
                    fn_ctx.finish_return(ret, def.span);
                }
                entry_fn.ctx.globals.push(def.name);
                entry_fn.ctx.top_level_items.push(IrTopLevelItem::Function {
                    is_public: false,
                    name: def.name,
                    type_params: Vec::new(),
                    function_id: Some(fn_id),
                    parameters: params.clone(),
                    parameter_types: Vec::new(),
                    return_type: None,
                    effects: Vec::new(),
                    body: crate::syntax::block::Block {
                        statements: Vec::new(),
                        span: def.span,
                    },
                    span: def.span,
                });
            } else {
                // Value binding — evaluate in the entry function.
                let val = entry_fn.lower_expr(&def.expr);
                let g_var = entry_fn.ctx.alloc_var();
                entry_fn.emit(IrInstr::Assign {
                    dest: g_var,
                    expr: IrExpr::Var(val),
                    metadata: IrMetadata::empty(),
                });
                entry_fn.ctx.globals.push(def.name);
                entry_fn.env.insert(def.name, g_var);
            }
        }

        // Terminate the entry function.
        let ret = entry_fn.last_value.unwrap_or_else(|| {
            let v = entry_fn.ctx.alloc_var();
            entry_fn.emit(IrInstr::Assign {
                dest: v,
                expr: IrExpr::Const(IrConst::Unit),
                metadata: IrMetadata::empty(),
            });
            v
        });
        entry_fn.finish_return(ret, Span::default());
    }

    fn finish(self) -> IrProgram {
        IrProgram {
            top_level_items: self.top_level_items,
            functions: self.functions,
            entry: FunctionId(0),
            globals: self.globals,
            hm_expr_types: HashMap::new(),
            core: None, // already consumed
        }
    }
}

// ── Per-function context ───────────────────────────────────────────────────────

struct FnCtx<'a> {
    ctx: &'a mut ToIrCtx,
    id: FunctionId,
    origin: IrFunctionOrigin,
    name: Option<Identifier>,
    params: Vec<IrParam>,
    blocks: Vec<IrBlock>,
    current_block: usize,
    env: HashMap<Identifier, IrVar>,
    last_value: Option<IrVar>,
}

impl<'a> FnCtx<'a> {
    fn new(ctx: &'a mut ToIrCtx, id: FunctionId, origin: IrFunctionOrigin, entry: BlockId) -> Self {
        Self {
            ctx,
            id,
            name: None,
            origin,
            params: Vec::new(),
            blocks: vec![IrBlock {
                id: entry,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            }],
            current_block: 0,
            env: HashMap::new(),
            last_value: None,
        }
    }

    fn emit(&mut self, instr: IrInstr) {
        self.blocks[self.current_block].instrs.push(instr);
    }

    fn new_block(&mut self) -> usize {
        let id = self.ctx.alloc_block();
        self.blocks.push(IrBlock {
            id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.blocks.len() - 1
    }

    fn set_terminator(&mut self, t: IrTerminator) {
        self.blocks[self.current_block].terminator = t;
    }

    fn finish_return(self, ret: IrVar, span: Span) {
        let mut s = self;
        if matches!(
            s.blocks[s.current_block].terminator,
            IrTerminator::Unreachable(_)
        ) {
            s.blocks[s.current_block].terminator =
                IrTerminator::Return(ret, IrMetadata::from_span(span));
        }
        let entry = s.blocks[0].id;
        s.ctx.functions.push(IrFunction {
            id: s.id,
            name: s.name,
            params: s.params,
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: span,
            ret_type: IrType::Any,
            blocks: s.blocks,
            entry,
            origin: s.origin,
            metadata: IrMetadata::empty(),
        });
    }

    /// Lower a `CoreExpr`, returning the `IrVar` that holds the result.
    fn lower_expr(&mut self, expr: &CoreExpr) -> IrVar {
        match expr {
            CoreExpr::Var(name, span) => {
                if let Some(&v) = self.env.get(name) {
                    v
                } else {
                    let dest = self.ctx.alloc_var();
                    self.emit(IrInstr::Assign {
                        dest,
                        expr: IrExpr::LoadName(*name),
                        metadata: IrMetadata::from_span(*span),
                    });
                    dest
                }
            }

            CoreExpr::Lit(lit, span) => {
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Const(lower_lit(lit)),
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::Lam { .. } => self.lower_lam_as_closure(expr),

            CoreExpr::App { func, args, span } => {
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dest = self.ctx.alloc_var();
                let meta = IrMetadata::from_span(*span);

                match func.as_ref() {
                    CoreExpr::Var(name, _) => {
                        if let Some(&fv) = self.env.get(name) {
                            self.emit(IrInstr::Call {
                                dest,
                                target: IrCallTarget::Var(fv),
                                args: arg_vars,
                                metadata: meta,
                            });
                        } else {
                            self.emit(IrInstr::Call {
                                dest,
                                target: IrCallTarget::Named(*name),
                                args: arg_vars,
                                metadata: meta,
                            });
                        }
                    }
                    other => {
                        let fv = self.lower_expr(other);
                        self.emit(IrInstr::Call {
                            dest,
                            target: IrCallTarget::Var(fv),
                            args: arg_vars,
                            metadata: meta,
                        });
                    }
                }
                dest
            }

            CoreExpr::Let { var, rhs, body, .. } => {
                let rhs_var = self.lower_expr(rhs);
                let old = self.env.insert(*var, rhs_var);
                let result = self.lower_expr(body);
                match old {
                    Some(v) => {
                        self.env.insert(*var, v);
                    }
                    None => {
                        self.env.remove(var);
                    }
                }
                result
            }

            CoreExpr::LetRec { var, rhs, body, .. } => {
                let rhs_var = self.lower_expr(rhs);
                let old = self.env.insert(*var, rhs_var);
                let result = self.lower_expr(body);
                match old {
                    Some(v) => {
                        self.env.insert(*var, v);
                    }
                    None => {
                        self.env.remove(var);
                    }
                }
                result
            }

            CoreExpr::Case {
                scrutinee,
                alts,
                span,
            } => self.lower_case(scrutinee, alts, *span),

            CoreExpr::Con { tag, fields, span } => {
                let field_vars: Vec<IrVar> = fields.iter().map(|f| self.lower_expr(f)).collect();
                let dest = self.ctx.alloc_var();
                let ir_expr = match tag {
                    CoreTag::None => IrExpr::None,
                    CoreTag::Some => IrExpr::Some(*field_vars.first().expect("Some needs 1 field")),
                    CoreTag::Left => IrExpr::Left(*field_vars.first().expect("Left needs 1 field")),
                    CoreTag::Right => {
                        IrExpr::Right(*field_vars.first().expect("Right needs 1 field"))
                    }
                    CoreTag::Nil => IrExpr::EmptyList,
                    CoreTag::Cons => IrExpr::Cons {
                        head: field_vars[0],
                        tail: field_vars[1],
                    },
                    CoreTag::Named(name) => IrExpr::MakeAdt(*name, field_vars),
                };
                self.emit(IrInstr::Assign {
                    dest,
                    expr: ir_expr,
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::PrimOp { op, args, span } => self.lower_primop(op, args, *span),

            CoreExpr::Perform {
                effect,
                operation,
                args,
                span,
            } => {
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Perform {
                        effect: *effect,
                        operation: *operation,
                        args: arg_vars,
                    },
                    metadata: IrMetadata::from_span(*span),
                });
                dest
            }

            CoreExpr::Handle {
                body,
                effect,
                handlers,
                span,
            } => {
                // Compile each handler arm as a separate closure function.
                let mut scope_arms = Vec::new();
                for h in handlers {
                    let fn_id = self.lower_handler_arm(h);
                    scope_arms.push(crate::cfg::HandleScopeArm {
                        operation_name: h.operation,
                        function_id: fn_id,
                    });
                }

                // Create a body block, lower the body into it, then jump to
                // a continuation block carrying the result.
                let body_block_idx = self.new_block();
                let body_block_id = self.blocks[body_block_idx].id;

                let cont_block_idx = self.new_block();
                let cont_block_id = self.blocks[cont_block_idx].id;
                let dest = self.ctx.alloc_var();
                self.blocks[cont_block_idx].params.push(IrBlockParam {
                    var: dest,
                    ty: IrType::Any,
                });

                // Emit HandleScope in the current block, then terminate with
                // a jump into the body entry block.
                let meta = IrMetadata::from_span(*span);
                self.emit(IrInstr::HandleScope {
                    effect: *effect,
                    arms: scope_arms,
                    body_entry: body_block_id,
                    body_result: dest, // will be set below via jump arg
                    dest,
                    metadata: meta.clone(),
                });
                self.set_terminator(IrTerminator::Jump(body_block_id, Vec::new(), meta.clone()));

                // Switch to body block, lower the body, jump to continuation.
                self.current_block = body_block_idx;
                let body_var = self.lower_expr(body);
                self.set_terminator(IrTerminator::Jump(cont_block_id, vec![body_var], meta));

                // Continue in the continuation block.
                self.current_block = cont_block_idx;
                dest
            }
        }
    }

    /// Lower a handler arm as a separate closure function.
    /// Parameters: [resume, param0, param1, …] — matches the VM calling convention.
    fn lower_handler_arm(&mut self, handler: &CoreHandler) -> FunctionId {
        // Collect free variables in the arm body that are bound in the enclosing scope.
        let mut free = HashSet::new();
        free_vars_rec(&handler.body, &mut HashSet::new(), &mut free);
        // Remove the arm's own parameters from the free set.
        free.remove(&handler.resume);
        for p in &handler.params {
            free.remove(p);
        }
        let mut captures: Vec<Identifier> = free
            .into_iter()
            .filter(|name| self.env.contains_key(name))
            .collect();
        captures.sort_by_key(|n| n.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(Identifier, IrVar)> = captures
            .iter()
            .filter_map(|n| self.env.get(n).map(|&v| (*n, v)))
            .collect();

        {
            let mut sub = FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: None,
                params: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                last_value: None,
            };

            // Captures first (matching VM convention for closures).
            for (handler_name, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(*handler_name, v);
                sub.params.push(IrParam {
                    name: *handler_name,
                    var: v,
                    ty: IrType::Any,
                });
            }
            // Resume param first, then operation params.
            let resume_var = sub.ctx.alloc_var();
            sub.env.insert(handler.resume, resume_var);
            sub.params.push(IrParam {
                name: handler.resume,
                var: resume_var,
                ty: IrType::Any,
            });
            for &p in &handler.params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p, v);
                sub.params.push(IrParam {
                    name: p,
                    var: v,
                    ty: IrType::Any,
                });
            }

            let ret = sub.lower_expr(&handler.body);
            sub.finish_return(ret, handler.span);
        }

        // Record captures on the generated function.
        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.clone();
        }

        fn_id
    }

    /// Lower a `Lam` node appearing inside an expression as a closure.
    fn lower_lam_as_closure(&mut self, expr: &CoreExpr) -> IrVar {
        let CoreExpr::Lam { params, body, span } = expr else {
            panic!("lower_lam_as_closure: not a Lam");
        };

        // Compute free variables that need to be captured.
        let free = collect_free_vars_core(expr);
        let mut captures: Vec<Identifier> = free
            .into_iter()
            .filter(|name| self.env.contains_key(name))
            .collect();
        captures.sort_by_key(|n| n.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(Identifier, IrVar)> = captures
            .iter()
            .filter_map(|n| self.env.get(n).map(|&v| (*n, v)))
            .collect();

        {
            let mut sub = FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: None,
                params: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                last_value: None,
            };

            // Captures are the first params (matching how the VM expects them).
            for (name, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(*name, v);
                sub.params.push(IrParam {
                    name: *name,
                    var: v,
                    ty: IrType::Any,
                });
            }
            for &p in params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p, v);
                sub.params.push(IrParam {
                    name: p,
                    var: v,
                    ty: IrType::Any,
                });
            }

            let ret = sub.lower_expr(body);
            sub.finish_return(ret, *span);
        }

        let capture_vars: Vec<IrVar> = capture_env
            .iter()
            .filter_map(|(n, _)| self.env.get(n).copied())
            .collect();

        let dest = self.ctx.alloc_var();
        self.emit(IrInstr::Assign {
            dest,
            expr: IrExpr::MakeClosure(fn_id, capture_vars),
            metadata: IrMetadata::from_span(*span),
        });
        dest
    }

    /// Lower a `PrimOp` node.
    fn lower_primop(&mut self, op: &CorePrimOp, args: &[CoreExpr], span: Span) -> IrVar {
        let dest = self.ctx.alloc_var();
        let meta = IrMetadata::from_span(span);
        match op {
            CorePrimOp::Add
            | CorePrimOp::Sub
            | CorePrimOp::Mul
            | CorePrimOp::Div
            | CorePrimOp::Mod
            | CorePrimOp::IAdd
            | CorePrimOp::ISub
            | CorePrimOp::IMul
            | CorePrimOp::IDiv
            | CorePrimOp::IMod
            | CorePrimOp::FAdd
            | CorePrimOp::FSub
            | CorePrimOp::FMul
            | CorePrimOp::FDiv
            | CorePrimOp::Eq
            | CorePrimOp::NEq
            | CorePrimOp::Lt
            | CorePrimOp::Le
            | CorePrimOp::Gt
            | CorePrimOp::Ge
            | CorePrimOp::And
            | CorePrimOp::Or
            | CorePrimOp::Concat => {
                let lv = self.lower_expr(&args[0]);
                let rv = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Binary(primop_to_binop(op), lv, rv),
                    metadata: meta,
                });
            }
            CorePrimOp::Neg => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "-".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Not => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "!".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Interpolate => {
                let parts: Vec<IrStringPart> = args
                    .iter()
                    .map(|a| IrStringPart::Interpolation(self.lower_expr(a)))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::InterpolatedString(parts),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeList => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeList(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeArray => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeArray(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeTuple => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeTuple(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeHash => {
                let pairs: Vec<(IrVar, IrVar)> = args
                    .chunks(2)
                    .map(|chunk| (self.lower_expr(&chunk[0]), self.lower_expr(&chunk[1])))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeHash(pairs),
                    metadata: meta,
                });
            }
            CorePrimOp::Index => {
                let left = self.lower_expr(&args[0]);
                let index = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Index { left, index },
                    metadata: meta,
                });
            }
            CorePrimOp::MemberAccess(member) => {
                let module_name = match &args[0] {
                    CoreExpr::Var(name, _) => Some(*name),
                    _ => None,
                };
                let object = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MemberAccess {
                        object,
                        member: *member,
                        module_name,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::TupleField(idx) => {
                let object = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::TupleFieldAccess {
                        object,
                        index: *idx,
                    },
                    metadata: meta,
                });
            }
        }
        dest
    }

    /// Lower a `Case` expression into branching IR blocks.
    fn lower_case(&mut self, scrutinee: &CoreExpr, alts: &[CoreAlt], span: Span) -> IrVar {
        let scrut_var = self.lower_expr(scrutinee);

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
                // On false guard, jump to an unreachable block (guard failure is UB here).
                let fail_idx = self.new_block();
                let fail_block_id = self.blocks[fail_idx].id;
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
            self.set_terminator(IrTerminator::Jump(
                join_block_id,
                vec![arm_result],
                IrMetadata::from_span(span),
            ));

            // Switch to the fail block so the next alt starts from it.
            if let Some(idx) = next_block_idx {
                self.current_block = idx;
            }
        }

        self.current_block = join_idx;
        result_var
    }

    /// Emit instructions that test whether `var` matches `pat`.
    /// Returns an `IrVar` holding a bool.
    fn emit_pattern_test(&mut self, var: IrVar, pat: &CorePat) -> IrVar {
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
            CorePat::Con { tag, .. } => {
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
                dest
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
                dest
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
    fn bind_pattern(&mut self, var: IrVar, pat: &CorePat) {
        match pat {
            CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
            CorePat::Var(name) => {
                self.env.insert(*name, var);
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

// ── Pattern helpers ───────────────────────────────────────────────────────────

fn is_irrefutable(pat: &CorePat) -> bool {
    matches!(pat, CorePat::Wildcard | CorePat::Var(_))
}

// ── Lit / op helpers ──────────────────────────────────────────────────────────

fn lower_lit(lit: &CoreLit) -> IrConst {
    match lit {
        CoreLit::Int(n) => IrConst::Int(*n),
        CoreLit::Float(f) => IrConst::Float(*f),
        CoreLit::Bool(b) => IrConst::Bool(*b),
        CoreLit::String(s) => IrConst::String(s.clone()),
        CoreLit::Unit => IrConst::Unit,
    }
}

fn primop_to_binop(op: &CorePrimOp) -> IrBinaryOp {
    match op {
        // Generic arithmetic
        CorePrimOp::Add | CorePrimOp::Concat => IrBinaryOp::Add,
        CorePrimOp::Sub => IrBinaryOp::Sub,
        CorePrimOp::Mul => IrBinaryOp::Mul,
        CorePrimOp::Div => IrBinaryOp::Div,
        CorePrimOp::Mod => IrBinaryOp::Mod,
        // Typed integer arithmetic — skip the runtime type-dispatch path
        CorePrimOp::IAdd => IrBinaryOp::IAdd,
        CorePrimOp::ISub => IrBinaryOp::ISub,
        CorePrimOp::IMul => IrBinaryOp::IMul,
        CorePrimOp::IDiv => IrBinaryOp::IDiv,
        CorePrimOp::IMod => IrBinaryOp::IMod,
        // Typed float arithmetic
        CorePrimOp::FAdd => IrBinaryOp::FAdd,
        CorePrimOp::FSub => IrBinaryOp::FSub,
        CorePrimOp::FMul => IrBinaryOp::FMul,
        CorePrimOp::FDiv => IrBinaryOp::FDiv,
        // Comparisons and logical
        CorePrimOp::Eq => IrBinaryOp::Eq,
        CorePrimOp::NEq => IrBinaryOp::NotEq,
        CorePrimOp::Lt => IrBinaryOp::Lt,
        CorePrimOp::Le => IrBinaryOp::Le,
        CorePrimOp::Gt => IrBinaryOp::Gt,
        CorePrimOp::Ge => IrBinaryOp::Ge,
        CorePrimOp::And => IrBinaryOp::And,
        CorePrimOp::Or => IrBinaryOp::Or,
        _ => unreachable!("not a binary op: {:?}", op),
    }
}

// ── Free variable analysis ────────────────────────────────────────────────────

/// Collect all free (unbound) variables in a `CoreExpr`.
pub fn collect_free_vars_core(expr: &CoreExpr) -> HashSet<Identifier> {
    let mut free = HashSet::new();
    free_vars_rec(expr, &mut HashSet::new(), &mut free);
    free
}

fn free_vars_rec(expr: &CoreExpr, bound: &mut HashSet<Identifier>, free: &mut HashSet<Identifier>) {
    match expr {
        CoreExpr::Var(name, _) => {
            if !bound.contains(name) {
                free.insert(*name);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(**p))
                .copied()
                .collect();
            free_vars_rec(body, bound, free);
            for p in new_params {
                bound.remove(&p);
            }
        }
        CoreExpr::App { func, args, .. } => {
            free_vars_rec(func, bound, free);
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            free_vars_rec(rhs, bound, free);
            let is_new = bound.insert(*var);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(var);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let is_new = bound.insert(*var);
            free_vars_rec(rhs, bound, free);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(var);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            free_vars_rec(scrutinee, bound, free);
            for alt in alts {
                let mut alt_bound = HashSet::new();
                collect_pat_binders(&alt.pat, &mut alt_bound);
                let new_binders: Vec<_> = alt_bound
                    .iter()
                    .filter(|b| bound.insert(**b))
                    .copied()
                    .collect();
                if let Some(guard) = &alt.guard {
                    free_vars_rec(guard, bound, free);
                }
                free_vars_rec(&alt.rhs, bound, free);
                for b in new_binders {
                    bound.remove(&b);
                }
            }
        }
        CoreExpr::Con { fields, .. } => {
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::PrimOp { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Perform { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            free_vars_rec(body, bound, free);
            for h in handlers {
                let mut new_binders = Vec::new();
                if bound.insert(h.resume) {
                    new_binders.push(h.resume);
                }
                for &p in &h.params {
                    if bound.insert(p) {
                        new_binders.push(p);
                    }
                }
                free_vars_rec(&h.body, bound, free);
                for b in new_binders {
                    bound.remove(&b);
                }
            }
        }
    }
}

fn collect_pat_binders(pat: &CorePat, out: &mut HashSet<Identifier>) {
    match pat {
        CorePat::Var(name) => {
            out.insert(*name);
        }
        CorePat::Con { fields, .. } => {
            for f in fields {
                collect_pat_binders(f, out);
            }
        }
        CorePat::Tuple(fields) => {
            for f in fields {
                collect_pat_binders(f, out);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::position::Span;
    use crate::nary::{CoreDef, CoreExpr, CoreLit, CorePrimOp, CoreProgram};
    use crate::syntax::interner::Interner;

    fn make_interner() -> Interner {
        Interner::new()
    }

    #[test]
    fn lower_empty_program() {
        let prog = CoreProgram { defs: Vec::new() };
        let ir = lower_core_to_ir(&prog);
        // Entry function is always emitted.
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn lower_constant_def() {
        let mut interner = make_interner();
        let name = interner.intern("answer");
        let prog = CoreProgram {
            defs: vec![CoreDef {
                name,
                expr: CoreExpr::Lit(CoreLit::Int(42), Span::default()),
                is_recursive: false,
                span: Span::default(),
            }],
        };
        let ir = lower_core_to_ir(&prog);
        assert!(!ir.functions.is_empty());
        assert!(ir.globals.contains(&name));
    }

    #[test]
    fn lower_identity_function() {
        let mut interner = make_interner();
        let f_name = interner.intern("id");
        let x_name = interner.intern("x");
        let prog = CoreProgram {
            defs: vec![CoreDef {
                name: f_name,
                expr: CoreExpr::Lam {
                    params: vec![x_name],
                    body: Box::new(CoreExpr::Var(x_name, Span::default())),
                    span: Span::default(),
                },
                is_recursive: false,
                span: Span::default(),
            }],
        };
        let ir = lower_core_to_ir(&prog);
        // Should produce: entry function + the id function body.
        assert!(ir.functions.len() >= 2);
        assert!(ir.globals.contains(&f_name));
    }

    #[test]
    fn lower_addition() {
        let mut interner = make_interner();
        let add_name = interner.intern("add");
        let a = interner.intern("a");
        let b = interner.intern("b");
        let prog = CoreProgram {
            defs: vec![CoreDef {
                name: add_name,
                expr: CoreExpr::Lam {
                    params: vec![a, b],
                    body: Box::new(CoreExpr::PrimOp {
                        op: CorePrimOp::Add,
                        args: vec![
                            CoreExpr::Var(a, Span::default()),
                            CoreExpr::Var(b, Span::default()),
                        ],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                is_recursive: false,
                span: Span::default(),
            }],
        };
        let ir = lower_core_to_ir(&prog);
        // add(a, b) = a + b: entry + the add function.
        assert!(ir.functions.len() >= 2);
        // The add function should have 2 params (a and b, uncurried).
        let add_fn = ir.functions.iter().find(|f| f.params.len() == 2).unwrap();
        assert_eq!(add_fn.params[0].name, a);
        assert_eq!(add_fn.params[1].name, b);
    }
}
