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

use crate::backend_ir::{
    BlockId as BackendBlockId, FunctionId as BackendFunctionId, IrCallTarget, IrConst,
    IrExpr as BackendIrExpr, IrFunction as BackendIrFunction, IrInstr as BackendIrInstr, IrProgram,
    IrTerminator as BackendIrTerminator, IrVar as BackendIrVar,
};
use crate::cfg::IrBinaryOp;
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
struct HelperFuncs {
    ids: HashMap<&'static str, FuncId>,
}

#[derive(Clone, Copy)]
struct JitFunctionMeta {
    id: FuncId,
    call_abi: JitCallAbi,
    function_index: usize,
    has_contract: bool,
}

/// Tracks variables in the current scope.
#[allow(dead_code)]
#[derive(Clone)]
struct LocalBinding {
    var: Variable,
    kind: JitValueKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JitValueKind {
    Boxed,
    Int,
    Float,
    Bool,
}

#[derive(Clone, Copy)]
struct JitValue {
    value: CraneliftValue,
    kind: JitValueKind,
}

impl JitValue {
    fn boxed(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Boxed,
        }
    }

    fn int(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Int,
        }
    }

    fn float(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Float,
        }
    }

    fn bool(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Bool,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
struct Scope {
    /// Maps interned identifier → Cranelift Variable
    locals: HashMap<Identifier, LocalBinding>,
    /// Maps interned identifier → global slot index
    globals: HashMap<Identifier, usize>,
    /// Maps interned identifier → base index
    base_functions: HashMap<Identifier, usize>,
    /// Base names excluded from unqualified lookup via `import Base except [...]`.
    excluded_base_symbols: HashSet<Identifier>,
    /// Maps interned identifier → JIT function metadata.
    functions: HashMap<Identifier, JitFunctionMeta>,
    /// Maps (module name, member name) -> JIT function metadata.
    module_functions: HashMap<(Identifier, Identifier), JitFunctionMeta>,
    /// Imported module names visible in current scope.
    imported_modules: HashSet<Identifier>,
    /// Import aliases: alias -> module name.
    import_aliases: HashMap<Identifier, Identifier>,
    /// Maps ADT constructor name (interned) → arity. Used to route constructor calls.
    adt_constructors: HashMap<Identifier, usize>,
    /// Maps ADT constructor name (interned) → owning ADT name.
    adt_constructor_owner: HashMap<Identifier, Identifier>,
    /// Maps ADT name → constructor names.
    adt_variants: HashMap<Identifier, Vec<Identifier>>,
    /// HM expression types used by the retired AST JIT path.
    hm_expr_types: Rc<HashMap<ExprId, InferType>>,
}

impl Scope {
    fn new(hm_expr_types: Rc<HashMap<ExprId, InferType>>) -> Self {
        Self {
            locals: HashMap::new(),
            globals: HashMap::new(),
            base_functions: HashMap::new(),
            excluded_base_symbols: HashSet::new(),
            functions: HashMap::new(),
            module_functions: HashMap::new(),
            imported_modules: HashSet::new(),
            import_aliases: HashMap::new(),
            adt_constructors: HashMap::new(),
            adt_constructor_owner: HashMap::new(),
            adt_variants: HashMap::new(),
            hm_expr_types,
        }
    }
}

fn resolve_module_name(scope: &Scope, interner: &Interner, name: Identifier) -> Option<Identifier> {
    scope.import_aliases.get(&name).copied().or_else(|| {
        if scope.imported_modules.contains(&name)
            || interner.resolve(name) == "Base"
            || scope
                .module_functions
                .keys()
                .any(|(module_name, _)| module_name == &name)
        {
            Some(name)
        } else {
            None
        }
    })
}

#[allow(dead_code)]
fn declare_local(builder: &mut FunctionBuilder, kind: JitValueKind) -> Variable {
    let ty = match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    };
    builder.declare_var(ty)
}

#[allow(dead_code)]
fn bind_local(
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    name: Identifier,
    value: JitValue,
) -> LocalBinding {
    let var = declare_local(builder, value.kind);
    builder.def_var(var, value.value);
    let binding = LocalBinding {
        var,
        kind: value.kind,
    };
    scope.locals.insert(name, binding.clone());
    binding
}

#[allow(dead_code)]
fn use_local(builder: &mut FunctionBuilder, binding: LocalBinding) -> JitValue {
    let value = builder.use_var(binding.var);
    match binding.kind {
        JitValueKind::Boxed => JitValue::boxed(value),
        JitValueKind::Int => JitValue::int(value),
        JitValueKind::Float => JitValue::float(value),
        JitValueKind::Bool => JitValue::bool(value),
    }
}

fn jit_value_tag(builder: &mut FunctionBuilder, kind: JitValueKind) -> CraneliftValue {
    let tag = match kind {
        JitValueKind::Boxed => JIT_TAG_PTR,
        JitValueKind::Int => JIT_TAG_INT,
        JitValueKind::Float => JIT_TAG_FLOAT,
        JitValueKind::Bool => JIT_TAG_BOOL,
    };
    builder.ins().iconst(types::I64, tag)
}

fn box_jit_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) -> CraneliftValue {
    match value.kind {
        JitValueKind::Boxed => value.value,
        JitValueKind::Int => {
            let box_value = get_helper_func_ref(module, helpers, builder, "rt_force_boxed");
            let tag = jit_value_tag(builder, value.kind);
            let call = builder.ins().call(box_value, &[ctx_val, tag, value.value]);
            builder.inst_results(call)[1]
        }
        JitValueKind::Float => {
            let box_value = get_helper_func_ref(module, helpers, builder, "rt_force_boxed");
            let tag = jit_value_tag(builder, value.kind);
            let call = builder.ins().call(box_value, &[ctx_val, tag, value.value]);
            builder.inst_results(call)[1]
        }
        JitValueKind::Bool => {
            let box_value = get_helper_func_ref(module, helpers, builder, "rt_force_boxed");
            let tag = jit_value_tag(builder, value.kind);
            let call = builder.ins().call(box_value, &[ctx_val, tag, value.value]);
            builder.inst_results(call)[1]
        }
    }
}

fn box_and_guard_jit_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) -> CraneliftValue {
    let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
    emit_return_on_null_value(builder, boxed);
    boxed
}

fn boxed_value_from_tagged_parts(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    tag: CraneliftValue,
    payload: CraneliftValue,
) -> CraneliftValue {
    let box_value = get_helper_func_ref(module, helpers, builder, "rt_force_boxed");
    let call = builder.ins().call(box_value, &[ctx_val, tag, payload]);
    builder.inst_results(call)[1]
}

fn emit_return_null_tagged(builder: &mut FunctionBuilder) {
    let null_tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    builder.ins().return_(&[null_tag, null_ptr]);
}

fn jit_value_to_tag_payload(
    builder: &mut FunctionBuilder,
    value: JitValue,
) -> (CraneliftValue, CraneliftValue) {
    (jit_value_tag(builder, value.kind), value.value)
}

#[allow(dead_code)]
fn append_return_block_params(builder: &mut FunctionBuilder, block: cranelift_codegen::ir::Block) {
    builder.append_block_param(block, types::I64);
    builder.append_block_param(block, PTR_TYPE);
}

#[allow(dead_code)]
fn jump_with_jit_value(
    builder: &mut FunctionBuilder,
    block: cranelift_codegen::ir::Block,
    value: JitValue,
) {
    let (tag, payload) = jit_value_to_tag_payload(builder, value);
    let args = [BlockArg::Value(tag), BlockArg::Value(payload)];
    builder.ins().jump(block, &args);
}

fn emit_tagged_stack_array(
    builder: &mut FunctionBuilder,
    values: &[JitValue],
) -> (cranelift_codegen::ir::StackSlot, CraneliftValue) {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (values.len().max(1) as u32) * 16,
        3,
    ));
    for (i, value) in values.iter().enumerate() {
        let (tag, payload) = jit_value_to_tag_payload(builder, *value);
        builder.ins().stack_store(tag, slot, (i * 16) as i32);
        builder
            .ins()
            .stack_store(payload, slot, (i * 16 + 8) as i32);
    }
    let ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    (slot, ptr)
}

fn emit_boxed_stack_array(
    builder: &mut FunctionBuilder,
    values: &[CraneliftValue],
) -> (cranelift_codegen::ir::StackSlot, CraneliftValue) {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (values.len().max(1) as u32) * 8,
        3,
    ));
    for (i, value) in values.iter().enumerate() {
        builder.ins().stack_store(*value, slot, (i * 8) as i32);
    }
    let ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    (slot, ptr)
}

#[allow(dead_code)]
fn emit_return_on_null_jit_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) {
    if value.kind == JitValueKind::Boxed {
        let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
        emit_return_on_null_value(builder, boxed);
    }
}

#[allow(dead_code)]
fn compile_truthiness_condition(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &Scope,
    ctx_val: CraneliftValue,
    expr: &Expression,
    value: JitValue,
) -> CraneliftValue {
    let truthy_i64 = if expr_has_known_bool_type(scope, expr) {
        match value.kind {
            JitValueKind::Bool => value.value,
            JitValueKind::Boxed => {
                let bool_value = get_helper_func_ref(module, helpers, builder, "rt_bool_value");
                let tag = jit_value_tag(builder, value.kind);
                let call = builder.ins().call(bool_value, &[ctx_val, tag, value.value]);
                builder.inst_results(call)[0]
            }
            JitValueKind::Int | JitValueKind::Float => {
                let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
                let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
                let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                let call = builder.ins().call(is_truthy, &[ctx_val, tag, boxed]);
                builder.inst_results(call)[0]
            }
        }
    } else {
        let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
        let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
        let call = builder.ins().call(is_truthy, &[ctx_val, tag, boxed]);
        builder.inst_results(call)[0]
    };

    builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0)
}

#[allow(dead_code)]
fn jit_value_type(kind: JitValueKind) -> types::Type {
    match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    }
}

#[allow(dead_code)]
fn merged_jit_value_kind(left: JitValue, right: JitValue) -> JitValueKind {
    if left.kind == right.kind {
        left.kind
    } else {
        JitValueKind::Boxed
    }
}

#[allow(dead_code)]
fn live_branch_locals(
    builder: &mut FunctionBuilder,
    scope: &Scope,
) -> Vec<(Identifier, LocalBinding, CraneliftValue)> {
    let mut locals: Vec<_> = scope
        .locals
        .iter()
        .map(|(name, binding)| (*name, binding.clone(), builder.use_var(binding.var)))
        .collect();
    locals.sort_by_key(|(name, _, _)| name.as_u32());
    locals
}

#[allow(dead_code)]
fn bind_branch_block_params(
    builder: &mut FunctionBuilder,
    block: cranelift_codegen::ir::Block,
    live_locals: &[(Identifier, LocalBinding, CraneliftValue)],
) {
    let params = builder.block_params(block).to_vec();
    for ((_, binding, _), param) in live_locals.iter().zip(params.iter()) {
        builder.def_var(binding.var, *param);
    }
}

#[allow(dead_code)]
fn expr_has_known_bool_type(scope: &Scope, expr: &Expression) -> bool {
    matches!(
        scope.hm_expr_types.get(&expr.expr_id()),
        Some(InferType::Con(TypeConstructor::Bool))
    )
}

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
    fn compile_identity_function(&mut self) -> Result<usize, String> {
        let sig = self.user_function_signature(JitCallAbi::from_arity(1));
        let func_id = self
            .module
            .declare_function("__flux_identity", cranelift_module::Linkage::Local, &sig)
            .map_err(|e| format!("declare __flux_identity: {}", e))?;

        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let entry_params = builder.block_params(entry).to_vec();
            let tag = entry_params[1];
            let payload = entry_params[2];
            builder.ins().return_(&[tag, payload]);
            builder.finalize();
        }

        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(func_id, &mut ctx)
            .map_err(|e| format!("define __flux_identity: {}", e))?;

        let function_index = self.jit_functions.len();
        self.jit_functions.push(JitFunctionCompileEntry {
            id: func_id,
            num_params: 1,
            call_abi: JitCallAbi::Reg1,
            contract: None,
            return_span: None,
        });
        Ok(function_index)
    }

    fn compile_simple_backend_ir_program(
        &mut self,
        ir_program: &IrProgram,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        let mut scope = Scope::new(Rc::clone(&self.hm_expr_types));
        register_base_functions(&mut scope, interner);
        let mut backend_function_metas = HashMap::new();
        let backend_function_defs: HashMap<BackendFunctionId, &BackendIrFunction> =
            ir_program.functions().iter().map(|f| (f.id, f)).collect();
        for (idx, name) in ir_program.globals().iter().enumerate() {
            scope.globals.insert(*name, idx);
        }
        let mut imported_modules = HashSet::new();
        let mut import_aliases = HashMap::new();
        let mut adt_constructors = HashMap::new();
        let global_binding_indices: HashMap<BackendIrVar, usize> = ir_program
            .global_bindings()
            .iter()
            .filter_map(|binding| {
                scope
                    .globals
                    .get(&binding.name)
                    .copied()
                    .map(|idx| (binding.var, idx))
            })
            .collect();

        for function in ir_program.functions() {
            let explicit_arity = function
                .params
                .len()
                .saturating_sub(function.captures.len());
            let call_abi = JitCallAbi::from_arity(explicit_arity);
            let sig = self.user_function_signature(call_abi);
            let function_name = function
                .name
                .map(|name| format!("flux_backend_{}", interner.resolve(name)))
                .unwrap_or_else(|| format!("flux_backend_fn{}", function.id.0));
            let id = self
                .module
                .declare_function(&function_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", function_name, e))?;
            let function_index = self.jit_functions.len();
            let contract = crate::runtime::function_contract::runtime_contract_from_annotations(
                &function.parameter_types,
                &function.return_type_annotation,
                &function.effects,
                interner,
            );
            let has_contract = contract.is_some();
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: explicit_arity,
                call_abi,
                contract,
                return_span: function
                    .return_type_annotation
                    .as_ref()
                    .map(|_| function.body_span),
            });
            let meta = JitFunctionMeta {
                id,
                call_abi,
                function_index,
                has_contract,
            };
            backend_function_metas.insert(function.id, meta);
            if let Some(name) = function.name {
                scope.functions.insert(name, meta);
            }
        }
        collect_backend_top_level_declaration_metadata(
            ir_program.top_level_items(),
            &mut imported_modules,
            &mut import_aliases,
            &mut adt_constructors,
        );
        scope.imported_modules.extend(imported_modules);
        scope.import_aliases.extend(import_aliases);
        scope.adt_constructors.extend(adt_constructors);
        register_backend_top_level_named_functions(
            ir_program.top_level_items(),
            &backend_function_metas,
            &mut scope,
        );
        register_backend_top_level_module_functions(
            ir_program.top_level_items(),
            None,
            &backend_function_metas,
            &mut scope,
        );

        for function in ir_program.functions() {
            self.compile_simple_backend_ir_function(
                function,
                &scope,
                &backend_function_metas,
                &backend_function_defs,
                &global_binding_indices,
                ir_program.entry(),
                interner,
            )?;
        }
        self.record_named_functions(&scope, interner);

        let entry_function = ir_program
            .functions()
            .iter()
            .find(|function| function.id == ir_program.entry())
            .ok_or_else(|| format!("missing backend entry function {:?}", ir_program.entry()))?;
        let entry_meta = backend_function_metas
            .get(&entry_function.id)
            .copied()
            .ok_or_else(|| "missing backend entry metadata".to_string())?;
        let main_meta = scope
            .functions
            .iter()
            .find_map(|(name, meta)| (interner.resolve(*name) == "main").then_some(*meta));

        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));
        let main_id = self
            .module
            .declare_function("flux_main", Linkage::Export, &sig)
            .map_err(|e| format!("declare flux_main: {}", e))?;
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let ctx_val = builder.block_params(entry)[0];
            let entry_result = compile_jit_cfg_user_function_call(
                module,
                helpers,
                &mut builder,
                ctx_val,
                entry_meta,
                &[],
                entry_function.body_span,
            )?;
            let result = if let Some(main_meta) = main_meta {
                compile_jit_cfg_user_function_call(
                    module,
                    helpers,
                    &mut builder,
                    ctx_val,
                    main_meta,
                    &[],
                    entry_function.body_span,
                )?
            } else {
                entry_result
            };
            let (tag, payload) = jit_value_to_tag_payload(&mut builder, result);
            builder.ins().return_(&[tag, payload]);
            builder.finalize();
        }
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(main_id, &mut ctx)
            .map_err(|e| format!("define flux_main: {}", e))?;
        self.identity_fn_index = self.compile_identity_function()?;
        Ok(main_id)
    }

    fn compile_simple_backend_ir_function(
        &mut self,
        function: &BackendIrFunction,
        scope: &Scope,
        backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
        backend_function_defs: &HashMap<BackendFunctionId, &BackendIrFunction>,
        global_binding_indices: &HashMap<BackendIrVar, usize>,
        entry_function_id: BackendFunctionId,
        interner: &Interner,
    ) -> Result<(), String> {
        let meta = backend_function_metas
            .get(&function.id)
            .copied()
            .ok_or_else(|| "missing backend function metadata".to_string())?;

        let sig = self.user_function_signature(meta.call_abi);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let prelude = builder.create_block();
            let mut block_map = HashMap::new();
            let block_defs: HashMap<BackendBlockId, &crate::backend_ir::IrBlock> = function
                .blocks
                .iter()
                .map(|block| (block.id, block))
                .collect();
            let block_order = ordered_backend_blocks(function);
            for block in &function.blocks {
                let cl_block = builder.create_block();
                block_map.insert(block.id, cl_block);
            }
            let entry = block_map[&function.entry];
            builder.append_block_params_for_function_params(prelude);
            for block in &function.blocks {
                if block.id == function.entry {
                    continue;
                }
                let cl_block = block_map[&block.id];
                for _ in &block.params {
                    builder.append_block_param(cl_block, PTR_TYPE);
                }
            }
            builder.switch_to_block(prelude);

            let mut env = HashMap::new();
            let mut module_env = HashMap::new();
            let mut function_env = HashMap::new();
            let mut handler_pop_counts: HashMap<BackendBlockId, usize> = HashMap::new();
            let mut block_envs: HashMap<BackendBlockId, HashMap<BackendIrVar, JitValue>> =
                HashMap::new();
            let mut block_module_envs: HashMap<BackendBlockId, HashMap<BackendIrVar, Identifier>> =
                HashMap::new();
            let mut block_function_envs: HashMap<
                BackendBlockId,
                HashMap<BackendIrVar, JitFunctionMeta>,
            > = HashMap::new();
            let ctx_val = builder.block_params(prelude)[0];
            let params = builder.block_params(prelude).to_vec();
            let args_ptr = if meta.call_abi.uses_array_args() {
                Some(params[1])
            } else {
                None
            };
            let captures_ptr = params[meta.call_abi.captures_param_index()];
            let capture_count = function.captures.len();
            let explicit_arity = function.params.len().saturating_sub(capture_count);
            let init_block = builder.create_block();
            if let Some(args_ptr) = args_ptr {
                let nargs = params[2];
                let want = builder.ins().iconst(PTR_TYPE, explicit_arity as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                let arity_fail = builder.create_block();
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                emit_return_null_tagged(&mut builder);
                builder.seal_block(arity_fail);

                builder.switch_to_block(init_block);
                let _ = args_ptr;
            } else {
                builder.ins().jump(init_block, &[]);
                builder.switch_to_block(init_block);
            }
            for (idx, param) in function.params.iter().take(capture_count).enumerate() {
                let cap_tag = builder.ins().load(
                    types::I64,
                    MemFlags::new(),
                    captures_ptr,
                    (idx * 16) as i32,
                );
                let cap_payload = builder.ins().load(
                    PTR_TYPE,
                    MemFlags::new(),
                    captures_ptr,
                    (idx * 16 + 8) as i32,
                );
                env.insert(
                    param.var,
                    JitValue::boxed(boxed_value_from_tagged_parts(
                        module,
                        helpers,
                        &mut builder,
                        ctx_val,
                        cap_tag,
                        cap_payload,
                    )),
                );
                module_env.remove(&param.var);
                function_env.remove(&param.var);
            }
            for (idx, param) in function.params.iter().skip(capture_count).enumerate() {
                let (tag, payload) = match args_ptr {
                    Some(args_ptr) => {
                        let tag = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            args_ptr,
                            (idx * 16) as i32,
                        );
                        let payload = builder.ins().load(
                            PTR_TYPE,
                            MemFlags::new(),
                            args_ptr,
                            (idx * 16 + 8) as i32,
                        );
                        (tag, payload)
                    }
                    None => {
                        let base = 1 + idx * 2;
                        (params[base], params[base + 1])
                    }
                };
                env.insert(
                    param.var,
                    JitValue::boxed(boxed_value_from_tagged_parts(
                        module,
                        helpers,
                        &mut builder,
                        ctx_val,
                        tag,
                        payload,
                    )),
                );
                module_env.remove(&param.var);
                function_env.remove(&param.var);
            }
            block_envs.insert(function.entry, env.clone());
            block_module_envs.insert(function.entry, module_env.clone());
            block_function_envs.insert(function.entry, function_env.clone());
            builder.ins().jump(entry, &[]);

            for block in &block_order {
                for instr in &block.instrs {
                    if let BackendIrInstr::HandleScope { body_result, .. } = instr {
                        let Some(cont_block) = function.blocks.iter().find(|candidate| {
                            candidate.params.iter().any(|p| p.var == *body_result)
                        }) else {
                            return Err("backend JIT path missing handle-scope continuation block"
                                .to_string());
                        };
                        *handler_pop_counts.entry(cont_block.id).or_insert(0) += 1;
                    }
                }
            }

            for block in &block_order {
                let cl_block = block_map[&block.id];
                builder.switch_to_block(cl_block);

                let mut env = block_envs.remove(&block.id).unwrap_or_default();
                let mut module_env = block_module_envs.remove(&block.id).unwrap_or_default();
                let mut function_env = block_function_envs.remove(&block.id).unwrap_or_default();

                if block.id != function.entry {
                    let block_params = builder.block_params(cl_block).to_vec();
                    for (idx, param) in block.params.iter().enumerate() {
                        env.insert(param.var, JitValue::boxed(block_params[idx]));
                        module_env.remove(&param.var);
                        function_env.remove(&param.var);
                    }
                }

                if let Some(pop_count) = handler_pop_counts.get(&block.id).copied() {
                    let rt_pop_handler =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_pop_handler");
                    for _ in 0..pop_count {
                        builder.ins().call(rt_pop_handler, &[ctx_val]);
                    }
                }

                for instr in &block.instrs {
                    match instr {
                        BackendIrInstr::Assign { dest, expr, .. } => {
                            let value = compile_simple_backend_ir_expr(
                                module,
                                helpers,
                                &mut builder,
                                ctx_val,
                                &env,
                                &module_env,
                                scope,
                                backend_function_metas,
                                backend_function_defs,
                                interner,
                                expr,
                            )?;
                            env.insert(*dest, value);
                            match expr {
                                BackendIrExpr::LoadName(name) => {
                                    if let Some(module_name) =
                                        resolve_module_name(scope, interner, *name)
                                    {
                                        module_env.insert(*dest, module_name);
                                    } else {
                                        module_env.remove(dest);
                                    }
                                    if let Some(meta) = scope.functions.get(name).copied() {
                                        function_env.insert(*dest, meta);
                                    } else {
                                        function_env.remove(dest);
                                    }
                                }
                                _ => {
                                    module_env.remove(dest);
                                    function_env.remove(dest);
                                }
                            }
                            if function.id == entry_function_id
                                && let Some(&global_idx) = global_binding_indices.get(dest)
                            {
                                let boxed = box_and_guard_jit_value(
                                    module,
                                    helpers,
                                    &mut builder,
                                    ctx_val,
                                    value,
                                );
                                let set_global = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_set_global",
                                );
                                let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
                            }
                        }
                        BackendIrInstr::Call {
                            dest,
                            target,
                            args,
                            metadata,
                        } => {
                            let arg_vals = args
                                .iter()
                                .map(|arg| {
                                    env.get(arg)
                                        .copied()
                                        .ok_or_else(|| format!("missing backend IR var {:?}", arg))
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            let value = match target {
                                IrCallTarget::Direct(function_id) => {
                                    let callee = backend_function_metas
                                        .get(function_id)
                                        .copied()
                                        .ok_or_else(|| {
                                        "missing direct backend callee metadata".to_string()
                                    })?;
                                    compile_jit_cfg_user_function_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?
                                }
                                IrCallTarget::Named(name) => {
                                    if let Some(callee) = scope.functions.get(name).copied() {
                                        compile_jit_cfg_user_function_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            callee,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(primop) =
                                        resolve_primop_call(interner.resolve(*name), arg_vals.len())
                                    {
                                        compile_jit_cfg_primop_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            primop,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                        let boxed_args = arg_vals
                                            .iter()
                                            .map(|value| {
                                                box_and_guard_jit_value(
                                                    module,
                                                    helpers,
                                                    &mut builder,
                                                    ctx_val,
                                                    *value,
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        compile_jit_cfg_base_function_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            base_idx,
                                            &boxed_args,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&global_idx) = scope.globals.get(name) {
                                        let get_global = get_helper_func_ref(
                                            module,
                                            helpers,
                                            &mut builder,
                                            "rt_get_global",
                                        );
                                        let idx_val =
                                            builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                        let call =
                                            builder.ins().call(get_global, &[ctx_val, idx_val]);
                                        let callee = JitValue::boxed(builder.inst_results(call)[0]);
                                        compile_jit_cfg_generic_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            callee,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&arity) = scope.adt_constructors.get(name) {
                                        if arity != arg_vals.len() {
                                            return Err(format!(
                                                "backend named constructor arity mismatch for {}",
                                                interner.resolve(*name)
                                            ));
                                        }
                                        compile_backend_named_adt_constructor_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            *name,
                                            &arg_vals,
                                            interner,
                                        )?
                                    } else {
                                        return Err(format!(
                                            "missing named backend callee metadata for {}",
                                            interner.resolve(*name)
                                        ));
                                    }
                                }
                                IrCallTarget::Var(var) => {
                                    let callee = env.get(var).copied().ok_or_else(|| {
                                        format!("missing backend indirect callee var {:?}", var)
                                    })?;
                                    compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?
                                }
                            };
                            env.insert(*dest, value);
                            module_env.remove(dest);
                            function_env.remove(dest);
                        }
                        BackendIrInstr::HandleScope { effect, arms, .. } => {
                            let mut op_sym_vals = Vec::with_capacity(arms.len());
                            let mut closure_vals = Vec::with_capacity(arms.len());
                            for arm in arms {
                                op_sym_vals.push(
                                    builder
                                        .ins()
                                        .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
                                );
                                let arm_fn = backend_function_defs
                                    .get(&arm.function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                    "missing backend handle arm function definition".to_string()
                                })?;
                                let meta = backend_function_metas
                                    .get(&arm.function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                        "missing backend handle arm metadata".to_string()
                                    })?;
                                if arm_fn.captures.len() != arm.capture_vars.len() {
                                    return Err(
                                        "backend JIT path requires explicit handle-arm capture metadata"
                                            .to_string(),
                                    );
                                }
                                let capture_vals = arm
                                    .capture_vars
                                    .iter()
                                    .map(|var| {
                                        env.get(var).copied().ok_or_else(|| {
                                            format!(
                                                "missing backend handle-arm capture var {:?}",
                                                var
                                            )
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                let (_, captures_ptr) =
                                    emit_tagged_stack_array(&mut builder, &capture_vals);
                                let ncaptures =
                                    builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
                                let make_closure = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_make_jit_closure",
                                );
                                let fn_idx =
                                    builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                                let call = builder.ins().call(
                                    make_closure,
                                    &[ctx_val, fn_idx, captures_ptr, ncaptures],
                                );
                                closure_vals.push(builder.inst_results(call)[0]);
                            }
                            let ops_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                                (op_sym_vals.len().max(1) as u32) * 8,
                                3,
                            ));
                            for (i, op) in op_sym_vals.iter().enumerate() {
                                builder.ins().stack_store(*op, ops_slot, (i * 8) as i32);
                            }
                            let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);
                            let (_, closures_ptr) =
                                emit_boxed_stack_array(&mut builder, &closure_vals);
                            let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
                            let narms_val = builder.ins().iconst(PTR_TYPE, arms.len() as i64);
                            let rt_push_handler = get_helper_func_ref(
                                module,
                                helpers,
                                &mut builder,
                                "rt_push_handler",
                            );
                            builder.ins().call(
                                rt_push_handler,
                                &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
                            );
                        }
                    }
                }

                match &block.terminator {
                    BackendIrTerminator::Return(var, _) => {
                        let value = env
                            .get(var)
                            .copied()
                            .ok_or_else(|| format!("missing backend return var {:?}", var))?;
                        let value_ptr =
                            box_and_guard_jit_value(module, helpers, &mut builder, ctx_val, value);
                        let result_ptr = if meta.has_contract {
                            let fn_index =
                                builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                            let zero = builder.ins().iconst(PTR_TYPE, 0);
                            let check_ret = get_helper_func_ref(
                                module,
                                helpers,
                                &mut builder,
                                "rt_check_jit_contract_return",
                            );
                            let checked_ret_call = builder.ins().call(
                                check_ret,
                                &[ctx_val, fn_index, value_ptr, zero, zero, zero, zero],
                            );
                            let checked_ret = builder.inst_results(checked_ret_call)[0];
                            emit_return_on_null_value(&mut builder, checked_ret);
                            checked_ret
                        } else {
                            value_ptr
                        };
                        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                        builder.ins().return_(&[tag, result_ptr]);
                    }
                    BackendIrTerminator::Jump(target, args, _) => {
                        let target_block = block_map[target];
                        if let Some(target_def) = block_defs.get(target).copied() {
                            let target_env = block_envs.entry(*target).or_default();
                            target_env.extend(env.iter().map(|(var, value)| (*var, *value)));
                            let target_module_env = block_module_envs.entry(*target).or_default();
                            target_module_env.extend(
                                module_env
                                    .iter()
                                    .map(|(var, module_name)| (*var, *module_name)),
                            );
                            let target_function_env =
                                block_function_envs.entry(*target).or_default();
                            target_function_env
                                .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                            for (param, arg) in target_def.params.iter().zip(args.iter()) {
                                if let Some(value) = env.get(arg).copied() {
                                    target_env.insert(param.var, value);
                                }
                                if let Some(module_name) = module_env.get(arg).copied() {
                                    target_module_env.insert(param.var, module_name);
                                } else {
                                    target_module_env.remove(&param.var);
                                }
                                if let Some(meta) = function_env.get(arg).copied() {
                                    target_function_env.insert(param.var, meta);
                                } else {
                                    target_function_env.remove(&param.var);
                                }
                            }
                        }
                        let block_args = args
                            .iter()
                            .map(|arg| {
                                env.get(arg)
                                    .copied()
                                    .ok_or_else(|| format!("missing backend jump var {:?}", arg))
                                    .map(|value| {
                                        BlockArg::Value(box_and_guard_jit_value(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            value,
                                        ))
                                    })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        builder.ins().jump(target_block, &block_args);
                    }
                    BackendIrTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                        ..
                    } => {
                        block_envs
                            .entry(*then_block)
                            .or_default()
                            .extend(env.iter().map(|(var, value)| (*var, *value)));
                        block_module_envs.entry(*then_block).or_default().extend(
                            module_env
                                .iter()
                                .map(|(var, module_name)| (*var, *module_name)),
                        );
                        block_function_envs
                            .entry(*then_block)
                            .or_default()
                            .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                        block_envs
                            .entry(*else_block)
                            .or_default()
                            .extend(env.iter().map(|(var, value)| (*var, *value)));
                        block_module_envs.entry(*else_block).or_default().extend(
                            module_env
                                .iter()
                                .map(|(var, module_name)| (*var, *module_name)),
                        );
                        block_function_envs
                            .entry(*else_block)
                            .or_default()
                            .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                        let cond_value = env
                            .get(cond)
                            .copied()
                            .ok_or_else(|| format!("missing backend branch var {:?}", cond))?;
                        let cond_bool = compile_simple_backend_ir_truthiness_condition(
                            module,
                            helpers,
                            &mut builder,
                            ctx_val,
                            cond_value,
                        );
                        builder.ins().brif(
                            cond_bool,
                            block_map[then_block],
                            &[],
                            block_map[else_block],
                            &[],
                        );
                    }
                    BackendIrTerminator::TailCall {
                        callee,
                        args,
                        metadata,
                    } => {
                        let arg_vals = args
                            .iter()
                            .map(|arg| {
                                env.get(arg).copied().ok_or_else(|| {
                                    format!("missing backend tailcall var {:?}", arg)
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        match callee {
                            IrCallTarget::Direct(function_id) => {
                                let callee = backend_function_metas
                                    .get(function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                        "missing direct backend tail callee metadata".to_string()
                                    })?;
                                emit_jit_cfg_user_function_tailcall(
                                    module,
                                    helpers,
                                    &mut builder,
                                    ctx_val,
                                    callee,
                                    &arg_vals,
                                );
                            }
                            IrCallTarget::Named(name) => {
                                if let Some(callee) = scope.functions.get(name).copied() {
                                    emit_jit_cfg_user_function_tailcall(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                    );
                                } else if let Some(primop) =
                                    resolve_primop_call(interner.resolve(*name), arg_vals.len())
                                {
                                    let value = compile_jit_cfg_primop_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        primop,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                    let boxed_args = arg_vals
                                        .iter()
                                        .map(|value| {
                                            box_and_guard_jit_value(
                                                module,
                                                helpers,
                                                &mut builder,
                                                ctx_val,
                                                *value,
                                            )
                                        })
                                        .collect::<Vec<_>>();
                                    let value = compile_jit_cfg_base_function_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        base_idx,
                                        &boxed_args,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&global_idx) = scope.globals.get(name) {
                                    let get_global = get_helper_func_ref(
                                        module,
                                        helpers,
                                        &mut builder,
                                        "rt_get_global",
                                    );
                                    let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                    let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                                    let callee = JitValue::boxed(builder.inst_results(call)[0]);
                                    let value = compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&arity) = scope.adt_constructors.get(name) {
                                    if arity != arg_vals.len() {
                                        return Err(format!(
                                            "backend named constructor arity mismatch for {}",
                                            interner.resolve(*name)
                                        ));
                                    }
                                    let value = compile_backend_named_adt_constructor_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        *name,
                                        &arg_vals,
                                        interner,
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else {
                                    return Err(format!(
                                        "missing named backend tail callee metadata for {}",
                                        interner.resolve(*name)
                                    ));
                                }
                            }
                            IrCallTarget::Var(var) => {
                                if let Some(callee) = function_env.get(var).copied() {
                                    emit_jit_cfg_user_function_tailcall(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                    );
                                } else {
                                    let callee = env.get(var).copied().ok_or_else(|| {
                                        format!(
                                            "missing backend indirect tail callee var {:?}",
                                            var
                                        )
                                    })?;
                                    let value = compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                }
                            }
                        }
                    }
                    BackendIrTerminator::Unreachable(_) => {
                        builder.ins().trap(TrapCode::INTEGER_OVERFLOW);
                    }
                }
            }

            builder.seal_all_blocks();
            builder.finalize();
        }
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(meta.id, &mut ctx)
            .map_err(|e| format!("define backend function: {e:?}"))?;
        Ok(())
    }

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

fn backend_ir_jit_support_error(ir_program: &IrProgram, interner: &Interner) -> Option<String> {
    if let Some(reason) = backend_ir_top_level_support_error(ir_program.top_level_items()) {
        return Some(reason);
    }

    let mut named_functions: HashSet<_> = ir_program
        .functions()
        .iter()
        .filter_map(|function| function.name)
        .collect();
    collect_backend_top_level_function_names(ir_program.top_level_items(), &mut named_functions);
    let global_names: HashSet<_> = ir_program.globals().iter().copied().collect();
    let mut imported_modules = HashSet::new();
    let mut import_aliases = HashMap::new();
    let mut adt_constructors = HashMap::new();
    collect_backend_top_level_declaration_metadata(
        ir_program.top_level_items(),
        &mut imported_modules,
        &mut import_aliases,
        &mut adt_constructors,
    );

    for function in ir_program.functions() {
        let Some(entry_block) = function
            .blocks
            .iter()
            .find(|block| block.id == function.entry)
        else {
            return Some("backend_ir JIT shape is missing a function entry block".to_string());
        };
        if !entry_block.params.is_empty() {
            return Some("backend_ir JIT shape has entry block parameters".to_string());
        }
        for block in &function.blocks {
            for instr in &block.instrs {
                match instr {
                    BackendIrInstr::Assign { expr, .. } => {
                        match expr {
                            BackendIrExpr::Const(_)
                            | BackendIrExpr::Var(_)
                            | BackendIrExpr::InterpolatedString(_)
                            | BackendIrExpr::Prefix { .. }
                            | BackendIrExpr::MakeTuple(_)
                            | BackendIrExpr::MakeArray(_)
                            | BackendIrExpr::MakeHash(_)
                            | BackendIrExpr::MakeList(_)
                            | BackendIrExpr::MakeAdt(_, _)
                            | BackendIrExpr::MakeClosure(_, _)
                            | BackendIrExpr::EmptyList
                            | BackendIrExpr::Index { .. }
                            | BackendIrExpr::MemberAccess { .. }
                            | BackendIrExpr::TupleFieldAccess { .. }
                            | BackendIrExpr::TupleArityTest { .. }
                            | BackendIrExpr::TagTest { .. }
                            | BackendIrExpr::TagPayload { .. }
                            | BackendIrExpr::ListTest { .. }
                            | BackendIrExpr::ListHead { .. }
                            | BackendIrExpr::ListTail { .. }
                            | BackendIrExpr::AdtTagTest { .. }
                            | BackendIrExpr::AdtField { .. }
                            | BackendIrExpr::None => {}
                            BackendIrExpr::Some(_)
                            | BackendIrExpr::Left(_)
                            | BackendIrExpr::Right(_)
                            | BackendIrExpr::Cons { .. }
                            | BackendIrExpr::Perform { .. } => {}
                            BackendIrExpr::LoadName(name) => {
                                let is_supported_load = named_functions.contains(name)
                                    || global_names.contains(name)
                                    || adt_constructors.get(name).copied() == Some(0)
                                    || resolve_backend_module_name(
                                        &imported_modules,
                                        &import_aliases,
                                        interner,
                                        *name,
                                    )
                                    .is_some()
                                    || crate::runtime::base::get_base_function_index(
                                        interner.resolve(*name),
                                    )
                                    .is_some();
                                if !is_supported_load {
                                    return Some(
                                    "backend_ir JIT shape has an unresolved non-function LoadName"
                                        .to_string(),
                                );
                                }
                            }
                            BackendIrExpr::Binary(op, _, _) => match op {
                                IrBinaryOp::Add
                                | IrBinaryOp::IAdd
                                | IrBinaryOp::Sub
                                | IrBinaryOp::ISub
                                | IrBinaryOp::Mul
                                | IrBinaryOp::IMul
                                | IrBinaryOp::Div
                                | IrBinaryOp::IDiv
                                | IrBinaryOp::Mod
                                | IrBinaryOp::IMod
                                | IrBinaryOp::Eq
                                | IrBinaryOp::NotEq
                                | IrBinaryOp::Gt
                                | IrBinaryOp::Ge
                                | IrBinaryOp::Le
                                | IrBinaryOp::Lt
                                | IrBinaryOp::FAdd
                                | IrBinaryOp::FSub
                                | IrBinaryOp::FMul
                                | IrBinaryOp::FDiv => {}
                                IrBinaryOp::And | IrBinaryOp::Or => {}
                            },
                            _ => return Some(
                                "backend_ir JIT shape contains an unsupported backend expression"
                                    .to_string(),
                            ),
                        }
                    }
                    BackendIrInstr::Call { target, args, .. } => {
                        if let IrCallTarget::Named(name) = target {
                            let is_supported_target = named_functions.contains(name)
                                || global_names.contains(name)
                                || adt_constructors.contains_key(name)
                                || resolve_primop_call(interner.resolve(*name), args.len())
                                    .is_some()
                                || crate::runtime::base::get_base_function_index(
                                    interner.resolve(*name),
                                )
                                .is_some();
                            if !is_supported_target {
                                return Some(format!(
                                    "backend_ir JIT shape has an unresolved named call target {}",
                                    interner.resolve(*name)
                                ));
                            }
                        }
                    }
                    BackendIrInstr::HandleScope {
                        body_entry,
                        body_result,
                        arms,
                        ..
                    } => {
                        if !function.blocks.iter().any(|b| b.id == *body_entry) {
                            return Some(
                                "backend_ir JIT shape references a missing handle-scope body entry block"
                                    .to_string(),
                            );
                        }
                        if !function
                            .blocks
                            .iter()
                            .any(|b| b.params.iter().any(|p| p.var == *body_result))
                        {
                            return Some(
                                "backend_ir JIT shape is missing handle-scope continuation block parameters"
                                    .to_string(),
                            );
                        }
                        for arm in arms {
                            let Some(arm_fn) = ir_program
                                .functions()
                                .iter()
                                .find(|f| f.id == arm.function_id)
                            else {
                                return Some(
                                    "backend_ir JIT shape is missing a handle arm function"
                                        .to_string(),
                                );
                            };
                            if arm_fn.captures.len() != arm.capture_vars.len() {
                                return Some(
                                    "backend_ir JIT shape has inconsistent handle-arm capture metadata"
                                        .to_string(),
                                );
                            }
                        }
                    }
                }
            }
            match &block.terminator {
                BackendIrTerminator::Return(_, _)
                | BackendIrTerminator::Jump(_, _, _)
                | BackendIrTerminator::Branch { .. }
                | BackendIrTerminator::Unreachable(_) => {}
                BackendIrTerminator::TailCall { .. } => {}
            }
        }
    }

    None
}

fn backend_ir_top_level_support_error(
    items: &[crate::backend_ir::IrTopLevelItem],
) -> Option<String> {
    for item in items {
        match item {
            crate::backend_ir::IrTopLevelItem::Function { function_id, .. } => {
                if function_id.is_none() {
                    return Some(
                        "backend_ir JIT shape has a top-level function without a backend function id"
                            .to_string(),
                    );
                }
            }
            crate::backend_ir::IrTopLevelItem::Module { body, .. } => {
                if let Some(reason) = backend_ir_top_level_support_error(body) {
                    return Some(reason);
                }
            }
            crate::backend_ir::IrTopLevelItem::Import { .. }
            | crate::backend_ir::IrTopLevelItem::Data { .. }
            | crate::backend_ir::IrTopLevelItem::EffectDecl { .. }
            | crate::backend_ir::IrTopLevelItem::Let { .. }
            | crate::backend_ir::IrTopLevelItem::LetDestructure { .. }
            | crate::backend_ir::IrTopLevelItem::Return { .. }
            | crate::backend_ir::IrTopLevelItem::Expression { .. }
            | crate::backend_ir::IrTopLevelItem::Assign { .. } => {}
        }
    }

    None
}

fn resolve_backend_module_name(
    imported_modules: &HashSet<Identifier>,
    import_aliases: &HashMap<Identifier, Identifier>,
    interner: &Interner,
    name: Identifier,
) -> Option<Identifier> {
    import_aliases.get(&name).copied().or_else(|| {
        if imported_modules.contains(&name) || interner.resolve(name) == "Base" {
            Some(name)
        } else {
            None
        }
    })
}

fn ordered_backend_blocks(function: &BackendIrFunction) -> Vec<&crate::backend_ir::IrBlock> {
    let block_defs: HashMap<BackendBlockId, &crate::backend_ir::IrBlock> = function
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect();
    let mut ordered = Vec::with_capacity(function.blocks.len());
    let mut seen = HashSet::new();
    let mut stack = vec![function.entry];

    while let Some(block_id) = stack.pop() {
        if !seen.insert(block_id) {
            continue;
        }
        let Some(block) = block_defs.get(&block_id).copied() else {
            continue;
        };
        ordered.push(block);
        for succ in backend_terminator_successors(&block.terminator)
            .into_iter()
            .rev()
        {
            stack.push(succ);
        }
    }

    for block in &function.blocks {
        if seen.insert(block.id) {
            ordered.push(block);
        }
    }

    ordered
}

fn backend_terminator_successors(terminator: &BackendIrTerminator) -> Vec<BackendBlockId> {
    let mut succs = Vec::with_capacity(2);
    match terminator {
        BackendIrTerminator::Jump(target, ..) => succs.push(*target),
        BackendIrTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            succs.push(*then_block);
            succs.push(*else_block);
        }
        BackendIrTerminator::Return(..)
        | BackendIrTerminator::TailCall { .. }
        | BackendIrTerminator::Unreachable(..) => {}
    }
    succs
}

fn collect_backend_top_level_declaration_metadata(
    items: &[crate::backend_ir::IrTopLevelItem],
    imported_modules: &mut HashSet<Identifier>,
    import_aliases: &mut HashMap<Identifier, Identifier>,
    adt_constructors: &mut HashMap<Identifier, usize>,
) {
    for item in items {
        match item {
            crate::backend_ir::IrTopLevelItem::Function { .. } => {}
            crate::backend_ir::IrTopLevelItem::Module { name, body, .. } => {
                imported_modules.insert(*name);
                collect_backend_top_level_declaration_metadata(
                    body,
                    imported_modules,
                    import_aliases,
                    adt_constructors,
                );
            }
            crate::backend_ir::IrTopLevelItem::Import { name, alias, .. } => {
                imported_modules.insert(*name);
                if let Some(alias) = alias {
                    import_aliases.insert(*alias, *name);
                }
            }
            crate::backend_ir::IrTopLevelItem::Data { name, variants, .. } => {
                for variant in variants {
                    adt_constructors.insert(variant.name, variant.fields.len());
                }
                let _ = name;
            }
            crate::backend_ir::IrTopLevelItem::EffectDecl { .. }
            | crate::backend_ir::IrTopLevelItem::Let { .. }
            | crate::backend_ir::IrTopLevelItem::LetDestructure { .. }
            | crate::backend_ir::IrTopLevelItem::Return { .. }
            | crate::backend_ir::IrTopLevelItem::Expression { .. }
            | crate::backend_ir::IrTopLevelItem::Assign { .. } => {}
        }
    }
}

fn register_backend_top_level_module_functions(
    items: &[crate::backend_ir::IrTopLevelItem],
    current_module: Option<Identifier>,
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    scope: &mut Scope,
) {
    for item in items {
        match item {
            crate::backend_ir::IrTopLevelItem::Function {
                name, function_id, ..
            } => {
                if let (Some(module_name), Some(function_id)) = (current_module, function_id)
                    && let Some(meta) = backend_function_metas.get(function_id).copied()
                {
                    scope.module_functions.insert((module_name, *name), meta);
                }
            }
            crate::backend_ir::IrTopLevelItem::Module { name, body, .. } => {
                register_backend_top_level_module_functions(
                    body,
                    Some(*name),
                    backend_function_metas,
                    scope,
                );
            }
            crate::backend_ir::IrTopLevelItem::Data { name, variants, .. } => {
                for variant in variants {
                    scope.adt_constructor_owner.insert(variant.name, *name);
                }
            }
            crate::backend_ir::IrTopLevelItem::Import { .. }
            | crate::backend_ir::IrTopLevelItem::EffectDecl { .. }
            | crate::backend_ir::IrTopLevelItem::Let { .. }
            | crate::backend_ir::IrTopLevelItem::LetDestructure { .. }
            | crate::backend_ir::IrTopLevelItem::Return { .. }
            | crate::backend_ir::IrTopLevelItem::Expression { .. }
            | crate::backend_ir::IrTopLevelItem::Assign { .. } => {}
        }
    }
}

fn collect_backend_top_level_function_names(
    items: &[crate::backend_ir::IrTopLevelItem],
    names: &mut HashSet<Identifier>,
) {
    for item in items {
        match item {
            crate::backend_ir::IrTopLevelItem::Function {
                name,
                function_id: Some(_),
                ..
            } => {
                names.insert(*name);
            }
            crate::backend_ir::IrTopLevelItem::Module { body, .. } => {
                collect_backend_top_level_function_names(body, names);
            }
            _ => {}
        }
    }
}

fn register_backend_top_level_named_functions(
    items: &[crate::backend_ir::IrTopLevelItem],
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    scope: &mut Scope,
) {
    for item in items {
        match item {
            crate::backend_ir::IrTopLevelItem::Function {
                name,
                function_id: Some(function_id),
                ..
            } => {
                if let Some(meta) = backend_function_metas.get(function_id).copied() {
                    scope.functions.insert(*name, meta);
                }
            }
            crate::backend_ir::IrTopLevelItem::Module { body, .. } => {
                register_backend_top_level_named_functions(body, backend_function_metas, scope);
            }
            _ => {}
        }
    }
}

fn compile_simple_backend_ir_expr(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    env: &HashMap<BackendIrVar, JitValue>,
    module_env: &HashMap<BackendIrVar, Identifier>,
    scope: &Scope,
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    _backend_function_defs: &HashMap<BackendFunctionId, &BackendIrFunction>,
    interner: &Interner,
    expr: &BackendIrExpr,
) -> Result<JitValue, String> {
    match expr {
        BackendIrExpr::Const(IrConst::Int(value)) => {
            Ok(JitValue::int(builder.ins().iconst(types::I64, *value)))
        }
        BackendIrExpr::Const(IrConst::Float(value)) => Ok(JitValue::float(
            builder.ins().iconst(types::I64, value.to_bits() as i64),
        )),
        BackendIrExpr::Const(IrConst::Bool(value)) => Ok(JitValue::bool(
            builder.ins().iconst(types::I64, *value as i64),
        )),
        BackendIrExpr::Const(IrConst::String(value)) => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Const(IrConst::Unit) | BackendIrExpr::None => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        BackendIrExpr::InterpolatedString(parts) => {
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_string_concat =
                get_helper_func_ref(module, helpers, builder, "rt_string_concat");
            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    crate::backend_ir::IrStringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = DataDescription::new();
                        desc.define(bytes.to_vec().into_boxed_slice());
                        module.define_data(data, &desc).map_err(|e| e.to_string())?;
                        let gv = module.declare_data_in_func(data, builder.func);
                        let ptr = builder.ins().global_value(PTR_TYPE, gv);
                        let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
                        let make_string =
                            get_helper_func_ref(module, helpers, builder, "rt_make_string");
                        let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
                        builder.inst_results(call)[0]
                    }
                    crate::backend_ir::IrStringPart::Interpolation(var) => {
                        let val = env.get(var).copied().ok_or_else(|| {
                            format!("missing backend IR interpolation var {:?}", var)
                        })?;
                        let val = box_jit_value(module, helpers, builder, ctx_val, val);
                        let call = builder.ins().call(rt_to_string, &[ctx_val, val]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let call = builder
                            .ins()
                            .call(rt_string_concat, &[ctx_val, prev, part_val]);
                        builder.inst_results(call)[0]
                    }
                });
            }
            match acc {
                Some(val) => Ok(JitValue::boxed(val)),
                None => {
                    let make_string =
                        get_helper_func_ref(module, helpers, builder, "rt_make_string");
                    let null = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder.ins().call(make_string, &[ctx_val, null, zero]);
                    Ok(JitValue::boxed(builder.inst_results(call)[0]))
                }
            }
        }
        BackendIrExpr::Prefix { operator, right } => {
            let operand = env
                .get(right)
                .copied()
                .ok_or_else(|| format!("missing backend IR prefix var {:?}", right))?;
            let (tag, payload) = jit_value_to_tag_payload(builder, operand);
            let helper = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unsupported backend prefix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(func_ref, &[ctx_val, tag, payload]);
            let result = boxed_value_from_tagged_parts(
                module,
                helpers,
                builder,
                ctx_val,
                builder.inst_results(call)[0],
                builder.inst_results(call)[1],
            );
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::Var(var) => env
            .get(var)
            .copied()
            .ok_or_else(|| format!("missing backend IR var {:?}", var)),
        BackendIrExpr::MakeTuple(vars) => {
            let vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR tuple var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let elems_ptr = emit_tagged_stack_array(builder, &vals).1;
            let len_val = builder.ins().iconst(PTR_TYPE, vals.len() as i64);
            let make_tuple = get_helper_func_ref(module, helpers, builder, "rt_make_tuple");
            let call = builder
                .ins()
                .call(make_tuple, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeArray(vars) => {
            let vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR array var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let elems_ptr = emit_tagged_stack_array(builder, &vals).1;
            let len_val = builder.ins().iconst(PTR_TYPE, vals.len() as i64);
            let make_array = get_helper_func_ref(module, helpers, builder, "rt_make_array");
            let call = builder
                .ins()
                .call(make_array, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeHash(pairs) => {
            let mut pair_vals = Vec::with_capacity(pairs.len() * 2);
            for (k, v) in pairs {
                pair_vals.push(
                    env.get(k)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR hash key var {:?}", k))?,
                );
                pair_vals.push(
                    env.get(v)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR hash value var {:?}", v))?,
                );
            }
            let pairs_ptr = emit_tagged_stack_array(builder, &pair_vals).1;
            let npairs_val = builder.ins().iconst(PTR_TYPE, pairs.len() as i64);
            let make_hash = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder
                .ins()
                .call(make_hash, &[ctx_val, pairs_ptr, npairs_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeList(vars) => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let none_call = builder.ins().call(make_none, &[ctx_val]);
            let mut acc = builder.inst_results(none_call)[1];
            for var in vars.iter().rev() {
                let val = env
                    .get(var)
                    .copied()
                    .ok_or_else(|| format!("missing backend IR list var {:?}", var))?;
                let val = box_jit_value(module, helpers, builder, ctx_val, val);
                let cons_call = builder.ins().call(make_cons, &[ctx_val, val, acc]);
                acc = builder.inst_results(cons_call)[0];
            }
            Ok(JitValue::boxed(acc))
        }
        BackendIrExpr::MakeAdt(constructor, vars) => {
            let name_str = interner.resolve(*constructor);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let global_value = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let boxed_vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR adt var {:?}", var))
                        .map(|v| box_jit_value(module, helpers, builder, ctx_val, v))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let call = match boxed_vals.len() {
                1 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
                    builder
                        .ins()
                        .call(helper, &[ctx_val, name_ptr, name_len, boxed_vals[0]])
                }
                2 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
                    builder.ins().call(
                        helper,
                        &[ctx_val, name_ptr, name_len, boxed_vals[0], boxed_vals[1]],
                    )
                }
                3 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                        ],
                    )
                }
                4 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                            boxed_vals[3],
                        ],
                    )
                }
                5 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                            boxed_vals[3],
                            boxed_vals[4],
                        ],
                    )
                }
                _ => {
                    let vals = vars
                        .iter()
                        .map(|var| {
                            env.get(var)
                                .copied()
                                .ok_or_else(|| format!("missing backend IR adt var {:?}", var))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let fields_ptr = emit_tagged_stack_array(builder, &vals).1;
                    let arity_val = builder.ins().iconst(PTR_TYPE, vars.len() as i64);
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
                    builder.ins().call(
                        helper,
                        &[ctx_val, name_ptr, name_len, fields_ptr, arity_val],
                    )
                }
            };
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeClosure(function_id, capture_vars) => {
            let meta = backend_function_metas
                .get(function_id)
                .copied()
                .ok_or_else(|| format!("missing backend closure metadata for {:?}", function_id))?;
            let capture_vals = capture_vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR capture var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (_slot, captures_ptr) = emit_tagged_stack_array(builder, &capture_vals);
            let ncaptures = builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
            let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
            let make_jit_closure =
                get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let call = builder.ins().call(
                make_jit_closure,
                &[ctx_val, fn_idx, captures_ptr, ncaptures],
            );
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Binary(op, lhs, rhs) => {
            let lhs = env
                .get(lhs)
                .copied()
                .ok_or_else(|| format!("missing backend IR lhs var {:?}", lhs))?;
            let rhs = env
                .get(rhs)
                .copied()
                .ok_or_else(|| format!("missing backend IR rhs var {:?}", rhs))?;
            compile_simple_backend_ir_binary(module, helpers, builder, ctx_val, *op, lhs, rhs)
        }
        BackendIrExpr::LoadName(name) => {
            if let Some(meta) = scope.functions.get(name).copied() {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&global_idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base =
                    get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if scope.adt_constructors.get(name).copied() == Some(0) {
                compile_backend_named_adt_constructor_call(
                    module,
                    helpers,
                    builder,
                    ctx_val,
                    *name,
                    &[],
                    interner,
                )
            } else if resolve_module_name(scope, interner, *name).is_some() {
                let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                let call = builder.ins().call(make_none, &[ctx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[1]))
            } else {
                Err("backend JIT path does not yet support non-function LoadName".to_string())
            }
        }
        BackendIrExpr::EmptyList => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        BackendIrExpr::Index { left, index } => {
            let left_val = env
                .get(left)
                .copied()
                .ok_or_else(|| format!("missing backend IR index left var {:?}", left))?;
            let index_val = env
                .get(index)
                .copied()
                .ok_or_else(|| format!("missing backend IR index right var {:?}", index))?;
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let left_val = box_jit_value(module, helpers, builder, ctx_val, left_val);
            let index_val = box_jit_value(module, helpers, builder, ctx_val, index_val);
            let call = builder
                .ins()
                .call(rt_index, &[ctx_val, left_val, index_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MemberAccess { object, member, .. } => {
            if let Some(module_name) = module_env.get(object).copied() {
                if let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied() {
                    let make_jit_closure =
                        get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder
                        .ins()
                        .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }
                if interner.resolve(module_name) == "Base"
                    && let Some(base_idx) =
                        crate::runtime::base::get_base_function_index(interner.resolve(*member))
                {
                    let make_base =
                        get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                    let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }
                return Err(format!(
                    "unknown module member: {}.{}",
                    interner.resolve(module_name),
                    interner.resolve(*member)
                ));
            }
            let object_val = env
                .get(object)
                .copied()
                .ok_or_else(|| format!("missing backend IR member object var {:?}", object))?;
            let member_name = interner.resolve(*member);
            let bytes = member_name.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let member_call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            let member_val = builder.inst_results(member_call)[0];
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let object_val = box_jit_value(module, helpers, builder, ctx_val, object_val);
            let index_call = builder
                .ins()
                .call(rt_index, &[ctx_val, object_val, member_val]);
            let indexed = builder.inst_results(index_call)[0];
            emit_return_on_null_value(builder, indexed);
            let unwrap_some = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let unwrap_call = builder.ins().call(unwrap_some, &[ctx_val, indexed]);
            let result = builder.inst_results(unwrap_call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::TupleFieldAccess { object, index } => {
            let tuple_val = env
                .get(object)
                .copied()
                .ok_or_else(|| format!("missing backend IR tuple object var {:?}", object))?;
            let index_val = builder.ins().iconst(PTR_TYPE, *index as i64);
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            let tuple_val = box_jit_value(module, helpers, builder, ctx_val, tuple_val);
            let call = builder
                .ins()
                .call(tuple_get, &[ctx_val, tuple_val, index_val]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::TupleArityTest { value, arity } => {
            let tuple_val = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tuple-arity var {:?}", value))?;
            let tuple_val = box_jit_value(module, helpers, builder, ctx_val, tuple_val);
            let arity_val = builder.ins().iconst(PTR_TYPE, *arity as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_tuple_len_eq");
            let call = builder.ins().call(helper, &[ctx_val, tuple_val, arity_val]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::TagTest { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tag-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::backend_ir::IrTagTest::None => "rt_is_none",
                crate::backend_ir::IrTagTest::Some => "rt_is_some",
                crate::backend_ir::IrTagTest::Left => "rt_is_left",
                crate::backend_ir::IrTagTest::Right => "rt_is_right",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::TagPayload { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tag-payload var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::backend_ir::IrTagTest::None => {
                    return Err("backend JIT path cannot extract payload from None".to_string());
                }
                crate::backend_ir::IrTagTest::Some => "rt_unwrap_some",
                crate::backend_ir::IrTagTest::Left => "rt_unwrap_left",
                crate::backend_ir::IrTagTest::Right => "rt_unwrap_right",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::ListTest { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::backend_ir::IrListTest::Empty => "rt_is_empty_list",
                crate::backend_ir::IrListTest::Cons => "rt_is_cons",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ListHead { value } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-head var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let call = builder.ins().call(helper, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::ListTail { value } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-tail var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let call = builder.ins().call(helper, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::AdtTagTest { value, constructor } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR adt-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let name_str = interner.resolve(*constructor);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder.ins().call(helper, &[ctx_val, boxed, ptr, len]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::AdtField { value, index } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR adt-field var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let idx_val = builder.ins().iconst(PTR_TYPE, *index as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_adt_field_or_none");
            let call = builder.ins().call(helper, &[ctx_val, boxed, idx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Some(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR some var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Left(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR left var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Right(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR right var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Cons { head, tail } => {
            let head = env
                .get(head)
                .copied()
                .ok_or_else(|| format!("missing backend IR cons head var {:?}", head))?;
            let tail = env
                .get(tail)
                .copied()
                .ok_or_else(|| format!("missing backend IR cons tail var {:?}", tail))?;
            let head = box_jit_value(module, helpers, builder, ctx_val, head);
            let tail = box_jit_value(module, helpers, builder, ctx_val, tail);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder.ins().call(helper, &[ctx_val, head, tail]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Perform {
            effect,
            operation,
            args,
        } => {
            let arg_vals = args
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR perform arg {:?}", var))
                        .map(|v| box_and_guard_jit_value(module, helpers, builder, ctx_val, v))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                (arg_vals.len().max(1) as u32) * 8,
                3,
            ));
            for (i, val) in arg_vals.iter().enumerate() {
                builder.ins().stack_store(*val, slot, (i * 8) as i32);
            }
            let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
            let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
            let op_val = builder.ins().iconst(PTR_TYPE, operation.as_u32() as i64);
            let effect_name = interner.resolve(*effect);
            let op_name = interner.resolve(*operation);
            let effect_data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut effect_desc = DataDescription::new();
            effect_desc.define(effect_name.as_bytes().to_vec().into_boxed_slice());
            module
                .define_data(effect_data, &effect_desc)
                .map_err(|e| e.to_string())?;
            let op_data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut op_desc = DataDescription::new();
            op_desc.define(op_name.as_bytes().to_vec().into_boxed_slice());
            module
                .define_data(op_data, &op_desc)
                .map_err(|e| e.to_string())?;
            let effect_gv = module.declare_data_in_func(effect_data, builder.func);
            let effect_ptr = builder.ins().global_value(PTR_TYPE, effect_gv);
            let effect_len = builder.ins().iconst(PTR_TYPE, effect_name.len() as i64);
            let op_gv = module.declare_data_in_func(op_data, builder.func);
            let op_ptr = builder.ins().global_value(PTR_TYPE, op_gv);
            let op_len = builder.ins().iconst(PTR_TYPE, op_name.len() as i64);
            let zero = builder.ins().iconst(PTR_TYPE, 0);
            let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
            let call = builder.ins().call(
                rt_perform,
                &[
                    ctx_val, effect_val, op_val, args_ptr, nargs_val, effect_ptr, effect_len,
                    op_ptr, op_len, zero, zero,
                ],
            );
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        _ => Err("unsupported backend IR expression in direct JIT path".to_string()),
    }
}

fn compile_backend_named_adt_constructor_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    constructor_name: Identifier,
    arg_vals: &[JitValue],
    interner: &Interner,
) -> Result<JitValue, String> {
    let name_str = interner.resolve(constructor_name);
    let bytes = name_str.as_bytes().to_vec();

    let data = module
        .declare_anonymous_data(false, false)
        .map_err(|e| e.to_string())?;
    let mut desc = DataDescription::new();
    desc.define(bytes.into_boxed_slice());
    module.define_data(data, &desc).map_err(|e| e.to_string())?;

    let global_value = module.declare_data_in_func(data, builder.func);
    let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
    let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);

    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();
    emit_push_gc_roots(module, helpers, builder, ctx_val, &boxed_arg_vals);

    let call = match arg_vals.len() {
        0 => {
            let fields_ptr = builder.ins().iconst(PTR_TYPE, 0);
            let arity_value = builder.ins().iconst(PTR_TYPE, 0);
            let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
            builder.ins().call(
                make_adt,
                &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
            )
        }
        1 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
            builder
                .ins()
                .call(helper, &[ctx_val, name_ptr, name_len, boxed_arg_vals[0]])
        }
        2 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                ],
            )
        }
        3 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                ],
            )
        }
        4 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                ],
            )
        }
        5 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                    boxed_arg_vals[4],
                ],
            )
        }
        _ => {
            let function_compiler = FunctionCompiler::new(builder, 0, arg_vals.len());
            let fields_ptr = function_compiler.emit_tagged_array(builder, arg_vals);
            let arity_value = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
            let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
            builder.ins().call(
                make_adt,
                &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
            )
        }
    };

    emit_pop_gc_roots(module, helpers, builder, ctx_val);
    Ok(JitValue::boxed(builder.inst_results(call)[0]))
}

fn compile_simple_backend_ir_binary(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    op: IrBinaryOp,
    lhs: JitValue,
    rhs: JitValue,
) -> Result<JitValue, String> {
    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int {
        match op {
            IrBinaryOp::Add | IrBinaryOp::IAdd => {
                return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value)));
            }
            IrBinaryOp::Sub | IrBinaryOp::ISub => {
                return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value)));
            }
            IrBinaryOp::Mul | IrBinaryOp::IMul => {
                return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value)));
            }
            _ => {}
        }
    }

    let (lhs_tag, lhs_payload) = jit_value_to_tag_payload(builder, lhs);
    let (rhs_tag, rhs_payload) = jit_value_to_tag_payload(builder, rhs);
    let helper_name = match op {
        IrBinaryOp::Add | IrBinaryOp::IAdd => "rt_add",
        IrBinaryOp::Sub | IrBinaryOp::ISub => "rt_sub",
        IrBinaryOp::Mul | IrBinaryOp::IMul => "rt_mul",
        IrBinaryOp::Div | IrBinaryOp::IDiv => "rt_div",
        IrBinaryOp::Mod | IrBinaryOp::IMod => "rt_mod",
        IrBinaryOp::FAdd => "rt_add",
        IrBinaryOp::FSub => "rt_sub",
        IrBinaryOp::FMul => "rt_mul",
        IrBinaryOp::FDiv => "rt_div",
        IrBinaryOp::Eq => "rt_equal",
        IrBinaryOp::NotEq => "rt_not_equal",
        IrBinaryOp::Gt => "rt_greater_than",
        IrBinaryOp::Ge => "rt_greater_than_or_equal",
        IrBinaryOp::Le => "rt_less_than_or_equal",
        IrBinaryOp::Lt => {
            let ge_ref = get_helper_func_ref(module, helpers, builder, "rt_greater_than_or_equal");
            let ge_call = builder.ins().call(
                ge_ref,
                &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
            );
            let ge_tag = builder.inst_results(ge_call)[0];
            let ge_payload = builder.inst_results(ge_call)[1];
            let not_ref = get_helper_func_ref(module, helpers, builder, "rt_not");
            let not_call = builder.ins().call(not_ref, &[ctx_val, ge_tag, ge_payload]);
            let result = boxed_value_from_tagged_parts(
                module,
                helpers,
                builder,
                ctx_val,
                builder.inst_results(not_call)[0],
                builder.inst_results(not_call)[1],
            );
            return Ok(JitValue::boxed(result));
        }
        IrBinaryOp::And | IrBinaryOp::Or => {
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let lhs_truthy_call = builder
                .ins()
                .call(is_truthy, &[ctx_val, lhs_tag, lhs_payload]);
            let lhs_truthy = builder.inst_results(lhs_truthy_call)[0];
            let lhs_is_truthy = builder.ins().icmp_imm(IntCC::NotEqual, lhs_truthy, 0);
            let lhs_boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, lhs);
            let rhs_boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, rhs);
            let lhs_block = builder.create_block();
            let rhs_block = builder.create_block();
            let done_block = builder.create_block();
            builder.append_block_param(done_block, PTR_TYPE);

            match op {
                IrBinaryOp::And => {
                    builder
                        .ins()
                        .brif(lhs_is_truthy, rhs_block, &[], lhs_block, &[]);
                }
                IrBinaryOp::Or => {
                    builder
                        .ins()
                        .brif(lhs_is_truthy, lhs_block, &[], rhs_block, &[]);
                }
                _ => unreachable!(),
            }

            builder.switch_to_block(lhs_block);
            builder
                .ins()
                .jump(done_block, &[BlockArg::Value(lhs_boxed)]);
            builder.seal_block(lhs_block);

            builder.switch_to_block(rhs_block);
            builder
                .ins()
                .jump(done_block, &[BlockArg::Value(rhs_boxed)]);
            builder.seal_block(rhs_block);

            builder.switch_to_block(done_block);
            let result = builder.block_params(done_block)[0];
            builder.seal_block(done_block);
            return Ok(JitValue::boxed(result));
        }
    };

    let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
    let call = builder.ins().call(
        func_ref,
        &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
    );
    let result = boxed_value_from_tagged_parts(
        module,
        helpers,
        builder,
        ctx_val,
        builder.inst_results(call)[0],
        builder.inst_results(call)[1],
    );
    Ok(JitValue::boxed(result))
}

fn compile_simple_backend_ir_truthiness_condition(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) -> CraneliftValue {
    let truthy_i64 = match value.kind {
        JitValueKind::Bool => value.value,
        _ => {
            let boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
            let call = builder.ins().call(is_truthy, &[ctx_val, tag, boxed]);
            builder.inst_results(call)[0]
        }
    };

    builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0)
}

/// After a runtime helper that may set `ctx.error`, emit a call to
/// `rt_render_error_with_span` so the raw error is rendered as a structured
/// diagnostic with source location.  This produces VM-parity error output.
#[allow(dead_code)]
fn emit_render_error_with_span(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    span: Span,
) {
    let render = get_helper_func_ref(module, helpers, builder, "rt_render_error_with_span");
    let start_line = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let start_col = builder
        .ins()
        .iconst(PTR_TYPE, (span.start.column + 1) as i64);
    let end_line = builder.ins().iconst(PTR_TYPE, span.end.line as i64);
    let end_col = builder.ins().iconst(PTR_TYPE, (span.end.column + 1) as i64);
    builder
        .ins()
        .call(render, &[ctx_val, start_line, start_col, end_line, end_col]);
}

#[allow(dead_code)]
fn emit_return_on_null_value(builder: &mut FunctionBuilder, value_ptr: CraneliftValue) {
    let is_null = builder.ins().icmp_imm(IntCC::Equal, value_ptr, 0);
    let null_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_null, null_block, &[], continue_block, &[]);

    builder.switch_to_block(null_block);
    emit_return_null_tagged(builder);
    builder.seal_block(null_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

/// Like `emit_return_on_null_value` but also renders the raw error in
/// `ctx.error` as a structured diagnostic with source span before returning.
/// This is the primary mechanism for VM/JIT error parity.
#[allow(dead_code)]
fn emit_return_on_null_with_render(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value_ptr: CraneliftValue,
    span: Span,
) {
    let is_null = builder.ins().icmp_imm(IntCC::Equal, value_ptr, 0);
    let null_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_null, null_block, &[], continue_block, &[]);

    builder.switch_to_block(null_block);
    emit_render_error_with_span(module, helpers, builder, ctx_val, span);
    emit_return_null_tagged(builder);
    builder.seal_block(null_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

#[allow(dead_code)]
fn compile_jit_cfg_user_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    meta: JitFunctionMeta,
    arg_vals: &[JitValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_and_guard_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();
    let (_slot, args_ptr) = emit_boxed_stack_array(builder, &boxed_arg_vals);
    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let nargs = builder.ins().iconst(PTR_TYPE, boxed_arg_vals.len() as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let call_jit = get_helper_func_ref(module, helpers, builder, "rt_call_jit_function");
    let call = builder.ins().call(
        call_jit,
        &[
            ctx_val,
            fn_index,
            args_ptr,
            nargs,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn emit_jit_cfg_user_function_tailcall(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    meta: JitFunctionMeta,
    arg_vals: &[JitValue],
) {
    let (_slot, args_ptr) = emit_tagged_stack_array(builder, arg_vals);
    let set_thunk = get_helper_func_ref(module, helpers, builder, "rt_set_thunk");
    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let nargs = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
    let call = builder
        .ins()
        .call(set_thunk, &[ctx_val, fn_index, args_ptr, nargs]);
    let tag = builder.inst_results(call)[0];
    let payload = builder.inst_results(call)[1];
    builder.ins().return_(&[tag, payload]);
}

#[allow(dead_code)]
fn compile_jit_cfg_base_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    base_idx: usize,
    boxed_args: &[CraneliftValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        boxed_args.len().max(1) as u32 * 8,
        3,
    ));
    for (i, value) in boxed_args.iter().enumerate() {
        builder.ins().stack_store(*value, slot, (i * 8) as i32);
    }
    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, boxed_args.len() as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let call_base = get_helper_func_ref(module, helpers, builder, "rt_call_base_function");
    let call = builder.ins().call(
        call_base,
        &[
            ctx_val,
            idx_val,
            args_ptr,
            nargs_val,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

#[allow(dead_code)]
fn compile_jit_cfg_primop_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    primop: PrimOp,
    arg_vals: &[JitValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let boxed_args: Vec<_> = arg_vals
        .iter()
        .map(|value| box_and_guard_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        boxed_args.len().max(1) as u32 * 8,
        3,
    ));
    for (i, value) in boxed_args.iter().enumerate() {
        builder.ins().stack_store(*value, slot, (i * 8) as i32);
    }
    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let primop_id = builder.ins().iconst(PTR_TYPE, primop.id() as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, boxed_args.len() as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let call_primop = get_helper_func_ref(module, helpers, builder, "rt_call_primop");
    let call = builder.ins().call(
        call_primop,
        &[
            ctx_val,
            primop_id,
            args_ptr,
            nargs_val,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

#[allow(dead_code)]
fn compile_jit_cfg_generic_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    callee: JitValue,
    arg_vals: &[JitValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let callee = box_and_guard_jit_value(module, helpers, builder, ctx_val, callee);
    let boxed_args: Vec<_> = arg_vals
        .iter()
        .map(|value| box_and_guard_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        boxed_args.len().max(1) as u32 * 8,
        3,
    ));
    for (i, value) in boxed_args.iter().enumerate() {
        builder.ins().stack_store(*value, slot, (i * 8) as i32);
    }
    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let nargs_val = builder.ins().iconst(PTR_TYPE, boxed_args.len() as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let call_value = get_helper_func_ref(module, helpers, builder, "rt_call_value");
    let call = builder.ins().call(
        call_value,
        &[
            ctx_val,
            callee,
            args_ptr,
            nargs_val,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    // Errors from rt_call_value / rt_call_base_function are already rendered
    // with span info inside the helper — do not re-render.
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn get_helper_func_ref(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    name: &str,
) -> cranelift_codegen::ir::FuncRef {
    let func_id = helpers.ids[name];
    module.declare_func_in_func(func_id, builder.func)
}

fn emit_push_gc_roots(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    roots: &[CraneliftValue],
) {
    let push = get_helper_func_ref(module, helpers, builder, "rt_push_gc_roots");
    let len = builder.ins().iconst(PTR_TYPE, roots.len() as i64);
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (roots.len() as u32).max(1) * 8,
        3,
    ));
    for (i, value) in roots.iter().enumerate() {
        builder.ins().stack_store(*value, slot, (i * 8) as i32);
    }
    let roots_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    builder.ins().call(push, &[ctx_val, roots_ptr, len]);
}

fn emit_pop_gc_roots(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
) {
    let pop = get_helper_func_ref(module, helpers, builder, "rt_pop_gc_roots");
    builder.ins().call(pop, &[ctx_val]);
}

fn register_base_functions(scope: &mut Scope, interner: &Interner) {
    use crate::runtime::base::BASE_FUNCTIONS;
    use crate::syntax::symbol::Symbol;
    // Scan the interner to find Symbols matching each base name.
    for (idx, base_fn) in BASE_FUNCTIONS.iter().enumerate() {
        for sym_idx in 0u32.. {
            let sym = Symbol::new(sym_idx);
            match interner.try_resolve(sym) {
                Some(name) if name == base_fn.name => {
                    scope.base_functions.insert(sym, idx);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
    }
}

#[allow(dead_code)]
fn is_base_symbol(name: Identifier, interner: &Interner) -> bool {
    interner
        .try_resolve(name)
        .is_some_and(|name| name == "Base")
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn convert_type_expr_for_contract(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let name_str = interner.try_resolve(*name)?;
            match (name_str, args.len()) {
                ("Any", 0) => Some(RuntimeType::Any),
                ("Int", 0) => Some(RuntimeType::Int),
                ("Float", 0) => Some(RuntimeType::Float),
                ("Bool", 0) => Some(RuntimeType::Bool),
                ("String", 0) => Some(RuntimeType::String),
                ("Unit", 0) => Some(RuntimeType::Unit),
                ("Option", 1) => Some(RuntimeType::Option(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
                ("List", 1) => Some(RuntimeType::List(Box::new(convert_type_expr_for_contract(
                    &args[0], interner,
                )?))),
                ("Either", 2) => Some(RuntimeType::Either(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                ("Array", 1) => Some(RuntimeType::Array(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
                ("Map", 2) => Some(RuntimeType::Map(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                _ => None,
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(RuntimeType::Tuple(
            elements
                .iter()
                .map(|e| convert_type_expr_for_contract(e, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function { .. } => None,
    }
}

#[allow(dead_code)]
fn runtime_contract_from_annotations(
    parameter_types: &[Option<TypeExpr>],
    return_type: &Option<TypeExpr>,
    effects: &[crate::syntax::effect_expr::EffectExpr],
    interner: &Interner,
) -> Option<FunctionContract> {
    let params = parameter_types
        .iter()
        .map(|ty| {
            ty.as_ref()
                .and_then(|t| convert_type_expr_for_contract(t, interner))
        })
        .collect::<Vec<_>>();
    let ret = return_type
        .as_ref()
        .and_then(|ty| convert_type_expr_for_contract(ty, interner));
    if !params.iter().any(|t| t.is_some()) && ret.is_none() && effects.is_empty() {
        None
    } else {
        let effects = effects
            .iter()
            .flat_map(crate::syntax::effect_expr::EffectExpr::normalized_names)
            .collect::<Vec<_>>();
        Some(FunctionContract {
            params,
            ret,
            effects,
        })
    }
}

struct HelperSig {
    num_params: usize,
    num_returns: usize,
}

#[allow(dead_code)]
struct FunctionCompiler {
    boxed_array_slot: Option<StackSlot>,
    tagged_array_slot: Option<StackSlot>,
}

#[allow(dead_code)]
impl FunctionCompiler {
    fn new(
        builder: &mut FunctionBuilder,
        boxed_array_capacity: usize,
        tagged_array_capacity: usize,
    ) -> Self {
        let boxed_array_slot = (boxed_array_capacity > 0).then(|| {
            builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                boxed_array_capacity as u32 * 8,
                3,
            ))
        });
        let tagged_array_slot = (tagged_array_capacity > 0).then(|| {
            builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                tagged_array_capacity as u32 * 16,
                3,
            ))
        });
        Self {
            boxed_array_slot,
            tagged_array_slot,
        }
    }

    fn emit_boxed_array(
        &self,
        builder: &mut FunctionBuilder,
        values: &[CraneliftValue],
    ) -> CraneliftValue {
        // Lazily allocate the slot if the capacity calculation missed this call site.
        let slot = match self.boxed_array_slot {
            Some(s) => s,
            None => {
                let capacity = values.len().max(4);
                builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    capacity as u32 * 8,
                    3,
                ))
            }
        };
        for (i, value) in values.iter().enumerate() {
            builder.ins().stack_store(*value, slot, (i * 8) as i32);
        }
        builder.ins().stack_addr(PTR_TYPE, slot, 0)
    }

    fn emit_tagged_array(
        &self,
        builder: &mut FunctionBuilder,
        values: &[JitValue],
    ) -> CraneliftValue {
        let slot = match self.tagged_array_slot {
            Some(s) => s,
            None => {
                let capacity = values.len().max(4);
                builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    capacity as u32 * 16,
                    3,
                ))
            }
        };
        for (i, value) in values.iter().enumerate() {
            let (tag, payload) = jit_value_to_tag_payload(builder, *value);
            builder.ins().stack_store(tag, slot, (i * 16) as i32);
            builder
                .ins()
                .stack_store(payload, slot, (i * 16 + 8) as i32);
        }
        builder.ins().stack_addr(PTR_TYPE, slot, 0)
    }
}

#[allow(dead_code)]
fn note_boxed_array_usage(current_max: &mut usize, len: usize) {
    *current_max = (*current_max).max(len.max(1));
}

fn helper_signatures() -> Vec<(&'static str, HelperSig)> {
    vec![
        // Value constructors
        (
            "rt_make_integer",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_make_float",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_make_bool",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_division_by_zero",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        // rt_render_error_with_span(ctx, start_line, start_col, end_line, end_col)
        (
            "rt_render_error_with_span",
            HelperSig {
                num_params: 5,
                num_returns: 0,
            },
        ),
        (
            "rt_make_none",
            HelperSig {
                num_params: 1,
                num_returns: 2,
            },
        ),
        (
            "rt_force_boxed",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_push_gc_roots",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_pop_gc_roots",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        (
            "rt_make_empty_list",
            HelperSig {
                num_params: 1,
                num_returns: 1,
            },
        ),
        (
            "rt_make_string",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_base_function",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_jit_closure",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        (
            "rt_make_cons",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Arithmetic
        (
            "rt_add",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_sub",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_mul",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_div",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_mod",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        // Prefix
        (
            "rt_negate",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_not",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_is_truthy",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_bool_value",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_is_cons",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_cons_head",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_cons_tail",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Comparisons
        (
            "rt_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_not_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_greater_than",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_less_than_or_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_greater_than_or_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        // rt_set_thunk(ctx, fn_index, args_ptr, nargs) -> JitTaggedValue
        (
            "rt_set_thunk",
            HelperSig {
                num_params: 4,
                num_returns: 2,
            },
        ),
        // BaseFunctions & globals
        // rt_call_base_function_tagged(ctx, idx, tagged_args_ptr, nargs, sl, sc, el, ec) -> *mut Value
        (
            "rt_call_base_function_tagged",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_base_function",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_primop",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_value",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_jit_function",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_get_global",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_set_global",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_set_arity_error",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_check_jit_contract_call",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call1",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call2",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call3",
            HelperSig {
                num_params: 9,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call4",
            HelperSig {
                num_params: 10,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_return",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        // Phase 4: value wrappers (ctx, value) -> *mut Value
        (
            "rt_make_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: pattern matching checks (ctx, value) -> i64
        (
            "rt_is_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_none",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_empty_list",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: unwrap helpers (ctx, value) -> *mut Value
        (
            "rt_unwrap_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_unwrap_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_unwrap_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: structural equality (ctx, a, b) -> i64
        (
            "rt_values_equal",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 4: collections
        (
            "rt_make_array",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_tuple",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_hash",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_index",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_is_tuple",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_tuple_len_eq",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_tuple_get",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 4: string ops (ctx, value) -> *mut Value
        (
            "rt_to_string",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // rt_string_concat(ctx, a_ptr, b_ptr) -> *mut Value
        (
            "rt_string_concat",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 5: ADT helpers
        // rt_intern_unit_adt(ctx, constructor_ptr, constructor_len) -> *mut Value
        (
            "rt_intern_unit_adt",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_make_adt(ctx, constructor_ptr, constructor_len, fields_ptr, arity) -> *mut Value
        (
            "rt_make_adt",
            HelperSig {
                num_params: 5,
                num_returns: 1,
            },
        ),
        // rt_make_adt1(ctx, constructor_ptr, constructor_len, f0) -> *mut Value
        (
            "rt_make_adt1",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        // rt_make_adt2(ctx, constructor_ptr, constructor_len, f0, f1) -> *mut Value
        (
            "rt_make_adt2",
            HelperSig {
                num_params: 5,
                num_returns: 1,
            },
        ),
        // rt_make_adt3(ctx, constructor_ptr, constructor_len, f0, f1, f2) -> *mut Value
        (
            "rt_make_adt3",
            HelperSig {
                num_params: 6,
                num_returns: 1,
            },
        ),
        // rt_make_adt4(ctx, constructor_ptr, constructor_len, f0, f1, f2, f3) -> *mut Value
        (
            "rt_make_adt4",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        // rt_make_adt5(ctx, constructor_ptr, constructor_len, f0, f1, f2, f3, f4) -> *mut Value
        (
            "rt_make_adt5",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        // rt_is_adt_constructor(ctx, value, constructor_ptr, constructor_len) -> i64
        (
            "rt_is_adt_constructor",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        // rt_adt_field(ctx, value, field_idx) -> *mut Value
        (
            "rt_adt_field",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_adt_field_or_none(ctx, value, field_idx) -> *mut Value
        (
            "rt_adt_field_or_none",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Algebraic effects
        // rt_push_handler(ctx, effect_id, ops_ptr, closures_ptr, narms) -> void
        (
            "rt_push_handler",
            HelperSig {
                num_params: 5,
                num_returns: 0,
            },
        ),
        // rt_pop_handler(ctx) -> void
        (
            "rt_pop_handler",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        // rt_perform(ctx, effect_id, op_id, args_ptr, nargs,
        //            effect_name_ptr, effect_name_len, op_name_ptr, op_name_len,
        //            line, column) -> *mut Value
        (
            "rt_perform",
            HelperSig {
                num_params: 11,
                num_returns: 1,
            },
        ),
    ]
}

fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    cranelift_module::default_libcall_names()
}

#[cfg(test)]
mod tests {
    use super::backend_ir_jit_support_error;
    use crate::{
        backend_ir::{
            BlockId, FunctionId, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction,
            IrFunctionOrigin, IrInstr, IrMetadata, IrProgram, IrTerminator, IrTopLevelItem, IrType,
            IrVar,
        },
        cfg::IrBinaryOp,
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
                    params: vec![crate::backend_ir::IrBlockParam {
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
        let captured_param = crate::backend_ir::IrParam {
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
            global_bindings: vec![crate::backend_ir::IrGlobalBinding {
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
            params: vec![crate::backend_ir::IrParam {
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
                    instrs: vec![crate::backend_ir::IrInstr::HandleScope {
                        effect: effect_name,
                        arms: vec![crate::backend_ir::HandleScopeArm {
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
                    params: vec![crate::backend_ir::IrBlockParam {
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
                crate::backend_ir::IrParam {
                    name: captured_name,
                    var: IrVar(10),
                    ty: IrType::Any,
                },
                crate::backend_ir::IrParam {
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
                        crate::backend_ir::IrInstr::HandleScope {
                            effect: effect_name,
                            arms: vec![crate::backend_ir::HandleScopeArm {
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
                    params: vec![crate::backend_ir::IrBlockParam {
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
