/// Core IR → backend IR lowering.
///
/// Translates `CoreProgram`/`CoreExpr` into the `IrFunction`/`IrBlock`
/// representation consumed by the VM bytecode compiler and Cranelift JIT.
///
/// Key design decisions:
/// - **Uncurrying**: Top-level `Lam` chains become multi-param `IrFunction`s.
/// - **Closures**: `Lam` inside expressions → `IrExpr::MakeClosure` with only the used outer binders captured.
/// - **Case compilation**: Patterns become sequences of tag/literal tests + jumps.
use std::collections::HashMap;

use crate::{
    aether::{AetherExpr, AetherProgram},
    cfg::{
        FunctionId, IrConst, IrExpr, IrFunctionOrigin, IrInstr, IrMetadata, IrParam, IrProgram,
        IrTopLevelItem, IrType,
    },
    core::{CoreExpr, CoreTopLevelItem},
    diagnostics::position::Span,
    syntax::{Identifier, effect_expr::EffectExpr, type_expr::TypeExpr},
};

mod case;
mod closure;
pub(super) mod fn_ctx;
pub mod free_vars;
mod primop;

pub use free_vars::collect_free_vars_core;

/// Convert a `CoreType` to the backend IR `IrType`.
fn core_type_to_ir_type(ct: &crate::core::CoreType) -> IrType {
    IrType::from_core_type(ct)
}

use fn_ctx::FnCtx;

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a `CoreProgram` into an `IrProgram` ready for backend code generation.
pub fn lower_core_to_ir(core: &crate::core::CoreProgram) -> IrProgram {
    let mut ctx = ToIrCtx::new();
    ctx.lower_program(core);
    ctx.finish()
}

/// Lower a backend-only Aether program into CFG IR for RC backends.
pub fn lower_aether_to_ir(aether: &crate::aether::AetherProgram) -> IrProgram {
    let mut ctx = ToIrCtx::new();
    ctx.lower_aether_program(aether);
    ctx.finish()
}

// ── Global context ────────────────────────────────────────────────────────────

pub(super) struct ToIrCtx {
    pub(super) next_var_id: u32,
    pub(super) next_block_id: u32,
    pub(super) next_function_id: u32,
    pub(super) functions: Vec<crate::cfg::IrFunction>,
    pub(super) top_level_items: Vec<IrTopLevelItem>,
    pub(super) globals: Vec<Identifier>,
    pub(super) global_bindings: Vec<crate::cfg::IrGlobalBinding>,
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

    pub(super) fn alloc_var(&mut self) -> crate::cfg::IrVar {
        let id = self.next_var_id;
        self.next_var_id += 1;
        crate::cfg::IrVar(id)
    }

    pub(super) fn alloc_block(&mut self) -> crate::cfg::BlockId {
        let id = self.next_block_id;
        self.next_block_id += 1;
        crate::cfg::BlockId(id)
    }

    pub(super) fn alloc_function(&mut self) -> FunctionId {
        let id = self.next_function_id;
        self.next_function_id += 1;
        FunctionId(id)
    }

    fn lower_program(&mut self, core: &crate::core::CoreProgram) {
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
            if let CoreExpr::Lam {
                params,
                param_types,
                result_ty,
                body,
                ..
            } = &def.expr
            {
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
                    if !param_types.is_empty() {
                        fn_ctx.inferred_param_types = param_types.clone();
                    } else if let Some(crate::core::CoreType::Function(ref inferred_params, _)) =
                        def.result_ty
                    {
                        // `CoreDef::result_ty` for named functions is normally the
                        // function result, not the full function signature. Only
                        // reuse it for parameter metadata if it actually matches
                        // the runtime arity.
                        if inferred_params.len() == params.len() {
                            fn_ctx.inferred_param_types =
                                inferred_params.iter().map(|ty| Some(ty.clone())).collect();
                        }
                    }
                    if fn_ctx.inferred_param_types.len() != params.len() {
                        // Some top-level lambda-valued defs still carry a
                        // result-type-shaped `CoreDef::result_ty` rather than a
                        // full parameter list. Keep runtime lowering correct by
                        // dropping mismatched semantic param metadata instead of
                        // emitting invalid IR.
                        fn_ctx.inferred_param_types.clear();
                    }
                    if let Some(ret_ty) = result_ty.clone() {
                        fn_ctx.inferred_return_type = Some(ret_ty);
                    } else if let Some(crate::core::CoreType::Function(_, ref ret_ty)) =
                        def.result_ty
                    {
                        fn_ctx.inferred_return_type = Some((**ret_ty).clone());
                    } else {
                        fn_ctx.inferred_return_type = def.result_ty.clone();
                    }
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
                    for (i, p) in params.iter().enumerate() {
                        let v = fn_ctx.ctx.alloc_var();
                        fn_ctx.env.insert(p.id, v);
                        fn_ctx.binder_names.insert(p.id, p.name);
                        let ir_ty = fn_ctx
                            .inferred_param_types
                            .get(i)
                            .and_then(|t| t.as_ref())
                            .map(core_type_to_ir_type)
                            .unwrap_or(IrType::Tagged);
                        fn_ctx.params.push(IrParam {
                            name: p.name,
                            var: v,
                            ty: ir_ty,
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
                    .push(crate::cfg::IrGlobalBinding {
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

    fn lower_aether_program(&mut self, aether: &AetherProgram) {
        self.top_level_items = aether
            .top_level_items()
            .iter()
            .map(lower_core_top_level_item)
            .collect();

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

        for def in aether.defs() {
            if matches!(def.expr, AetherExpr::Lam { .. }) {
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

        for def in aether.defs() {
            if let AetherExpr::Lam { params, body, .. } = &def.expr {
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
                    if let Some(crate::core::CoreType::Function(ref param_tys, ref ret_ty)) =
                        def.result_ty
                    {
                        if param_tys.len() == params.len() {
                            fn_ctx.inferred_param_types =
                                param_tys.iter().map(|t| Some(t.clone())).collect();
                        }
                        fn_ctx.inferred_return_type = Some((**ret_ty).clone());
                    } else if let Some(ref ty) = def.result_ty {
                        fn_ctx.inferred_return_type = Some(ty.clone());
                    }
                    if fn_ctx.inferred_param_types.len() != params.len() {
                        fn_ctx.inferred_param_types.clear();
                    }
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
                    for (i, p) in params.iter().enumerate() {
                        let v = fn_ctx.ctx.alloc_var();
                        fn_ctx.env.insert(p.id, v);
                        fn_ctx.binder_names.insert(p.id, p.name);
                        let ir_ty = fn_ctx
                            .inferred_param_types
                            .get(i)
                            .and_then(|t| t.as_ref())
                            .map(core_type_to_ir_type)
                            .unwrap_or(IrType::Tagged);
                        fn_ctx.params.push(IrParam {
                            name: p.name,
                            var: v,
                            ty: ir_ty,
                        });
                    }
                    let ret = fn_ctx.lower_expr_aether(body);
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
                let val = entry_fn.lower_expr_aether(&def.expr);
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
                    .push(crate::cfg::IrGlobalBinding {
                        name: def.name,
                        var: g_var,
                    });
                entry_fn.env.insert(def.binder.id, g_var);
                entry_fn.binder_names.insert(def.binder.id, def.binder.name);
                if def.is_anonymous {
                    entry_fn.last_value = Some(val);
                }
            }
        }

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
            exposing,
            span,
        } => IrTopLevelItem::Import {
            name: *name,
            alias: *alias,
            except: except.clone(),
            exposing: exposing.clone(),
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
        CoreTopLevelItem::Class {
            name,
            type_params,
            superclasses,
            methods,
            span,
        } => IrTopLevelItem::Class {
            name: *name,
            type_params: type_params.clone(),
            superclasses: superclasses.clone(),
            methods: methods.clone(),
            span: *span,
        },
        CoreTopLevelItem::Instance {
            class_name,
            type_args,
            context,
            methods,
            span,
        } => IrTopLevelItem::Instance {
            class_name: *class_name,
            type_args: type_args.clone(),
            context: context.clone(),
            methods: methods.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aether::{AetherDef, AetherExpr, AetherProgram};
    use crate::cfg::{IrExpr, IrFunctionOrigin, IrInstr, IrTerminator};
    use crate::core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePrimOp, CoreProgram,
        CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;
    use std::borrow::Borrow;

    fn make_interner() -> Interner {
        Interner::new()
    }

    fn binder(raw: u32, name: Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), name)
    }

    fn var_expr<B: Borrow<CoreBinder>>(binder: B) -> CoreExpr {
        CoreExpr::bound_var(binder.borrow(), Span::default())
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
                    param_types: vec![],
                    result_ty: None,
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
                    param_types: vec![],
                    result_ty: None,
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
    fn lower_preserves_hash_and_never_ir_return_types() {
        let mut interner = make_interner();
        let hash_name = interner.intern("hashy");
        let abort_name = interner.intern("aborty");
        let hash_binder = binder(0, hash_name);
        let abort_binder = binder(1, abort_name);

        let mut hash_def = CoreDef::new(
            hash_binder,
            CoreExpr::Lam {
                params: Vec::new(),
                param_types: vec![],
                result_ty: None,
                body: Box::new(CoreExpr::PrimOp {
                    op: CorePrimOp::MakeHash,
                    args: Vec::new(),
                    span: Span::default(),
                }),
                span: Span::default(),
            },
            false,
            Span::default(),
        );
        hash_def.result_ty = Some(crate::core::CoreType::Function(
            Vec::new(),
            Box::new(crate::core::CoreType::Map(
                Box::new(crate::core::CoreType::String),
                Box::new(crate::core::CoreType::Int),
            )),
        ));

        let mut abort_def = CoreDef::new(
            abort_binder,
            CoreExpr::Lam {
                params: Vec::new(),
                param_types: vec![],
                result_ty: None,
                body: Box::new(CoreExpr::PrimOp {
                    op: CorePrimOp::Panic,
                    args: vec![CoreExpr::Lit(
                        CoreLit::String("boom".to_string()),
                        Span::default(),
                    )],
                    span: Span::default(),
                }),
                span: Span::default(),
            },
            false,
            Span::default(),
        );
        abort_def.result_ty = Some(crate::core::CoreType::Function(
            Vec::new(),
            Box::new(crate::core::CoreType::Never),
        ));

        let ir = lower_core_to_ir(&CoreProgram {
            defs: vec![hash_def, abort_def],
            top_level_items: Vec::new(),
        });

        let hash_fn = ir
            .functions
            .iter()
            .find(|f| f.name == Some(hash_name))
            .expect("expected lowered hash function");
        assert_eq!(hash_fn.ret_type, IrType::Hash);

        let abort_fn = ir
            .functions
            .iter()
            .find(|f| f.name == Some(abort_name))
            .expect("expected lowered never-returning function");
        assert_eq!(abort_fn.ret_type, IrType::Never);
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
                CoreExpr::bound_var(&bogus, Span::default()),
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
                            field_names: None,
                            span: Span::default(),
                        },
                        crate::syntax::data_variant::DataVariant {
                            name: ctor_none,
                            fields: Vec::new(),
                            field_names: None,
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

    #[test]
    fn lower_drop_specialized_emits_is_unique_and_branch() {
        let mut interner = make_interner();
        let f_name = interner.intern("spec");
        let xs_name = interner.intern("xs");
        let f_binder = binder(0, f_name);
        let xs_binder = binder(1, xs_name);

        let core = CoreProgram {
            defs: vec![CoreDef::new(
                f_binder,
                CoreExpr::Lam {
                    params: vec![xs_binder],
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(CoreExpr::Lit(CoreLit::Int(0), Span::default())),
                    span: Span::default(),
                },
                false,
                Span::default(),
            )],
            top_level_items: Vec::new(),
        };
        let aether = AetherProgram::new(
            core.clone(),
            vec![AetherDef {
                name: f_name,
                binder: f_binder,
                expr: AetherExpr::Lam {
                    params: vec![xs_binder],
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(AetherExpr::DropSpecialized {
                        scrutinee: CoreVarRef::resolved(&xs_binder),
                        unique_body: Box::new(AetherExpr::Lit(CoreLit::Int(1), Span::default())),
                        shared_body: Box::new(AetherExpr::Lit(CoreLit::Int(2), Span::default())),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                borrow_signature: None,
                result_ty: None,
                is_anonymous: false,
                is_recursive: false,
                fip: None,
                span: Span::default(),
            }],
            Vec::new(),
        );

        let ir = lower_aether_to_ir(&aether);
        let spec_fn = ir
            .functions
            .iter()
            .find(|f| f.params.len() == 1 && f.params[0].name == xs_name)
            .expect("expected lowered function for drop-specialized test");

        let has_is_unique = spec_fn.blocks.iter().any(|b| {
            b.instrs.iter().any(|instr| {
                matches!(
                    instr,
                    crate::cfg::IrInstr::Assign {
                        expr: IrExpr::IsUnique(_),
                        ..
                    }
                )
            })
        });
        assert!(
            has_is_unique,
            "DropSpecialized should lower to IrExpr::IsUnique"
        );

        let has_branch = spec_fn
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, IrTerminator::Branch { .. }));
        assert!(
            has_branch,
            "DropSpecialized should lower to a Branch terminator"
        );
    }

    #[test]
    fn lower_closure_captures_only_used_outer_binders() {
        let mut interner = make_interner();
        let main_name = interner.intern("main");
        let x_name = interner.intern("x");
        let y_name = interner.intern("y");
        let f_name = interner.intern("f");
        let z_name = interner.intern("z");

        let main_binder = binder(0, main_name);
        let x_binder = binder(1, x_name);
        let y_binder = binder(2, y_name);
        let f_binder = binder(3, f_name);
        let z_binder = binder(4, z_name);

        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                main_binder,
                CoreExpr::Lam {
                    params: vec![x_binder, y_binder],
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(CoreExpr::Let {
                        var: f_binder,
                        rhs: Box::new(CoreExpr::Lam {
                            params: vec![z_binder],
                            param_types: vec![],
                            result_ty: None,
                            body: Box::new(CoreExpr::PrimOp {
                                op: CorePrimOp::Add,
                                args: vec![var_expr(x_binder), var_expr(z_binder)],
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }),
                        body: Box::new(var_expr(f_binder)),
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
        let closure_fn = ir
            .functions
            .iter()
            .find(|func| {
                matches!(func.origin, IrFunctionOrigin::FunctionLiteral)
                    && func.name.is_none()
                    && func.captures == vec![x_name]
            })
            .expect("expected function literal to capture only x");
        assert_eq!(closure_fn.captures, vec![x_name]);

        let make_closure_capture_len = ir
            .functions
            .iter()
            .flat_map(|func| func.blocks.iter())
            .flat_map(|block| block.instrs.iter())
            .find_map(|instr| match instr {
                IrInstr::Assign {
                    expr: IrExpr::MakeClosure(fid, captures),
                    ..
                } if *fid == closure_fn.id => Some(captures.len()),
                _ => None,
            })
            .expect("expected MakeClosure for inner lambda");
        assert_eq!(make_closure_capture_len, 1);
    }

    #[test]
    fn lower_recursive_closure_excludes_self_and_keeps_used_outer_capture() {
        let mut interner = make_interner();
        let main_name = interner.intern("main");
        let x_name = interner.intern("x");
        let f_name = interner.intern("f");
        let n_name = interner.intern("n");
        let tmp_name = interner.intern("tmp");

        let main_binder = binder(10, main_name);
        let x_binder = binder(11, x_name);
        let f_binder = binder(12, f_name);
        let n_binder = binder(13, n_name);
        let tmp_binder = binder(14, tmp_name);

        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                main_binder,
                CoreExpr::Lam {
                    params: vec![x_binder],
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(CoreExpr::LetRec {
                        var: f_binder,
                        rhs: Box::new(CoreExpr::Lam {
                            params: vec![n_binder],
                            param_types: vec![],
                            result_ty: None,
                            body: Box::new(CoreExpr::Let {
                                var: tmp_binder,
                                rhs: Box::new(CoreExpr::App {
                                    func: Box::new(var_expr(f_binder)),
                                    args: vec![var_expr(n_binder)],
                                    span: Span::default(),
                                }),
                                body: Box::new(var_expr(x_binder)),
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }),
                        body: Box::new(var_expr(f_binder)),
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
        let closure_fn = ir
            .functions
            .iter()
            .find(|func| {
                matches!(func.origin, IrFunctionOrigin::FunctionLiteral)
                    && func.name == Some(f_name)
            })
            .expect("expected named recursive function literal");
        assert_eq!(closure_fn.captures, vec![x_name]);
    }

    #[test]
    fn lower_handler_arm_captures_only_used_outer_binders() {
        let mut interner = make_interner();
        let main_name = interner.intern("main");
        let x_name = interner.intern("x");
        let y_name = interner.intern("y");
        let resume_name = interner.intern("resume");
        let arg_name = interner.intern("arg");
        let effect_name = interner.intern("Config");
        let op_name = interner.intern("get");

        let main_binder = binder(20, main_name);
        let x_binder = binder(21, x_name);
        let y_binder = binder(22, y_name);
        let resume_binder = binder(23, resume_name);
        let arg_binder = binder(24, arg_name);

        let prog = CoreProgram {
            defs: vec![CoreDef::new(
                main_binder,
                CoreExpr::Lam {
                    params: vec![x_binder, y_binder],
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(CoreExpr::Handle {
                        body: Box::new(CoreExpr::Lit(CoreLit::Int(0), Span::default())),
                        effect: effect_name,
                        handlers: vec![CoreHandler {
                            operation: op_name,
                            params: vec![arg_binder],
                            param_types: vec![],
                            resume: resume_binder,
                            resume_ty: None,
                            body: CoreExpr::App {
                                func: Box::new(var_expr(resume_binder)),
                                args: vec![var_expr(x_binder)],
                                span: Span::default(),
                            },
                            span: Span::default(),
                        }],
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
        let handler_fn = ir
            .functions
            .iter()
            .find(|func| {
                matches!(func.origin, IrFunctionOrigin::FunctionLiteral)
                    && func.name.is_none()
                    && func.captures == vec![x_name]
            })
            .expect("expected handler arm closure to capture only x");
        assert_eq!(handler_fn.captures, vec![x_name]);
    }
}
