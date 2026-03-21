#![allow(clippy::too_many_arguments)]

//! AST → Cranelift IR compiler (Phase 1: expressions, let bindings, calls).

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::diagnostics::Diagnostic;
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlags, TrapCode, UserFuncName,
    Value as CraneliftValue, condcodes::IntCC, types,
};
use cranelift_codegen::ir::{StackSlot, StackSlotData};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::JITModule;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use crate::cfg::IrBinaryOp;
use crate::cfg::{
    BlockId as BackendBlockId, FunctionId as BackendFunctionId, IrCallTarget, IrConst,
    IrExpr as BackendIrExpr, IrFunction as BackendIrFunction, IrInstr as BackendIrInstr, IrProgram,
    IrTerminator as BackendIrTerminator, IrVar as BackendIrVar,
};
use crate::diagnostics::position::Span;
use crate::primop::{PrimOp, resolve_primop_call};
use crate::runtime::{function_contract::FunctionContract, runtime_type::RuntimeType};
use crate::syntax::{
    Identifier, expression::ExprId, expression::Expression, interner::Interner, type_expr::TypeExpr,
};
use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

use super::context::{
    JIT_TAG_BOOL, JIT_TAG_FLOAT, JIT_TAG_INT, JIT_TAG_PTR, JitCallAbi, JitFunctionEntry,
};
use super::runtime_helpers::rt_symbols;

/// Pointer type used for all Value pointers in JIT code.
const PTR_TYPE: types::Type = types::I64;

/// Maps runtime helper names to their Cranelift FuncIds.
mod calls;
mod contracts;
mod entry;
mod expressions;
mod function;
mod helpers;
mod support;
mod symbols;

use self::{
    calls::*,
    expressions::{
        compile_backend_named_adt_constructor_call, compile_simple_backend_ir_expr,
        compile_simple_backend_ir_truthiness_condition,
    },
    function::FunctionCompiler,
    helpers::*,
    support::*,
    symbols::*,
};

#[derive(Clone, Copy)]
pub(super) struct JitFunctionMeta {
    pub(super) id: FuncId,
    pub(super) call_abi: JitCallAbi,
    pub(super) function_index: usize,
    pub(super) has_contract: bool,
}

/// Tracks variables in the current scope.
#[allow(dead_code)]
pub struct JitCompiler {
    pub module: JITModule,
    builder_ctx: FunctionBuilderContext,
    helpers: HelperFuncs,
    jit_functions: Vec<JitFunctionCompileEntry>,
    named_functions: HashMap<String, usize>,
    hm_expr_types: Rc<HashMap<ExprId, InferType>>,
    /// Index in `jit_functions` of the compiled identity function used as
    /// the `resume` value for shallow JIT handlers.
    pub identity_fn_index: usize,
    /// Source file path for diagnostic rendering.
    source_file: Option<String>,
    /// Source text for diagnostic rendering.
    source_text: Option<String>,
}

struct JitFunctionCompileEntry {
    id: FuncId,
    num_params: usize,
    call_abi: JitCallAbi,
    contract: Option<FunctionContract>,
    return_span: Option<Span>,
}

impl JitCompiler {
    pub fn new(hm_expr_types: HashMap<ExprId, InferType>) -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .map_err(|e| e.to_string())?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| e.to_string())?;

        let isa_builder =
            cranelift_native::builder().map_err(|e| format!("native ISA error: {}", e))?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| e.to_string())?;

        let mut builder = cranelift_jit::JITBuilder::with_isa(isa, default_libcall_names());

        // Register all runtime helper symbols
        for (name, ptr) in rt_symbols() {
            builder.symbol(name, ptr);
        }

        let module = JITModule::new(builder);
        let builder_ctx = FunctionBuilderContext::new();

        let mut compiler = Self {
            module,
            builder_ctx,
            helpers: HelperFuncs {
                ids: HashMap::new(),
            },
            jit_functions: Vec::new(),
            named_functions: HashMap::new(),
            hm_expr_types: Rc::new(hm_expr_types),
            identity_fn_index: usize::MAX,
            source_file: None,
            source_text: None,
        };

        compiler.declare_helpers()?;

        Ok(compiler)
    }

    /// Set source context for diagnostic rendering.
    pub fn set_source_context(&mut self, file: Option<String>, text: Option<String>) {
        self.source_file = file;
        self.source_text = text;
    }

    /// Parse a compile-time error string (e.g. `"error[E1000]: ...\n  at line:col:end"`)
    /// and convert it to a structured diagnostic.
    pub fn render_compile_error_string(&self, err: &str) -> Diagnostic {
        use crate::diagnostics::{
            Diagnostic, DiagnosticPhase, ErrorType,
            position::{Position, Span},
        };

        // Parse "error[CODE]: title\n  at line:col:end_col"
        let (first_line, rest) = err.split_once('\n').unwrap_or((err, ""));
        let (code, title) = if let Some(after_bracket) = first_line.strip_prefix("error[") {
            if let Some((code, title)) = after_bracket.split_once("]: ") {
                (code, title)
            } else {
                ("E1009", first_line)
            }
        } else {
            ("E1009", first_line)
        };

        // Parse span from "  at line:col:end_col"
        let span = rest
            .trim()
            .strip_prefix("at ")
            .and_then(|s| {
                let parts: Vec<&str> = s.split(':').collect();
                if parts.len() >= 3 {
                    let line = parts[0].parse::<usize>().ok()?;
                    let col = parts[1].parse::<usize>().ok()?;
                    let end_col = parts[2].parse::<usize>().ok()?;
                    Some(Span::new(
                        Position::new(line, col.saturating_sub(1)),
                        Position::new(line, end_col.saturating_sub(1)),
                    ))
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let file = self
            .source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string());
        Diagnostic::make_error_dynamic(
            code,
            title,
            ErrorType::Runtime,
            "",
            None,
            file.clone(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    }

    /// Declare all runtime helper functions in the JIT module.
    fn declare_helpers(&mut self) -> Result<(), String> {
        let sigs = helper_signatures();
        for (name, sig_spec) in &sigs {
            let mut sig = self.module.make_signature();
            for _ in 0..sig_spec.num_params {
                sig.params.push(AbiParam::new(PTR_TYPE));
            }
            for _ in 0..sig_spec.num_returns {
                sig.returns.push(AbiParam::new(PTR_TYPE));
            }

            let func_id = self
                .module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| format!("declare_function({}): {}", name, e))?;
            self.helpers.ids.insert(name, func_id);
        }
        Ok(())
    }

    pub fn try_compile_backend_ir_program(
        &mut self,
        ir_program: &IrProgram,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        if let Some(reason) = backend_ir_jit_support_error(ir_program, interner) {
            return Err(format!(
                "unsupported backend_ir JIT program shape: {}",
                reason
            ));
        }

        let main_id = self.compile_simple_backend_ir_program(ir_program, interner)?;
        Ok(main_id)
    }

    /// Compile a trivial identity closure that returns its first argument unchanged.
    /// JIT function. Its function_index is stored in `self.identity_fn_index` and exposed
    /// to the JIT context so `rt_perform` can build a callable `resume` closure.
    pub fn named_functions(&self) -> HashMap<String, usize> {
        self.named_functions.clone()
    }

    fn record_named_functions(&mut self, scope: &Scope, interner: &Interner) {
        self.named_functions.clear();
        for (name, meta) in &scope.functions {
            self.named_functions
                .insert(interner.resolve(*name).to_string(), meta.function_index);
        }
        for ((module_name, member_name), meta) in &scope.module_functions {
            let full_name = format!(
                "{}.{}",
                interner.resolve(*module_name),
                interner.resolve(*member_name)
            );
            self.named_functions.insert(full_name, meta.function_index);
        }
    }

    fn user_function_signature(
        &mut self,
        call_abi: JitCallAbi,
    ) -> cranelift_codegen::ir::Signature {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE)); // ctx
        match call_abi {
            JitCallAbi::Array => {
                sig.params.push(AbiParam::new(PTR_TYPE)); // args ptr
                sig.params.push(AbiParam::new(PTR_TYPE)); // nargs
            }
            JitCallAbi::Reg1 => {
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg0 tag
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg0 payload
            }
            JitCallAbi::Reg2 => {
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg0 tag
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg0 payload
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg1 tag
                sig.params.push(AbiParam::new(PTR_TYPE)); // arg1 payload
            }
            JitCallAbi::Reg3 => {
                for _ in 0..6 {
                    sig.params.push(AbiParam::new(PTR_TYPE));
                }
            }
            JitCallAbi::Reg4 => {
                for _ in 0..8 {
                    sig.params.push(AbiParam::new(PTR_TYPE));
                }
            }
        }
        sig.params.push(AbiParam::new(PTR_TYPE)); // captures ptr
        sig.params.push(AbiParam::new(PTR_TYPE)); // ncaptures
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result tag
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result payload
        sig
    }

    /// Finalize all functions and make them callable.
    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    /// Get a callable function pointer for the given FuncId.
    pub fn get_func_ptr(&self, id: FuncId) -> *const u8 {
        self.module.get_finalized_function(id)
    }

    pub fn jit_function_entries(&self) -> Vec<JitFunctionEntry> {
        self.jit_functions
            .iter()
            .map(|entry| JitFunctionEntry {
                ptr: self.module.get_finalized_function(entry.id),
                num_params: entry.num_params,
                call_abi: entry.call_abi,
                contract: entry.contract.clone(),
                return_span: entry.return_span,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::backend_ir_jit_support_error;
    use crate::{
        cfg::IrBinaryOp,
        cfg::{
            BlockId, FunctionId, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction,
            IrFunctionOrigin, IrInstr, IrMetadata, IrProgram, IrTerminator, IrTopLevelItem, IrType,
            IrVar,
        },
        diagnostics::position::Span,
        syntax::interner::Interner,
    };
    use std::collections::HashMap;

    #[test]
    fn backend_ir_jit_support_accepts_declaration_only_top_level_items() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let fn_id = FunctionId(0);
        let entry_block = BlockId(0);
        let ret_var = IrVar(0);
        let ir_program = IrProgram {
            functions: vec![IrFunction {
                id: fn_id,
                name: Some(main_name),
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                captures: Vec::new(),
                body_span: Span::default(),
                ret_type: IrType::Any,
                blocks: vec![IrBlock {
                    id: entry_block,
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: ret_var,
                        expr: IrExpr::Const(IrConst::Int(1)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Return(ret_var, IrMetadata::empty()),
                }],
                entry: entry_block,
                origin: IrFunctionOrigin::NamedFunction,
                metadata: IrMetadata::empty(),
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            }],
            top_level_items: vec![IrTopLevelItem::Function {
                is_public: false,
                name: main_name,
                type_params: Vec::new(),
                function_id: Some(fn_id),
                parameters: Vec::new(),
                parameter_types: Vec::new(),
                return_type: None,
                effects: Vec::new(),
                body: crate::syntax::block::Block {
                    statements: Vec::new(),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            core: None,
            entry: fn_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
        };

        let reason = backend_ir_jit_support_error(&ir_program, &interner);
        assert!(
            reason.is_none(),
            "declaration-only top-level items should stay on the direct backend JIT path, got {reason:?}"
        );
    }

    #[test]
    fn backend_ir_jit_support_accepts_module_data_and_effect_items() {
        let mut interner = Interner::new();
        let module_name = interner.intern("Demo");
        let function_name = interner.intern("value");
        let data_name = interner.intern("MaybeInt");
        let ctor_name = interner.intern("SomeInt");
        let effect_name = interner.intern("Console");
        let op_name = interner.intern("print");
        let string_name = interner.intern("String");
        let unit_name = interner.intern("Unit");
        let fn_id = FunctionId(0);
        let entry_block = BlockId(0);
        let ret_var = IrVar(0);
        let ir_program = IrProgram {
            functions: vec![IrFunction {
                id: fn_id,
                name: Some(function_name),
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                captures: Vec::new(),
                body_span: Span::default(),
                ret_type: IrType::Any,
                blocks: vec![IrBlock {
                    id: entry_block,
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: ret_var,
                        expr: IrExpr::Const(IrConst::Int(1)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Return(ret_var, IrMetadata::empty()),
                }],
                entry: entry_block,
                origin: IrFunctionOrigin::NamedFunction,
                metadata: IrMetadata::empty(),
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            }],
            top_level_items: vec![
                IrTopLevelItem::Module {
                    name: module_name,
                    body: vec![IrTopLevelItem::Function {
                        is_public: false,
                        name: function_name,
                        type_params: Vec::new(),
                        function_id: Some(fn_id),
                        parameters: Vec::new(),
                        parameter_types: Vec::new(),
                        return_type: None,
                        effects: Vec::new(),
                        body: crate::syntax::block::Block {
                            statements: Vec::new(),
                            span: Span::default(),
                        },
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
                IrTopLevelItem::Data {
                    name: data_name,
                    type_params: Vec::new(),
                    variants: vec![crate::syntax::data_variant::DataVariant {
                        name: ctor_name,
                        fields: vec![crate::syntax::type_expr::TypeExpr::Named {
                            name: string_name,
                            args: Vec::new(),
                            span: Span::default(),
                        }],
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
                IrTopLevelItem::EffectDecl {
                    name: effect_name,
                    ops: vec![crate::syntax::effect_ops::EffectOp {
                        name: op_name,
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
            core: None,
            entry: fn_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
        };

        let reason = backend_ir_jit_support_error(&ir_program, &interner);
        assert!(
            reason.is_none(),
            "module/data/effect declaration items should stay on the direct backend JIT path, got {reason:?}"
        );
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_simple_direct_subset() {
        let mut interner = Interner::new();
        let helper_id = FunctionId(0);
        let entry_id = FunctionId(1);
        let helper_ret = IrVar(0);
        let entry_ret = IrVar(1);

        let helper = IrFunction {
            id: helper_id,
            name: None.or_else(|| Some(interner.intern("helper"))),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: helper_ret,
                    expr: IrExpr::Const(IrConst::Int(7)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(helper_ret, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let entry = IrFunction {
            id: entry_id,
            name: None.or_else(|| Some(interner.intern("entry"))),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(1),
                params: Vec::new(),
                instrs: vec![IrInstr::Call {
                    dest: entry_ret,
                    target: IrCallTarget::Direct(helper_id),
                    args: Vec::new(),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(entry_ret, IrMetadata::empty()),
            }],
            entry: BlockId(1),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![helper, entry],
            entry: entry_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_named_loads_for_direct_functions() {
        let mut interner = Interner::new();
        let helper_name = interner.intern("helper");
        let entry_name = interner.intern("entry");
        let helper_id = FunctionId(0);
        let entry_id = FunctionId(1);
        let helper_val = IrVar(0);
        let helper_ref = IrVar(1);
        let entry_ret = IrVar(2);

        let helper = IrFunction {
            id: helper_id,
            name: Some(helper_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: helper_val,
                    expr: IrExpr::Const(IrConst::Int(9)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(helper_val, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let entry = IrFunction {
            id: entry_id,
            name: Some(entry_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(1),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: helper_ref,
                        expr: IrExpr::LoadName(helper_name),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Call {
                        dest: entry_ret,
                        target: IrCallTarget::Named(helper_name),
                        args: Vec::new(),
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(entry_ret, IrMetadata::empty()),
            }],
            entry: BlockId(1),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![helper, entry],
            entry: entry_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_module_member_calls() {
        let mut interner = Interner::new();
        let module_name = interner.intern("Demo");
        let helper_name = interner.intern("value");
        let entry_name = interner.intern("entry");
        let helper_id = FunctionId(0);
        let entry_id = FunctionId(1);
        let helper_val = IrVar(0);
        let module_ref = IrVar(1);
        let member_ref = IrVar(2);
        let entry_ret = IrVar(3);

        let helper = IrFunction {
            id: helper_id,
            name: Some(helper_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: helper_val,
                    expr: IrExpr::Const(IrConst::Int(11)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(helper_val, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let entry = IrFunction {
            id: entry_id,
            name: Some(entry_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(1),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: module_ref,
                        expr: IrExpr::LoadName(module_name),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: member_ref,
                        expr: IrExpr::MemberAccess {
                            object: module_ref,
                            member: helper_name,
                            module_name: Some(module_name),
                        },
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Call {
                        dest: entry_ret,
                        target: IrCallTarget::Var(member_ref),
                        args: Vec::new(),
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(entry_ret, IrMetadata::empty()),
            }],
            entry: BlockId(1),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: vec![IrTopLevelItem::Module {
                name: module_name,
                body: vec![IrTopLevelItem::Function {
                    is_public: false,
                    name: helper_name,
                    type_params: Vec::new(),
                    function_id: Some(helper_id),
                    parameters: Vec::new(),
                    parameter_types: Vec::new(),
                    return_type: None,
                    effects: Vec::new(),
                    body: crate::syntax::block::Block {
                        statements: Vec::new(),
                        span: Span::default(),
                    },
                    span: Span::default(),
                }],
                span: Span::default(),
            }],
            functions: vec![helper, entry],
            entry: entry_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_binary_add() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let main_id = FunctionId(0);
        let lhs = IrVar(0);
        let rhs = IrVar(1);
        let sum = IrVar(2);

        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: lhs,
                        expr: IrExpr::Const(IrConst::Int(20)),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: rhs,
                        expr: IrExpr::Const(IrConst::Int(22)),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: sum,
                        expr: IrExpr::Binary(IrBinaryOp::Add, lhs, rhs),
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(sum, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![main],
            entry: main_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let direct = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = direct;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_jump_with_block_param() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let main_id = FunctionId(0);
        let initial = IrVar(0);
        let result = IrVar(1);

        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![
                IrBlock {
                    id: BlockId(0),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: initial,
                        expr: IrExpr::Const(IrConst::Int(33)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Jump(BlockId(1), vec![initial], IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(1),
                    params: vec![crate::cfg::IrBlockParam {
                        var: result,
                        ty: IrType::Any,
                    }],
                    instrs: Vec::new(),
                    terminator: IrTerminator::Return(result, IrMetadata::empty()),
                },
            ],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![main],
            entry: main_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let direct = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = direct;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_multi_block_branch() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let main_id = FunctionId(0);
        let cond = IrVar(0);
        let then_ret = IrVar(1);
        let else_ret = IrVar(2);

        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![
                IrBlock {
                    id: BlockId(0),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: cond,
                        expr: IrExpr::Const(IrConst::Bool(true)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Branch {
                        cond,
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                        metadata: IrMetadata::empty(),
                    },
                },
                IrBlock {
                    id: BlockId(1),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: then_ret,
                        expr: IrExpr::Const(IrConst::Int(1)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Return(then_ret, IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(2),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: else_ret,
                        expr: IrExpr::Const(IrConst::Int(0)),
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Return(else_ret, IrMetadata::empty()),
                },
            ],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![main],
            entry: main_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let direct = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = direct;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_closure_and_indirect_call() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let closure_name = interner.intern("closure_fn");
        let captured_name = interner.intern("captured");
        let closure_id = FunctionId(0);
        let main_id = FunctionId(1);
        let captured_value = IrVar(0);
        let closure_value = IrVar(1);
        let closure_ret = IrVar(2);
        let captured_param = crate::cfg::IrParam {
            name: captured_name,
            var: IrVar(10),
            ty: IrType::Any,
        };

        let closure_fn = IrFunction {
            id: closure_id,
            name: Some(closure_name),
            params: vec![captured_param],
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: vec![captured_name],
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Return(IrVar(10), IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::FunctionLiteral,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let main_fn = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(1),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: captured_value,
                        expr: IrExpr::Const(IrConst::Int(41)),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: closure_value,
                        expr: IrExpr::MakeClosure(closure_id, vec![captured_value]),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Call {
                        dest: closure_ret,
                        target: IrCallTarget::Var(closure_value),
                        args: Vec::new(),
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(closure_ret, IrMetadata::empty()),
            }],
            entry: BlockId(1),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![closure_fn, main_fn],
            entry: main_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let direct = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = direct;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_global_init_and_global_load() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let count_name = interner.intern("count");
        let entry_id = FunctionId(0);
        let main_id = FunctionId(1);
        let init_tmp = IrVar(0);
        let global_count = IrVar(1);
        let main_ret = IrVar(2);

        let entry = IrFunction {
            id: entry_id,
            name: Some(interner.intern("entry")),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: init_tmp,
                        expr: IrExpr::Const(IrConst::Int(10)),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: global_count,
                        expr: IrExpr::Var(init_tmp),
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(global_count, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(1),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: main_ret,
                    expr: IrExpr::LoadName(count_name),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(main_ret, IrMetadata::empty()),
            }],
            entry: BlockId(1),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![entry, main],
            entry: entry_id,
            globals: vec![count_name],
            global_bindings: vec![crate::cfg::IrGlobalBinding {
                name: count_name,
                var: global_count,
            }],
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_handle_scope_with_perform() {
        let mut interner = Interner::new();
        let entry_name = interner.intern("entry");
        let main_name = interner.intern("main");
        let effect_name = interner.intern("Demo");
        let op_name = interner.intern("ping");
        let resume_name = interner.intern("resume");
        let arm_id = FunctionId(0);
        let entry_id = FunctionId(1);
        let main_id = FunctionId(2);
        let handle_result = IrVar(0);
        let perform_result = IrVar(1);

        let arm_fn = IrFunction {
            id: arm_id,
            name: Some(interner.intern("demo_ping_arm")),
            params: vec![crate::cfg::IrParam {
                name: resume_name,
                var: IrVar(10),
                ty: IrType::Any,
            }],
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: IrVar(11),
                    expr: IrExpr::Const(IrConst::Int(42)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(IrVar(11), IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::FunctionLiteral,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let entry = IrFunction {
            id: entry_id,
            name: Some(entry_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![
                IrBlock {
                    id: BlockId(1),
                    params: Vec::new(),
                    instrs: vec![crate::cfg::IrInstr::HandleScope {
                        effect: effect_name,
                        arms: vec![crate::cfg::HandleScopeArm {
                            operation_name: op_name,
                            function_id: arm_id,
                            capture_vars: Vec::new(),
                        }],
                        body_entry: BlockId(2),
                        body_result: handle_result,
                        dest: handle_result,
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Jump(BlockId(2), Vec::new(), IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(2),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: perform_result,
                        expr: IrExpr::Perform {
                            effect: effect_name,
                            operation: op_name,
                            args: Vec::new(),
                        },
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Jump(
                        BlockId(3),
                        vec![perform_result],
                        IrMetadata::empty(),
                    ),
                },
                IrBlock {
                    id: BlockId(3),
                    params: vec![crate::cfg::IrBlockParam {
                        var: handle_result,
                        ty: IrType::Any,
                    }],
                    instrs: Vec::new(),
                    terminator: IrTerminator::Return(handle_result, IrMetadata::empty()),
                },
            ],
            entry: BlockId(1),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(4),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: IrVar(20),
                    expr: IrExpr::Const(IrConst::Int(0)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(IrVar(20), IrMetadata::empty()),
            }],
            entry: BlockId(4),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![arm_fn, entry, main],
            entry: entry_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_captured_handle_scope_arm() {
        let mut interner = Interner::new();
        let entry_name = interner.intern("entry");
        let main_name = interner.intern("main");
        let effect_name = interner.intern("Demo");
        let op_name = interner.intern("ping");
        let captured_name = interner.intern("captured");
        let resume_name = interner.intern("resume");
        let arm_id = FunctionId(0);
        let entry_id = FunctionId(1);
        let main_id = FunctionId(2);
        let captured_value = IrVar(0);
        let handle_result = IrVar(1);
        let perform_result = IrVar(2);

        let arm_fn = IrFunction {
            id: arm_id,
            name: Some(interner.intern("demo_ping_arm_capture")),
            params: vec![
                crate::cfg::IrParam {
                    name: captured_name,
                    var: IrVar(10),
                    ty: IrType::Any,
                },
                crate::cfg::IrParam {
                    name: resume_name,
                    var: IrVar(11),
                    ty: IrType::Any,
                },
            ],
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: vec![captured_name],
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Return(IrVar(10), IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::FunctionLiteral,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let entry = IrFunction {
            id: entry_id,
            name: Some(entry_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![
                IrBlock {
                    id: BlockId(1),
                    params: Vec::new(),
                    instrs: vec![
                        IrInstr::Assign {
                            dest: captured_value,
                            expr: IrExpr::Const(IrConst::Int(77)),
                            metadata: IrMetadata::empty(),
                        },
                        crate::cfg::IrInstr::HandleScope {
                            effect: effect_name,
                            arms: vec![crate::cfg::HandleScopeArm {
                                operation_name: op_name,
                                function_id: arm_id,
                                capture_vars: vec![captured_value],
                            }],
                            body_entry: BlockId(2),
                            body_result: handle_result,
                            dest: handle_result,
                            metadata: IrMetadata::empty(),
                        },
                    ],
                    terminator: IrTerminator::Jump(BlockId(2), Vec::new(), IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(2),
                    params: Vec::new(),
                    instrs: vec![IrInstr::Assign {
                        dest: perform_result,
                        expr: IrExpr::Perform {
                            effect: effect_name,
                            operation: op_name,
                            args: Vec::new(),
                        },
                        metadata: IrMetadata::empty(),
                    }],
                    terminator: IrTerminator::Jump(
                        BlockId(3),
                        vec![perform_result],
                        IrMetadata::empty(),
                    ),
                },
                IrBlock {
                    id: BlockId(3),
                    params: vec![crate::cfg::IrBlockParam {
                        var: handle_result,
                        ty: IrType::Any,
                    }],
                    instrs: Vec::new(),
                    terminator: IrTerminator::Return(handle_result, IrMetadata::empty()),
                },
            ],
            entry: BlockId(1),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let main = IrFunction {
            id: main_id,
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(4),
                params: Vec::new(),
                instrs: vec![IrInstr::Assign {
                    dest: IrVar(20),
                    expr: IrExpr::Const(IrConst::Int(0)),
                    metadata: IrMetadata::empty(),
                }],
                terminator: IrTerminator::Return(IrVar(20), IrMetadata::empty()),
            }],
            entry: BlockId(4),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![arm_fn, entry, main],
            entry: entry_id,
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let main_id = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = main_id;
    }

    #[test]
    fn try_compile_backend_ir_program_accepts_tuple_arity_test() {
        let mut interner = Interner::new();
        let main_name = interner.intern("main");
        let tuple_var = IrVar(0);
        let check_var = IrVar(1);

        let main_fn = IrFunction {
            id: FunctionId(0),
            name: Some(main_name),
            params: Vec::new(),
            parameter_types: Vec::new(),
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: Span::default(),
            ret_type: IrType::Any,
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: vec![
                    IrInstr::Assign {
                        dest: tuple_var,
                        expr: IrExpr::MakeTuple(vec![]),
                        metadata: IrMetadata::empty(),
                    },
                    IrInstr::Assign {
                        dest: check_var,
                        expr: IrExpr::TupleArityTest {
                            value: tuple_var,
                            arity: 0,
                        },
                        metadata: IrMetadata::empty(),
                    },
                ],
                terminator: IrTerminator::Return(check_var, IrMetadata::empty()),
            }],
            entry: BlockId(0),
            origin: IrFunctionOrigin::NamedFunction,
            metadata: IrMetadata::empty(),
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        };
        let ir_program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![main_fn],
            entry: FunctionId(0),
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        let mut jit = crate::jit::compiler::JitCompiler::new(HashMap::new()).expect("jit");
        let direct = jit
            .try_compile_backend_ir_program(&ir_program, &interner)
            .expect("compile ok");
        let _ = direct;
    }
}
