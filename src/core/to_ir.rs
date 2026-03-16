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
    backend_ir::{
        BlockId, FunctionId, IrBinaryOp, IrBlock, IrBlockParam, IrCallTarget, IrConst, IrExpr,
        IrFunction, IrFunctionOrigin, IrInstr, IrListTest, IrMetadata, IrParam, IrProgram,
        IrStringPart, IrTagTest, IrTerminator, IrTopLevelItem, IrType, IrVar,
    },
    core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp,
        CoreProgram, CoreTag, CoreTopLevelItem,
    },
    diagnostics::position::Span,
    syntax::{Identifier, effect_expr::EffectExpr, type_expr::TypeExpr},
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
    global_bindings: Vec<crate::backend_ir::IrGlobalBinding>,
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
            global_bindings: Vec::new(),
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
        self.top_level_items = core
            .top_level_items
            .iter()
            .map(lower_core_top_level_item)
            .collect();

        // Create the module-level entry function.
        let entry_id = self.alloc_function();
        let entry_block = self.alloc_block();
        let mut entry_fn = FnCtx::new(
            self,
            entry_id,
            IrFunctionOrigin::ModuleTopLevel,
            entry_block,
            Vec::new(),
            None,
            Vec::new(),
        );

        // Seed top-level named functions into the entry environment so later
        // top-level values and named function bodies can resolve Core binders
        // for globals/functions through ordinary lexical lookup.
        for def in &core.defs {
            if matches!(def.expr, CoreExpr::Lam { .. }) {
                let f_var = entry_fn.ctx.alloc_var();
                entry_fn.emit(IrInstr::Assign {
                    dest: f_var,
                    expr: IrExpr::LoadName(def.name),
                    metadata: IrMetadata::from_span(def.span),
                });
                entry_fn.env.insert(def.binder.id, f_var);
                entry_fn.binder_names.insert(def.binder.id, def.binder.name);
            }
        }

        for def in &core.defs {
            if let CoreExpr::Lam { params, body, .. } = &def.expr {
                // This def is a function — emit as a named IrFunction.
                let fn_id = entry_fn.ctx.alloc_function();
                let fn_block = entry_fn.ctx.alloc_block();
                let (parameter_types, return_type_annotation, effects) =
                    find_function_decl_metadata(&entry_fn.ctx.top_level_items, def.name)
                        .unwrap_or_else(|| (Vec::new(), None, Vec::new()));
                {
                    let mut fn_ctx = FnCtx::new(
                        entry_fn.ctx,
                        fn_id,
                        IrFunctionOrigin::NamedFunction,
                        fn_block,
                        parameter_types.clone(),
                        return_type_annotation.clone(),
                        effects.clone(),
                    );
                    fn_ctx.name = Some(def.name);
                    for (&binder_id, &binder_name) in &entry_fn.binder_names {
                        let v = fn_ctx.ctx.alloc_var();
                        fn_ctx.emit(IrInstr::Assign {
                            dest: v,
                            expr: IrExpr::LoadName(binder_name),
                            metadata: IrMetadata::from_span(def.span),
                        });
                        fn_ctx.env.insert(binder_id, v);
                        fn_ctx.binder_names.insert(binder_id, binder_name);
                    }
                    for &p in params {
                        let v = fn_ctx.ctx.alloc_var();
                        fn_ctx.env.insert(p.id, v);
                        fn_ctx.binder_names.insert(p.id, p.name);
                        fn_ctx.params.push(IrParam {
                            name: p.name,
                            var: v,
                            ty: IrType::Any,
                        });
                    }
                    let ret = fn_ctx.lower_expr(body);
                    fn_ctx.finish_return(ret, def.span);
                }
                entry_fn.ctx.globals.push(def.name);
                if !bind_function_id_in_items(&mut entry_fn.ctx.top_level_items, def.name, fn_id) {
                    entry_fn.ctx.top_level_items.push(IrTopLevelItem::Function {
                        is_public: false,
                        name: def.name,
                        type_params: Vec::new(),
                        function_id: Some(fn_id),
                        parameters: params.iter().map(|p| p.name).collect(),
                        parameter_types,
                        return_type: return_type_annotation,
                        effects,
                        body: crate::syntax::block::Block {
                            statements: Vec::new(),
                            span: def.span,
                        },
                        span: def.span,
                    });
                }
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
                entry_fn
                    .ctx
                    .global_bindings
                    .push(crate::backend_ir::IrGlobalBinding {
                        name: def.name,
                        var: g_var,
                    });
                entry_fn.env.insert(def.binder.id, g_var);
                entry_fn.binder_names.insert(def.binder.id, def.binder.name);
                if def.is_anonymous() {
                    // Anonymous top-level expression statements still
                    // contribute the program result when they are the last
                    // evaluated definition, matching the VM's top-level
                    // expression semantics.
                    entry_fn.last_value = Some(val);
                }
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
            global_bindings: self.global_bindings,
            hm_expr_types: HashMap::new(),
            core: None, // already consumed
        }
    }
}

fn lower_core_top_level_item(item: &CoreTopLevelItem) -> IrTopLevelItem {
    match item {
        CoreTopLevelItem::Function {
            is_public,
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            span,
        } => IrTopLevelItem::Function {
            is_public: *is_public,
            name: *name,
            type_params: type_params.clone(),
            function_id: None,
            parameters: parameters.clone(),
            parameter_types: parameter_types.clone(),
            return_type: return_type.clone(),
            effects: effects.clone(),
            body: crate::syntax::block::Block {
                statements: Vec::new(),
                span: *span,
            },
            span: *span,
        },
        CoreTopLevelItem::Module { name, body, span } => IrTopLevelItem::Module {
            name: *name,
            body: body.iter().map(lower_core_top_level_item).collect(),
            span: *span,
        },
        CoreTopLevelItem::Import {
            name,
            alias,
            except,
            span,
        } => IrTopLevelItem::Import {
            name: *name,
            alias: *alias,
            except: except.clone(),
            span: *span,
        },
        CoreTopLevelItem::Data {
            name,
            type_params,
            variants,
            span,
        } => IrTopLevelItem::Data {
            name: *name,
            type_params: type_params.clone(),
            variants: variants.clone(),
            span: *span,
        },
        CoreTopLevelItem::EffectDecl { name, ops, span } => IrTopLevelItem::EffectDecl {
            name: *name,
            ops: ops.clone(),
            span: *span,
        },
    }
}

fn bind_function_id_in_items(
    items: &mut [IrTopLevelItem],
    name: Identifier,
    function_id: FunctionId,
) -> bool {
    for item in items {
        match item {
            IrTopLevelItem::Function {
                name: item_name,
                function_id: item_function_id,
                ..
            } if *item_name == name && item_function_id.is_none() => {
                *item_function_id = Some(function_id);
                return true;
            }
            IrTopLevelItem::Module { body, .. } => {
                if bind_function_id_in_items(body, name, function_id) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

type FunctionDeclarationMetadata = (Vec<Option<TypeExpr>>, Option<TypeExpr>, Vec<EffectExpr>);

fn find_function_decl_metadata(
    items: &[IrTopLevelItem],
    name: Identifier,
) -> Option<FunctionDeclarationMetadata> {
    for item in items {
        match item {
            IrTopLevelItem::Function {
                name: item_name,
                parameter_types,
                return_type,
                effects,
                ..
            } if *item_name == name => {
                return Some((
                    parameter_types.clone(),
                    return_type.clone(),
                    effects.clone(),
                ));
            }
            IrTopLevelItem::Module { body, .. } => {
                if let Some(found) = find_function_decl_metadata(body, name) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

// ── Per-function context ───────────────────────────────────────────────────────

struct FnCtx<'a> {
    ctx: &'a mut ToIrCtx,
    id: FunctionId,
    origin: IrFunctionOrigin,
    name: Option<Identifier>,
    params: Vec<IrParam>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type_annotation: Option<TypeExpr>,
    effects: Vec<EffectExpr>,
    blocks: Vec<IrBlock>,
    current_block: usize,
    env: HashMap<CoreBinderId, IrVar>,
    binder_names: HashMap<CoreBinderId, Identifier>,
    last_value: Option<IrVar>,
}

impl<'a> FnCtx<'a> {
    fn new(
        ctx: &'a mut ToIrCtx,
        id: FunctionId,
        origin: IrFunctionOrigin,
        entry: BlockId,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type_annotation: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
    ) -> Self {
        Self {
            ctx,
            id,
            name: None,
            origin,
            params: Vec::new(),
            parameter_types,
            return_type_annotation,
            effects,
            blocks: vec![IrBlock {
                id: entry,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            }],
            current_block: 0,
            env: HashMap::new(),
            binder_names: HashMap::new(),
            last_value: None,
        }
    }

    fn emit(&mut self, instr: IrInstr) {
        self.blocks[self.current_block].instrs.push(instr);
    }

    fn bound_var(&self, binder: CoreBinderId, name: Identifier) -> IrVar {
        *self.env.get(&binder).unwrap_or_else(|| {
            panic!(
                "Core binder resolution invariant failed during Core→IR lowering: missing env entry for {}#{}",
                name.as_u32(),
                binder.0
            )
        })
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

    fn with_bound_var<T>(
        &mut self,
        binder_id: CoreBinderId,
        name: Identifier,
        ir_var: IrVar,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let old_var = self.env.insert(binder_id, ir_var);
        let old_name = self.binder_names.insert(binder_id, name);
        let result = f(self);
        match old_var {
            Some(var) => {
                self.env.insert(binder_id, var);
            }
            None => {
                self.env.remove(&binder_id);
            }
        }
        match old_name {
            Some(existing_name) => {
                self.binder_names.insert(binder_id, existing_name);
            }
            None => {
                self.binder_names.remove(&binder_id);
            }
        }
        result
    }

    fn set_terminator(&mut self, t: IrTerminator) {
        self.blocks[self.current_block].terminator = t;
    }

    fn current_block_is_open(&self) -> bool {
        matches!(
            self.blocks[self.current_block].terminator,
            IrTerminator::Unreachable(_)
        )
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
            parameter_types: s.parameter_types,
            return_type_annotation: s.return_type_annotation,
            effects: s.effects,
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
            CoreExpr::Var { var, span } => {
                if let Some(binder) = var.binder {
                    return self.bound_var(binder, var.name);
                }
                let dest = self.ctx.alloc_var();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::LoadName(var.name),
                    metadata: IrMetadata::from_span(*span),
                });
                dest
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

            CoreExpr::Lam { .. } => self.lower_lam_as_closure(None, None, expr),

            CoreExpr::App { func, args, span } => {
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                let dest = self.ctx.alloc_var();
                let meta = IrMetadata::from_span(*span);

                match func.as_ref() {
                    CoreExpr::Var { var, .. } => {
                        if let Some(binder) = var.binder {
                            let fv = self.bound_var(binder, var.name);
                            self.emit(IrInstr::Call {
                                dest,
                                target: IrCallTarget::Var(fv),
                                args: arg_vars,
                                metadata: meta,
                            });
                            return dest;
                        }
                        self.emit(IrInstr::Call {
                            dest,
                            target: IrCallTarget::Named(var.name),
                            args: arg_vars,
                            metadata: meta,
                        });
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
                self.with_bound_var(var.id, var.name, rhs_var, |this| this.lower_expr(body))
            }

            CoreExpr::LetRec { var, rhs, body, .. } => {
                let rhs_var = match rhs.as_ref() {
                    CoreExpr::Lam { .. } => {
                        let placeholder = self.ctx.alloc_var();
                        self.emit(IrInstr::Assign {
                            dest: placeholder,
                            expr: IrExpr::LoadName(var.name),
                            metadata: IrMetadata::empty(),
                        });
                        self.with_bound_var(var.id, var.name, placeholder, |this| {
                            this.lower_lam_as_closure(Some(var.name), Some(var.id), rhs.as_ref())
                        })
                    }
                    _ => self.lower_expr(rhs),
                };
                self.with_bound_var(var.id, var.name, rhs_var, |this| this.lower_expr(body))
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

            CoreExpr::Return { value, span } => {
                let ret = self.lower_expr(value);
                if self.current_block_is_open() {
                    self.set_terminator(IrTerminator::Return(ret, IrMetadata::from_span(*span)));
                }
                ret
            }

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
                    let (fn_id, capture_vars) = self.lower_handler_arm(h);
                    scope_arms.push(crate::backend_ir::HandleScopeArm {
                        operation_name: h.operation,
                        function_id: fn_id,
                        capture_vars,
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
    fn lower_handler_arm(&mut self, handler: &CoreHandler) -> (FunctionId, Vec<IrVar>) {
        // Collect free variables in the arm body that are bound in the enclosing scope.
        let mut free = HashSet::new();
        free_vars_rec(&handler.body, &mut HashSet::new(), &mut free);
        // Remove the arm's own parameters from the free set.
        free.remove(&handler.resume.id);
        for p in &handler.params {
            free.remove(&p.id);
        }
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter_map(|binder| {
                self.env.get(&binder).map(|_| CoreBinder {
                    id: binder,
                    name: self.binder_names[&binder],
                })
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: None,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
            };

            // Captures first (matching VM convention for closures).
            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Any,
                });
            }
            // Resume param first, then operation params.
            let resume_var = sub.ctx.alloc_var();
            sub.env.insert(handler.resume.id, resume_var);
            sub.binder_names
                .insert(handler.resume.id, handler.resume.name);
            sub.params.push(IrParam {
                name: handler.resume.name,
                var: resume_var,
                ty: IrType::Any,
            });
            for &p in &handler.params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: IrType::Any,
                });
            }

            let ret = sub.lower_expr(&handler.body);
            sub.finish_return(ret, handler.span);
        }

        // Record captures on the generated function.
        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        (fn_id, capture_env.into_iter().map(|(_, var)| var).collect())
    }

    /// Lower a `Lam` node appearing inside an expression as a closure.
    fn lower_lam_as_closure(
        &mut self,
        forced_name: Option<Identifier>,
        recursive_binder: Option<CoreBinderId>,
        expr: &CoreExpr,
    ) -> IrVar {
        let CoreExpr::Lam { params, body, span } = expr else {
            panic!("lower_lam_as_closure: not a Lam");
        };

        // Compute free variables that need to be captured.
        let free = collect_free_vars_core(expr);
        let mut captures: Vec<CoreBinder> = free
            .into_iter()
            .filter(|binder| Some(*binder) != recursive_binder)
            .filter_map(|binder| {
                self.env.get(&binder).map(|_| CoreBinder {
                    id: binder,
                    name: self.binder_names[&binder],
                })
            })
            .collect();
        captures.sort_by_key(|b| b.name.as_u32());

        let fn_id = self.ctx.alloc_function();
        let fn_block = self.ctx.alloc_block();

        let capture_env: Vec<(CoreBinder, IrVar)> = captures
            .iter()
            .filter_map(|b| self.env.get(&b.id).map(|&v| (*b, v)))
            .collect();

        {
            let mut sub = FnCtx {
                ctx: self.ctx,
                id: fn_id,
                origin: IrFunctionOrigin::FunctionLiteral,
                name: forced_name,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                blocks: vec![IrBlock {
                    id: fn_block,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                }],
                current_block: 0,
                env: HashMap::new(),
                binder_names: HashMap::new(),
                last_value: None,
            };

            // Captures are the first params (matching how the VM expects them).
            for (binder, _) in &capture_env {
                let v = sub.ctx.alloc_var();
                sub.env.insert(binder.id, v);
                sub.binder_names.insert(binder.id, binder.name);
                sub.params.push(IrParam {
                    name: binder.name,
                    var: v,
                    ty: IrType::Any,
                });
            }
            if let (Some(name), Some(binder_id)) = (forced_name, recursive_binder) {
                let self_capture_vars: Vec<IrVar> = captures
                    .iter()
                    .filter_map(|binder| sub.env.get(&binder.id).copied())
                    .collect();
                let self_var = sub.ctx.alloc_var();
                sub.emit(IrInstr::Assign {
                    dest: self_var,
                    expr: IrExpr::MakeClosure(fn_id, self_capture_vars),
                    metadata: IrMetadata::from_span(*span),
                });
                sub.env.insert(binder_id, self_var);
                sub.binder_names.insert(binder_id, name);
            }
            for &p in params {
                let v = sub.ctx.alloc_var();
                sub.env.insert(p.id, v);
                sub.binder_names.insert(p.id, p.name);
                sub.params.push(IrParam {
                    name: p.name,
                    var: v,
                    ty: IrType::Any,
                });
            }

            let ret = sub.lower_expr(body);
            sub.finish_return(ret, *span);
        }

        if let Some(func) = self.ctx.functions.iter_mut().find(|f| f.id == fn_id) {
            func.captures = captures.iter().map(|b| b.name).collect();
        }

        let capture_vars: Vec<IrVar> = capture_env
            .iter()
            .filter_map(|(b, _)| self.env.get(&b.id).copied())
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
                    CoreExpr::Var { var, .. } => Some(var.name),
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
    fn bind_pattern(&mut self, var: IrVar, pat: &CorePat) {
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
pub fn collect_free_vars_core(expr: &CoreExpr) -> HashSet<CoreBinderId> {
    let mut free = HashSet::new();
    free_vars_rec(expr, &mut HashSet::new(), &mut free);
    free
}

fn free_vars_rec(
    expr: &CoreExpr,
    bound: &mut HashSet<CoreBinderId>,
    free: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(binder) = var.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .copied()
                .collect();
            free_vars_rec(body, bound, free);
            for p in new_params {
                bound.remove(&p.id);
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
            let is_new = bound.insert(var.id);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let is_new = bound.insert(var.id);
            free_vars_rec(rhs, bound, free);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(&var.id);
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
        CoreExpr::Return { value, .. } => free_vars_rec(value, bound, free),
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
                if bound.insert(h.resume.id) {
                    new_binders.push(h.resume.id);
                }
                for p in &h.params {
                    if bound.insert(p.id) {
                        new_binders.push(p.id);
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

fn collect_pat_binders(pat: &CorePat, out: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            out.insert(binder.id);
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
    use crate::core::{CoreBinder, CoreDef, CoreExpr, CoreLit, CorePrimOp, CoreProgram};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    fn make_interner() -> Interner {
        Interner::new()
    }

    fn binder(raw: u32, name: Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), name)
    }

    fn var_expr(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(binder, Span::default())
    }

    #[test]
    fn lower_empty_program() {
        let prog = CoreProgram {
            defs: Vec::new(),
            top_level_items: Vec::new(),
        };
        let ir = lower_core_to_ir(&prog);
        // Entry function is always emitted.
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn lower_constant_def() {
        let mut interner = make_interner();
        let name = interner.intern("answer");
        let binder = binder(0, name);
        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                binder,
                CoreExpr::Lit(CoreLit::Int(42), Span::default()),
                false,
                Span::default(),
            )],
            top_level_items: Vec::new(),
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
        let f_binder = binder(0, f_name);
        let x_binder = binder(1, x_name);
        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                f_binder,
                CoreExpr::Lam {
                    params: vec![x_binder],
                    body: Box::new(var_expr(x_binder)),
                    span: Span::default(),
                },
                false,
                Span::default(),
            )],
            top_level_items: Vec::new(),
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
        let add_binder = binder(0, add_name);
        let a_binder = binder(1, a);
        let b_binder = binder(2, b);
        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                add_binder,
                CoreExpr::Lam {
                    params: vec![a_binder, b_binder],
                    body: Box::new(CoreExpr::PrimOp {
                        op: CorePrimOp::Add,
                        args: vec![var_expr(a_binder), var_expr(b_binder)],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                false,
                Span::default(),
            )],
            top_level_items: Vec::new(),
        };
        let ir = lower_core_to_ir(&prog);
        // add(a, b) = a + b: entry + the add function.
        assert!(ir.functions.len() >= 2);
        // The add function should have 2 params (a and b, uncurried).
        let add_fn = ir.functions.iter().find(|f| f.params.len() == 2).unwrap();
        assert_eq!(add_fn.params[0].name, a);
        assert_eq!(add_fn.params[1].name, b);
    }

    #[test]
    #[should_panic(expected = "Core binder resolution invariant failed during Core→IR lowering")]
    fn lower_panics_on_missing_bound_binder() {
        let mut interner = make_interner();
        let name = interner.intern("bad");
        let bogus = binder(99, interner.intern("x"));
        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                binder(0, name),
                CoreExpr::bound_var(bogus, Span::default()),
                false,
                Span::default(),
            )],
            top_level_items: Vec::new(),
        };
        let _ = lower_core_to_ir(&prog);
    }

    #[test]
    fn lower_preserves_core_owned_declaration_items() {
        let mut interner = make_interner();
        let module_name = interner.intern("Demo");
        let function_name = interner.intern("value");
        let data_name = interner.intern("MaybeInt");
        let ctor_some = interner.intern("SomeInt");
        let ctor_none = interner.intern("NoneInt");
        let effect_name = interner.intern("Console");
        let print_name = interner.intern("print");
        let string_name = interner.intern("String");
        let unit_name = interner.intern("Unit");

        let core = CoreProgram {
            defs: Vec::new(),
            top_level_items: vec![
                crate::core::CoreTopLevelItem::Module {
                    name: module_name,
                    body: vec![crate::core::CoreTopLevelItem::Function {
                        is_public: false,
                        name: function_name,
                        type_params: Vec::new(),
                        parameters: Vec::new(),
                        parameter_types: Vec::new(),
                        return_type: None,
                        effects: Vec::new(),
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
                crate::core::CoreTopLevelItem::Data {
                    name: data_name,
                    type_params: Vec::new(),
                    variants: vec![
                        crate::syntax::data_variant::DataVariant {
                            name: ctor_some,
                            fields: vec![crate::syntax::type_expr::TypeExpr::Named {
                                name: string_name,
                                args: Vec::new(),
                                span: Span::default(),
                            }],
                            span: Span::default(),
                        },
                        crate::syntax::data_variant::DataVariant {
                            name: ctor_none,
                            fields: Vec::new(),
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                },
                crate::core::CoreTopLevelItem::EffectDecl {
                    name: effect_name,
                    ops: vec![crate::syntax::effect_ops::EffectOp {
                        name: print_name,
                        type_expr: crate::syntax::type_expr::TypeExpr::Function {
                            params: vec![crate::syntax::type_expr::TypeExpr::Named {
                                name: string_name,
                                args: Vec::new(),
                                span: Span::default(),
                            }],
                            ret: Box::new(crate::syntax::type_expr::TypeExpr::Named {
                                name: unit_name,
                                args: Vec::new(),
                                span: Span::default(),
                            }),
                            effects: Vec::new(),
                            span: Span::default(),
                        },
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
            ],
        };
        let ir = lower_core_to_ir(&core);

        assert!(matches!(
            ir.top_level_items.first(),
            Some(IrTopLevelItem::Module { .. })
        ));
        assert!(matches!(
            ir.top_level_items.get(1),
            Some(IrTopLevelItem::Data { .. })
        ));
        assert!(matches!(
            ir.top_level_items.get(2),
            Some(IrTopLevelItem::EffectDecl { .. })
        ));
    }
}
