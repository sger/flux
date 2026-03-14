#![allow(clippy::too_many_arguments)]

//! AST → Cranelift IR compiler (Phase 1: expressions, let bindings, calls).

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use cranelift_codegen::ir::{StackSlot, StackSlotData};
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlags, UserFuncName, Value as CraneliftValue,
    condcodes::{FloatCC, IntCC}, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::JITModule;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use crate::ast::free_vars::collect_free_vars;
use crate::diagnostics::position::Span;
use crate::cfg::{
    BlockId, FunctionId, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr, IrProgram,
    IrTerminator, IrTopLevelItem, IrVar,
};
use crate::primop::{PrimOp, resolve_primop_call};
use crate::runtime::base::{BaseModule, is_base_fastcall_allowlisted};
use crate::runtime::{function_contract::FunctionContract, runtime_type::RuntimeType};
use crate::syntax::expression::ExprId;
use crate::syntax::{
    Identifier, block::Block, expression::Expression, expression::HandleArm, expression::MatchArm,
    expression::Pattern, expression::StringPart, interner::Interner, program::Program,
    statement::Statement, type_expr::TypeExpr,
};
use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

use super::context::{JitCallAbi, JitFunctionEntry, JIT_TAG_BOOL, JIT_TAG_FLOAT, JIT_TAG_INT, JIT_TAG_PTR};
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
    num_params: usize,
    call_abi: JitCallAbi,
    function_index: usize,
    has_contract: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LiteralKey {
    sl: usize,
    sc: usize,
    el: usize,
    ec: usize,
    bsl: usize,
    bsc: usize,
    bel: usize,
    bec: usize,
    arity: usize,
    first_param: u32,
}

impl LiteralKey {
    fn from_expr(expr: &Expression) -> Self {
        match expr {
            Expression::Function {
                parameters,
                body,
                span,
                ..
            } => Self {
                sl: span.start.line,
                sc: span.start.column,
                el: span.end.line,
                ec: span.end.column,
                bsl: body.span.start.line,
                bsc: body.span.start.column,
                bel: body.span.end.line,
                bec: body.span.end.column,
                arity: parameters.len(),
                first_param: parameters.first().map(|p| p.as_u32()).unwrap_or(0),
            },
            _ => {
                let span = expr.span();
                Self::from_span(span)
            }
        }
    }

    fn from_span(span: crate::diagnostics::position::Span) -> Self {
        Self {
            sl: span.start.line,
            sc: span.start.column,
            el: span.end.line,
            ec: span.end.column,
            bsl: 0,
            bsc: 0,
            bel: 0,
            bec: 0,
            arity: 0,
            first_param: 0,
        }
    }

    fn from_ir_function(
        parameters: &[Identifier],
        body_span: crate::diagnostics::position::Span,
        span: crate::diagnostics::position::Span,
    ) -> Self {
        Self {
            sl: span.start.line,
            sc: span.start.column,
            el: span.end.line,
            ec: span.end.column,
            bsl: body_span.start.line,
            bsc: body_span.start.column,
            bel: body_span.end.line,
            bec: body_span.end.column,
            arity: parameters.len(),
            first_param: parameters.first().map(|p| p.as_u32()).unwrap_or(0),
        }
    }
}

#[derive(Clone)]
struct LiteralFunctionSpec {
    key: LiteralKey,
    parameters: Vec<Identifier>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type: Option<TypeExpr>,
    body: Block,
    captures: Vec<Identifier>,
    self_name: Option<Identifier>,
}

#[derive(Clone)]
struct IrLiteralFunctionSpec {
    key: LiteralKey,
    parameters: Vec<Identifier>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type: Option<TypeExpr>,
    body: Block,
    captures: Vec<Identifier>,
    self_name: Option<Identifier>,
}

struct CompiledFunctionSpec<'a> {
    meta: JitFunctionMeta,
    parameters: &'a [Identifier],
    body: &'a Block,
    captures: &'a [Identifier],
    self_name: Option<Identifier>,
    function_name: Option<Identifier>,
    local_functions: &'a [(Identifier, JitFunctionMeta)],
}

struct IrCompiledFunctionSpec<'a> {
    meta: JitFunctionMeta,
    parameters: &'a [Identifier],
    body: &'a Block,
    cfg_function: Option<&'a IrFunction>,
    /// Full IR program — needed by CFG handle-scope compilation to look up
    /// handler-arm functions by `FunctionId`.
    program: Option<&'a IrProgram>,
    captures: &'a [Identifier],
    self_name: Option<Identifier>,
    function_name: Option<Identifier>,
    local_functions: &'a [(Identifier, JitFunctionMeta)],
}

/// Tracks variables in the current scope.
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
    /// Maps IR function ids → JIT function metadata.
    ir_functions: HashMap<FunctionId, JitFunctionMeta>,
    /// Maps (module name, member name) -> JIT function metadata.
    module_functions: HashMap<(Identifier, Identifier), JitFunctionMeta>,
    /// Imported module names visible in current scope.
    imported_modules: HashSet<Identifier>,
    /// Import aliases: alias -> module name.
    import_aliases: HashMap<Identifier, Identifier>,
    /// Maps literal function key -> JIT function metadata.
    literal_functions: HashMap<LiteralKey, JitFunctionMeta>,
    /// Statically resolved capture order per literal.
    literal_captures: HashMap<LiteralKey, Vec<Identifier>>,
    /// Maps ADT constructor name (interned) → arity. Used to route constructor calls.
    adt_constructors: HashMap<Identifier, usize>,
    /// Maps ADT constructor name (interned) → owning ADT name.
    adt_constructor_owner: HashMap<Identifier, Identifier>,
    /// Maps ADT name → constructor names.
    adt_variants: HashMap<Identifier, Vec<Identifier>>,
    /// HM expression types for the final AST allocation used during JIT codegen.
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
            ir_functions: HashMap::new(),
            module_functions: HashMap::new(),
            imported_modules: HashSet::new(),
            import_aliases: HashMap::new(),
            literal_functions: HashMap::new(),
            literal_captures: HashMap::new(),
            adt_constructors: HashMap::new(),
            adt_constructor_owner: HashMap::new(),
            adt_variants: HashMap::new(),
            hm_expr_types,
        }
    }
}

fn declare_local(builder: &mut FunctionBuilder, kind: JitValueKind) -> Variable {
    let ty = match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    };
    builder.declare_var(ty)
}

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

fn append_return_block_params(builder: &mut FunctionBuilder, block: cranelift_codegen::ir::Block) {
    builder.append_block_param(block, types::I64);
    builder.append_block_param(block, PTR_TYPE);
}

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
        builder.ins().stack_store(payload, slot, (i * 16 + 8) as i32);
    }
    let ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    (slot, ptr)
}

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

fn compile_ir_truthiness_condition(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &Scope,
    ctx_val: CraneliftValue,
    expr: &Expression,
    value: JitValue,
) -> CraneliftValue {
    let truthy_i64 = if ir_expr_has_known_bool_type(scope, expr) {
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

fn jit_value_type(kind: JitValueKind) -> types::Type {
    match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    }
}

fn merged_jit_value_kind(left: JitValue, right: JitValue) -> JitValueKind {
    if left.kind == right.kind {
        left.kind
    } else {
        JitValueKind::Boxed
    }
}

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

fn expr_has_known_bool_type(scope: &Scope, expr: &Expression) -> bool {
    matches!(
        scope.hm_expr_types.get(&expr.expr_id()),
        Some(InferType::Con(TypeConstructor::Bool))
    )
}

fn ir_expr_id(expr: &Expression) -> ExprId {
    match expr {
        Expression::Identifier { id, .. }
        | Expression::Integer { id, .. }
        | Expression::Float { id, .. }
        | Expression::String { id, .. }
        | Expression::InterpolatedString { id, .. }
        | Expression::Boolean { id, .. }
        | Expression::Prefix { id, .. }
        | Expression::Infix { id, .. }
        | Expression::If { id, .. }
        | Expression::DoBlock { id, .. }
        | Expression::Function { id, .. }
        | Expression::Call { id, .. }
        | Expression::ListLiteral { id, .. }
        | Expression::ArrayLiteral { id, .. }
        | Expression::TupleLiteral { id, .. }
        | Expression::EmptyList { id, .. }
        | Expression::Index { id, .. }
        | Expression::Hash { id, .. }
        | Expression::MemberAccess { id, .. }
        | Expression::TupleFieldAccess { id, .. }
        | Expression::Match { id, .. }
        | Expression::None { id, .. }
        | Expression::Some { id, .. }
        | Expression::Left { id, .. }
        | Expression::Right { id, .. }
        | Expression::Cons { id, .. }
        | Expression::Perform { id, .. }
        | Expression::Handle { id, .. } => *id,
    }
}

fn ir_expr_has_known_bool_type(scope: &Scope, expr: &Expression) -> bool {
    matches!(
        scope.hm_expr_types.get(&ir_expr_id(expr)),
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
}

struct JitFunctionCompileEntry {
    id: FuncId,
    num_params: usize,
    call_abi: JitCallAbi,
    contract: Option<FunctionContract>,
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
        };

        compiler.declare_helpers()?;

        Ok(compiler)
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

    /// Compile a program's top-level statements into a single "main" function.
    /// Returns the FuncId of the compiled main function.
    pub fn compile_program(
        &mut self,
        program: &Program,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        self.compile_top_level_statements(&program.statements, interner)
    }

    fn compile_top_level_statements(
        &mut self,
        top_level_statements: &[Statement],
        interner: &Interner,
    ) -> Result<FuncId, String> {
        // main signature: (ctx: i64) -> (tag: i64, payload: i64)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE)); // ctx
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result tag
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result payload

        let main_id = self
            .module
            .declare_function("flux_main", Linkage::Export, &sig)
            .map_err(|e| format!("declare flux_main: {}", e))?;

        let mut func = Function::with_name_signature(UserFuncName::default(), sig.clone());

        let mut scope = Scope::new(Rc::clone(&self.hm_expr_types));

        // Register base_functions
        register_base_functions(&mut scope, interner);
        apply_base_directives(top_level_statements, &mut scope, interner)?;
        self.predeclare_imports(top_level_statements, &mut scope, interner);
        self.predeclare_globals(top_level_statements, &mut scope);
        collect_adt_definitions(top_level_statements, &mut scope, interner);
        let literal_specs = collect_literal_function_specs(top_level_statements);
        // Predeclare/compile user functions first so calls (and recursion) resolve.
        self.predeclare_functions(top_level_statements, &mut scope, interner)?;
        self.predeclare_literal_functions(&literal_specs, &mut scope, interner)?;
        self.compile_functions(top_level_statements, &scope, interner)?;
        self.compile_literal_functions(&literal_specs, &scope, interner)?;
        self.record_named_functions(&scope, interner);

        {
            // Destructure self to avoid borrow conflicts: builder_ctx is
            // mutably borrowed by FunctionBuilder, but we also need module
            // and helpers inside compilation functions.
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let ctx_val = builder.block_params(entry_block)[0];
            let top_level_compiler = FunctionCompiler::new(
                &mut builder,
                max_boxed_array_len_in_statements(top_level_statements),
                max_tagged_array_len_in_statements(top_level_statements),
            );

            // Compile each statement
            let mut last_val = None;
            for stmt in top_level_statements {
                if matches!(stmt, Statement::Function { .. } | Statement::Data { .. }) {
                    continue;
                }
                let outcome = compile_statement(
                    module,
                    helpers,
                    &mut builder,
                    &top_level_compiler,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    true,
                    stmt,
                    interner,
                )?;
                match outcome {
                    StmtOutcome::Value(v) => last_val = Some(v),
                    StmtOutcome::Returned => break,
                    StmtOutcome::None => {}
                }
            }

            // Entry-point convention: if `fn main()` exists and there is no explicit
            // top-level `main()` call, invoke it once after top-level initialization.
            let main_meta = scope
                .functions
                .iter()
                .find_map(|(name, meta)| (interner.resolve(*name) == "main").then_some(*meta));
            let has_explicit_top_level_main_call = top_level_statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(function.as_ref(), Expression::Identifier { name, .. } if interner.resolve(*name) == "main")
                        && arguments.is_empty()
                )
            });
            if let Some(meta) = main_meta
                && !has_explicit_top_level_main_call
            {
                let main_result = compile_user_function_call(
                    module,
                    helpers,
                    &mut builder,
                    &top_level_compiler,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    meta,
                    crate::diagnostics::position::Span::default(),
                    &[],
                    interner,
                )?;
                last_val = Some(main_result);
            }

            // Return the last expression value, or None
            let ret = match last_val {
                Some(v) => v,
                None => {
                    let make_none =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    JitValue::boxed(builder.inst_results(call)[1])
                }
            };
            let (ret_tag, ret_payload) = jit_value_to_tag_payload(&mut builder, ret);
            builder.ins().return_(&[ret_tag, ret_payload]);
            builder.finalize();
        }

        // Define the function in the module
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(main_id, &mut ctx)
            .map_err(|e| format!("define flux_main: {}", e))?;

        // Compile the identity function used as the `resume` value in JIT shallow handlers.
        self.identity_fn_index = self.compile_identity_function()?;

        Ok(main_id)
    }

    pub fn compile_ir_program(
        &mut self,
        program: &IrProgram,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        // main signature: (ctx: i64) -> (tag: i64, payload: i64)
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));

        let main_id = self
            .module
            .declare_function("flux_main", Linkage::Export, &sig)
            .map_err(|e| format!("declare flux_main: {}", e))?;

        let mut func = Function::with_name_signature(UserFuncName::default(), sig.clone());
        let mut scope = Scope::new(Rc::clone(&self.hm_expr_types));

        register_base_functions(&mut scope, interner);
        apply_ir_base_directives(&program.top_level_items, &mut scope, interner)?;
        self.predeclare_ir_imports(&program.top_level_items, &mut scope, interner);
        self.predeclare_ir_globals(&program.top_level_items, &mut scope);
        collect_ir_adt_definitions(&program.top_level_items, &mut scope, interner);
        let literal_specs = collect_ir_literal_function_specs(&program.top_level_items);
        self.predeclare_ir_functions(&program.top_level_items, &mut scope, interner)?;
        self.predeclare_ir_literal_functions(&literal_specs, &mut scope, interner)?;
        self.predeclare_handle_arm_functions(&program, &mut scope)?;
        self.compile_ir_functions(program, &scope, interner)?;
        self.compile_ir_literal_functions(&literal_specs, &program, &scope, interner)?;
        self.compile_handle_arm_functions(&program, &scope, interner)?;
        self.record_named_functions(&scope, interner);

        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let ctx_val = builder.block_params(entry_block)[0];
            let top_level_compiler = FunctionCompiler::new(
                &mut builder,
                max_boxed_array_len_in_ir_items(&program.top_level_items),
                max_tagged_array_len_in_ir_items(&program.top_level_items),
            );

            let mut last_val = None;
            for item in &program.top_level_items {
                if matches!(item, IrTopLevelItem::Function { .. } | IrTopLevelItem::Data { .. }) {
                    continue;
                }
                let outcome = compile_ir_top_level_item(
                    module,
                    helpers,
                    &mut builder,
                    &top_level_compiler,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    true,
                    item,
                    interner,
                )?;
                match outcome {
                    StmtOutcome::Value(v) => last_val = Some(v),
                    StmtOutcome::Returned => break,
                    StmtOutcome::None => {}
                }
            }

            let main_meta = scope
                .functions
                .iter()
                .find_map(|(name, meta)| (interner.resolve(*name) == "main").then_some(*meta));
            let has_explicit_top_level_main_call =
                ir_top_level_has_explicit_main_call(&program.top_level_items, interner);
            if let Some(meta) = main_meta
                && !has_explicit_top_level_main_call
            {
                let main_result = compile_user_function_call(
                    module,
                    helpers,
                    &mut builder,
                    &top_level_compiler,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    meta,
                    crate::diagnostics::position::Span::default(),
                    &[],
                    interner,
                )?;
                last_val = Some(main_result);
            }

            let ret = match last_val {
                Some(v) => v,
                None => {
                    let make_none =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    JitValue::boxed(builder.inst_results(call)[1])
                }
            };
            let (ret_tag, ret_payload) = jit_value_to_tag_payload(&mut builder, ret);
            builder.ins().return_(&[ret_tag, ret_payload]);
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
        });
        Ok(function_index)
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

    fn predeclare_functions(
        &mut self,
        top_level_statements: &[Statement],
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for stmt in top_level_statements {
            match stmt {
                Statement::Function {
                    name,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    ..
                } => {
                    if scope.functions.contains_key(name) {
                        continue;
                    }

                    let call_abi = JitCallAbi::from_arity(parameters.len());
                    let sig = self.user_function_signature(call_abi);
                    let fn_name = format!("flux_fn_{}", interner.resolve(*name));
                    let id = self
                        .module
                        .declare_function(&fn_name, Linkage::Local, &sig)
                        .map_err(|e| format!("declare {}: {}", fn_name, e))?;
                    let function_index = self.jit_functions.len();
                    let contract = runtime_contract_from_annotations(
                        parameter_types,
                        return_type,
                        effects,
                        interner,
                    );
                    let has_contract = contract.is_some();
                    self.jit_functions.push(JitFunctionCompileEntry {
                        id,
                        num_params: parameters.len(),
                        call_abi,
                        contract,
                    });
                    scope.functions.insert(
                        *name,
                        JitFunctionMeta {
                            id,
                            num_params: parameters.len(),
                            call_abi,
                            function_index,
                            has_contract,
                        },
                    );
                }
                Statement::Module {
                    name: module_name,
                    body,
                    ..
                } => {
                    scope.imported_modules.insert(*module_name);
                    for inner in &body.statements {
                        let Statement::Function {
                            name: fn_name,
                            parameters,
                            parameter_types,
                            return_type,
                            effects,
                            ..
                        } = inner
                        else {
                            continue;
                        };

                        let key = (*module_name, *fn_name);
                        if scope.module_functions.contains_key(&key) {
                            continue;
                        }

                        let call_abi = JitCallAbi::from_arity(parameters.len());
                        let sig = self.user_function_signature(call_abi);
                        let label = format!(
                            "flux_mod_{}_{}",
                            interner.resolve(*module_name),
                            interner.resolve(*fn_name)
                        );
                        let id = self
                            .module
                            .declare_function(&label, Linkage::Local, &sig)
                            .map_err(|e| format!("declare {}: {}", label, e))?;
                        let function_index = self.jit_functions.len();
                        let contract = runtime_contract_from_annotations(
                            parameter_types,
                            return_type,
                            effects,
                            interner,
                        );
                        let has_contract = contract.is_some();
                        self.jit_functions.push(JitFunctionCompileEntry {
                            id,
                            num_params: parameters.len(),
                            call_abi,
                            contract,
                        });
                        scope.module_functions.insert(
                            key,
                            JitFunctionMeta {
                                id,
                                num_params: parameters.len(),
                                call_abi,
                                function_index,
                                has_contract,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn predeclare_imports(&self, top_level_statements: &[Statement], scope: &mut Scope, interner: &Interner) {
        for stmt in top_level_statements {
            if let Statement::Import {
                name,
                alias,
                except: _,
                ..
            } = stmt
            {
                if is_base_symbol(*name, interner) {
                    continue;
                }
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
        }
    }

    fn predeclare_ir_imports(
        &self,
        top_level_items: &[IrTopLevelItem],
        scope: &mut Scope,
        interner: &Interner,
    ) {
        for item in top_level_items {
            if let IrTopLevelItem::Import {
                name,
                alias,
                except: _,
                ..
            } = item
            {
                if is_base_symbol(*name, interner) {
                    continue;
                }
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
        }
    }

    fn predeclare_globals(&self, top_level_statements: &[Statement], scope: &mut Scope) {
        fn collect_pattern_names(pattern: &Pattern, out: &mut Vec<Identifier>) {
            match pattern {
                Pattern::Identifier { name, .. } => out.push(*name),
                Pattern::Some { pattern, .. }
                | Pattern::Left { pattern, .. }
                | Pattern::Right { pattern, .. } => collect_pattern_names(pattern, out),
                Pattern::Cons { head, tail, .. } => {
                    collect_pattern_names(head, out);
                    collect_pattern_names(tail, out);
                }
                Pattern::Tuple { elements, .. } => {
                    for element in elements {
                        collect_pattern_names(element, out);
                    }
                }
                Pattern::Constructor { fields, .. } => {
                    for field in fields {
                        collect_pattern_names(field, out);
                    }
                }
                Pattern::Wildcard { .. }
                | Pattern::Literal { .. }
                | Pattern::None { .. }
                | Pattern::EmptyList { .. } => {}
            }
        }

        let mut next_idx = scope.globals.len();
        for stmt in top_level_statements {
            match stmt {
                Statement::Let { name, .. } => {
                    scope.globals.entry(*name).or_insert_with(|| {
                        let idx = next_idx;
                        next_idx += 1;
                        idx
                    });
                }
                Statement::LetDestructure { pattern, .. } => {
                    let mut names = Vec::new();
                    collect_pattern_names(pattern, &mut names);
                    for name in names {
                        scope.globals.entry(name).or_insert_with(|| {
                            let idx = next_idx;
                            next_idx += 1;
                            idx
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn predeclare_ir_globals(&self, top_level_items: &[IrTopLevelItem], scope: &mut Scope) {
        fn collect_ir_pattern_names(pattern: &Pattern, out: &mut Vec<Identifier>) {
            match pattern {
                Pattern::Identifier { name, .. } => out.push(*name),
                Pattern::Some { pattern, .. }
                | Pattern::Left { pattern, .. }
                | Pattern::Right { pattern, .. } => collect_ir_pattern_names(pattern, out),
                Pattern::Cons { head, tail, .. } => {
                    collect_ir_pattern_names(head, out);
                    collect_ir_pattern_names(tail, out);
                }
                Pattern::Tuple { elements, .. } => {
                    for element in elements {
                        collect_ir_pattern_names(element, out);
                    }
                }
                Pattern::Constructor { fields, .. } => {
                    for field in fields {
                        collect_ir_pattern_names(field, out);
                    }
                }
                Pattern::Wildcard { .. }
                | Pattern::Literal { .. }
                | Pattern::None { .. }
                | Pattern::EmptyList { .. } => {}
            }
        }

        let mut next_idx = scope.globals.len();
        for item in top_level_items {
            match item {
                IrTopLevelItem::Let { name, .. } => {
                    scope.globals.entry(*name).or_insert_with(|| {
                        let idx = next_idx;
                        next_idx += 1;
                        idx
                    });
                }
                IrTopLevelItem::LetDestructure { pattern, .. } => {
                    let mut names = Vec::new();
                    collect_ir_pattern_names(pattern, &mut names);
                    for name in names {
                        scope.globals.entry(name).or_insert_with(|| {
                            let idx = next_idx;
                            next_idx += 1;
                            idx
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn compile_functions(
        &mut self,
        top_level_statements: &[Statement],
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for stmt in top_level_statements {
            let Statement::Function {
                name,
                parameters,
                body,
                ..
            } = stmt
            else {
                continue;
            };

            let Some(meta) = scope.functions.get(name).copied() else {
                continue;
            };
            self.compile_function_body(
                scope,
                CompiledFunctionSpec {
                    meta,
                    parameters,
                    body,
                    captures: &[],
                    self_name: None,
                    function_name: Some(*name),
                    local_functions: &[],
                },
                interner,
                &format!("define function {}", interner.resolve(*name)),
            )?;
        }

        for stmt in top_level_statements {
            let Statement::Module {
                name: module_name,
                body,
                ..
            } = stmt
            else {
                continue;
            };

            for inner in &body.statements {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    ..
                } = inner
                else {
                    continue;
                };

                let Some(meta) = scope.module_functions.get(&(*module_name, *name)).copied() else {
                    continue;
                };
                let local_functions = scope
                    .module_functions
                    .iter()
                    .filter_map(|((mod_name, member_name), member_meta)| {
                        (*mod_name == *module_name).then_some((*member_name, *member_meta))
                    })
                    .collect::<Vec<_>>();
                self.compile_function_body(
                    scope,
                    CompiledFunctionSpec {
                        meta,
                        parameters,
                        body,
                        captures: &[],
                        self_name: None,
                        function_name: Some(*name),
                        local_functions: &local_functions,
                    },
                    interner,
                    &format!(
                        "define module function {}.{}",
                        interner.resolve(*module_name),
                        interner.resolve(*name)
                    ),
                )?;
            }
        }
        Ok(())
    }

    fn predeclare_ir_functions(
        &mut self,
        top_level_items: &[IrTopLevelItem],
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for item in top_level_items {
            match item {
                IrTopLevelItem::Function {
                    name,
                    function_id,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    ..
                } => {
                    if scope.functions.contains_key(name) {
                        continue;
                    }

                    let call_abi = JitCallAbi::from_arity(parameters.len());
                    let sig = self.user_function_signature(call_abi);
                    let fn_name = format!("flux_fn_{}", interner.resolve(*name));
                    let id = self
                        .module
                        .declare_function(&fn_name, Linkage::Local, &sig)
                        .map_err(|e| format!("declare {}: {}", fn_name, e))?;
                    let function_index = self.jit_functions.len();
                    let contract = runtime_contract_from_annotations(
                        parameter_types,
                        return_type,
                        effects,
                        interner,
                    );
                    let has_contract = contract.is_some();
                    self.jit_functions.push(JitFunctionCompileEntry {
                        id,
                        num_params: parameters.len(),
                        call_abi,
                        contract,
                    });
                    let meta = JitFunctionMeta {
                        id,
                        num_params: parameters.len(),
                        call_abi,
                        function_index,
                        has_contract,
                    };
                    scope.functions.insert(*name, meta);
                    if let Some(function_id) = function_id {
                        scope.ir_functions.insert(*function_id, meta);
                    }
                }
                IrTopLevelItem::Module {
                    name: module_name,
                    body,
                    ..
                } => {
                    scope.imported_modules.insert(*module_name);
                    for inner in body {
                        let IrTopLevelItem::Function {
                            name: fn_name,
                            function_id,
                            parameters,
                            parameter_types,
                            return_type,
                            effects,
                            ..
                        } = inner
                        else {
                            continue;
                        };

                        let key = (*module_name, *fn_name);
                        if scope.module_functions.contains_key(&key) {
                            continue;
                        }

                        let call_abi = JitCallAbi::from_arity(parameters.len());
                        let sig = self.user_function_signature(call_abi);
                        let label = format!(
                            "flux_mod_{}_{}",
                            interner.resolve(*module_name),
                            interner.resolve(*fn_name)
                        );
                        let id = self
                            .module
                            .declare_function(&label, Linkage::Local, &sig)
                            .map_err(|e| format!("declare {}: {}", label, e))?;
                        let function_index = self.jit_functions.len();
                        let contract = runtime_contract_from_annotations(
                            parameter_types,
                            return_type,
                            effects,
                            interner,
                        );
                        let has_contract = contract.is_some();
                        self.jit_functions.push(JitFunctionCompileEntry {
                            id,
                            num_params: parameters.len(),
                            call_abi,
                            contract,
                        });
                        let meta = JitFunctionMeta {
                            id,
                            num_params: parameters.len(),
                            call_abi,
                            function_index,
                            has_contract,
                        };
                        scope.module_functions.insert(key, meta);
                        if let Some(function_id) = function_id {
                            scope.ir_functions.insert(*function_id, meta);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_ir_functions(
        &mut self,
        program: &IrProgram,
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for item in &program.top_level_items {
            let IrTopLevelItem::Function {
                name,
                function_id,
                parameters,
                body,
                ..
            } = item
            else {
                continue;
            };

            let Some(meta) = scope.functions.get(name).copied() else {
                continue;
            };
            self.compile_ir_function_body(
                scope,
                IrCompiledFunctionSpec {
                    meta,
                    parameters,
                    body,
                    cfg_function: function_id.and_then(|id| program.function(id)),
                    program: Some(program),
                    captures: &[],
                    self_name: None,
                    function_name: Some(*name),
                    local_functions: &[],
                },
                interner,
                &format!("define function {}", interner.resolve(*name)),
            )?;
        }

        for item in &program.top_level_items {
            let IrTopLevelItem::Module {
                name: module_name,
                body,
                ..
            } = item
            else {
                continue;
            };

            for inner in body {
                let IrTopLevelItem::Function {
                    name,
                    function_id,
                    parameters,
                    body,
                    ..
                } = inner
                else {
                    continue;
                };

                let Some(meta) = scope.module_functions.get(&(*module_name, *name)).copied() else {
                    continue;
                };
                let local_functions = scope
                    .module_functions
                    .iter()
                    .filter_map(|((mod_name, member_name), member_meta)| {
                        (*mod_name == *module_name).then_some((*member_name, *member_meta))
                    })
                    .collect::<Vec<_>>();
                self.compile_ir_function_body(
                    scope,
                    IrCompiledFunctionSpec {
                        meta,
                        parameters,
                        body,
                        cfg_function: function_id.and_then(|id| program.function(id)),
                        program: Some(program),
                        captures: &[],
                        self_name: None,
                        function_name: Some(*name),
                        local_functions: &local_functions,
                    },
                    interner,
                    &format!(
                        "define module function {}.{}",
                        interner.resolve(*module_name),
                        interner.resolve(*name)
                    ),
                )?;
            }
        }
        Ok(())
    }

    fn predeclare_literal_functions(
        &mut self,
        specs: &[LiteralFunctionSpec],
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for spec in specs {
            if scope.literal_functions.contains_key(&spec.key) {
                continue;
            }
            let call_abi = JitCallAbi::from_arity(spec.parameters.len());
            let sig = self.user_function_signature(call_abi);
            let fn_name = format!(
                "flux_lit_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
                spec.key.sl,
                spec.key.sc,
                spec.key.el,
                spec.key.ec,
                spec.key.bsl,
                spec.key.bsc,
                spec.key.bel,
                spec.key.bec,
                spec.key.arity,
                spec.key.first_param
            );
            let id = self
                .module
                .declare_function(&fn_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", fn_name, e))?;
            let function_index = self.jit_functions.len();
            let contract = runtime_contract_from_annotations(
                &spec.parameter_types,
                &spec.return_type,
                &[],
                interner,
            );
            let has_contract = contract.is_some();
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: spec.parameters.len(),
                call_abi,
                contract,
            });
            scope.literal_functions.insert(
                spec.key,
                JitFunctionMeta {
                    id,
                    num_params: spec.parameters.len(),
                    call_abi,
                    function_index,
                    has_contract,
                },
            );
            scope
                .literal_captures
                .insert(spec.key, spec.captures.clone());
        }
        Ok(())
    }

    fn predeclare_ir_literal_functions(
        &mut self,
        specs: &[IrLiteralFunctionSpec],
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for spec in specs {
            if scope.literal_functions.contains_key(&spec.key) {
                continue;
            }
            let call_abi = JitCallAbi::from_arity(spec.parameters.len());
            let sig = self.user_function_signature(call_abi);
            let fn_name = format!(
                "flux_lit_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
                spec.key.sl,
                spec.key.sc,
                spec.key.el,
                spec.key.ec,
                spec.key.bsl,
                spec.key.bsc,
                spec.key.bel,
                spec.key.bec,
                spec.key.arity,
                spec.key.first_param
            );
            let id = self
                .module
                .declare_function(&fn_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", fn_name, e))?;
            let function_index = self.jit_functions.len();
            let contract = runtime_contract_from_annotations(
                &spec.parameter_types,
                &spec.return_type,
                &[],
                interner,
            );
            let has_contract = contract.is_some();
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: spec.parameters.len(),
                call_abi,
                contract,
            });
            scope.literal_functions.insert(
                spec.key,
                JitFunctionMeta {
                    id,
                    num_params: spec.parameters.len(),
                    call_abi,
                    function_index,
                    has_contract,
                },
            );
            scope
                .literal_captures
                .insert(spec.key, spec.captures.clone());
        }
        Ok(())
    }

    fn compile_literal_functions(
        &mut self,
        specs: &[LiteralFunctionSpec],
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for spec in specs {
            let Some(meta) = scope.literal_functions.get(&spec.key).copied() else {
                continue;
            };
            let sig = self.user_function_signature(meta.call_abi);
            let mut func = Function::with_name_signature(UserFuncName::default(), sig);
            {
                let module = &mut self.module;
                let helpers = &self.helpers;
                let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
                let mut fn_scope = scope.clone();
                fn_scope.locals.clear();

                let entry = builder.create_block();
                let init_block = builder.create_block();
                let body_block = builder.create_block();
                let arity_fail = builder.create_block();
                let return_block = builder.create_block();
                append_return_block_params(&mut builder, return_block);
                builder.append_block_params_for_function_params(entry);
                builder.switch_to_block(entry);
                builder.seal_block(entry);

                let entry_params = builder.block_params(entry).to_vec();
                let ctx_val = entry_params[0];
                let args_ptr = if meta.call_abi.uses_array_args() {
                    Some(entry_params[1])
                } else {
                    None
                };
                let captures_ptr = entry_params[meta.call_abi.captures_param_index()];
                let ncaptures = entry_params[meta.call_abi.ncaptures_param_index()];
                if args_ptr.is_some() {
                    let nargs = entry_params[2];
                    let want = builder.ins().iconst(PTR_TYPE, spec.parameters.len() as i64);
                    let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                    builder
                        .ins()
                        .brif(arity_ok, init_block, &[], arity_fail, &[]);

                    builder.switch_to_block(arity_fail);
                    let set_arity_error =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                    builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                    emit_return_null_tagged(&mut builder);
                    builder.seal_block(arity_fail);
                } else {
                    builder.ins().jump(init_block, &[]);
                    builder.seal_block(arity_fail);
                }

                builder.switch_to_block(init_block);
                let mut param_bindings: Vec<(Identifier, Variable)> =
                    Vec::with_capacity(spec.parameters.len());

                for (idx, ident) in spec.captures.iter().enumerate() {
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
                    let cap_ptr = boxed_value_from_tagged_parts(
                        module, helpers, &mut builder, ctx_val, cap_tag, cap_payload,
                    );
                    let binding = LocalBinding {
                        var: declare_local(&mut builder, JitValueKind::Boxed),
                        kind: JitValueKind::Boxed,
                    };
                    builder.def_var(binding.var, cap_ptr);
                    fn_scope.locals.insert(*ident, binding);
                }

                for (idx, ident) in spec.parameters.iter().enumerate() {
                    let arg_ptr = match args_ptr {
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
                            boxed_value_from_tagged_parts(
                                module, helpers, &mut builder, ctx_val, tag, payload,
                            )
                        }
                        None => {
                            let base = 1 + idx * 2;
                            boxed_value_from_tagged_parts(
                                module,
                                helpers,
                                &mut builder,
                                ctx_val,
                                entry_params[base],
                                entry_params[base + 1],
                            )
                        }
                    };
                    let binding = LocalBinding {
                        var: declare_local(&mut builder, JitValueKind::Boxed),
                        kind: JitValueKind::Boxed,
                    };
                    builder.def_var(binding.var, arg_ptr);
                    fn_scope.locals.insert(*ident, binding.clone());
                    param_bindings.push((*ident, binding.var));
                }

                if let Some(self_name) = spec.self_name {
                    let make_jit_closure =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_make_jit_closure");
                    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                    let call = builder.ins().call(
                        make_jit_closure,
                        &[ctx_val, fn_idx, captures_ptr, ncaptures],
                    );
                    let closure = builder.inst_results(call)[0];
                    let binding = LocalBinding {
                        var: declare_local(&mut builder, JitValueKind::Boxed),
                        kind: JitValueKind::Boxed,
                    };
                    builder.def_var(binding.var, closure);
                    fn_scope.locals.insert(self_name, binding);
                }
                builder.ins().jump(body_block, &[]);
                builder.seal_block(init_block);

                let function_compiler = FunctionCompiler::new(
                    &mut builder,
                    max_boxed_array_len_in_block(&spec.body),
                    max_tagged_array_len_in_block(&spec.body),
                );
                let tail_ctx = TailCallContext {
                    function_name: spec.self_name,
                    loop_block: body_block,
                    params: param_bindings,
                };

                builder.switch_to_block(body_block);

                let mut last_val = None;
                let mut returned = false;
                let last_index = spec.body.statements.len().saturating_sub(1);
                for (idx, body_stmt) in spec.body.statements.iter().enumerate() {
                    if idx == last_index
                        && let Some(outcome) = try_compile_tail_expression_statement(
                            module,
                            helpers,
                            &mut builder,
                            &function_compiler,
                            &mut fn_scope,
                            ctx_val,
                            Some(return_block),
                            &tail_ctx,
                            body_stmt,
                            interner,
                        )?
                    {
                        match outcome {
                            StmtOutcome::Returned => {
                                returned = true;
                                break;
                            }
                            StmtOutcome::Value(v) => {
                                last_val = Some(v);
                                continue;
                            }
                            StmtOutcome::None => continue,
                        }
                    }
                    let outcome = compile_statement(
                        module,
                        helpers,
                        &mut builder,
                        &function_compiler,
                        &mut fn_scope,
                        ctx_val,
                        Some(return_block),
                        Some(&tail_ctx),
                        false,
                        body_stmt,
                        interner,
                    )?;
                    match outcome {
                        StmtOutcome::Value(v) => last_val = Some(v),
                        StmtOutcome::Returned => {
                            returned = true;
                            break;
                        }
                        StmtOutcome::None => {}
                    }
                }

                if !returned {
                    let ret = match last_val {
                        Some(v) => v,
                        None => {
                            let make_none =
                                get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                            let call = builder.ins().call(make_none, &[ctx_val]);
                            JitValue::boxed(builder.inst_results(call)[1])
                        }
                    };
                    jump_with_jit_value(&mut builder, return_block, ret);
                }
                builder.seal_block(body_block);
                builder.switch_to_block(return_block);
                let ret_tag = builder.block_params(return_block)[0];
                let ret_payload = builder.block_params(return_block)[1];
                builder.ins().return_(&[ret_tag, ret_payload]);
                builder.seal_block(return_block);
                builder.finalize();
            }

            let mut ctx = cranelift_codegen::Context::new();
            ctx.func = func;
            self.module
                .define_function(meta.id, &mut ctx)
                .map_err(|e| format!("define literal function: {}", e))?;
        }
        Ok(())
    }

    fn compile_ir_literal_functions(
        &mut self,
        specs: &[IrLiteralFunctionSpec],
        program: &IrProgram,
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        let cfg_functions: HashMap<LiteralKey, &IrFunction> = program
            .functions
            .iter()
            .filter(|function| {
                matches!(
                    function.origin,
                    crate::cfg::IrFunctionOrigin::FunctionLiteral
                )
            })
            .map(|function| {
                let parameters = function
                    .params
                    .iter()
                    .skip(function.captures.len())
                    .map(|param| param.name)
                    .collect::<Vec<_>>();
                let span = function.metadata.span.unwrap_or(function.body_span);
                (
                    LiteralKey::from_ir_function(&parameters, function.body_span, span),
                    function,
                )
            })
            .collect();

        for spec in specs {
            let Some(meta) = scope.literal_functions.get(&spec.key).copied() else {
                continue;
            };
            self.compile_ir_function_body(
                scope,
                IrCompiledFunctionSpec {
                    meta,
                    parameters: &spec.parameters,
                    body: &spec.body,
                    cfg_function: cfg_functions.get(&spec.key).copied(),
                    program: Some(program),
                    captures: &spec.captures,
                    self_name: spec.self_name,
                    function_name: spec.self_name,
                    local_functions: &[],
                },
                interner,
                "define literal function",
            )?;
        }
        Ok(())
    }

    /// Pre-declare and compile handle-arm functions found via `IrInstr::HandleScope`.
    /// Collect all handle-arm function IDs from `HandleScope` instructions across
    /// all `IrFunction` blocks.
    fn collect_handle_arm_fn_ids(program: &IrProgram) -> Vec<FunctionId> {
        let mut arm_fn_ids: Vec<FunctionId> = Vec::new();
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instrs {
                    if let IrInstr::HandleScope { arms, .. } = instr {
                        for arm in arms {
                            arm_fn_ids.push(arm.function_id);
                        }
                    }
                }
            }
        }
        arm_fn_ids
    }

    /// Pre-declare handle arm functions so they are available in
    /// `scope.ir_functions` when named functions containing `HandleScope`
    /// instructions are compiled.
    fn predeclare_handle_arm_functions(
        &mut self,
        program: &IrProgram,
        scope: &mut Scope,
    ) -> Result<(), String> {
        let arm_fn_ids = Self::collect_handle_arm_fn_ids(program);
        for &fn_id in &arm_fn_ids {
            if scope.ir_functions.contains_key(&fn_id) {
                continue;
            }
            let Some(ir_fn) = program.function(fn_id) else {
                continue;
            };
            let real_params = ir_fn.params.len().saturating_sub(ir_fn.captures.len());
            let call_abi = JitCallAbi::from_arity(real_params);
            let sig = self.user_function_signature(call_abi);
            let fn_name = format!("flux_handle_arm_{}", fn_id.0);
            let id = self
                .module
                .declare_function(&fn_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", fn_name, e))?;
            let function_index = self.jit_functions.len();
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: real_params,
                call_abi,
                contract: None,
            });
            let meta = JitFunctionMeta {
                id,
                num_params: real_params,
                call_abi,
                function_index,
                has_contract: false,
            };
            scope.ir_functions.insert(fn_id, meta);
        }
        Ok(())
    }

    /// Compile the bodies of handle arm functions via CFG.  Must be called
    /// after `predeclare_handle_arm_functions` has registered them.
    fn compile_handle_arm_functions(
        &mut self,
        program: &IrProgram,
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        let arm_fn_ids = Self::collect_handle_arm_fn_ids(program);
        for &fn_id in &arm_fn_ids {
            let Some(meta) = scope.ir_functions.get(&fn_id).copied() else {
                continue;
            };
            let Some(ir_fn) = program.function(fn_id) else {
                continue;
            };
            let params: Vec<Identifier> = ir_fn
                .params
                .iter()
                .skip(ir_fn.captures.len())
                .map(|p| p.name)
                .collect();
            let captures: Vec<Identifier> = ir_fn.captures.clone();
            // Use a minimal structured block as placeholder — CFG path will handle compilation.
            let placeholder_body = Block {
                statements: Vec::new(),
                span: ir_fn.body_span,
            };
            self.compile_ir_function_body(
                scope,
                IrCompiledFunctionSpec {
                    meta,
                    parameters: &params,
                    body: &placeholder_body,
                    cfg_function: Some(ir_fn),
                    program: Some(program),
                    captures: &captures,
                    self_name: None,
                    function_name: None,
                    local_functions: &[],
                },
                interner,
                &format!("define handle arm fn{}", fn_id.0),
            )?;
        }
        Ok(())
    }

    fn compile_function_body(
        &mut self,
        scope: &Scope,
        spec: CompiledFunctionSpec<'_>,
        interner: &Interner,
        define_error_prefix: &str,
    ) -> Result<(), String> {
        let sig = self.user_function_signature(spec.meta.call_abi);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let mut fn_scope = scope.clone();
            fn_scope.locals.clear();
            for (name, meta) in spec.local_functions {
                fn_scope.functions.insert(*name, *meta);
            }

            let entry = builder.create_block();
            let init_block = builder.create_block();
            let body_block = builder.create_block();
            let arity_fail = builder.create_block();
            let return_block = builder.create_block();
            append_return_block_params(&mut builder, return_block);
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let entry_params = builder.block_params(entry).to_vec();
            let ctx_val = entry_params[0];
            let args_ptr = if spec.meta.call_abi.uses_array_args() {
                Some(entry_params[1])
            } else {
                None
            };
            let captures_ptr = (!spec.captures.is_empty() || spec.self_name.is_some())
                .then(|| entry_params[spec.meta.call_abi.captures_param_index()]);
            let ncaptures = captures_ptr
                .map(|_| entry_params[spec.meta.call_abi.ncaptures_param_index()]);

            if args_ptr.is_some() {
                let nargs = entry_params[2];
                let want = builder.ins().iconst(PTR_TYPE, spec.parameters.len() as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                emit_return_null_tagged(&mut builder);
                builder.seal_block(arity_fail);
            } else {
                builder.ins().jump(init_block, &[]);
                builder.seal_block(arity_fail);
            }

            builder.switch_to_block(init_block);
            let mut param_bindings: Vec<(Identifier, Variable)> =
                Vec::with_capacity(spec.parameters.len());

            if let Some(captures_ptr) = captures_ptr {
                for (idx, ident) in spec.captures.iter().enumerate() {
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
                    let cap_ptr = boxed_value_from_tagged_parts(
                        module, helpers, &mut builder, ctx_val, cap_tag, cap_payload,
                    );
                    let binding = LocalBinding {
                        var: declare_local(&mut builder, JitValueKind::Boxed),
                        kind: JitValueKind::Boxed,
                    };
                    builder.def_var(binding.var, cap_ptr);
                    fn_scope.locals.insert(*ident, binding);
                }
            }

            for (idx, ident) in spec.parameters.iter().enumerate() {
                let arg_ptr = match args_ptr {
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
                        boxed_value_from_tagged_parts(
                            module, helpers, &mut builder, ctx_val, tag, payload,
                        )
                    }
                    None => {
                        let base = 1 + idx * 2;
                        boxed_value_from_tagged_parts(
                            module,
                            helpers,
                            &mut builder,
                            ctx_val,
                            entry_params[base],
                            entry_params[base + 1],
                        )
                    }
                };
                let binding = LocalBinding {
                    var: declare_local(&mut builder, JitValueKind::Boxed),
                    kind: JitValueKind::Boxed,
                };
                builder.def_var(binding.var, arg_ptr);
                fn_scope.locals.insert(*ident, binding.clone());
                param_bindings.push((*ident, binding.var));
            }

            if let (Some(self_name), Some(captures_ptr), Some(ncaptures)) =
                (spec.self_name, captures_ptr, ncaptures)
            {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, spec.meta.function_index as i64);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, captures_ptr, ncaptures]);
                let closure = builder.inst_results(call)[0];
                let binding = LocalBinding {
                    var: declare_local(&mut builder, JitValueKind::Boxed),
                    kind: JitValueKind::Boxed,
                };
                builder.def_var(binding.var, closure);
                fn_scope.locals.insert(self_name, binding);
            }

            builder.ins().jump(body_block, &[]);
            builder.seal_block(init_block);

            let function_compiler = FunctionCompiler::new(
                &mut builder,
                max_boxed_array_len_in_block(spec.body),
                max_tagged_array_len_in_block(spec.body),
            );
            let tail_ctx = TailCallContext {
                function_name: spec.function_name,
                loop_block: body_block,
                params: param_bindings,
            };

            builder.switch_to_block(body_block);

            let mut last_val = None;
            let mut returned = false;
            let last_index = spec.body.statements.len().saturating_sub(1);
            for (idx, body_stmt) in spec.body.statements.iter().enumerate() {
                if idx == last_index
                    && let Some(outcome) = try_compile_tail_expression_statement(
                        module,
                        helpers,
                        &mut builder,
                        &function_compiler,
                        &mut fn_scope,
                        ctx_val,
                        Some(return_block),
                        &tail_ctx,
                        body_stmt,
                        interner,
                    )?
                {
                    match outcome {
                        StmtOutcome::Returned => {
                            returned = true;
                            break;
                        }
                        StmtOutcome::Value(v) => {
                            last_val = Some(v);
                            continue;
                        }
                        StmtOutcome::None => continue,
                    }
                }
                let outcome = compile_statement(
                    module,
                    helpers,
                    &mut builder,
                    &function_compiler,
                    &mut fn_scope,
                    ctx_val,
                    Some(return_block),
                    Some(&tail_ctx),
                    false,
                    body_stmt,
                    interner,
                )?;
                match outcome {
                    StmtOutcome::Value(v) => last_val = Some(v),
                    StmtOutcome::Returned => {
                        returned = true;
                        break;
                    }
                    StmtOutcome::None => {}
                }
            }

            if !returned {
                let ret = match last_val {
                    Some(v) => v,
                    None => {
                        let make_none =
                            get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                        let call = builder.ins().call(make_none, &[ctx_val]);
                        JitValue::boxed(builder.inst_results(call)[1])
                    }
                };
                jump_with_jit_value(&mut builder, return_block, ret);
            }
            builder.seal_block(body_block);
            builder.switch_to_block(return_block);
            let ret_tag = builder.block_params(return_block)[0];
            let ret_payload = builder.block_params(return_block)[1];
            builder.ins().return_(&[ret_tag, ret_payload]);
            builder.seal_block(return_block);
            builder.finalize();
        }

        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(spec.meta.id, &mut ctx)
            .map_err(|e| format!("{define_error_prefix}: {} ({:?})", e, e))?;
        Ok(())
    }

    fn compile_ir_function_body(
        &mut self,
        scope: &Scope,
        spec: IrCompiledFunctionSpec<'_>,
        interner: &Interner,
        define_error_prefix: &str,
    ) -> Result<(), String> {
        let sig = self.user_function_signature(spec.meta.call_abi);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let mut fn_scope = scope.clone();
            fn_scope.locals.clear();
            for (name, meta) in spec.local_functions {
                fn_scope.functions.insert(*name, *meta);
            }

            let entry = builder.create_block();
            let init_block = builder.create_block();
            let body_block = builder.create_block();
            let arity_fail = builder.create_block();
            let return_block = builder.create_block();
            append_return_block_params(&mut builder, return_block);
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let entry_params = builder.block_params(entry).to_vec();
            let ctx_val = entry_params[0];
            let args_ptr = if spec.meta.call_abi.uses_array_args() {
                Some(entry_params[1])
            } else {
                None
            };
            let captures_ptr = (!spec.captures.is_empty() || spec.self_name.is_some())
                .then(|| entry_params[spec.meta.call_abi.captures_param_index()]);
            let ncaptures = captures_ptr
                .map(|_| entry_params[spec.meta.call_abi.ncaptures_param_index()]);

            if args_ptr.is_some() {
                let nargs = entry_params[2];
                let want = builder.ins().iconst(PTR_TYPE, spec.parameters.len() as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                emit_return_null_tagged(&mut builder);
                builder.seal_block(arity_fail);
            } else {
                builder.ins().jump(init_block, &[]);
                builder.seal_block(arity_fail);
            }

            builder.switch_to_block(init_block);
            let mut param_bindings: Vec<(Identifier, Variable)> =
                Vec::with_capacity(spec.parameters.len());

            if let Some(captures_ptr) = captures_ptr {
                for (idx, ident) in spec.captures.iter().enumerate() {
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
                    let cap_ptr = boxed_value_from_tagged_parts(
                        module, helpers, &mut builder, ctx_val, cap_tag, cap_payload,
                    );
                    let binding = LocalBinding {
                        var: declare_local(&mut builder, JitValueKind::Boxed),
                        kind: JitValueKind::Boxed,
                    };
                    builder.def_var(binding.var, cap_ptr);
                    fn_scope.locals.insert(*ident, binding);
                }
            }

            for (idx, ident) in spec.parameters.iter().enumerate() {
                let arg_ptr = match args_ptr {
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
                        boxed_value_from_tagged_parts(
                            module, helpers, &mut builder, ctx_val, tag, payload,
                        )
                    }
                    None => {
                        let base = 1 + idx * 2;
                        boxed_value_from_tagged_parts(
                            module,
                            helpers,
                            &mut builder,
                            ctx_val,
                            entry_params[base],
                            entry_params[base + 1],
                        )
                    }
                };
                let binding = LocalBinding {
                    var: declare_local(&mut builder, JitValueKind::Boxed),
                    kind: JitValueKind::Boxed,
                };
                builder.def_var(binding.var, arg_ptr);
                fn_scope.locals.insert(*ident, binding.clone());
                param_bindings.push((*ident, binding.var));
            }

            if let (Some(self_name), Some(captures_ptr), Some(ncaptures)) =
                (spec.self_name, captures_ptr, ncaptures)
            {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, spec.meta.function_index as i64);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, captures_ptr, ncaptures]);
                let closure = builder.inst_results(call)[0];
                let binding = LocalBinding {
                    var: declare_local(&mut builder, JitValueKind::Boxed),
                    kind: JitValueKind::Boxed,
                };
                builder.def_var(binding.var, closure);
                fn_scope.locals.insert(self_name, binding);
            }

            builder.ins().jump(body_block, &[]);
            builder.seal_block(init_block);

            let function_compiler = FunctionCompiler::new(
                &mut builder,
                max_boxed_array_len_in_ir_block(spec.body),
                max_tagged_array_len_in_ir_block(spec.body),
            );
            let tail_ctx = TailCallContext {
                function_name: spec.function_name,
                loop_block: body_block,
                params: param_bindings,
            };

            let cfg_handled = if let Some(cfg_function) = spec.cfg_function {
                try_compile_jit_ir_cfg_function_body(
                    module,
                    helpers,
                    &mut builder,
                    &mut fn_scope,
                    ctx_val,
                    return_block,
                    body_block,
                    cfg_function,
                    spec.parameters,
                    &tail_ctx,
                    interner,
                    spec.program,
                )
                .transpose()?
                .is_some()
            } else {
                false
            };

            if !cfg_handled {
                builder.switch_to_block(body_block);

                let mut last_val = None;
                let mut returned = false;
                let last_index = spec.body.statements.len().saturating_sub(1);
                for (idx, body_item) in spec.body.statements.iter().enumerate() {
                    if idx == last_index
                        && let Some(outcome) = try_compile_tail_expression_statement(
                            module,
                            helpers,
                            &mut builder,
                            &function_compiler,
                            &mut fn_scope,
                            ctx_val,
                            Some(return_block),
                            &tail_ctx,
                            body_item,
                            interner,
                        )?
                    {
                        match outcome {
                            StmtOutcome::Returned => {
                                returned = true;
                                break;
                            }
                            StmtOutcome::Value(v) => {
                                last_val = Some(v);
                                continue;
                            }
                            StmtOutcome::None => continue,
                        }
                    }
                    let outcome = compile_statement(
                        module,
                        helpers,
                        &mut builder,
                        &function_compiler,
                        &mut fn_scope,
                        ctx_val,
                        Some(return_block),
                        Some(&tail_ctx),
                        false,
                        body_item,
                        interner,
                    )?;
                    match outcome {
                        StmtOutcome::Value(v) => last_val = Some(v),
                        StmtOutcome::Returned => {
                            returned = true;
                            break;
                        }
                        StmtOutcome::None => {}
                    }
                }

                if !returned {
                    let ret = match last_val {
                        Some(v) => v,
                        None => {
                            let make_none =
                                get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                            let call = builder.ins().call(make_none, &[ctx_val]);
                            JitValue::boxed(builder.inst_results(call)[1])
                        }
                    };
                    jump_with_jit_value(&mut builder, return_block, ret);
                }
                builder.seal_block(body_block);
            }
            builder.switch_to_block(return_block);
            let ret_tag = builder.block_params(return_block)[0];
            let ret_payload = builder.block_params(return_block)[1];
            builder.ins().return_(&[ret_tag, ret_payload]);
            builder.seal_block(return_block);
            builder.finalize();
        }

        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(spec.meta.id, &mut ctx)
            .map_err(|e| format!("{define_error_prefix}: {} ({:?})", e, e))?;
        Ok(())
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
            })
            .collect()
    }
}

fn compile_jit_ir_cfg_truthiness_condition(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) -> CraneliftValue {
    let truthy_i64 = match value.kind {
        JitValueKind::Bool => value.value,
        JitValueKind::Boxed => {
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
            let call = builder.ins().call(is_truthy, &[ctx_val, tag, value.value]);
            builder.inst_results(call)[0]
        }
        JitValueKind::Int | JitValueKind::Float => {
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
            let call = builder.ins().call(is_truthy, &[ctx_val, tag, boxed]);
            builder.inst_results(call)[0]
        }
    };
    builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0)
}

fn compile_jit_ir_cfg_expr(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &Scope,
    ctx_val: CraneliftValue,
    values: &HashMap<IrVar, LocalBinding>,
    expr: &IrExpr,
    interner: &Interner,
) -> Result<JitValue, String> {
    match expr {
        IrExpr::Const(IrConst::Int(value)) => {
            Ok(JitValue::int(builder.ins().iconst(types::I64, *value)))
        }
        IrExpr::Const(IrConst::Float(value)) => {
            let bits = i64::from_ne_bytes(value.to_bits().to_ne_bytes());
            Ok(JitValue::float(builder.ins().iconst(types::I64, bits)))
        }
        IrExpr::Const(IrConst::Bool(value)) => Ok(JitValue::bool(
            builder.ins().iconst(types::I64, i64::from(*value)),
        )),
        IrExpr::Const(IrConst::String(value)) => {
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(value.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let global_value = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, global_value);
            let len = builder.ins().iconst(PTR_TYPE, value.len() as i64);
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::Const(IrConst::Unit) | IrExpr::None => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        IrExpr::Var(var) => {
            let binding = values
                .get(var)
                .cloned()
                .ok_or_else(|| "missing JIT CFG value binding".to_string())?;
            Ok(use_local(builder, binding))
        }
        IrExpr::TagTest { value, tag } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG tag-test binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper_name = match tag {
                crate::cfg::IrTagTest::None => "rt_is_none",
                crate::cfg::IrTagTest::Some => "rt_is_some",
                crate::cfg::IrTagTest::Left => "rt_is_left",
                crate::cfg::IrTagTest::Right => "rt_is_right",
            };
            let helper = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(helper, &[ctx_val, value]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        IrExpr::TagPayload { value, tag } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG tag-payload binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper_name = match tag {
                crate::cfg::IrTagTest::Some => "rt_unwrap_some",
                crate::cfg::IrTagTest::Left => "rt_unwrap_left",
                crate::cfg::IrTagTest::Right => "rt_unwrap_right",
                crate::cfg::IrTagTest::None => {
                    return Err("invalid JIT CFG None payload".to_string());
                }
            };
            let helper = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(helper, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::ListTest { value, tag } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG list-test binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper_name = match tag {
                crate::cfg::IrListTest::Empty => "rt_is_empty_list",
                crate::cfg::IrListTest::Cons => "rt_is_cons",
            };
            let helper = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(helper, &[ctx_val, value]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        IrExpr::ListHead { value } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG list-head binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let call = builder.ins().call(helper, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::ListTail { value } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG list-tail binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let call = builder.ins().call(helper, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::TupleArityTest { value, arity } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG tuple-test binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_tuple_len_eq");
            let len = builder.ins().iconst(PTR_TYPE, *arity as i64);
            let call = builder.ins().call(helper, &[ctx_val, value, len]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        IrExpr::TupleFieldAccess { object, index } => {
            let object = use_local(
                builder,
                values
                    .get(object)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG tuple-field binding".to_string())?,
            );
            let object = box_and_guard_jit_value(module, helpers, builder, ctx_val, object);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            let index = builder.ins().iconst(PTR_TYPE, *index as i64);
            let call = builder.ins().call(helper, &[ctx_val, object, index]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::AdtTagTest { value, constructor } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG adt-tag binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let name_str = interner.resolve(*constructor);
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(name_str.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let global_value = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder
                .ins()
                .call(helper, &[ctx_val, value, name_ptr, name_len]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        IrExpr::AdtField { value, index } => {
            let value = use_local(
                builder,
                values
                    .get(value)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG adt-field binding".to_string())?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            let index = builder.ins().iconst(PTR_TYPE, *index as i64);
            let call = builder.ins().call(helper, &[ctx_val, value, index]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::Binary(op, lhs, rhs) => {
            let lhs = use_local(
                builder,
                values
                    .get(lhs)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG lhs binding".to_string())?,
            );
            let rhs = use_local(
                builder,
                values
                    .get(rhs)
                    .cloned()
                    .ok_or_else(|| "missing JIT CFG rhs binding".to_string())?,
            );
            match op {
                crate::cfg::IrBinaryOp::Add
                    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
                {
                    Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Sub
                    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
                {
                    Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Mul
                    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
                {
                    Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Div
                    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
                {
                    Ok(JitValue::int(builder.ins().sdiv(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Mod
                    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
                {
                    Ok(JitValue::int(builder.ins().srem(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Add
                    if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
                {
                    Ok(JitValue::float(builder.ins().fadd(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Sub
                    if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
                {
                    Ok(JitValue::float(builder.ins().fsub(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Mul
                    if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
                {
                    Ok(JitValue::float(builder.ins().fmul(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Div
                    if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
                {
                    Ok(JitValue::float(builder.ins().fdiv(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::Eq if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => {
                                builder.ins().icmp(IntCC::Equal, lhs.value, rhs.value)
                            }
                            JitValueKind::Float => {
                                builder.ins().fcmp(FloatCC::Equal, lhs.value, rhs.value)
                            }
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG equality".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                crate::cfg::IrBinaryOp::NotEq if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => {
                                builder.ins().icmp(IntCC::NotEqual, lhs.value, rhs.value)
                            }
                            JitValueKind::Float => {
                                builder.ins().fcmp(FloatCC::NotEqual, lhs.value, rhs.value)
                            }
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG inequality".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                crate::cfg::IrBinaryOp::Lt if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => {
                                builder.ins().icmp(IntCC::SignedLessThan, lhs.value, rhs.value)
                            }
                            JitValueKind::Float => {
                                builder.ins().fcmp(FloatCC::LessThan, lhs.value, rhs.value)
                            }
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG less-than".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                crate::cfg::IrBinaryOp::Gt if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => {
                                builder.ins().icmp(IntCC::SignedGreaterThan, lhs.value, rhs.value)
                            }
                            JitValueKind::Float => {
                                builder.ins().fcmp(FloatCC::GreaterThan, lhs.value, rhs.value)
                            }
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG greater-than".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                crate::cfg::IrBinaryOp::Ge if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs.value, rhs.value),
                            JitValueKind::Float => builder
                                .ins()
                                .fcmp(FloatCC::GreaterThanOrEqual, lhs.value, rhs.value),
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG greater-equal".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                crate::cfg::IrBinaryOp::Le if lhs.kind == rhs.kind => Ok(JitValue::bool(
                    {
                        let cmp = match lhs.kind {
                            JitValueKind::Int | JitValueKind::Bool => {
                                builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs.value, rhs.value)
                            }
                            JitValueKind::Float => {
                                builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs.value, rhs.value)
                            }
                            JitValueKind::Boxed => {
                                return Err("unsupported boxed JIT CFG less-equal".to_string())
                            }
                        };
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().select(cmp, one, zero)
                    },
                )),
                // Typed integer ops — emitted by the type_directed_unboxing IR pass.
                // No kind guard needed: the IR guarantees both operands are Int.
                crate::cfg::IrBinaryOp::IAdd => {
                    Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::ISub => {
                    Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::IMul => {
                    Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::IDiv => {
                    Ok(JitValue::int(builder.ins().sdiv(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::IMod => {
                    Ok(JitValue::int(builder.ins().srem(lhs.value, rhs.value)))
                }
                // Typed float ops — same guarantee for Float operands.
                crate::cfg::IrBinaryOp::FAdd => {
                    Ok(JitValue::float(builder.ins().fadd(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::FSub => {
                    Ok(JitValue::float(builder.ins().fsub(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::FMul => {
                    Ok(JitValue::float(builder.ins().fmul(lhs.value, rhs.value)))
                }
                crate::cfg::IrBinaryOp::FDiv => {
                    Ok(JitValue::float(builder.ins().fdiv(lhs.value, rhs.value)))
                }
                _ => Err("unsupported JIT CFG expression".to_string()),
            }
        }
        // Wrapper constructors
        IrExpr::EmptyList => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let call = builder.ins().call(helper, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::Some(var) => {
            let value = use_local(
                builder,
                values.get(var).cloned().ok_or("missing JIT CFG some binding")?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(helper, &[ctx_val, value]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::Left(var) => {
            let value = use_local(
                builder,
                values.get(var).cloned().ok_or("missing JIT CFG left binding")?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(helper, &[ctx_val, value]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::Right(var) => {
            let value = use_local(
                builder,
                values.get(var).cloned().ok_or("missing JIT CFG right binding")?,
            );
            let value = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(helper, &[ctx_val, value]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::Cons { head, tail } => {
            let head = use_local(
                builder,
                values.get(head).cloned().ok_or("missing JIT CFG cons head")?,
            );
            let tail = use_local(
                builder,
                values.get(tail).cloned().ok_or("missing JIT CFG cons tail")?,
            );
            let head = box_and_guard_jit_value(module, helpers, builder, ctx_val, head);
            let tail = box_and_guard_jit_value(module, helpers, builder, ctx_val, tail);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder.ins().call(helper, &[ctx_val, head, tail]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        // Data construction
        IrExpr::MakeTuple(vars) | IrExpr::MakeArray(vars) => {
            let mut jit_vals = Vec::with_capacity(vars.len());
            for var in vars {
                let v = use_local(
                    builder,
                    values.get(var).cloned().ok_or("missing JIT CFG collection elem")?,
                );
                jit_vals.push(v);
            }
            let (_slot, elems_ptr) = emit_tagged_stack_array(builder, &jit_vals);
            let len_val = builder.ins().iconst(PTR_TYPE, vars.len() as i64);
            let helper_name = if matches!(expr, IrExpr::MakeTuple(_)) {
                "rt_make_tuple"
            } else {
                "rt_make_array"
            };
            let helper = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(helper, &[ctx_val, elems_ptr, len_val]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::MakeList(vars) => {
            if vars.is_empty() {
                let helper = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
                let call = builder.ins().call(helper, &[ctx_val]);
                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let empty_call = builder.ins().call(make_empty, &[ctx_val]);
            let mut acc = builder.inst_results(empty_call)[0];
            for var in vars.iter().rev() {
                let v = use_local(
                    builder,
                    values.get(var).cloned().ok_or("missing JIT CFG list elem")?,
                );
                let v = box_and_guard_jit_value(module, helpers, builder, ctx_val, v);
                let call = builder.ins().call(make_cons, &[ctx_val, v, acc]);
                acc = builder.inst_results(call)[0];
            }
            Ok(JitValue::boxed(acc))
        }
        IrExpr::MakeHash(pairs) => {
            let mut jit_vals = Vec::with_capacity(pairs.len() * 2);
            for (k, v) in pairs {
                let kv = use_local(
                    builder,
                    values.get(k).cloned().ok_or("missing JIT CFG hash key")?,
                );
                jit_vals.push(kv);
                let vv = use_local(
                    builder,
                    values.get(v).cloned().ok_or("missing JIT CFG hash value")?,
                );
                jit_vals.push(vv);
            }
            let (_slot, pairs_ptr) = emit_tagged_stack_array(builder, &jit_vals);
            let npairs_val = builder.ins().iconst(PTR_TYPE, pairs.len() as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder.ins().call(helper, &[ctx_val, pairs_ptr, npairs_val]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::MakeAdt(name, fields) => {
            let name_str = interner.resolve(*name);
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(name_str.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, gv);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let mut jit_vals = Vec::with_capacity(fields.len());
            for var in fields {
                let v = use_local(
                    builder,
                    values.get(var).cloned().ok_or("missing JIT CFG adt field")?,
                );
                jit_vals.push(v);
            }
            let (_slot, fields_ptr) = emit_tagged_stack_array(builder, &jit_vals);
            let nfields = builder.ins().iconst(PTR_TYPE, fields.len() as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
            let call = builder.ins().call(helper, &[ctx_val, name_ptr, name_len, fields_ptr, nfields]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::MakeClosure(fn_id, captures) => {
            let meta = scope
                .ir_functions
                .get(fn_id)
                .copied()
                .ok_or("missing JIT CFG closure function")?;
            let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
            if captures.is_empty() {
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let make_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let call = builder.ins().call(make_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            let mut cap_vals = Vec::with_capacity(captures.len());
            for var in captures {
                let v = use_local(
                    builder,
                    values.get(var).cloned().ok_or("missing JIT CFG closure capture")?,
                );
                cap_vals.push(v);
            }
            let (_slot, captures_ptr) = emit_tagged_stack_array(builder, &cap_vals);
            let ncaptures = builder.ins().iconst(PTR_TYPE, captures.len() as i64);
            let make_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let call = builder.ins().call(make_closure, &[ctx_val, fn_idx, captures_ptr, ncaptures]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        // Operations
        IrExpr::LoadName(name) => {
            if let Some(&idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base = get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            if let Some(meta) = scope.functions.get(name).copied() {
                let make_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder.ins().call(make_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            Err(format!("unresolved JIT CFG LoadName: {:?}", name))
        }
        IrExpr::Prefix { operator, right } => {
            let operand = use_local(
                builder,
                values.get(right).cloned().ok_or("missing JIT CFG prefix operand")?,
            );
            if operator == "-" && operand.kind == JitValueKind::Int {
                return Ok(JitValue::int(builder.ins().ineg(operand.value)));
            }
            if operator == "!" && operand.kind == JitValueKind::Bool {
                let one = builder.ins().iconst(types::I64, 1);
                return Ok(JitValue::bool(builder.ins().bxor(operand.value, one)));
            }
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unknown JIT CFG prefix operator: {}", operator)),
            };
            let (tag, payload) = jit_value_to_tag_payload(builder, operand);
            let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(func_ref, &[ctx_val, tag, payload]);
            let result = boxed_value_from_tagged_parts(
                module,
                helpers,
                builder,
                ctx_val,
                builder.inst_results(call)[0],
                builder.inst_results(call)[1],
            );
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::Index { left, index } => {
            let left = use_local(
                builder,
                values.get(left).cloned().ok_or("missing JIT CFG index left")?,
            );
            let index_v = use_local(
                builder,
                values.get(index).cloned().ok_or("missing JIT CFG index right")?,
            );
            let left = box_and_guard_jit_value(module, helpers, builder, ctx_val, left);
            let index_v = box_and_guard_jit_value(module, helpers, builder, ctx_val, index_v);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_index");
            let call = builder.ins().call(helper, &[ctx_val, left, index_v]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        IrExpr::InterpolatedString(parts) => {
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_add = get_helper_func_ref(module, helpers, builder, "rt_add");
            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    crate::cfg::IrStringPart::Literal(s) => {
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = DataDescription::new();
                        desc.define(s.as_bytes().to_vec().into_boxed_slice());
                        module.define_data(data, &desc).map_err(|e| e.to_string())?;
                        let gv = module.declare_data_in_func(data, builder.func);
                        let ptr = builder.ins().global_value(PTR_TYPE, gv);
                        let len = builder.ins().iconst(PTR_TYPE, s.len() as i64);
                        let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
                        let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
                        builder.inst_results(call)[0]
                    }
                    crate::cfg::IrStringPart::Interpolation(var) => {
                        let v = use_local(
                            builder,
                            values.get(var).cloned().ok_or("missing JIT CFG interpolation var")?,
                        );
                        let v = box_and_guard_jit_value(module, helpers, builder, ctx_val, v);
                        let call = builder.ins().call(rt_to_string, &[ctx_val, v]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                        let call = builder.ins().call(rt_add, &[ctx_val, tag, prev, tag, part_val]);
                        builder.inst_results(call)[1]
                    }
                });
            }
            match acc {
                Some(val) => Ok(JitValue::boxed(val)),
                None => {
                    let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
                    let null = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder.ins().call(make_string, &[ctx_val, null, zero]);
                    Ok(JitValue::boxed(builder.inst_results(call)[0]))
                }
            }
        }
        IrExpr::Perform { effect, operation, args } => {
            let mut arg_vals: Vec<CraneliftValue> = Vec::with_capacity(args.len());
            for var in args {
                let v = use_local(
                    builder,
                    values.get(var).cloned().ok_or("missing JIT CFG perform arg")?,
                );
                arg_vals.push(box_and_guard_jit_value(module, helpers, builder, ctx_val, v));
            }

            let args_ptr = if arg_vals.is_empty() {
                builder.ins().iconst(PTR_TYPE, 0)
            } else {
                let slot = builder.create_sized_stack_slot(
                    cranelift_codegen::ir::StackSlotData::new(
                        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                        (arg_vals.len() as u32).max(1) * 8,
                        3,
                    ),
                );
                for (i, val) in arg_vals.iter().enumerate() {
                    builder.ins().stack_store(*val, slot, (i * 8) as i32);
                }
                builder.ins().stack_addr(PTR_TYPE, slot, 0)
            };
            let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
            let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
            let op_val = builder.ins().iconst(PTR_TYPE, operation.as_u32() as i64);

            let effect_str: &'static str = Box::leak(interner.resolve(*effect).to_owned().into_boxed_str());
            let op_str: &'static str = Box::leak(interner.resolve(*operation).to_owned().into_boxed_str());
            let effect_name_ptr = builder.ins().iconst(PTR_TYPE, effect_str.as_ptr() as i64);
            let effect_name_len = builder.ins().iconst(PTR_TYPE, effect_str.len() as i64);
            let op_name_ptr = builder.ins().iconst(PTR_TYPE, op_str.as_ptr() as i64);
            let op_name_len = builder.ins().iconst(PTR_TYPE, op_str.len() as i64);
            // No span available from CFG IR metadata; use 0,0.
            let line_val = builder.ins().iconst(PTR_TYPE, 0);
            let col_val = builder.ins().iconst(PTR_TYPE, 0);

            let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
            let call = builder.ins().call(
                rt_perform,
                &[
                    ctx_val,
                    effect_val,
                    op_val,
                    args_ptr,
                    nargs_val,
                    effect_name_ptr,
                    effect_name_len,
                    op_name_ptr,
                    op_name_len,
                    line_val,
                    col_val,
                ],
            );
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        IrExpr::Handle { .. } => {
            // Handle is compiled as IrInstr::HandleScope at the instruction level,
            // not as an expression. This variant should not appear in CFG IR.
            Err("IrExpr::Handle should not appear in CFG IR".to_string())
        }
        _ => Err("unsupported JIT CFG expression".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Free functions for compilation (avoids borrow conflicts with builder_ctx)
// ---------------------------------------------------------------------------

fn try_compile_jit_ir_cfg_function_body(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: cranelift_codegen::ir::Block,
    body_entry_block: cranelift_codegen::ir::Block,
    function: &IrFunction,
    _parameter_names: &[Identifier],
    tail_ctx: &TailCallContext,
    interner: &Interner,
    program: Option<&IrProgram>,
) -> Option<Result<(), String>> {
    if !can_compile_jit_ir_cfg_function(function) {
        return None;
    }

    // Core IR may merge declared params with returned lambda params
    // (e.g. `fn f(a, b) { \x -> ... }` becomes `Lam(a, Lam(b, Lam(x, ...)))`).
    // When the IR function has more params than the AST declared, bail out
    // to the structured path which handles currying correctly.
    let expected_params = _parameter_names.len()
        + function.captures.len();
    if function.params.len() != expected_params {
        return None;
    }

    Some((|| {
        let mut bindings: HashMap<IrVar, LocalBinding> = HashMap::new();
        // IR params include captures (first) followed by actual parameters.
        // Use the IrParam's own `name` field to look up bindings in scope.locals,
        // which the outer compile_ir_function_body already populated for both
        // captures and declared parameters.
        for param in &function.params {
            let binding = scope
                .locals
                .get(&param.name)
                .cloned()
                .ok_or_else(|| {
                    "missing JIT CFG parameter binding".to_string()
                })?;
            bindings.insert(param.var, binding);
        }

        let mut block_map = HashMap::new();
        for (index, block) in function.blocks.iter().enumerate() {
            let cl_block = if index == 0 {
                body_entry_block
            } else {
                builder.create_block()
            };
            block_map.insert(block.id, cl_block);
        }

        for block in &function.blocks {
            if block.id == function.entry {
                continue;
            }
            let cl_block = block_map[&block.id];
            for _ in &block.params {
                builder.append_block_param(cl_block, PTR_TYPE);
            }
        }

        // The caller (compile_ir_function_body) has already emitted
        //   jump body_entry_block
        // from init_block and sealed init_block before calling here.
        // Do NOT emit a second terminator — switch_to_block below is enough.

        // Build the set of blocks that have backward-edge predecessors.
        // These blocks must not be sealed until all their predecessors have
        // been emitted. Self-tail-calls produce a back-edge to the entry
        // block; `lower_case` join blocks are allocated before their arm
        // blocks, so arms that Jump backward also produce back-edges.
        let block_indices_map: HashMap<_, _> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let mut deferred_seal: HashSet<BlockId> = HashSet::new();
        for (src_index, block) in function.blocks.iter().enumerate() {
            let targets: Vec<BlockId> = match &block.terminator {
                IrTerminator::Jump(target, _, _) => vec![*target],
                IrTerminator::Branch { then_block, else_block, .. } => {
                    vec![*then_block, *else_block]
                }
                IrTerminator::TailCall { callee, .. } => {
                    let is_self = match callee {
                        IrCallTarget::Named(name) => function.name == Some(*name),
                        IrCallTarget::Direct(id) => *id == function.id,
                        IrCallTarget::Var(_) => false,
                    };
                    if is_self { vec![function.entry] } else { vec![] }
                }
                _ => vec![],
            };
            for target in targets {
                if let Some(&target_index) = block_indices_map.get(&target)
                    && target_index <= src_index
                {
                    deferred_seal.insert(target);
                }
            }
        }

        // Identify continuation blocks for HandleScope — these need
        // rt_pop_handler emitted at the start of the block.
        let mut handle_scope_cont_blocks: HashSet<BlockId> = HashSet::new();
        for block in &function.blocks {
            for instr in &block.instrs {
                if let IrInstr::HandleScope { body_result, .. } = instr {
                    // The continuation block has body_result as a block param.
                    for b in &function.blocks {
                        if b.params.iter().any(|p| p.var == *body_result) {
                            handle_scope_cont_blocks.insert(b.id);
                            break;
                        }
                    }
                }
            }
        }

        for block in &function.blocks {
            let cl_block = block_map[&block.id];
            builder.switch_to_block(cl_block);
            // Seal blocks whose predecessors have all been emitted.
            // Blocks with backward-edge predecessors are sealed after
            // the main loop.
            if !deferred_seal.contains(&block.id) {
                builder.seal_block(cl_block);
            }

            // At the entry block, unbox Int/Bool function params from their
            // *mut Value arena pointers to raw i64 payloads.  The outer
            // compile_ir_function_body always stores params as JitValueKind::Boxed;
            // by loading `payload = *(ptr + 8)` here we get a raw i64 that typed
            // arithmetic instructions (iadd, isub, …) can operate on directly.
            if block.id == function.entry {
                for param in &function.params {
                    let target_kind = match param.ty {
                        crate::cfg::IrType::Int => JitValueKind::Int,
                        crate::cfg::IrType::Bool => JitValueKind::Bool,
                        _ => continue,
                    };
                    if let Some(existing) = bindings.get(&param.var).cloned()
                        && existing.kind == JitValueKind::Boxed
                    {
                        let ptr = builder.use_var(existing.var);
                        let payload = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            ptr,
                            8,
                        );
                        let unboxed_var = declare_local(builder, target_kind);
                        builder.def_var(unboxed_var, payload);
                        bindings.insert(
                            param.var,
                            LocalBinding { var: unboxed_var, kind: target_kind },
                        );
                    }
                }
            }

            for (idx, param) in block.params.iter().enumerate() {
                let value = builder.block_params(cl_block)[idx];
                let binding = bindings.entry(param.var).or_insert_with(|| LocalBinding {
                    var: declare_local(builder, JitValueKind::Boxed),
                    kind: JitValueKind::Boxed,
                });
                builder.def_var(binding.var, value);
            }

            // Emit rt_pop_handler at the start of HandleScope continuation blocks.
            if handle_scope_cont_blocks.contains(&block.id) {
                let rt_pop = get_helper_func_ref(module, helpers, builder, "rt_pop_handler");
                builder.ins().call(rt_pop, &[ctx_val]);
            }

            for instr in &block.instrs {
                match instr {
                    IrInstr::Assign { dest, expr, .. } => {
                        let value = compile_jit_ir_cfg_expr(
                            module,
                            helpers,
                            builder,
                            scope,
                            ctx_val,
                            &bindings,
                            expr,
                            interner,
                        )?;
                        emit_return_on_null_jit_value(module, helpers, builder, ctx_val, value);
                        let binding = bindings.entry(*dest).or_insert_with(|| LocalBinding {
                            var: declare_local(builder, value.kind),
                            kind: value.kind,
                        });
                        builder.def_var(binding.var, value.value);
                    }
                    IrInstr::Call {
                        dest,
                        target,
                        args,
                        metadata,
                    } => {
                        let mut arg_vals = Vec::with_capacity(args.len());
                        for arg in args {
                            let value = use_local(
                                builder,
                                bindings.get(arg).cloned().ok_or_else(|| {
                                    "missing JIT CFG call arg binding".to_string()
                                })?,
                            );
                            arg_vals.push(value);
                        }
                        let call_span = metadata.span.unwrap_or_default();
                        let value = match target {
                            IrCallTarget::Named(name) => {
                                if let Some(meta) = scope.functions.get(name).copied() {
                                    compile_jit_cfg_user_function_call(
                                        module, helpers, builder, ctx_val, meta, &arg_vals,
                                        call_span,
                                    )?
                                } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                    compile_jit_cfg_base_function_call(
                                        module, helpers, builder, ctx_val, base_idx,
                                        &arg_vals, call_span,
                                    )?
                                } else {
                                    return Err(format!(
                                        "missing JIT CFG named call target: {:?}",
                                        interner.resolve(*name),
                                    ));
                                }
                            }
                            IrCallTarget::Direct(id) => {
                                let meta = scope.ir_functions.get(id).copied().ok_or_else(|| {
                                    "missing JIT CFG direct call target".to_string()
                                })?;
                                compile_jit_cfg_user_function_call(
                                    module, helpers, builder, ctx_val, meta, &arg_vals,
                                    call_span,
                                )?
                            }
                            IrCallTarget::Var(var) => {
                                let callee = use_local(
                                    builder,
                                    bindings.get(var).cloned().ok_or_else(|| {
                                        "missing JIT CFG var call target".to_string()
                                    })?,
                                );
                                compile_jit_cfg_generic_call(
                                    module, helpers, builder, ctx_val, callee, &arg_vals,
                                    call_span,
                                )?
                            }
                        };
                        let binding = bindings.entry(*dest).or_insert_with(|| LocalBinding {
                            var: declare_local(builder, value.kind),
                            kind: value.kind,
                        });
                        builder.def_var(binding.var, value.value);
                    }
                    IrInstr::HandleScope {
                        effect,
                        arms,
                        dest,
                        ..
                    } => {
                        // Create a closure for each handler arm and push the handler.
                        let num_arms = arms.len();
                        let mut op_sym_vals: Vec<CraneliftValue> = Vec::new();
                        let mut closure_vals: Vec<CraneliftValue> = Vec::new();

                        for arm in arms {
                            op_sym_vals.push(
                                builder
                                    .ins()
                                    .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
                            );
                            let arm_meta = scope
                                .ir_functions
                                .get(&arm.function_id)
                                .copied()
                                .ok_or("missing JIT CFG handle arm function")?;
                            let arm_fn = program
                                .and_then(|p| p.function(arm.function_id))
                                .ok_or("missing handle arm IrFunction")?;

                            // Build closure with captures from current bindings.
                            let fn_idx = builder
                                .ins()
                                .iconst(PTR_TYPE, arm_meta.function_index as i64);
                            if arm_fn.captures.is_empty() {
                                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                                let zero = builder.ins().iconst(PTR_TYPE, 0);
                                let make_closure = get_helper_func_ref(
                                    module, helpers, builder, "rt_make_jit_closure",
                                );
                                let call = builder.ins().call(
                                    make_closure,
                                    &[ctx_val, fn_idx, null_ptr, zero],
                                );
                                closure_vals.push(builder.inst_results(call)[0]);
                            } else {
                                let mut cap_vals: Vec<JitValue> = Vec::new();
                                for cap_name in &arm_fn.captures {
                                    // Captures are by name — look up in scope.locals.
                                    let cap_binding = scope
                                        .locals
                                        .get(cap_name)
                                        .cloned()
                                        .ok_or_else(|| {
                                            format!(
                                                "missing handle arm capture: {:?}",
                                                cap_name
                                            )
                                        })?;
                                    cap_vals.push(use_local(builder, cap_binding));
                                }
                                let (_slot, captures_ptr) =
                                    emit_tagged_stack_array(builder, &cap_vals);
                                let ncaptures = builder
                                    .ins()
                                    .iconst(PTR_TYPE, cap_vals.len() as i64);
                                let make_closure = get_helper_func_ref(
                                    module, helpers, builder, "rt_make_jit_closure",
                                );
                                let call = builder.ins().call(
                                    make_closure,
                                    &[ctx_val, fn_idx, captures_ptr, ncaptures],
                                );
                                closure_vals.push(builder.inst_results(call)[0]);
                            }
                        }

                        // Store op symbols in a stack slot.
                        let ops_slot = builder.create_sized_stack_slot(
                            cranelift_codegen::ir::StackSlotData::new(
                                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                                (num_arms.max(1) as u32) * 8,
                                3,
                            ),
                        );
                        for (i, ov) in op_sym_vals.iter().enumerate() {
                            builder.ins().stack_store(*ov, ops_slot, (i * 8) as i32);
                        }
                        let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);

                        // Store closures in a stack slot.
                        let closures_slot = builder.create_sized_stack_slot(
                            cranelift_codegen::ir::StackSlotData::new(
                                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                                (num_arms.max(1) as u32) * 8,
                                3,
                            ),
                        );
                        for (i, cv) in closure_vals.iter().enumerate() {
                            builder
                                .ins()
                                .stack_store(*cv, closures_slot, (i * 8) as i32);
                        }
                        let closures_ptr =
                            builder.ins().stack_addr(PTR_TYPE, closures_slot, 0);

                        let effect_val =
                            builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
                        let narms_val =
                            builder.ins().iconst(PTR_TYPE, num_arms as i64);

                        let rt_push = get_helper_func_ref(
                            module, helpers, builder, "rt_push_handler",
                        );
                        builder.ins().call(
                            rt_push,
                            &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
                        );

                        // dest will be set by the continuation block's block param.
                        // Initialise it to null so Cranelift sees a definition.
                        let binding =
                            bindings.entry(*dest).or_insert_with(|| LocalBinding {
                                var: declare_local(builder, JitValueKind::Boxed),
                                kind: JitValueKind::Boxed,
                            });
                        let null = builder.ins().iconst(PTR_TYPE, 0);
                        builder.def_var(binding.var, null);
                    }
                }
            }

            match &block.terminator {
                IrTerminator::Return(var, _) => {
                    let value = use_local(
                        builder,
                        bindings
                            .get(var)
                            .cloned()
                            .ok_or_else(|| "missing JIT CFG return binding".to_string())?,
                    );
                    jump_with_jit_value(builder, return_block, value);
                }
                IrTerminator::Jump(target, args, _) => {
                    let target_block = block_map[target];
                    let mut jump_args = Vec::with_capacity(args.len());
                    for arg in args {
                        let value = use_local(
                            builder,
                            bindings
                                .get(arg)
                                .cloned()
                                .ok_or_else(|| "missing JIT CFG jump binding".to_string())?,
                        );
                        let boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
                        jump_args.push(BlockArg::Value(boxed));
                    }
                    builder.ins().jump(target_block, &jump_args);
                }
                IrTerminator::Branch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => {
                    let cond_value = use_local(
                        builder,
                        bindings
                            .get(cond)
                            .cloned()
                            .ok_or_else(|| "missing JIT CFG branch binding".to_string())?,
                    );
                    let cond_b1 = compile_jit_ir_cfg_truthiness_condition(
                        module,
                        helpers,
                        builder,
                        ctx_val,
                        cond_value,
                    );
                    builder.ins().brif(
                        cond_b1,
                        block_map[then_block],
                        &[],
                        block_map[else_block],
                        &[],
                    );
                }
                IrTerminator::TailCall { callee, args, .. } => {
                    let is_self_tail_call = match callee {
                        IrCallTarget::Named(name) => function.name == Some(*name),
                        IrCallTarget::Direct(function_id) => *function_id == function.id,
                        IrCallTarget::Var(_) => false,
                    };
                    if is_self_tail_call {
                        if args.len() != tail_ctx.params.len() {
                            return Err("unsupported JIT CFG self tail call".to_string());
                        }
                        for (idx, (_, var)) in tail_ctx.params.iter().enumerate() {
                            let value = use_local(
                                builder,
                                bindings
                                    .get(&args[idx])
                                    .cloned()
                                    .ok_or_else(|| "missing JIT CFG tail-call binding".to_string())?,
                            );
                            let value =
                                box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
                            builder.def_var(*var, value);
                        }
                        builder.ins().jump(tail_ctx.loop_block, &[]);
                        continue;
                    }

                    let mut arg_vals = Vec::with_capacity(args.len());
                    for arg in args {
                        let value = use_local(
                            builder,
                            bindings
                                .get(arg)
                                .cloned()
                                .ok_or_else(|| "missing JIT CFG tail-call arg binding".to_string())?,
                        );
                        arg_vals.push(value);
                    }

                    // `None` means the thunk was emitted and the function already
                    // returned — skip the jump_with_jit_value call below.
                    let value: Option<JitValue> = match callee {
                        IrCallTarget::Named(name) => {
                            if let Some(meta) = scope.functions.get(name).copied() {
                                if !meta.has_contract && meta.num_params == arg_vals.len() {
                                    emit_mutual_tail_thunk(
                                        module,
                                        helpers,
                                        builder,
                                        ctx_val,
                                        meta.function_index,
                                        &arg_vals,
                                    );
                                    None
                                } else {
                                    Some(compile_jit_cfg_user_function_call(
                                        module,
                                        helpers,
                                        builder,
                                        ctx_val,
                                        meta,
                                        &arg_vals,
                                        Span::default(),
                                    )?)
                                }
                            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                Some(compile_jit_cfg_base_function_call(
                                    module,
                                    helpers,
                                    builder,
                                    ctx_val,
                                    base_idx,
                                    &arg_vals,
                                    Span::default(),
                                )?)
                            } else {
                                return Err("missing JIT CFG named tail-call target".to_string());
                            }
                        }
                        IrCallTarget::Direct(id) => {
                            let meta = scope.ir_functions.get(id).copied().ok_or_else(|| {
                                "missing JIT CFG direct tail-call target".to_string()
                            })?;
                            if !meta.has_contract && meta.num_params == arg_vals.len() {
                                emit_mutual_tail_thunk(
                                    module,
                                    helpers,
                                    builder,
                                    ctx_val,
                                    meta.function_index,
                                    &arg_vals,
                                );
                                None
                            } else {
                                Some(compile_jit_cfg_user_function_call(
                                    module,
                                    helpers,
                                    builder,
                                    ctx_val,
                                    meta,
                                    &arg_vals,
                                    Span::default(),
                                )?)
                            }
                        }
                        IrCallTarget::Var(var) => {
                            let callee = use_local(
                                builder,
                                bindings.get(var).cloned().ok_or_else(|| {
                                    "missing JIT CFG var tail-call target".to_string()
                                })?,
                            );
                            Some(compile_jit_cfg_generic_call(
                                module,
                                helpers,
                                builder,
                                ctx_val,
                                callee,
                                &arg_vals,
                                Span::default(),
                            )?)
                        }
                    };
                    if let Some(value) = value {
                        jump_with_jit_value(builder, return_block, value);
                    }
                }
                IrTerminator::Unreachable(_) => {
                    builder.ins().trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
                }
            }
        }

        // Seal all deferred blocks now that all predecessors have been
        // emitted (self-tail-call back-edges, join block back-edges, etc.).
        for block_id in &deferred_seal {
            builder.seal_block(block_map[block_id]);
        }

        Ok(())
    })())
}

fn compile_statement(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    top_level: bool,
    stmt: &Statement,
    interner: &Interner,
) -> Result<StmtOutcome, String> {
    match stmt {
        Statement::Let { name, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
            if top_level {
                if let Some(&idx) = scope.globals.get(name) {
                    let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                    let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                    let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
                    builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
                } else {
                    let boxed =
                        JitValue::boxed(box_jit_value(module, helpers, builder, ctx_val, val));
                    bind_local(builder, scope, *name, boxed);
                }
            } else {
                bind_local(builder, scope, *name, val);
            }
            Ok(StmtOutcome::None)
        }
        Statement::LetDestructure { pattern, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
            emit_return_on_null_value(builder, boxed);
            if top_level {
                bind_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, pattern, boxed,
                )?;
            } else {
                bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, boxed)?;
            }
            Ok(StmtOutcome::None)
        }
        Statement::Expression {
            expression,
            has_semicolon,
            ..
        } => {
            if !*has_semicolon
                && let Some(tc) = tail_call
                && let Some(fn_name) = tc.function_name
                && let Expression::Call {
                    function,
                    arguments,
                    ..
                } = expression
                && let Expression::Identifier { name, .. } = function.as_ref()
                && *name == fn_name
                && arguments.len() == tc.params.len()
            {
                let mut arg_vals = Vec::with_capacity(arguments.len());
                for arg in arguments {
                    arg_vals.push(compile_expression(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?);
                }
                for (idx, (_, var)) in tc.params.iter().enumerate() {
                    let boxed = box_jit_value(module, helpers, builder, ctx_val, arg_vals[idx]);
                    builder.def_var(*var, boxed);
                }
                builder.ins().jump(tc.loop_block, &[]);
                return Ok(StmtOutcome::Returned);
            }

            let val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                expression,
                interner,
            )?;
            if *has_semicolon {
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
                Ok(StmtOutcome::None)
            } else {
                Ok(StmtOutcome::Value(val))
            }
        }
        Statement::Assign { name, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
            if let Some(binding) = scope.locals.get(name).cloned() {
                if binding.kind == val.kind {
                    builder.def_var(binding.var, val.value);
                } else {
                    bind_local(builder, scope, *name, val);
                }
            } else if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
                builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
            }
            Ok(StmtOutcome::None)
        }
        Statement::Return { value, .. } => {
            let Some(rb) = return_block else {
                return Err("return outside function is not supported in JIT".to_string());
            };
            if let (
                Some(tc),
                Some(Expression::Call {
                    function,
                    arguments,
                    ..
                }),
            ) = (tail_call, value)
                && let Some(fn_name) = tc.function_name
                && let Expression::Identifier { name, .. } = function.as_ref()
                && *name == fn_name
                && arguments.len() == tc.params.len()
            {
                let mut arg_vals = Vec::with_capacity(arguments.len());
                for arg in arguments {
                    arg_vals.push(compile_expression(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?);
                }
                for (idx, (_, var)) in tc.params.iter().enumerate() {
                    let boxed = box_jit_value(module, helpers, builder, ctx_val, arg_vals[idx]);
                    builder.def_var(*var, boxed);
                }
                builder.ins().jump(tc.loop_block, &[]);
                return Ok(StmtOutcome::Returned);
            }
            let ret = match value {
                Some(v) => compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    v,
                    interner,
                )?,
                None => {
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    JitValue::boxed(builder.inst_results(call)[1])
                }
            };
            jump_with_jit_value(builder, rb, ret);
            Ok(StmtOutcome::Returned)
        }
        Statement::Function { name, .. } => {
            let Statement::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } = stmt
            else {
                unreachable!()
            };
            let expr = Expression::Function {
                parameters: parameters.clone(),
                parameter_types: parameter_types.clone(),
                return_type: return_type.clone(),
                effects: effects.clone(),
                body: body.clone(),
                span: *span,
                id: ExprId::UNSET,
            };
            let fn_val = compile_function_literal(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                &expr,
                interner,
            )?;
            bind_local(builder, scope, *name, JitValue::boxed(fn_val));
            Ok(StmtOutcome::None)
        }
        Statement::Import {
            name,
            alias,
            except: _,
            ..
        } => {
            if !is_base_symbol(*name, interner) {
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
            Ok(StmtOutcome::None)
        }
        Statement::Module { name, .. } => {
            scope.imported_modules.insert(*name);
            Ok(StmtOutcome::None)
        }
        Statement::Data { .. } => {
            // ADT declarations are no-ops at runtime; constructors are called directly.
            Ok(StmtOutcome::None)
        }
        Statement::EffectDecl { .. } => {
            // Effect declarations are syntax-only; no JIT code emitted.
            Ok(StmtOutcome::None)
        }
    }
}

#[allow(dead_code)]
fn compile_ir_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    interner: &Interner,
) -> Result<JitValue, String> {
    match expr {
        Expression::Identifier { name, .. } => {
            if let Some(binding) = scope.locals.get(name).cloned() {
                Ok(use_local(builder, binding))
            } else if let Some(meta) = scope.functions.get(name).copied() {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base =
                    get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if scope.adt_constructors.get(name).copied() == Some(0) {
                let name_str = interner.resolve(*name);
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

                let intern_adt =
                    get_helper_func_ref(module, helpers, builder, "rt_intern_unit_adt");
                let call = builder.ins().call(intern_adt, &[ctx_val, name_ptr, name_len]);

                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else {
                Err(format!("undefined identifier: {}", interner.resolve(*name)))
            }
        }
        Expression::MemberAccess { object, member, .. } => {
            if let Expression::Identifier { name, .. } = object.as_ref() {
                if is_base_symbol(*name, interner) {
                    let member_name = interner.resolve(*member);
                    let Some(index) = BaseModule::new().index_of(member_name) else {
                        return Err(format!("unknown Base member: {}", member_name));
                    };
                    let make_base =
                        get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                    let idx = builder.ins().iconst(PTR_TYPE, index as i64);
                    let call = builder.ins().call(make_base, &[ctx_val, idx]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }

                let module_name = scope.import_aliases.get(name).copied().or_else(|| {
                    if scope.imported_modules.contains(name)
                        || scope
                            .module_functions
                            .keys()
                            .any(|(module_name, _)| module_name == name)
                    {
                        Some(*name)
                    } else {
                        None
                    }
                });

                if let Some(module_name) = module_name {
                    if let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied()
                    {
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

                    return Err(format!(
                        "unknown module member: {}.{}",
                        interner.resolve(module_name),
                        interner.resolve(*member)
                    ));
                }
            }

            Err("unsupported member access in JIT (only Module.member is supported)".to_string())
        }
        Expression::Prefix {
            operator, right, ..
        } => {
            let operand = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            if operator == "-" && operand.kind == JitValueKind::Int {
                return Ok(JitValue::int(builder.ins().ineg(operand.value)));
            }
            if operator == "!" && operand.kind == JitValueKind::Bool {
                let one = builder.ins().iconst(types::I64, 1);
                return Ok(JitValue::bool(builder.ins().bxor(operand.value, one)));
            }
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unknown prefix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
            let (operand_tag, operand_payload) = jit_value_to_tag_payload(builder, operand);
            let call = builder
                .ins()
                .call(func_ref, &[ctx_val, operand_tag, operand_payload]);
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
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            if operator == "&&" || operator == "||" {
                return compile_ir_short_circuit_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    left,
                    operator,
                    right,
                    interner,
                );
            }
            let lhs = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let rhs = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int {
                match operator.as_str() {
                    "+" => return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value))),
                    "-" => return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value))),
                    "*" => return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value))),
                    "/" | "%" => {
                        let is_zero = builder.ins().icmp_imm(IntCC::Equal, rhs.value, 0);
                        let err_block = builder.create_block();
                        let ok_block = builder.create_block();
                        builder.ins().brif(is_zero, err_block, &[], ok_block, &[]);

                        builder.switch_to_block(err_block);
                        let dbz =
                            get_helper_func_ref(module, helpers, builder, "rt_division_by_zero");
                        builder.ins().call(dbz, &[ctx_val]);
                        emit_return_null_tagged(builder);
                        builder.seal_block(err_block);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let result = if operator == "/" {
                            builder.ins().sdiv(lhs.value, rhs.value)
                        } else {
                            builder.ins().srem(lhs.value, rhs.value)
                        };
                        return Ok(JitValue::int(result));
                    }
                    "==" | "!=" | ">" | "<" | "<=" | ">=" => {
                        let cc = match operator.as_str() {
                            "==" => IntCC::Equal,
                            "!=" => IntCC::NotEqual,
                            ">" => IntCC::SignedGreaterThan,
                            "<" => IntCC::SignedLessThan,
                            "<=" => IntCC::SignedLessThanOrEqual,
                            ">=" => IntCC::SignedGreaterThanOrEqual,
                            _ => unreachable!(),
                        };
                        let cmp = builder.ins().icmp(cc, lhs.value, rhs.value);
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let bool_i64 = builder.ins().select(cmp, one, zero);
                        return Ok(JitValue::bool(bool_i64));
                    }
                    _ => {}
                }
            }
            if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                match operator.as_str() {
                    "+" => {
                        let result = builder.ins().fadd(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "-" => {
                        let result = builder.ins().fsub(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "*" => {
                        let result = builder.ins().fmul(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "/" => {
                        let result = builder.ins().fdiv(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "==" | "!=" | ">" | "<" | "<=" | ">=" => {
                        let cc = match operator.as_str() {
                            "==" => cranelift_codegen::ir::condcodes::FloatCC::Equal,
                            "!=" => cranelift_codegen::ir::condcodes::FloatCC::NotEqual,
                            ">" => cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                            "<" => cranelift_codegen::ir::condcodes::FloatCC::LessThan,
                            "<=" => {
                                cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual
                            }
                            ">=" => {
                                cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual
                            }
                            _ => unreachable!(),
                        };
                        let cmp = builder.ins().fcmp(cc, lhsf, rhsf);
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let bool_i64 = builder.ins().select(cmp, one, zero);
                        return Ok(JitValue::bool(bool_i64));
                    }
                    _ => {}
                }
            }
            let (lhs_tag, lhs_payload) = jit_value_to_tag_payload(builder, lhs);
            let (rhs_tag, rhs_payload) = jit_value_to_tag_payload(builder, rhs);
            let helper_name = match operator.as_str() {
                "+" => "rt_add",
                "-" => "rt_sub",
                "*" => "rt_mul",
                "/" => "rt_div",
                "%" => "rt_mod",
                "==" => "rt_equal",
                "!=" => "rt_not_equal",
                ">" => "rt_greater_than",
                "<=" => "rt_less_than_or_equal",
                ">=" => "rt_greater_than_or_equal",
                "<" => {
                    let ge_ref =
                        get_helper_func_ref(module, helpers, builder, "rt_greater_than_or_equal");
                    let ge_call = builder.ins().call(
                        ge_ref,
                        &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
                    );
                    let ge_tag = builder.inst_results(ge_call)[0];
                    let ge_payload = builder.inst_results(ge_call)[1];
                    let not_ref = get_helper_func_ref(module, helpers, builder, "rt_not");
                    let not_call =
                        builder.ins().call(not_ref, &[ctx_val, ge_tag, ge_payload]);
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
                _ => return Err(format!("unknown infix operator: {}", operator)),
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
        Expression::Function {
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => {
            let closure = compile_ir_function_literal(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                *span,
                interner,
            )?;
            Ok(JitValue::boxed(closure))
        }
        Expression::Integer { value, .. } => {
            Ok(JitValue::int(builder.ins().iconst(types::I64, *value)))
        }
        Expression::Float { value, .. } => Ok(JitValue::float(
            builder.ins().iconst(types::I64, value.to_bits() as i64),
        )),
        Expression::Boolean { value, .. } => {
            Ok(JitValue::bool(builder.ins().iconst(types::I64, *value as i64)))
        }
        Expression::None { .. } => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        Expression::EmptyList { .. } => {
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let call = builder.ins().call(make_empty, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::String { value, .. } => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = cranelift_module::DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::TupleLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let elems_ptr = function_compiler.emit_tagged_array(builder, &elem_vals);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_tuple = get_helper_func_ref(module, helpers, builder, "rt_make_tuple");
            let call = builder
                .ins()
                .call(make_tuple, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::InterpolatedString { parts, .. } => {
            use crate::syntax::expression::StringPart;
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_add = get_helper_func_ref(module, helpers, builder, "rt_add");

            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    StringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = cranelift_module::DataDescription::new();
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
                    StringPart::Interpolation(expr) => {
                        let val = compile_ir_expression(
                            module,
                            helpers,
                            builder,
                            function_compiler,
                            scope,
                            ctx_val,
                            return_block,
                            tail_call,
                            expr,
                            interner,
                        )?;
                        let val = box_jit_value(module, helpers, builder, ctx_val, val);
                        let call = builder.ins().call(rt_to_string, &[ctx_val, val]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                        let call = builder
                            .ins()
                            .call(rt_add, &[ctx_val, tag, prev, tag, part_val]);
                        builder.inst_results(call)[1]
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
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => compile_ir_if_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            condition,
            consequence,
            alternative.as_ref(),
            interner,
        ),
        Expression::DoBlock { block, .. } => {
            compile_ir_block_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                block,
                interner,
            )
            .map(|eval| match eval {
                BlockEval::Value(v) => v,
                BlockEval::Returned => {
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    JitValue::boxed(builder.inst_results(call)[1])
                }
            })
        }
        Expression::Call {
            function,
            arguments,
            span,
            ..
        } => {
            if let Some(primop) = resolve_ir_call_primop(scope, function, arguments, interner) {
                return compile_ir_primop_call(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    primop,
                    *span,
                    arguments,
                    interner,
                );
            }
            if let Expression::Identifier { name, .. } = function.as_ref() {
                if let Some(&arity) = scope.adt_constructors.get(name) {
                    return compile_ir_adt_constructor_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        *name,
                        arity,
                        arguments,
                        interner,
                    );
                }
                if let Some(&base_idx) = scope.base_functions.get(name) {
                    return compile_ir_base_function_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        *span,
                        base_idx,
                        arguments,
                        interner,
                    );
                }
                if let Some(meta) = scope.functions.get(name).copied() {
                    return compile_ir_user_function_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        meta,
                        *span,
                        arguments,
                        interner,
                    );
                }
            }
            if let Expression::MemberAccess { object, member, .. } = function.as_ref()
                && let Expression::Identifier { name, .. } = object.as_ref()
            {
                let module_name = scope.import_aliases.get(name).copied().or_else(|| {
                    if scope.imported_modules.contains(name)
                        || scope
                            .module_functions
                            .keys()
                            .any(|(module_name, _)| module_name == name)
                    {
                        Some(*name)
                    } else {
                        None
                    }
                });
                if let Some(module_name) = module_name
                    && let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied()
                {
                    return compile_ir_user_function_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        meta,
                        *span,
                        arguments,
                        interner,
                    );
                }
            }
            compile_ir_generic_call(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                *span,
                function,
                arguments,
                interner,
            )
        }
        Expression::ListLiteral { .. }
        | Expression::ArrayLiteral { .. }
        | Expression::Hash { .. }
        | Expression::Index { .. }
        | Expression::TupleFieldAccess { .. }
        | Expression::Some { .. }
        | Expression::Left { .. }
        | Expression::Right { .. }
        | Expression::Cons { .. } => compile_ir_collection_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            expr,
            interner,
        ),
        Expression::Match {
            scrutinee, arms, ..
        } => {
            compile_ir_match_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                scrutinee,
                arms,
                interner,
            )
        }
        Expression::Perform {
            effect,
            operation,
            args,
            span,
            ..
        } => compile_ir_jit_perform(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            *effect,
            *operation,
            args,
            interner,
            *span,
        ),
        Expression::Handle {
            expr,
            effect,
            arms,
            ..
        } => compile_ir_jit_handle(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            expr,
            *effect,
            arms,
            interner,
        ),
    }
}

fn compile_ir_collection_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    interner: &Interner,
) -> Result<JitValue, String> {
    match expr {
        Expression::TupleFieldAccess { object, index, .. } => {
            let tuple_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                object,
                interner,
            )?;
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
        Expression::Cons { head, tail, .. } => {
            let head_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                head,
                interner,
            )?;
            let tail_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                tail,
                interner,
            )?;
            let head_val = box_jit_value(module, helpers, builder, ctx_val, head_val);
            let tail_val = box_jit_value(module, helpers, builder, ctx_val, tail_val);
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder
                .ins()
                .call(make_cons, &[ctx_val, head_val, tail_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Some { value, .. } => {
            let inner = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_some = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(make_some, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Left { value, .. } => {
            let inner = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_left = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(make_left, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Right { value, .. } => {
            let inner = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_right = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(make_right, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::ArrayLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let elems_ptr = function_compiler.emit_tagged_array(builder, &elem_vals);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_array = get_helper_func_ref(module, helpers, builder, "rt_make_array");
            let call = builder
                .ins()
                .call(make_array, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::ListLiteral { elements, .. } => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let none_call = builder.ins().call(make_none, &[ctx_val]);
            let mut acc = builder.inst_results(none_call)[1];
            for elem in elements.iter().rev() {
                let val = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                let val = box_jit_value(module, helpers, builder, ctx_val, val);
                let cons_call = builder.ins().call(make_cons, &[ctx_val, val, acc]);
                acc = builder.inst_results(cons_call)[0];
            }
            Ok(JitValue::boxed(acc))
        }
        Expression::Hash { pairs, .. } => {
            let npairs = pairs.len();
            let mut pair_vals = Vec::with_capacity(npairs * 2);
            for (key, value) in pairs {
                let k = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    key,
                    interner,
                )?;
                let v = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    value,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, k);
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, v);
                pair_vals.push(k);
                pair_vals.push(v);
            }
            let pairs_ptr = function_compiler.emit_tagged_array(builder, &pair_vals);
            let npairs_val = builder.ins().iconst(PTR_TYPE, npairs as i64);
            let make_hash = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder
                .ins()
                .call(make_hash, &[ctx_val, pairs_ptr, npairs_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Index { left, index, .. } => {
            let left_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let index_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                index,
                interner,
            )?;
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let left_val = box_jit_value(module, helpers, builder, ctx_val, left_val);
            let index_val = box_jit_value(module, helpers, builder, ctx_val, index_val);
            let call = builder
                .ins()
                .call(rt_index, &[ctx_val, left_val, index_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        _ => unreachable!("non-collection expression passed to compile_ir_collection_expression"),
    }
}

fn compile_ir_top_level_item(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    top_level: bool,
    item: &IrTopLevelItem,
    interner: &Interner,
) -> Result<StmtOutcome, String> {
    match item {
        IrTopLevelItem::Let { name, value, .. } => {
            let val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
            if top_level {
                if let Some(&idx) = scope.globals.get(name) {
                    let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                    let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                    let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
                    builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
                } else {
                    let boxed =
                        JitValue::boxed(box_jit_value(module, helpers, builder, ctx_val, val));
                    bind_local(builder, scope, *name, boxed);
                }
            } else {
                bind_local(builder, scope, *name, val);
            }
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::LetDestructure { pattern, value, .. } => {
            let val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
            emit_return_on_null_value(builder, boxed);
            if top_level {
                bind_ir_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, pattern, boxed,
                )?;
            } else {
                bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, pattern, boxed)?;
            }
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::Expression {
            expression,
            has_semicolon,
            ..
        } => {
            let value = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                expression,
                interner,
            )?;
            if *has_semicolon {
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, value);
                Ok(StmtOutcome::None)
            } else {
                Ok(StmtOutcome::Value(value))
            }
        }
        IrTopLevelItem::Assign { name, value, .. } => {
            let val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
            if let Some(binding) = scope.locals.get(name).cloned() {
                if binding.kind == val.kind {
                    builder.def_var(binding.var, val.value);
                } else {
                    bind_local(builder, scope, *name, val);
                }
            } else if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let boxed = box_jit_value(module, helpers, builder, ctx_val, val);
                builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
            }
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::Return { value, .. } => {
            let Some(rb) = return_block else {
                return Err("return outside function is not supported in JIT".to_string());
            };
            let ret = match value {
                Some(v) => compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    v,
                    interner,
                )?,
                None => {
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    JitValue::boxed(builder.inst_results(call)[1])
                }
            };
            jump_with_jit_value(builder, rb, ret);
            Ok(StmtOutcome::Returned)
        }
        IrTopLevelItem::Function {
            name,
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => {
            let closure = compile_ir_function_literal(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                *span,
                interner,
            )?;
            bind_local(builder, scope, *name, JitValue::boxed(closure));
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::Import {
            name,
            alias,
            except: _,
            ..
        } => {
            if !is_base_symbol(*name, interner) {
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::Module { name, .. } => {
            scope.imported_modules.insert(*name);
            Ok(StmtOutcome::None)
        }
        IrTopLevelItem::Data { .. } | IrTopLevelItem::EffectDecl { .. } => Ok(StmtOutcome::None),
    }
}

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

fn try_compile_tail_expression_statement(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_ctx: &TailCallContext,
    stmt: &Statement,
    interner: &Interner,
) -> Result<Option<StmtOutcome>, String> {
    let Statement::Expression { expression, .. } = stmt else {
        return Ok(None);
    };
    let Expression::Call {
        function,
        arguments,
        ..
    } = expression
    else {
        return Ok(None);
    };
    let Expression::Identifier { name, .. } = function.as_ref() else {
        return Ok(None);
    };

    // Case 1: self-tail-call — jump back to the loop block without a call frame.
    if let Some(fn_name) = tail_ctx.function_name
        && *name == fn_name
        && arguments.len() == tail_ctx.params.len()
    {
        let mut arg_vals = Vec::with_capacity(arguments.len());
        for arg in arguments {
            arg_vals.push(compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                Some(tail_ctx),
                arg,
                interner,
            )?);
        }
        for (idx, (_, var)) in tail_ctx.params.iter().enumerate() {
            let boxed = box_jit_value(module, helpers, builder, ctx_val, arg_vals[idx]);
            builder.def_var(*var, boxed);
        }
        builder.ins().jump(tail_ctx.loop_block, &[]);
        return Ok(Some(StmtOutcome::Returned));
    }

    // Case 2: mutual tail call to another known JIT function — use the trampoline.
    // Only applies to contract-free functions where we can skip runtime type checks.
    if let Some(meta) = scope.functions.get(name).copied()
        && !meta.has_contract
        && arguments.len() == meta.num_params
    {
        let mut arg_vals = Vec::with_capacity(arguments.len());
        for arg in arguments {
            arg_vals.push(compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                Some(tail_ctx),
                arg,
                interner,
            )?);
        }
        emit_mutual_tail_thunk(
            module,
            helpers,
            builder,
            ctx_val,
            meta.function_index,
            &arg_vals,
        );
        return Ok(Some(StmtOutcome::Returned));
    }

    Ok(None)
}

fn compile_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    interner: &Interner,
) -> Result<JitValue, String> {
    match expr {
        // --- Literals ---
        Expression::Integer { value, .. } => {
            Ok(JitValue::int(builder.ins().iconst(types::I64, *value)))
        }
        Expression::Float { value, .. } => {
            Ok(JitValue::float(
                builder.ins().iconst(types::I64, value.to_bits() as i64),
            ))
        }
        Expression::Boolean { value, .. } => {
            let v = builder.ins().iconst(types::I64, *value as i64);
            Ok(JitValue::bool(v))
        }
        Expression::None { .. } => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        Expression::EmptyList { .. } => {
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let call = builder.ins().call(make_empty, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::String { value, .. } => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = cranelift_module::DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::TupleLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let elems_ptr = function_compiler.emit_tagged_array(builder, &elem_vals);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_tuple = get_helper_func_ref(module, helpers, builder, "rt_make_tuple");
            let call = builder
                .ins()
                .call(make_tuple, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::TupleFieldAccess { object, index, .. } => {
            let tuple_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                object,
                interner,
            )?;
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

        // --- Identifiers ---
        Expression::Identifier { name, .. } => {
            if let Some(binding) = scope.locals.get(name).cloned() {
                Ok(use_local(builder, binding))
            } else if let Some(meta) = scope.functions.get(name).copied() {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base =
                    get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if scope.adt_constructors.get(name).copied() == Some(0) {
                // Zero-arg ADT constructor used as a value (e.g. `Point`, `None_`).
                // Use the interning helper so each unique name is allocated only once.
                let name_str = interner.resolve(*name);
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

                let intern_adt =
                    get_helper_func_ref(module, helpers, builder, "rt_intern_unit_adt");
                let call = builder.ins().call(intern_adt, &[ctx_val, name_ptr, name_len]);

                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else {
                Err(format!("undefined identifier: {}", interner.resolve(*name)))
            }
        }
        Expression::MemberAccess { object, member, .. } => {
            if let Expression::Identifier { name, .. } = object.as_ref() {
                if is_base_symbol(*name, interner) {
                    let member_name = interner.resolve(*member);
                    let Some(index) = BaseModule::new().index_of(member_name) else {
                        return Err(format!("unknown Base member: {}", member_name));
                    };
                    let make_base =
                        get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                    let idx = builder.ins().iconst(PTR_TYPE, index as i64);
                    let call = builder.ins().call(make_base, &[ctx_val, idx]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }

                let module_name = scope.import_aliases.get(name).copied().or_else(|| {
                    if scope.imported_modules.contains(name)
                        || scope
                            .module_functions
                            .keys()
                            .any(|(module_name, _)| module_name == name)
                    {
                        Some(*name)
                    } else {
                        None
                    }
                });

                if let Some(module_name) = module_name {
                    if let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied()
                    {
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

                    return Err(format!(
                        "unknown module member: {}.{}",
                        interner.resolve(module_name),
                        interner.resolve(*member)
                    ));
                }
            }

            Err("unsupported member access in JIT (only Module.member is supported)".to_string())
        }

        // --- Prefix operators ---
        Expression::Prefix {
            operator, right, ..
        } => {
            let operand = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            if operator == "-" && operand.kind == JitValueKind::Int {
                return Ok(JitValue::int(builder.ins().ineg(operand.value)));
            }
            if operator == "!" && operand.kind == JitValueKind::Bool {
                let one = builder.ins().iconst(types::I64, 1);
                return Ok(JitValue::bool(builder.ins().bxor(operand.value, one)));
            }
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unknown prefix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
            let (operand_tag, operand_payload) = jit_value_to_tag_payload(builder, operand);
            let call = builder
                .ins()
                .call(func_ref, &[ctx_val, operand_tag, operand_payload]);
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

        // --- Infix operators ---
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            if operator == "&&" || operator == "||" {
                return compile_short_circuit_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    left,
                    operator,
                    right,
                    interner,
                );
            }
            let lhs = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let rhs = compile_expression(
                module,
                helpers,
                builder,
                    function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int {
                match operator.as_str() {
                    "+" => return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value))),
                    "-" => return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value))),
                    "*" => return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value))),
                    "/" | "%" => {
                        // Inline div/mod with div-by-zero guard.
                        let is_zero = builder.ins().icmp_imm(IntCC::Equal, rhs.value, 0);
                        let err_block = builder.create_block();
                        let ok_block = builder.create_block();
                        builder.ins().brif(is_zero, err_block, &[], ok_block, &[]);

                        // Error path: set "division by zero" and return null.
                        builder.switch_to_block(err_block);
                        let dbz =
                            get_helper_func_ref(module, helpers, builder, "rt_division_by_zero");
                        builder.ins().call(dbz, &[ctx_val]);
                        emit_return_null_tagged(builder);
                        builder.seal_block(err_block);

                        // Normal path: emit sdiv/srem.
                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let result = if operator == "/" {
                            builder.ins().sdiv(lhs.value, rhs.value)
                        } else {
                            builder.ins().srem(lhs.value, rhs.value)
                        };
                        return Ok(JitValue::int(result));
                    }
                    "==" | "!=" | ">" | "<" | "<=" | ">=" => {
                        let cc = match operator.as_str() {
                            "==" => IntCC::Equal,
                            "!=" => IntCC::NotEqual,
                            ">" => IntCC::SignedGreaterThan,
                            "<" => IntCC::SignedLessThan,
                            "<=" => IntCC::SignedLessThanOrEqual,
                            ">=" => IntCC::SignedGreaterThanOrEqual,
                            _ => unreachable!(),
                        };
                        let cmp = builder.ins().icmp(cc, lhs.value, rhs.value);
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let bool_i64 = builder.ins().select(cmp, one, zero);
                        return Ok(JitValue::bool(bool_i64));
                    }
                    _ => {}
                }
            }
            if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                match operator.as_str() {
                    "+" => {
                        let result = builder.ins().fadd(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "-" => {
                        let result = builder.ins().fsub(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "*" => {
                        let result = builder.ins().fmul(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "/" => {
                        let result = builder.ins().fdiv(lhsf, rhsf);
                        let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                        return Ok(JitValue::float(bits));
                    }
                    "==" | "!=" | ">" | "<" | "<=" | ">=" => {
                        let cc = match operator.as_str() {
                            "==" => cranelift_codegen::ir::condcodes::FloatCC::Equal,
                            "!=" => cranelift_codegen::ir::condcodes::FloatCC::NotEqual,
                            ">" => cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                            "<" => cranelift_codegen::ir::condcodes::FloatCC::LessThan,
                            "<=" => {
                                cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual
                            }
                            ">=" => {
                                cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual
                            }
                            _ => unreachable!(),
                        };
                        let cmp = builder.ins().fcmp(cc, lhsf, rhsf);
                        let one = builder.ins().iconst(types::I64, 1);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let bool_i64 = builder.ins().select(cmp, one, zero);
                        return Ok(JitValue::bool(bool_i64));
                    }
                    _ => {}
                }
            }
            let (lhs_tag, lhs_payload) = jit_value_to_tag_payload(builder, lhs);
            let (rhs_tag, rhs_payload) = jit_value_to_tag_payload(builder, rhs);
            let helper_name = match operator.as_str() {
                "+" => "rt_add",
                "-" => "rt_sub",
                "*" => "rt_mul",
                "/" => "rt_div",
                "%" => "rt_mod",
                "==" => "rt_equal",
                "!=" => "rt_not_equal",
                ">" => "rt_greater_than",
                "<=" => "rt_less_than_or_equal",
                ">=" => "rt_greater_than_or_equal",
                "<" => {
                    // a < b  ⟹  !(a >= b)
                    let ge_ref =
                        get_helper_func_ref(module, helpers, builder, "rt_greater_than_or_equal");
                    let ge_call = builder.ins().call(
                        ge_ref,
                        &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
                    );
                    let ge_tag = builder.inst_results(ge_call)[0];
                    let ge_payload = builder.inst_results(ge_call)[1];
                    let not_ref = get_helper_func_ref(module, helpers, builder, "rt_not");
                    let not_call =
                        builder.ins().call(not_ref, &[ctx_val, ge_tag, ge_payload]);
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
                _ => return Err(format!("unknown infix operator: {}", operator)),
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
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => compile_if_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            condition,
            consequence,
            alternative.as_ref(),
            interner,
        ),
        Expression::DoBlock { block, .. } => {
            match compile_block_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                block,
                interner,
            )? {
                BlockEval::Returned => {
                    // The do-block already terminated control-flow (e.g. tail-call jump).
                    // Continue emission in a fresh block to avoid appending instructions
                    // after a terminator in the previous block.
                    let continue_block = builder.create_block();
                    builder.switch_to_block(continue_block);
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    builder.seal_block(continue_block);
                    Ok(JitValue::boxed(builder.inst_results(call)[1]))
                }
                BlockEval::Value(v) => Ok(v),
            }
        }

        // --- Function calls ---
        Expression::Call {
            function,
            arguments,
            span,
            ..
        } => {
            if let Some(primop) = resolve_call_primop(scope, function, arguments, interner) {
                return compile_primop_call(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    primop,
                    *span,
                    arguments,
                    interner,
                );
            }
            // Check if calling a registered ADT constructor
            if let Expression::Identifier { name, .. } = function.as_ref()
                && let Some(&arity) = scope.adt_constructors.get(name)
            {
                let name_str = interner.resolve(*name);
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

                let mut arg_vals = Vec::with_capacity(arguments.len());

                for arg in arguments {
                    let value = compile_expression(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?;
                    emit_return_on_null_jit_value(module, helpers, builder, ctx_val, value);
                    arg_vals.push(value);
                }

                let boxed_arg_vals: Vec<_> = arg_vals
                    .iter()
                    .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
                    .collect();

                emit_push_gc_roots(module, helpers, builder, ctx_val, &boxed_arg_vals);

                // Use specialized helpers for arity 1-5 to avoid stack-slot + loop overhead.
                let call = match arity {
                    1 => {
                        let make_adt1 =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
                        builder
                            .ins()
                            .call(make_adt1, &[ctx_val, name_ptr, name_len, boxed_arg_vals[0]])
                    }
                    2 => {
                        let make_adt2 =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
                        builder.ins().call(
                            make_adt2,
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
                        let make_adt3 =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
                        builder.ins().call(
                            make_adt3,
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
                        let make_adt4 =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
                        builder.ins().call(
                            make_adt4,
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
                        let make_adt5 =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
                        builder.ins().call(
                            make_adt5,
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
                        // Fallback for arity 0 and arity >= 6: use generic rt_make_adt.
                        let fields_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
                        let arity_value = builder.ins().iconst(PTR_TYPE, arity as i64);
                        let make_adt =
                            get_helper_func_ref(module, helpers, builder, "rt_make_adt");
                        builder.ins().call(
                            make_adt,
                            &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
                        )
                    }
                };
                emit_pop_gc_roots(module, helpers, builder, ctx_val);

                return Ok(JitValue::boxed(builder.inst_results(call)[0]));
            }
            // Check if calling a base directly
            if let Expression::Identifier { name, .. } = function.as_ref() {
                if let Some(meta) = scope.functions.get(name).copied() {
                    return compile_user_function_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        meta,
                        *span,
                        arguments,
                        interner,
                    );
                }
                if should_use_base_fastcall(scope, *name, interner)
                    && let Some(&base_idx) = scope.base_functions.get(name)
                {
                    return compile_base_function_call(
                        module,
                        helpers,
                        builder,
                        function_compiler,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        *span,
                        base_idx,
                        arguments,
                        interner,
                    );
                }
            }
            compile_generic_call(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                *span,
                function,
                arguments,
                interner,
            )
        }
        Expression::Function { .. } => {
            compile_function_literal(module, helpers, builder, scope, ctx_val, expr, interner)
                .map(JitValue::boxed)
        }
        Expression::Cons { head, tail, .. } => {
            let head_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                head,
                interner,
            )?;
            let tail_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                tail,
                interner,
            )?;
            let head_val = box_jit_value(module, helpers, builder, ctx_val, head_val);
            let tail_val = box_jit_value(module, helpers, builder, ctx_val, tail_val);
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder
                .ins()
                .call(make_cons, &[ctx_val, head_val, tail_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Match {
            scrutinee, arms, ..
        } => compile_match_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            scrutinee,
            arms,
            interner,
        ),

        Expression::Some { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_some = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(make_some, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Left { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_left = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(make_left, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Right { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let make_right = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(make_right, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::ArrayLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, val);
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let elems_ptr = function_compiler.emit_tagged_array(builder, &elem_vals);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_array = get_helper_func_ref(module, helpers, builder, "rt_make_array");
            let call = builder
                .ins()
                .call(make_array, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::ListLiteral { elements, .. } => {
            // Build cons chain in reverse: start with None, prepend each element
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let none_call = builder.ins().call(make_none, &[ctx_val]);
            let mut acc = builder.inst_results(none_call)[1];
            for elem in elements.iter().rev() {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                let val = box_jit_value(module, helpers, builder, ctx_val, val);
                let cons_call = builder.ins().call(make_cons, &[ctx_val, val, acc]);
                acc = builder.inst_results(cons_call)[0];
            }
            Ok(JitValue::boxed(acc))
        }
        Expression::Hash { pairs, .. } => {
            let npairs = pairs.len();
            let mut pair_vals = Vec::with_capacity(npairs * 2);
            for (key, value) in pairs {
                let k = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    key,
                    interner,
                )?;
                let v = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    value,
                    interner,
                )?;
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, k);
                emit_return_on_null_jit_value(module, helpers, builder, ctx_val, v);
                pair_vals.push(k);
                pair_vals.push(v);
            }
            let pairs_ptr = function_compiler.emit_tagged_array(builder, &pair_vals);
            let npairs_val = builder.ins().iconst(PTR_TYPE, npairs as i64);
            let make_hash = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder
                .ins()
                .call(make_hash, &[ctx_val, pairs_ptr, npairs_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::Index { left, index, .. } => {
            let left_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let index_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                return_block,
                tail_call,
                index,
                interner,
            )?;
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let left_val = box_jit_value(module, helpers, builder, ctx_val, left_val);
            let index_val = box_jit_value(module, helpers, builder, ctx_val, index_val);
            let call = builder
                .ins()
                .call(rt_index, &[ctx_val, left_val, index_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        Expression::InterpolatedString { parts, .. } => {
            use crate::syntax::expression::StringPart;
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_add = get_helper_func_ref(module, helpers, builder, "rt_add");

            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    StringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = cranelift_module::DataDescription::new();
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
                    StringPart::Interpolation(expr) => {
                        let val = compile_expression(
                            module,
                            helpers,
                            builder,
                            function_compiler,
                            scope,
                            ctx_val,
                            return_block,
                            tail_call,
                            expr,
                            interner,
                        )?;
                        let val = box_jit_value(module, helpers, builder, ctx_val, val);
                        let call = builder.ins().call(rt_to_string, &[ctx_val, val]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                        let call = builder
                            .ins()
                            .call(rt_add, &[ctx_val, tag, prev, tag, part_val]);
                        builder.inst_results(call)[1]
                    }
                });
            }
            // Empty interpolated string edge case
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
        Expression::Perform {
            effect,
            operation,
            args,
            span,
            ..
        } => compile_jit_perform(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            *effect,
            *operation,
            args,
            interner,
            *span,
        ),
        Expression::Handle {
            expr, effect, arms, ..
        } => compile_jit_handle(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            expr,
            *effect,
            arms,
            interner,
        ),
    }
}

/// Compile `perform Effect.op(args)` in JIT mode.
#[allow(clippy::too_many_arguments)]
fn compile_jit_perform(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    effect: crate::syntax::symbol::Symbol,
    op: crate::syntax::symbol::Symbol,
    args: &[Expression],
    interner: &Interner,
    span: crate::diagnostics::position::Span,
) -> Result<JitValue, String> {
    let mut arg_vals: Vec<CraneliftValue> = Vec::new();
    for arg in args {
        let val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs.max(1) as u32) * 8,
        3,
    ));
    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }
    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let op_val = builder.ins().iconst(PTR_TYPE, op.as_u32() as i64);

    // Leak the name strings as stable pointers for the JIT runtime error messages.
    let effect_str: &'static str = Box::leak(interner.resolve(effect).to_owned().into_boxed_str());
    let op_str: &'static str = Box::leak(interner.resolve(op).to_owned().into_boxed_str());
    let effect_name_ptr = builder.ins().iconst(PTR_TYPE, effect_str.as_ptr() as i64);
    let effect_name_len = builder.ins().iconst(PTR_TYPE, effect_str.len() as i64);
    let op_name_ptr = builder.ins().iconst(PTR_TYPE, op_str.as_ptr() as i64);
    let op_name_len = builder.ins().iconst(PTR_TYPE, op_str.len() as i64);
    let line_val = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let col_val = builder.ins().iconst(PTR_TYPE, span.start.column as i64);

    let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
    let call = builder.ins().call(
        rt_perform,
        &[
            ctx_val,
            effect_val,
            op_val,
            args_ptr,
            nargs_val,
            effect_name_ptr,
            effect_name_len,
            op_name_ptr,
            op_name_len,
            line_val,
            col_val,
        ],
    );
    let result = builder.inst_results(call)[0];

    // Null result means rt_perform set an error in the context; propagate upward.
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn compile_ir_jit_perform(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    effect: crate::syntax::symbol::Symbol,
    op: crate::syntax::symbol::Symbol,
    args: &[Expression],
    interner: &Interner,
    span: crate::diagnostics::position::Span,
) -> Result<JitValue, String> {
    let mut arg_vals: Vec<CraneliftValue> = Vec::new();
    for arg in args {
        let val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let args_ptr = function_compiler.emit_boxed_array(builder, &arg_vals);
    let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let op_val = builder.ins().iconst(PTR_TYPE, op.as_u32() as i64);

    let effect_str: &'static str = Box::leak(interner.resolve(effect).to_owned().into_boxed_str());
    let op_str: &'static str = Box::leak(interner.resolve(op).to_owned().into_boxed_str());
    let effect_name_ptr = builder.ins().iconst(PTR_TYPE, effect_str.as_ptr() as i64);
    let effect_name_len = builder.ins().iconst(PTR_TYPE, effect_str.len() as i64);
    let op_name_ptr = builder.ins().iconst(PTR_TYPE, op_str.as_ptr() as i64);
    let op_name_len = builder.ins().iconst(PTR_TYPE, op_str.len() as i64);
    let line_val = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let col_val = builder.ins().iconst(PTR_TYPE, span.start.column as i64);

    let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
    let call = builder.ins().call(
        rt_perform,
        &[
            ctx_val,
            effect_val,
            op_val,
            args_ptr,
            nargs_val,
            effect_name_ptr,
            effect_name_len,
            op_name_ptr,
            op_name_len,
            line_val,
            col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

/// Compile `expr handle Effect { arms... }` in JIT mode.
#[allow(clippy::too_many_arguments)]
fn compile_jit_handle(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    effect: crate::syntax::symbol::Symbol,
    arms: &[crate::syntax::expression::HandleArm],
    interner: &Interner,
) -> Result<JitValue, String> {
    let num_arms = arms.len();
    let mut op_sym_vals: Vec<CraneliftValue> = Vec::new();
    let mut closure_vals: Vec<CraneliftValue> = Vec::new();

    for arm in arms {
        op_sym_vals.push(
            builder
                .ins()
                .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
        );

        // Build a synthetic Function expression for the arm body
        let mut params = vec![arm.resume_param];
        params.extend_from_slice(&arm.params);
        let arm_span = arm.body.span();
        let arm_fn_expr = Expression::Function {
            parameters: params,
            parameter_types: vec![None; 1 + arm.params.len()],
            return_type: None,
            effects: vec![],
            body: crate::syntax::block::Block {
                statements: vec![crate::syntax::statement::Statement::Expression {
                    expression: arm.body.clone(),
                    has_semicolon: false,
                    span: arm_span,
                }],
                span: arm_span,
            },
            span: arm.span,
            id: ExprId::UNSET,
        };
        let cv = compile_function_literal(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            &arm_fn_expr,
            interner,
        )?;
        closure_vals.push(cv);
    }

    // Store op symbols in a stack slot
    let ops_slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (num_arms.max(1) as u32) * 8,
        3,
    ));
    for (i, ov) in op_sym_vals.iter().enumerate() {
        builder.ins().stack_store(*ov, ops_slot, (i * 8) as i32);
    }
    let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);

    // Store closures in a stack slot
    let closures_ptr = function_compiler.emit_boxed_array(builder, &closure_vals);

    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let narms_val = builder.ins().iconst(PTR_TYPE, num_arms as i64);

    let rt_push_handler = get_helper_func_ref(module, helpers, builder, "rt_push_handler");
    builder.ins().call(
        rt_push_handler,
        &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
    );

    let result = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        expr,
        interner,
    )?;

    let rt_pop_handler = get_helper_func_ref(module, helpers, builder, "rt_pop_handler");
    builder.ins().call(rt_pop_handler, &[ctx_val]);

    Ok(result)
}

fn compile_ir_jit_handle(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    effect: crate::syntax::symbol::Symbol,
    arms: &[HandleArm],
    interner: &Interner,
) -> Result<JitValue, String> {
    let num_arms = arms.len();
    let mut op_sym_vals: Vec<CraneliftValue> = Vec::new();
    let mut closure_vals: Vec<CraneliftValue> = Vec::new();

    for arm in arms {
        op_sym_vals.push(
            builder
                .ins()
                .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
        );

        let mut params = vec![arm.resume_param];
        params.extend_from_slice(&arm.params);
        let arm_block = Block {
            statements: vec![Statement::Expression {
                expression: arm.body.clone(),
                has_semicolon: false,
                span: arm.span,
            }],
            span: arm.span,
        };
        let cv = compile_ir_function_literal(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            &params,
            &vec![None; params.len()],
            &None,
            &[],
            &arm_block,
            arm.span,
            interner,
        )?;
        closure_vals.push(cv);
    }

    let ops_slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (num_arms.max(1) as u32) * 8,
        3,
    ));
    for (i, ov) in op_sym_vals.iter().enumerate() {
        builder.ins().stack_store(*ov, ops_slot, (i * 8) as i32);
    }
    let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);

    let closures_ptr = function_compiler.emit_boxed_array(builder, &closure_vals);

    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let narms_val = builder.ins().iconst(PTR_TYPE, num_arms as i64);

    let rt_push_handler = get_helper_func_ref(module, helpers, builder, "rt_push_handler");
    builder.ins().call(
        rt_push_handler,
        &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
    );

    let result = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        expr,
        interner,
    )?;

    let rt_pop_handler = get_helper_func_ref(module, helpers, builder, "rt_pop_handler");
    builder.ins().call(rt_pop_handler, &[ctx_val]);

    Ok(result)
}

fn collect_adt_definitions(
    top_level_statements: &[Statement],
    scope: &mut Scope,
    interner: &Interner,
) {
    for statement in top_level_statements {
        collect_adt_definitions_from_stmt(statement, scope, interner);
    }
}

fn collect_ir_adt_definitions(
    top_level_items: &[IrTopLevelItem],
    scope: &mut Scope,
    interner: &Interner,
) {
    for item in top_level_items {
        collect_ir_adt_definitions_from_item(item, scope, interner);
    }
}

fn collect_ir_adt_definitions_from_item(
    item: &IrTopLevelItem,
    scope: &mut Scope,
    interner: &Interner,
) {
    let _ = interner;
    match item {
        IrTopLevelItem::Data { name, variants, .. } => {
            let mut constructor_names = Vec::with_capacity(variants.len());
            for variant in variants {
                let name_sym = variant.name;
                scope.adt_constructors.insert(name_sym, variant.fields.len());
                scope.adt_constructor_owner.insert(name_sym, *name);
                constructor_names.push(name_sym);
            }
            scope.adt_variants.insert(*name, constructor_names);
        }
        IrTopLevelItem::Module { body, .. } => {
            for item in body {
                collect_ir_adt_definitions_from_item(item, scope, interner);
            }
        }
        _ => {}
    }
}

fn collect_adt_definitions_from_stmt(
    statement: &Statement,
    scope: &mut Scope,
    interner: &Interner,
) {
    let _ = interner;
    match statement {
        Statement::Data { name, variants, .. } => {
            let mut constructor_names = Vec::with_capacity(variants.len());
            for variant in variants {
                let name_sym = variant.name;
                scope
                    .adt_constructors
                    .insert(name_sym, variant.fields.len());
                scope.adt_constructor_owner.insert(name_sym, *name);
                constructor_names.push(name_sym);
            }
            scope.adt_variants.insert(*name, constructor_names);
        }
        Statement::Module { body, .. } => {
            for statement in &body.statements {
                collect_adt_definitions_from_stmt(statement, scope, interner);
            }
        }
        _ => {}
    }
}

fn compile_match_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    scrutinee: &Expression,
    arms: &[crate::syntax::expression::MatchArm],
    interner: &Interner,
) -> Result<JitValue, String> {
    if arms.is_empty() {
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        return Ok(JitValue::boxed(builder.inst_results(call)[1]));
    }

    validate_jit_match_arms(scope, arms, interner)?;

    let scrutinee_val = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        scrutinee,
        interner,
    )?;
    let scrutinee_val = box_jit_value(module, helpers, builder, ctx_val, scrutinee_val);
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, PTR_TYPE);

    let initial_test = builder.create_block();
    builder.ins().jump(initial_test, &[]);
    let mut pending_test = Some(initial_test);

    for arm in arms {
        let Some(test_block) = pending_test else {
            break;
        };
        builder.switch_to_block(test_block);

        validate_pattern_constructors_for_jit(&arm.pattern, scope, interner)?;

        let arm_block = builder.create_block();
        let mut next_test: Option<cranelift_codegen::ir::Block> = None;
        let mut matched_block = arm_block;
        let has_guard = arm.guard.is_some();
        if has_guard {
            matched_block = builder.create_block();
        }

        match &arm.pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
                builder.ins().jump(matched_block, &[]);
                if has_guard {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                } else {
                    pending_test = None;
                }
            }
            Pattern::Cons { .. } => {
                let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
                let call = builder.ins().call(is_cons, &[ctx_val, scrutinee_val]);
                let is_cons_i64 = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, is_cons_i64, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::None { .. } => {
                let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
                let call = builder.ins().call(is_none, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::EmptyList { .. } => {
                let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
                let call = builder.ins().call(is_el, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Some { .. } => {
                let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
                let call = builder.ins().call(is_some, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Left { .. } => {
                let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
                let call = builder.ins().call(is_left, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Right { .. } => {
                let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
                let call = builder.ins().call(is_right, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Literal { expression, .. } => {
                // Compile the literal value, then compare with scrutinee
                let lit_val = compile_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    expression,
                    interner,
                )?;
                let lit_val = box_jit_value(module, helpers, builder, ctx_val, lit_val);
                let vals_eq = get_helper_func_ref(module, helpers, builder, "rt_values_equal");
                let call = builder
                    .ins()
                    .call(vals_eq, &[ctx_val, scrutinee_val, lit_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Tuple { elements, .. } => {
                let next = builder.create_block();
                let mut test_scope = scope.clone();
                // Use emit_pattern_check to recursively validate the tuple
                // and each of its elements against their sub-patterns.
                emit_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    &mut test_scope,
                    ctx_val,
                    &arm.pattern,
                    scrutinee_val,
                    matched_block,
                    next,
                    interner,
                )?;
                // Seal intermediate element-check blocks created inside
                // emit_pattern_check (they were created and immediately switched to).
                // We only need to track elements as a reference to satisfy the compiler.
                let _ = elements;
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Constructor { .. } => {
                let next = builder.create_block();
                let mut test_scope = scope.clone();
                emit_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    &mut test_scope,
                    ctx_val,
                    &arm.pattern,
                    scrutinee_val,
                    matched_block,
                    next,
                    interner,
                )?;
                next_test = Some(next);
                pending_test = Some(next);
            }
        }

        builder.seal_block(test_block);

        builder.switch_to_block(matched_block);
        let mut arm_scope = scope.clone();
        bind_pattern_value(
            module,
            helpers,
            builder,
            &mut arm_scope,
            ctx_val,
            &arm.pattern,
            scrutinee_val,
        )?;
        if let Some(guard_expr) = &arm.guard {
            let guard_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                &mut arm_scope,
                ctx_val,
                return_block,
                tail_call,
                guard_expr,
                interner,
            )?;
            let cond = compile_truthiness_condition(
                module,
                helpers,
                builder,
                &arm_scope,
                ctx_val,
                guard_expr,
                guard_val,
            );
            let fail_block = match next_test {
                Some(next) => next,
                None => {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                    next
                }
            };
            builder.ins().brif(cond, arm_block, &[], fail_block, &[]);
            builder.seal_block(matched_block);
            builder.switch_to_block(arm_block);
        }
        let arm_val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            &mut arm_scope,
            ctx_val,
            return_block,
            tail_call,
            &arm.body,
            interner,
        )?;
        let arm_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, arm_val);
        let args = [BlockArg::Value(arm_val)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(arm_block);

        if let Some(next) = next_test {
            builder.switch_to_block(next);
        }
    }

    if let Some(unmatched) = pending_test {
        builder.switch_to_block(unmatched);
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        let fallback = builder.inst_results(call)[1];
        let args = [BlockArg::Value(fallback)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(unmatched);
    }

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
}

fn compile_ir_match_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    scrutinee: &Expression,
    arms: &[MatchArm],
    interner: &Interner,
) -> Result<JitValue, String> {
    if arms.is_empty() {
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        return Ok(JitValue::boxed(builder.inst_results(call)[1]));
    }

    validate_ir_jit_match_arms(scope, arms, interner)?;

    let scrutinee_val = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        scrutinee,
        interner,
    )?;
    let scrutinee_val = box_jit_value(module, helpers, builder, ctx_val, scrutinee_val);
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, PTR_TYPE);

    let initial_test = builder.create_block();
    builder.ins().jump(initial_test, &[]);
    let mut pending_test = Some(initial_test);

    for arm in arms {
        let Some(test_block) = pending_test else {
            break;
        };
        builder.switch_to_block(test_block);

        validate_ir_pattern_constructors_for_jit(&arm.pattern, scope, interner)?;

        let arm_block = builder.create_block();
        let mut next_test: Option<cranelift_codegen::ir::Block> = None;
        let mut matched_block = arm_block;
        let has_guard = arm.guard.is_some();
        if has_guard {
            matched_block = builder.create_block();
        }

        match &arm.pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
                builder.ins().jump(matched_block, &[]);
                if has_guard {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                } else {
                    pending_test = None;
                }
            }
            Pattern::Cons { .. } => {
                let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
                let call = builder.ins().call(is_cons, &[ctx_val, scrutinee_val]);
                let is_cons_i64 = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, is_cons_i64, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::None { .. } => {
                let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
                let call = builder.ins().call(is_none, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::EmptyList { .. } => {
                let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
                let call = builder.ins().call(is_el, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Some { .. } => {
                let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
                let call = builder.ins().call(is_some, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Left { .. } => {
                let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
                let call = builder.ins().call(is_left, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Right { .. } => {
                let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
                let call = builder.ins().call(is_right, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Literal { expression, .. } => {
                let lit_val = compile_ir_expression(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    expression,
                    interner,
                )?;
                let lit_val = box_jit_value(module, helpers, builder, ctx_val, lit_val);
                let vals_eq = get_helper_func_ref(module, helpers, builder, "rt_values_equal");
                let call = builder
                    .ins()
                    .call(vals_eq, &[ctx_val, scrutinee_val, lit_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Tuple { .. } | Pattern::Constructor { .. } => {
                let next = builder.create_block();
                let mut test_scope = scope.clone();
                emit_ir_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    &mut test_scope,
                    ctx_val,
                    &arm.pattern,
                    scrutinee_val,
                    matched_block,
                    next,
                    interner,
                )?;
                next_test = Some(next);
                pending_test = Some(next);
            }
        }

        builder.seal_block(test_block);

        builder.switch_to_block(matched_block);
        let mut arm_scope = scope.clone();
        bind_ir_pattern_value(
            module,
            helpers,
            builder,
            &mut arm_scope,
            ctx_val,
            &arm.pattern,
            scrutinee_val,
        )?;
        if let Some(guard_expr) = &arm.guard {
            let guard_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                &mut arm_scope,
                ctx_val,
                return_block,
                tail_call,
                guard_expr,
                interner,
            )?;
            let cond = compile_ir_truthiness_condition(
                module,
                helpers,
                builder,
                &arm_scope,
                ctx_val,
                guard_expr,
                guard_val,
            );
            let fail_block = match next_test {
                Some(next) => next,
                None => {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                    next
                }
            };
            builder.ins().brif(cond, arm_block, &[], fail_block, &[]);
            builder.seal_block(matched_block);
            builder.switch_to_block(arm_block);
        }
        let arm_val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            &mut arm_scope,
            ctx_val,
            return_block,
            tail_call,
            &arm.body,
            interner,
        )?;
        let arm_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, arm_val);
        let args = [BlockArg::Value(arm_val)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(arm_block);

        if let Some(next) = next_test {
            builder.switch_to_block(next);
        }
    }

    if let Some(unmatched) = pending_test {
        builder.switch_to_block(unmatched);
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        let fallback = builder.inst_results(call)[1];
        let args = [BlockArg::Value(fallback)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(unmatched);
    }

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
}

fn validate_jit_match_arms(
    scope: &Scope,
    arms: &[crate::syntax::expression::MatchArm],
    interner: &Interner,
) -> Result<(), String> {
    let all_constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| match &arm.pattern {
            Pattern::Constructor { name, .. } => Some(*name),
            _ => None,
        })
        .collect();
    if all_constructor_names.is_empty() {
        return Ok(());
    }

    let constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| {
            if arm.guard.is_none()
                && let Pattern::Constructor { name, .. } = &arm.pattern
            {
                return Some(*name);
            }
            None
        })
        .collect();

    let first = all_constructor_names[0];
    let Some(first_adt) = scope.adt_constructor_owner.get(&first).copied() else {
        return Err(format!(
            "Unknown constructor `{}`.",
            interner.resolve(first)
        ));
    };
    for constructor in &all_constructor_names {
        let Some(owner) = scope.adt_constructor_owner.get(constructor).copied() else {
            return Err(format!(
                "Unknown constructor `{}`.",
                interner.resolve(*constructor)
            ));
        };
        if owner != first_adt {
            return Err(format!(
                "Match arms mix constructors from different ADTs: `{}` and `{}`.",
                interner.resolve(first_adt),
                interner.resolve(owner)
            ));
        }
    }

    let has_catch_all = arms.iter().any(|arm| {
        arm.guard.is_none()
            && matches!(
                arm.pattern,
                Pattern::Wildcard { .. } | Pattern::Identifier { .. }
            )
    });
    if has_catch_all {
        return Ok(());
    }

    let Some(variants) = scope.adt_variants.get(&first_adt) else {
        return Ok(());
    };

    if constructor_names.is_empty() {
        let all = variants
            .iter()
            .map(|name| interner.resolve(*name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Match on `{}` is non-exhaustive because all constructor arms are guarded. Missing: {}.",
            interner.resolve(first_adt),
            all
        ));
    }

    let covered: HashSet<Identifier> = constructor_names.into_iter().collect();
    let missing = variants
        .iter()
        .filter(|name| !covered.contains(name))
        .map(|name| interner.resolve(*name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Match on `{}` is missing constructors: {}.",
            interner.resolve(first_adt),
            missing.join(", ")
        ));
    }

    Ok(())
}

fn validate_ir_jit_match_arms(
    scope: &Scope,
    arms: &[MatchArm],
    interner: &Interner,
) -> Result<(), String> {
    let all_constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| match &arm.pattern {
            Pattern::Constructor { name, .. } => Some(*name),
            _ => None,
        })
        .collect();
    if all_constructor_names.is_empty() {
        return Ok(());
    }

    let constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| {
            if arm.guard.is_none()
                && let Pattern::Constructor { name, .. } = &arm.pattern
            {
                return Some(*name);
            }
            None
        })
        .collect();

    let first = all_constructor_names[0];
    let Some(first_adt) = scope.adt_constructor_owner.get(&first).copied() else {
        return Err(format!("Unknown constructor `{}`.", interner.resolve(first)));
    };
    for constructor in &all_constructor_names {
        let Some(owner) = scope.adt_constructor_owner.get(constructor).copied() else {
            return Err(format!(
                "Unknown constructor `{}`.",
                interner.resolve(*constructor)
            ));
        };
        if owner != first_adt {
            return Err(format!(
                "Match arms mix constructors from different ADTs: `{}` and `{}`.",
                interner.resolve(first_adt),
                interner.resolve(owner)
            ));
        }
    }

    let has_catch_all = arms.iter().any(|arm| {
        arm.guard.is_none()
            && matches!(
                arm.pattern,
                Pattern::Wildcard { .. } | Pattern::Identifier { .. }
            )
    });
    if has_catch_all {
        return Ok(());
    }

    let Some(variants) = scope.adt_variants.get(&first_adt) else {
        return Ok(());
    };

    if constructor_names.is_empty() {
        let all = variants
            .iter()
            .map(|name| interner.resolve(*name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Match on `{}` is non-exhaustive because all constructor arms are guarded. Missing: {}.",
            interner.resolve(first_adt),
            all
        ));
    }

    let covered: HashSet<Identifier> = constructor_names.into_iter().collect();
    let missing = variants
        .iter()
        .filter(|name| !covered.contains(name))
        .map(|name| interner.resolve(*name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Match on `{}` is missing constructors: {}.",
            interner.resolve(first_adt),
            missing.join(", ")
        ));
    }

    Ok(())
}

fn validate_pattern_constructors_for_jit(
    pattern: &Pattern,
    scope: &Scope,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Constructor { name, fields, .. } => {
            let Some(expected_arity) = scope.adt_constructors.get(name).copied() else {
                return Err(format!(
                    "Unknown constructor `{}`.",
                    interner.resolve(*name)
                ));
            };
            if fields.len() != expected_arity {
                return Err(format!(
                    "Constructor `{}` expects {} argument(s) but got {}.",
                    interner.resolve(*name),
                    expected_arity,
                    fields.len()
                ));
            }
            for field in fields {
                validate_pattern_constructors_for_jit(field, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            validate_pattern_constructors_for_jit(pattern, scope, interner)
        }
        Pattern::Cons { head, tail, .. } => {
            validate_pattern_constructors_for_jit(head, scope, interner)?;
            validate_pattern_constructors_for_jit(tail, scope, interner)
        }
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                validate_pattern_constructors_for_jit(element, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::Identifier { .. }
        | Pattern::Literal { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. } => Ok(()),
    }
}

fn validate_ir_pattern_constructors_for_jit(
    pattern: &Pattern,
    scope: &Scope,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Constructor { name, fields, .. } => {
            let Some(expected_arity) = scope.adt_constructors.get(name).copied() else {
                return Err(format!("Unknown constructor `{}`.", interner.resolve(*name)));
            };
            if fields.len() != expected_arity {
                return Err(format!(
                    "Constructor `{}` expects {} argument(s) but got {}.",
                    interner.resolve(*name),
                    expected_arity,
                    fields.len()
                ));
            }
            for field in fields {
                validate_ir_pattern_constructors_for_jit(field, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            validate_ir_pattern_constructors_for_jit(pattern, scope, interner)
        }
        Pattern::Cons { head, tail, .. } => {
            validate_ir_pattern_constructors_for_jit(head, scope, interner)?;
            validate_ir_pattern_constructors_for_jit(tail, scope, interner)
        }
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                validate_ir_pattern_constructors_for_jit(element, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::Identifier { .. }
        | Pattern::Literal { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. } => Ok(()),
    }
}

fn bind_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } => Ok(()),
        Pattern::Identifier { name, .. } => {
            let binding = LocalBinding {
                var: declare_local(builder, JitValueKind::Boxed),
                kind: JitValueKind::Boxed,
            };
            builder.def_var(binding.var, value);
            scope.locals.insert(*name, binding);
            Ok(())
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            emit_return_on_null_value(builder, h_val);
            emit_return_on_null_value(builder, t_val);
            bind_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::None { .. } | Pattern::EmptyList { .. } | Pattern::Literal { .. } => {
            // No bindings for these patterns
            Ok(())
        }
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_pattern_value(module, helpers, builder, scope, ctx_val, element, item)?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
    }
}

fn bind_ir_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } => Ok(()),
        Pattern::Identifier { name, .. } => {
            let binding = LocalBinding {
                var: declare_local(builder, JitValueKind::Boxed),
                kind: JitValueKind::Boxed,
            };
            builder.def_var(binding.var, value);
            scope.locals.insert(*name, binding);
            Ok(())
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            emit_return_on_null_value(builder, h_val);
            emit_return_on_null_value(builder, t_val);
            bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::None { .. }
        | Pattern::EmptyList { .. }
        | Pattern::Literal { .. } => Ok(()),
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, element, item)?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_ir_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
    }
}

/// Emits a chain of feasibility checks for `pattern` applied to `value`.
///
/// If `value` satisfies the pattern, control falls to `pass_block`.
/// If it does not, control jumps to `fail_block`.
/// The caller must switch to `pass_block` afterwards to continue.
///
/// Only the _outer_ shape of the value is checked here — identifier/wildcard
/// sub-patterns always pass since they bind unconditionally.
fn emit_pattern_check(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
    pass_block: cranelift_codegen::ir::Block,
    fail_block: cranelift_codegen::ir::Block,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
            builder.ins().jump(pass_block, &[]);
        }
        Pattern::Cons { .. } => {
            let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
            let call = builder.ins().call(is_cons, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::EmptyList { .. } => {
            let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
            let call = builder.ins().call(is_el, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::None { .. } => {
            let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
            let call = builder.ins().call(is_none, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Some { .. } => {
            let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
            let call = builder.ins().call(is_some, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Left { .. } => {
            let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
            let call = builder.ins().call(is_left, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Right { .. } => {
            let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
            let call = builder.ins().call(is_right, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Literal { expression, .. } => {
            let lit_val = compile_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                None,
                None,
                expression,
                interner,
            )?;
            let lit_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, lit_val);
            let vals_eq = get_helper_func_ref(module, helpers, builder, "rt_values_equal");
            let call = builder.ins().call(vals_eq, &[ctx_val, value, lit_val]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Tuple { elements, .. } => {
            // Check rt_is_tuple first, then chain checks for each element.
            let is_tuple = get_helper_func_ref(module, helpers, builder, "rt_is_tuple");
            let call = builder.ins().call(is_tuple, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            // Build a chain: check_tuple → check_el_0 → ... → pass_block
            // Each step jumps to fail_block on failure.
            // Build a step block per element so we can chain checks forward.
            let step_blocks: Vec<cranelift_codegen::ir::Block> =
                elements.iter().map(|_| builder.create_block()).collect();
            // The first check is rt_is_tuple; on success jump to step_blocks[0] or pass_block.
            let first = step_blocks.first().copied().unwrap_or(pass_block);
            builder.ins().brif(cond, first, &[], fail_block, &[]);
            // Now emit each element check, chaining into the next step or pass_block.
            for (i, (element, &step)) in elements.iter().zip(step_blocks.iter()).enumerate() {
                builder.seal_block(step);
                builder.switch_to_block(step);
                let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
                let idx_val = builder.ins().iconst(PTR_TYPE, i as i64);
                let elem_call = builder.ins().call(tuple_get, &[ctx_val, value, idx_val]);
                let elem_val = builder.inst_results(elem_call)[0];
                let next = step_blocks.get(i + 1).copied().unwrap_or(pass_block);
                emit_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    element,
                    elem_val,
                    next,
                    fail_block,
                    interner,
                )?;
            }
        }
        Pattern::Constructor { name, fields, .. } => {
            let name_str = interner.resolve(*name);
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
            let is_adt = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder
                .ins()
                .call(is_adt, &[ctx_val, value, name_ptr, name_len]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            let step_blocks: Vec<cranelift_codegen::ir::Block> =
                fields.iter().map(|_| builder.create_block()).collect();
            let first = step_blocks.first().copied().unwrap_or(pass_block);
            builder.ins().brif(cond, first, &[], fail_block, &[]);
            for (i, (field_pattern, &step)) in fields.iter().zip(step_blocks.iter()).enumerate() {
                builder.seal_block(step);
                builder.switch_to_block(step);
                let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
                let idx_val = builder.ins().iconst(PTR_TYPE, i as i64);
                let field_call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let field_val = builder.inst_results(field_call)[0];
                emit_return_on_null_value(builder, field_val);
                let next = step_blocks.get(i + 1).copied().unwrap_or(pass_block);
                emit_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    field_pattern,
                    field_val,
                    next,
                    fail_block,
                    interner,
                )?;
            }
        }
    }
    Ok(())
}

fn emit_ir_pattern_check(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
    pass_block: cranelift_codegen::ir::Block,
    fail_block: cranelift_codegen::ir::Block,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
            builder.ins().jump(pass_block, &[]);
        }
        Pattern::Cons { .. } => {
            let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
            let call = builder.ins().call(is_cons, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::EmptyList { .. } => {
            let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
            let call = builder.ins().call(is_el, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::None { .. } => {
            let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
            let call = builder.ins().call(is_none, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Some { .. } => {
            let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
            let call = builder.ins().call(is_some, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Left { .. } => {
            let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
            let call = builder.ins().call(is_left, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Right { .. } => {
            let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
            let call = builder.ins().call(is_right, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Literal { expression, .. } => {
            let lit_val = compile_ir_expression(
                module,
                helpers,
                builder,
                function_compiler,
                scope,
                ctx_val,
                None,
                None,
                expression,
                interner,
            )?;
            let lit_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, lit_val);
            let vals_eq = get_helper_func_ref(module, helpers, builder, "rt_values_equal");
            let call = builder.ins().call(vals_eq, &[ctx_val, value, lit_val]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Tuple { elements, .. } => {
            let is_tuple = get_helper_func_ref(module, helpers, builder, "rt_is_tuple");
            let call = builder.ins().call(is_tuple, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            let step_blocks: Vec<cranelift_codegen::ir::Block> =
                elements.iter().map(|_| builder.create_block()).collect();
            let first = step_blocks.first().copied().unwrap_or(pass_block);
            builder.ins().brif(cond, first, &[], fail_block, &[]);
            for (i, (element, &step)) in elements.iter().zip(step_blocks.iter()).enumerate() {
                builder.seal_block(step);
                builder.switch_to_block(step);
                let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
                let idx_val = builder.ins().iconst(PTR_TYPE, i as i64);
                let elem_call = builder.ins().call(tuple_get, &[ctx_val, value, idx_val]);
                let elem_val = builder.inst_results(elem_call)[0];
                let next = step_blocks.get(i + 1).copied().unwrap_or(pass_block);
                emit_ir_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    element,
                    elem_val,
                    next,
                    fail_block,
                    interner,
                )?;
            }
        }
        Pattern::Constructor { name, fields, .. } => {
            let name_str = interner.resolve(*name);
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
            let is_adt = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder
                .ins()
                .call(is_adt, &[ctx_val, value, name_ptr, name_len]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            let step_blocks: Vec<cranelift_codegen::ir::Block> =
                fields.iter().map(|_| builder.create_block()).collect();
            let first = step_blocks.first().copied().unwrap_or(pass_block);
            builder.ins().brif(cond, first, &[], fail_block, &[]);
            for (i, (field_pattern, &step)) in fields.iter().zip(step_blocks.iter()).enumerate() {
                builder.seal_block(step);
                builder.switch_to_block(step);
                let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
                let idx_val = builder.ins().iconst(PTR_TYPE, i as i64);
                let field_call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let field_val = builder.inst_results(field_call)[0];
                emit_return_on_null_value(builder, field_val);
                let next = step_blocks.get(i + 1).copied().unwrap_or(pass_block);
                emit_ir_pattern_check(
                    module,
                    helpers,
                    builder,
                    function_compiler,
                    scope,
                    ctx_val,
                    field_pattern,
                    field_val,
                    next,
                    fail_block,
                    interner,
                )?;
            }
        }
    }
    Ok(())
}

fn bind_top_level_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Identifier { name, .. } => {
            if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                builder.ins().call(set_global, &[ctx_val, idx_val, value]);
                Ok(())
            } else {
                bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, value)
            }
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            emit_return_on_null_value(builder, h_val);
            emit_return_on_null_value(builder, t_val);
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, element, item,
                )?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_top_level_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. }
        | Pattern::Literal { .. } => Ok(()),
    }
}

fn bind_ir_top_level_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Identifier { name, .. } => {
            if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                builder.ins().call(set_global, &[ctx_val, idx_val, value]);
                Ok(())
            } else {
                bind_ir_pattern_value(module, helpers, builder, scope, ctx_val, pattern, value)
            }
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            emit_return_on_null_value(builder, h_val);
            emit_return_on_null_value(builder, t_val);
            bind_ir_top_level_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_ir_top_level_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, inner);
            bind_ir_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_ir_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, element, item,
                )?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                emit_return_on_null_value(builder, item);
                bind_ir_top_level_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. }
        | Pattern::Literal { .. } => Ok(()),
    }
}

fn compile_block_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    block: &Block,
    interner: &Interner,
) -> Result<BlockEval, String> {
    let mut block_scope = scope.clone();
    for (idx, stmt) in block.statements.iter().enumerate() {
        let is_last = idx + 1 == block.statements.len();
        let outcome = compile_statement(
            module,
            helpers,
            builder,
            function_compiler,
            &mut block_scope,
            ctx_val,
            return_block,
            tail_call,
            false,
            stmt,
            interner,
        )?;
        match outcome {
            StmtOutcome::Value(v) if is_last => return Ok(BlockEval::Value(v)),
            StmtOutcome::Returned => return Ok(BlockEval::Returned),
            StmtOutcome::Value(_) | StmtOutcome::None => {}
        }
    }
    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
    let call = builder.ins().call(make_none, &[ctx_val]);
    Ok(BlockEval::Value(JitValue::boxed(
        builder.inst_results(call)[1],
    )))
}

fn compile_ir_block_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    block: &Block,
    interner: &Interner,
) -> Result<BlockEval, String> {
    compile_block_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        block,
        interner,
    )
}

fn compile_if_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    condition: &Expression,
    consequence: &Block,
    alternative: Option<&Block>,
    interner: &Interner,
) -> Result<JitValue, String> {
    let cond_val = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        condition,
        interner,
    )?;
    let cond_b1 =
        compile_truthiness_condition(module, helpers, builder, scope, ctx_val, condition, cond_val);

    let then_block = builder.create_block();
    let else_block = builder.create_block();
    let merge_block = builder.create_block();
    let live_locals = live_branch_locals(builder, scope);
    let branch_args: Vec<BlockArg> = live_locals
        .iter()
        .map(|(_, _, value)| BlockArg::Value(*value))
        .collect();

    for (_, binding, _) in &live_locals {
        builder.append_block_param(then_block, jit_value_type(binding.kind));
        builder.append_block_param(else_block, jit_value_type(binding.kind));
    }

    builder
        .ins()
        .brif(cond_b1, then_block, &branch_args, else_block, &branch_args);

    builder.switch_to_block(then_block);
    bind_branch_block_params(builder, then_block, &live_locals);
    let then_eval = compile_block_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        consequence,
        interner,
    )?;
    let then_exit_block = builder
        .current_block()
        .expect("then branch should leave an active block");

    builder.switch_to_block(else_block);
    bind_branch_block_params(builder, else_block, &live_locals);
    let else_eval = match alternative {
        Some(alt) => compile_block_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            alt,
            interner,
        )?,
        None => BlockEval::Value({
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            JitValue::boxed(builder.inst_results(call)[1])
        }),
    };
    let else_exit_block = builder
        .current_block()
        .expect("else branch should leave an active block");

    let merge_kind = match (&then_eval, &else_eval) {
        (BlockEval::Value(then_val), BlockEval::Value(else_val))
            if then_val.kind == JitValueKind::Bool && else_val.kind == JitValueKind::Bool =>
        {
            JitValueKind::Bool
        }
        _ => JitValueKind::Boxed,
    };

    if merge_kind == JitValueKind::Bool {
        builder.append_block_param(merge_block, types::I64);

        let mut has_merge_value = false;

        builder.switch_to_block(then_exit_block);
        if let BlockEval::Value(then_val) = then_eval {
            let then_args = [BlockArg::Value(then_val.value)];
            builder.ins().jump(merge_block, &then_args);
            has_merge_value = true;
        }
        builder.seal_block(then_exit_block);

        builder.switch_to_block(else_exit_block);
        if let BlockEval::Value(else_val) = else_eval {
            let else_args = [BlockArg::Value(else_val.value)];
            builder.ins().jump(merge_block, &else_args);
            has_merge_value = true;
        }
        builder.seal_block(else_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        if has_merge_value {
            Ok(JitValue::bool(builder.block_params(merge_block)[0]))
        } else {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
    } else {
        builder.append_block_param(merge_block, PTR_TYPE);

        let mut has_merge_value = false;

        builder.switch_to_block(then_exit_block);
        if let BlockEval::Value(then_val) = then_eval {
            let then_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, then_val);
            let then_args = [BlockArg::Value(then_val)];
            builder.ins().jump(merge_block, &then_args);
            has_merge_value = true;
        }
        builder.seal_block(then_exit_block);

        builder.switch_to_block(else_exit_block);
        if let BlockEval::Value(else_val) = else_eval {
            let else_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, else_val);
            let else_args = [BlockArg::Value(else_val)];
            builder.ins().jump(merge_block, &else_args);
            has_merge_value = true;
        }
        builder.seal_block(else_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        if has_merge_value {
            Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
        } else {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
    }
}

fn compile_ir_if_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    condition: &Expression,
    consequence: &Block,
    alternative: Option<&Block>,
    interner: &Interner,
) -> Result<JitValue, String> {
    let cond_val = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        condition,
        interner,
    )?;
    let cond_b1 = compile_ir_truthiness_condition(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        condition,
        cond_val,
    );

    let then_block = builder.create_block();
    let else_block = builder.create_block();
    let merge_block = builder.create_block();
    let live_locals = live_branch_locals(builder, scope);
    let branch_args: Vec<BlockArg> = live_locals
        .iter()
        .map(|(_, _, value)| BlockArg::Value(*value))
        .collect();

    for (_, binding, _) in &live_locals {
        builder.append_block_param(then_block, jit_value_type(binding.kind));
        builder.append_block_param(else_block, jit_value_type(binding.kind));
    }

    builder
        .ins()
        .brif(cond_b1, then_block, &branch_args, else_block, &branch_args);

    builder.switch_to_block(then_block);
    bind_branch_block_params(builder, then_block, &live_locals);
    let then_eval = compile_ir_block_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        consequence,
        interner,
    )?;
    let then_exit_block = builder
        .current_block()
        .expect("then branch should leave an active block");

    builder.switch_to_block(else_block);
    bind_branch_block_params(builder, else_block, &live_locals);
    let else_eval = match alternative {
        Some(alt) => compile_ir_block_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            alt,
            interner,
        )?,
        None => BlockEval::Value({
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            JitValue::boxed(builder.inst_results(call)[1])
        }),
    };
    let else_exit_block = builder
        .current_block()
        .expect("else branch should leave an active block");

    let merge_kind = match (&then_eval, &else_eval) {
        (BlockEval::Value(then_val), BlockEval::Value(else_val))
            if then_val.kind == JitValueKind::Bool && else_val.kind == JitValueKind::Bool =>
        {
            JitValueKind::Bool
        }
        _ => JitValueKind::Boxed,
    };

    if merge_kind == JitValueKind::Bool {
        builder.append_block_param(merge_block, types::I64);

        let mut has_merge_value = false;

        builder.switch_to_block(then_exit_block);
        if let BlockEval::Value(then_val) = then_eval {
            let then_args = [BlockArg::Value(then_val.value)];
            builder.ins().jump(merge_block, &then_args);
            has_merge_value = true;
        }
        builder.seal_block(then_exit_block);

        builder.switch_to_block(else_exit_block);
        if let BlockEval::Value(else_val) = else_eval {
            let else_args = [BlockArg::Value(else_val.value)];
            builder.ins().jump(merge_block, &else_args);
            has_merge_value = true;
        }
        builder.seal_block(else_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        if has_merge_value {
            Ok(JitValue::bool(builder.block_params(merge_block)[0]))
        } else {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
    } else {
        builder.append_block_param(merge_block, PTR_TYPE);

        let mut has_merge_value = false;

        builder.switch_to_block(then_exit_block);
        if let BlockEval::Value(then_val) = then_eval {
            let then_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, then_val);
            let then_args = [BlockArg::Value(then_val)];
            builder.ins().jump(merge_block, &then_args);
            has_merge_value = true;
        }
        builder.seal_block(then_exit_block);

        builder.switch_to_block(else_exit_block);
        if let BlockEval::Value(else_val) = else_eval {
            let else_val = box_and_guard_jit_value(module, helpers, builder, ctx_val, else_val);
            let else_args = [BlockArg::Value(else_val)];
            builder.ins().jump(merge_block, &else_args);
            has_merge_value = true;
        }
        builder.seal_block(else_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        if has_merge_value {
            Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
        } else {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
    }
}

fn compile_short_circuit_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    left: &Expression,
    operator: &str,
    right: &Expression,
    interner: &Interner,
) -> Result<JitValue, String> {
    let lhs = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        left,
        interner,
    )?;
    let cond_b1 = compile_truthiness_condition(module, helpers, builder, scope, ctx_val, left, lhs);
    let short_value = if expr_has_known_bool_type(scope, left) {
        lhs
    } else {
        JitValue::boxed(box_jit_value(module, helpers, builder, ctx_val, lhs))
    };

    let short_block = builder.create_block();
    let eval_rhs_block = builder.create_block();
    let merge_block = builder.create_block();
    let live_locals = live_branch_locals(builder, scope);
    let branch_args: Vec<BlockArg> = live_locals
        .iter()
        .map(|(_, _, value)| BlockArg::Value(*value))
        .collect();

    for (_, binding, _) in &live_locals {
        builder.append_block_param(short_block, jit_value_type(binding.kind));
        builder.append_block_param(eval_rhs_block, jit_value_type(binding.kind));
    }

    match operator {
        "&&" => {
            builder.ins().brif(
                cond_b1,
                eval_rhs_block,
                &branch_args,
                short_block,
                &branch_args,
            );
        }
        "||" => {
            builder.ins().brif(
                cond_b1,
                short_block,
                &branch_args,
                eval_rhs_block,
                &branch_args,
            );
        }
        _ => return Err(format!("unknown short-circuit operator: {}", operator)),
    }

    builder.switch_to_block(short_block);
    bind_branch_block_params(builder, short_block, &live_locals);
    let short_exit_block = builder
        .current_block()
        .expect("short-circuit path should leave an active block");

    builder.switch_to_block(eval_rhs_block);
    bind_branch_block_params(builder, eval_rhs_block, &live_locals);
    let rhs = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        right,
        interner,
    )?;
    let rhs_exit_block = builder
        .current_block()
        .expect("rhs path should leave an active block");

    let merge_kind = merged_jit_value_kind(short_value, rhs);
    if merge_kind == JitValueKind::Bool {
        builder.append_block_param(merge_block, types::I64);

        builder.switch_to_block(short_exit_block);
        let short_args = [BlockArg::Value(short_value.value)];
        builder.ins().jump(merge_block, &short_args);
        builder.seal_block(short_exit_block);

        builder.switch_to_block(rhs_exit_block);
        let rhs_args = [BlockArg::Value(rhs.value)];
        builder.ins().jump(merge_block, &rhs_args);
        builder.seal_block(rhs_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        Ok(JitValue::bool(builder.block_params(merge_block)[0]))
    } else {
        builder.append_block_param(merge_block, PTR_TYPE);

        builder.switch_to_block(short_exit_block);
        let short_arg = box_and_guard_jit_value(module, helpers, builder, ctx_val, short_value);
        let short_args = [BlockArg::Value(short_arg)];
        builder.ins().jump(merge_block, &short_args);
        builder.seal_block(short_exit_block);

        builder.switch_to_block(rhs_exit_block);
        let rhs = box_and_guard_jit_value(module, helpers, builder, ctx_val, rhs);
        let rhs_args = [BlockArg::Value(rhs)];
        builder.ins().jump(merge_block, &rhs_args);
        builder.seal_block(rhs_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
    }
}

fn compile_ir_short_circuit_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    left: &Expression,
    operator: &str,
    right: &Expression,
    interner: &Interner,
) -> Result<JitValue, String> {
    let lhs = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        left,
        interner,
    )?;
    let cond_b1 =
        compile_ir_truthiness_condition(module, helpers, builder, scope, ctx_val, left, lhs);
    let short_value = if ir_expr_has_known_bool_type(scope, left) {
        lhs
    } else {
        JitValue::boxed(box_jit_value(module, helpers, builder, ctx_val, lhs))
    };

    let short_block = builder.create_block();
    let eval_rhs_block = builder.create_block();
    let merge_block = builder.create_block();
    let live_locals = live_branch_locals(builder, scope);
    let branch_args: Vec<BlockArg> = live_locals
        .iter()
        .map(|(_, _, value)| BlockArg::Value(*value))
        .collect();

    for (_, binding, _) in &live_locals {
        builder.append_block_param(short_block, jit_value_type(binding.kind));
        builder.append_block_param(eval_rhs_block, jit_value_type(binding.kind));
    }

    match operator {
        "&&" => {
            builder.ins().brif(
                cond_b1,
                eval_rhs_block,
                &branch_args,
                short_block,
                &branch_args,
            );
        }
        "||" => {
            builder.ins().brif(
                cond_b1,
                short_block,
                &branch_args,
                eval_rhs_block,
                &branch_args,
            );
        }
        _ => return Err(format!("unknown short-circuit operator: {}", operator)),
    }

    builder.switch_to_block(short_block);
    bind_branch_block_params(builder, short_block, &live_locals);
    let short_exit_block = builder
        .current_block()
        .expect("short-circuit path should leave an active block");

    builder.switch_to_block(eval_rhs_block);
    bind_branch_block_params(builder, eval_rhs_block, &live_locals);
    let rhs = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        right,
        interner,
    )?;
    let rhs_exit_block = builder
        .current_block()
        .expect("rhs path should leave an active block");

    let merge_kind = merged_jit_value_kind(short_value, rhs);
    if merge_kind == JitValueKind::Bool {
        builder.append_block_param(merge_block, types::I64);

        builder.switch_to_block(short_exit_block);
        let short_args = [BlockArg::Value(short_value.value)];
        builder.ins().jump(merge_block, &short_args);
        builder.seal_block(short_exit_block);

        builder.switch_to_block(rhs_exit_block);
        let rhs_args = [BlockArg::Value(rhs.value)];
        builder.ins().jump(merge_block, &rhs_args);
        builder.seal_block(rhs_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        Ok(JitValue::bool(builder.block_params(merge_block)[0]))
    } else {
        builder.append_block_param(merge_block, PTR_TYPE);

        builder.switch_to_block(short_exit_block);
        let short_arg = box_and_guard_jit_value(module, helpers, builder, ctx_val, short_value);
        let short_args = [BlockArg::Value(short_arg)];
        builder.ins().jump(merge_block, &short_args);
        builder.seal_block(short_exit_block);

        builder.switch_to_block(rhs_exit_block);
        let rhs = box_and_guard_jit_value(module, helpers, builder, ctx_val, rhs);
        let rhs_args = [BlockArg::Value(rhs)];
        builder.ins().jump(merge_block, &rhs_args);
        builder.seal_block(rhs_exit_block);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        Ok(JitValue::boxed(builder.block_params(merge_block)[0]))
    }
}

/// Emit Cranelift IR for a mutual tail call via the trampoline.
///
/// Stores the callee index and tagged args into `JitContext::pending_thunk`
/// via `rt_set_thunk`, then emits a direct `return` of the `JIT_TAG_THUNK`
/// pair. The trampoline loop in `jit_execute` will re-invoke the callee
/// without growing the native stack.
fn emit_mutual_tail_thunk(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    fn_index: usize,
    arg_vals: &[JitValue],
) {
    let (_slot, args_ptr) = emit_tagged_stack_array(builder, arg_vals);
    let fn_idx_val = builder.ins().iconst(PTR_TYPE, fn_index as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
    let set_thunk = get_helper_func_ref(module, helpers, builder, "rt_set_thunk");
    let call = builder
        .ins()
        .call(set_thunk, &[ctx_val, fn_idx_val, args_ptr, nargs_val]);
    let thunk_tag = builder.inst_results(call)[0];
    let thunk_payload = builder.inst_results(call)[1];
    builder.ins().return_(&[thunk_tag, thunk_payload]);
}

fn compile_base_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    call_span: crate::diagnostics::position::Span,
    base_idx: usize,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    // Compile all arguments, keeping them as JitValues (no boxing).
    // The tagged-array path passes tag+payload pairs directly, avoiding
    // one rt_force_boxed arena allocation per Int/Bool/Float argument.
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);

    let call_base =
        get_helper_func_ref(module, helpers, builder, "rt_call_base_function_tagged");
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

fn compile_ir_base_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    call_span: crate::diagnostics::position::Span,
    base_idx: usize,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);

    let call_base =
        get_helper_func_ref(module, helpers, builder, "rt_call_base_function_tagged");
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

fn compile_primop_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    primop: PrimOp,
    span: Span,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let mut raw_arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        raw_arg_vals.push(val);
    }

    if arguments.len() == 2 {
        let lhs = raw_arg_vals[0];
        let rhs = raw_arg_vals[1];
        match primop {
            PrimOp::IAdd => return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value))),
            PrimOp::ISub => return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value))),
            PrimOp::IMul => return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value))),
            PrimOp::IDiv | PrimOp::IMod if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int => {
                let is_zero = builder.ins().icmp_imm(IntCC::Equal, rhs.value, 0);
                let err_block = builder.create_block();
                let ok_block = builder.create_block();
                builder.ins().brif(is_zero, err_block, &[], ok_block, &[]);
                builder.switch_to_block(err_block);
                let dbz = get_helper_func_ref(module, helpers, builder, "rt_division_by_zero");
                builder.ins().call(dbz, &[ctx_val]);
                emit_return_null_tagged(builder);
                builder.seal_block(err_block);
                builder.switch_to_block(ok_block);
                builder.seal_block(ok_block);
                let result = if primop == PrimOp::IDiv {
                    builder.ins().sdiv(lhs.value, rhs.value)
                } else {
                    builder.ins().srem(lhs.value, rhs.value)
                };
                return Ok(JitValue::int(result));
            }
            PrimOp::ICmpEq
            | PrimOp::ICmpNe
            | PrimOp::ICmpLt
            | PrimOp::ICmpLe
            | PrimOp::ICmpGt
            | PrimOp::ICmpGe if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int => {
                let cc = match primop {
                    PrimOp::ICmpEq => IntCC::Equal,
                    PrimOp::ICmpNe => IntCC::NotEqual,
                    PrimOp::ICmpLt => IntCC::SignedLessThan,
                    PrimOp::ICmpLe => IntCC::SignedLessThanOrEqual,
                    PrimOp::ICmpGt => IntCC::SignedGreaterThan,
                    PrimOp::ICmpGe => IntCC::SignedGreaterThanOrEqual,
                    _ => unreachable!(),
                };
                let cmp = builder.ins().icmp(cc, lhs.value, rhs.value);
                let one = builder.ins().iconst(types::I64, 1);
                let zero = builder.ins().iconst(types::I64, 0);
                let bool_i64 = builder.ins().select(cmp, one, zero);
                return Ok(JitValue::bool(bool_i64));
            }
            PrimOp::FAdd | PrimOp::FSub | PrimOp::FMul | PrimOp::FDiv
                if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
            {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                let result = match primop {
                    PrimOp::FAdd => builder.ins().fadd(lhsf, rhsf),
                    PrimOp::FSub => builder.ins().fsub(lhsf, rhsf),
                    PrimOp::FMul => builder.ins().fmul(lhsf, rhsf),
                    PrimOp::FDiv => builder.ins().fdiv(lhsf, rhsf),
                    _ => unreachable!(),
                };
                let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                return Ok(JitValue::float(bits));
            }
            PrimOp::FCmpEq
            | PrimOp::FCmpNe
            | PrimOp::FCmpLt
            | PrimOp::FCmpLe
            | PrimOp::FCmpGt
            | PrimOp::FCmpGe if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float => {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                let cc = match primop {
                    PrimOp::FCmpEq => cranelift_codegen::ir::condcodes::FloatCC::Equal,
                    PrimOp::FCmpNe => cranelift_codegen::ir::condcodes::FloatCC::NotEqual,
                    PrimOp::FCmpLt => cranelift_codegen::ir::condcodes::FloatCC::LessThan,
                    PrimOp::FCmpLe => cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
                    PrimOp::FCmpGt => cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                    PrimOp::FCmpGe => cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual,
                    _ => unreachable!(),
                };
                let cmp = builder.ins().fcmp(cc, lhsf, rhsf);
                let one = builder.ins().iconst(types::I64, 1);
                let zero = builder.ins().iconst(types::I64, 0);
                let bool_i64 = builder.ins().select(cmp, one, zero);
                return Ok(JitValue::bool(bool_i64));
            }
            _ => {}
        }
    }

    let mut arg_vals = Vec::with_capacity(raw_arg_vals.len());
    for val in raw_arg_vals {
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_boxed_array(builder, &arg_vals);
    let primop_val = builder.ins().iconst(PTR_TYPE, primop.id() as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, span.end.line as i64);
    let end_col_val = builder.ins().iconst(PTR_TYPE, (span.end.column + 1) as i64);
    let call_primop = get_helper_func_ref(module, helpers, builder, "rt_call_primop");
    let call = builder.ins().call(
        call_primop,
        &[
            ctx_val,
            primop_val,
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

fn compile_ir_primop_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    primop: PrimOp,
    span: Span,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let mut raw_arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        raw_arg_vals.push(val);
    }

    if arguments.len() == 2 {
        let lhs = raw_arg_vals[0];
        let rhs = raw_arg_vals[1];
        match primop {
            PrimOp::IAdd => return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value))),
            PrimOp::ISub => return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value))),
            PrimOp::IMul => return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value))),
            PrimOp::IDiv | PrimOp::IMod
                if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
            {
                let is_zero = builder.ins().icmp_imm(IntCC::Equal, rhs.value, 0);
                let err_block = builder.create_block();
                let ok_block = builder.create_block();
                builder.ins().brif(is_zero, err_block, &[], ok_block, &[]);
                builder.switch_to_block(err_block);
                let dbz = get_helper_func_ref(module, helpers, builder, "rt_division_by_zero");
                builder.ins().call(dbz, &[ctx_val]);
                emit_return_null_tagged(builder);
                builder.seal_block(err_block);
                builder.switch_to_block(ok_block);
                builder.seal_block(ok_block);
                let result = if primop == PrimOp::IDiv {
                    builder.ins().sdiv(lhs.value, rhs.value)
                } else {
                    builder.ins().srem(lhs.value, rhs.value)
                };
                return Ok(JitValue::int(result));
            }
            PrimOp::ICmpEq
            | PrimOp::ICmpNe
            | PrimOp::ICmpLt
            | PrimOp::ICmpLe
            | PrimOp::ICmpGt
            | PrimOp::ICmpGe
                if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int =>
            {
                let cc = match primop {
                    PrimOp::ICmpEq => IntCC::Equal,
                    PrimOp::ICmpNe => IntCC::NotEqual,
                    PrimOp::ICmpLt => IntCC::SignedLessThan,
                    PrimOp::ICmpLe => IntCC::SignedLessThanOrEqual,
                    PrimOp::ICmpGt => IntCC::SignedGreaterThan,
                    PrimOp::ICmpGe => IntCC::SignedGreaterThanOrEqual,
                    _ => unreachable!(),
                };
                let cmp = builder.ins().icmp(cc, lhs.value, rhs.value);
                let one = builder.ins().iconst(types::I64, 1);
                let zero = builder.ins().iconst(types::I64, 0);
                let bool_i64 = builder.ins().select(cmp, one, zero);
                return Ok(JitValue::bool(bool_i64));
            }
            PrimOp::FAdd | PrimOp::FSub | PrimOp::FMul | PrimOp::FDiv
                if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
            {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                let result = match primop {
                    PrimOp::FAdd => builder.ins().fadd(lhsf, rhsf),
                    PrimOp::FSub => builder.ins().fsub(lhsf, rhsf),
                    PrimOp::FMul => builder.ins().fmul(lhsf, rhsf),
                    PrimOp::FDiv => builder.ins().fdiv(lhsf, rhsf),
                    _ => unreachable!(),
                };
                let bits = builder.ins().bitcast(types::I64, MemFlags::new(), result);
                return Ok(JitValue::float(bits));
            }
            PrimOp::FCmpEq
            | PrimOp::FCmpNe
            | PrimOp::FCmpLt
            | PrimOp::FCmpLe
            | PrimOp::FCmpGt
            | PrimOp::FCmpGe
                if lhs.kind == JitValueKind::Float && rhs.kind == JitValueKind::Float =>
            {
                let lhsf = builder.ins().bitcast(types::F64, MemFlags::new(), lhs.value);
                let rhsf = builder.ins().bitcast(types::F64, MemFlags::new(), rhs.value);
                let cc = match primop {
                    PrimOp::FCmpEq => cranelift_codegen::ir::condcodes::FloatCC::Equal,
                    PrimOp::FCmpNe => cranelift_codegen::ir::condcodes::FloatCC::NotEqual,
                    PrimOp::FCmpLt => cranelift_codegen::ir::condcodes::FloatCC::LessThan,
                    PrimOp::FCmpLe => cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
                    PrimOp::FCmpGt => cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                    PrimOp::FCmpGe => {
                        cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual
                    }
                    _ => unreachable!(),
                };
                let cmp = builder.ins().fcmp(cc, lhsf, rhsf);
                let one = builder.ins().iconst(types::I64, 1);
                let zero = builder.ins().iconst(types::I64, 0);
                let bool_i64 = builder.ins().select(cmp, one, zero);
                return Ok(JitValue::bool(bool_i64));
            }
            _ => {}
        }
    }

    let mut arg_vals = Vec::with_capacity(raw_arg_vals.len());
    for val in raw_arg_vals {
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_boxed_array(builder, &arg_vals);
    let primop_val = builder.ins().iconst(PTR_TYPE, primop.id() as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (span.end.column + 1) as i64);
    let call_primop = get_helper_func_ref(module, helpers, builder, "rt_call_primop");
    let call = builder.ins().call(
        call_primop,
        &[
            ctx_val,
            primop_val,
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

fn resolve_call_primop(
    scope: &Scope,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Option<PrimOp> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    if scope.excluded_base_symbols.contains(name) {
        return None;
    }

    // Shadowed names must resolve through the regular call path.
    if scope.locals.contains_key(name)
        || scope.functions.contains_key(name)
        || scope.globals.contains_key(name)
    {
        return None;
    }

    let name = interner.try_resolve(*name)?;
    resolve_primop_call(name, arguments.len())
}

fn resolve_ir_call_primop(
    scope: &Scope,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Option<PrimOp> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    if scope.excluded_base_symbols.contains(name) {
        return None;
    }
    if scope.locals.contains_key(name)
        || scope.functions.contains_key(name)
        || scope.globals.contains_key(name)
    {
        return None;
    }
    let name = interner.try_resolve(*name)?;
    resolve_primop_call(name, arguments.len())
}

fn should_use_base_fastcall(scope: &Scope, name: Identifier, interner: &Interner) -> bool {
    if scope.excluded_base_symbols.contains(&name) {
        return false;
    }
    if scope.locals.contains_key(&name)
        || scope.functions.contains_key(&name)
        || scope.globals.contains_key(&name)
    {
        return false;
    }
    let Some(name_str) = interner.try_resolve(name) else {
        return false;
    };
    is_base_fastcall_allowlisted(name_str)
}

fn compile_user_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    meta: JitFunctionMeta,
    call_span: crate::diagnostics::position::Span,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }
    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();

    let nargs = arg_vals.len();
    if nargs != meta.num_params {
        return Err(format!(
            "wrong number of arguments in JIT call: want={}, got={}",
            meta.num_params, nargs
        ));
    }

    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    let zero = builder.ins().iconst(PTR_TYPE, 0);

    // Fast path: skip all contract checks for unannotated JIT-to-JIT calls.
    // HM inference already proved type correctness; runtime re-checking is redundant.
    if !meta.has_contract {
        let callee_ref = module.declare_func_in_func(meta.id, builder.func);
        let call = match meta.call_abi {
            JitCallAbi::Array => {
                let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
                let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
            }
            JitCallAbi::Reg1 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
            }
            JitCallAbi::Reg2 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let args = [ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero];
                builder.ins().call(callee_ref, &args)
            }
            JitCallAbi::Reg3 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                let args = [
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
                ];
                builder.ins().call(callee_ref, &args)
            }
            JitCallAbi::Reg4 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
                let args = [
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                    null_ptr, zero,
                ];
                builder.ins().call(callee_ref, &args)
            }
        };
        let raw_tag = builder.inst_results(call)[0];
        let raw_payload = builder.inst_results(call)[1];
        let raw_result = boxed_value_from_tagged_parts(
            module, helpers, builder, ctx_val, raw_tag, raw_payload,
        );
        emit_return_on_null_value(builder, raw_result);
        return Ok(JitValue::boxed(raw_result));
    }

    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);

    let call_ok = match meta.call_abi {
        JitCallAbi::Array => {
            let args_ptr = function_compiler.emit_boxed_array(builder, &boxed_arg_vals);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    args_ptr,
                    nargs_val,
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg1 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call1");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg2 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call2");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg3 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call3");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg4 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call4");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
    };
    let call_ok_val = builder.inst_results(call_ok)[0];
    let call_ok_bool = builder.ins().icmp_imm(IntCC::NotEqual, call_ok_val, 0);

    let call_block = builder.create_block();
    let fail_block = builder.create_block();
    let done_block = builder.create_block();
    builder.append_block_param(done_block, PTR_TYPE);
    builder
        .ins()
        .brif(call_ok_bool, call_block, &[], fail_block, &[]);

    builder.switch_to_block(fail_block);
    let fail_args = [BlockArg::Value(null_ptr)];
    builder.ins().jump(done_block, &fail_args);
    builder.seal_block(fail_block);

    builder.switch_to_block(call_block);
    let callee_ref = module.declare_func_in_func(meta.id, builder.func);
    let call = match meta.call_abi {
        JitCallAbi::Array => {
            let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
        }
        JitCallAbi::Reg1 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
        }
        JitCallAbi::Reg2 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let args = [ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero];
            builder.ins().call(callee_ref, &args)
        }
        JitCallAbi::Reg3 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            let args = [
                ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
            ];
            builder.ins().call(callee_ref, &args)
        }
        JitCallAbi::Reg4 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
            let args = [
                ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                null_ptr, zero,
            ];
            builder.ins().call(callee_ref, &args)
        }
    };
    let raw_tag = builder.inst_results(call)[0];
    let raw_payload = builder.inst_results(call)[1];
    let raw_result = boxed_value_from_tagged_parts(
        module, helpers, builder, ctx_val, raw_tag, raw_payload,
    );
    let check_ret = get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_return");
    let checked_ret_call = builder.ins().call(
        check_ret,
        &[
            ctx_val,
            fn_index,
            raw_result,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let checked_ret = builder.inst_results(checked_ret_call)[0];
    emit_return_on_null_value(builder, checked_ret);
    let ok_args = [BlockArg::Value(checked_ret)];
    builder.ins().jump(done_block, &ok_args);
    builder.seal_block(call_block);

    builder.switch_to_block(done_block);
    let result = builder.block_params(done_block)[0];
    builder.seal_block(done_block);
    Ok(JitValue::boxed(result))
}

fn compile_ir_user_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    meta: JitFunctionMeta,
    call_span: crate::diagnostics::position::Span,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }
    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();

    let nargs = arg_vals.len();
    if nargs != meta.num_params {
        return Err(format!(
            "wrong number of arguments in JIT call: want={}, got={}",
            meta.num_params, nargs
        ));
    }

    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    let zero = builder.ins().iconst(PTR_TYPE, 0);

    if !meta.has_contract {
        let callee_ref = module.declare_func_in_func(meta.id, builder.func);
        let call = match meta.call_abi {
            JitCallAbi::Array => {
                let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
                let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
            }
            JitCallAbi::Reg1 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
            }
            JitCallAbi::Reg2 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let args = [ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero];
                builder.ins().call(callee_ref, &args)
            }
            JitCallAbi::Reg3 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                let args = [
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
                ];
                builder.ins().call(callee_ref, &args)
            }
            JitCallAbi::Reg4 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
                let args = [
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                    null_ptr, zero,
                ];
                builder.ins().call(callee_ref, &args)
            }
        };
        let raw_tag = builder.inst_results(call)[0];
        let raw_payload = builder.inst_results(call)[1];
        let raw_result = boxed_value_from_tagged_parts(
            module, helpers, builder, ctx_val, raw_tag, raw_payload,
        );
        emit_return_on_null_value(builder, raw_result);
        return Ok(JitValue::boxed(raw_result));
    }

    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);

    let call_ok = match meta.call_abi {
        JitCallAbi::Array => {
            let args_ptr = function_compiler.emit_boxed_array(builder, &boxed_arg_vals);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    args_ptr,
                    nargs_val,
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg1 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call1");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg2 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call2");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg3 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call3");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg4 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call4");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
    };
    let call_ok_val = builder.inst_results(call_ok)[0];
    let call_ok_bool = builder.ins().icmp_imm(IntCC::NotEqual, call_ok_val, 0);

    let call_block = builder.create_block();
    let fail_block = builder.create_block();
    let done_block = builder.create_block();
    builder.append_block_param(done_block, PTR_TYPE);
    builder
        .ins()
        .brif(call_ok_bool, call_block, &[], fail_block, &[]);

    builder.switch_to_block(fail_block);
    let fail_args = [BlockArg::Value(null_ptr)];
    builder.ins().jump(done_block, &fail_args);
    builder.seal_block(fail_block);

    builder.switch_to_block(call_block);
    let callee_ref = module.declare_func_in_func(meta.id, builder.func);
    let call = match meta.call_abi {
        JitCallAbi::Array => {
            let args_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
        }
        JitCallAbi::Reg1 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
        }
        JitCallAbi::Reg2 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let args = [ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero];
            builder.ins().call(callee_ref, &args)
        }
        JitCallAbi::Reg3 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            let args = [
                ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
            ];
            builder.ins().call(callee_ref, &args)
        }
        JitCallAbi::Reg4 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
            let args = [
                ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                null_ptr, zero,
            ];
            builder.ins().call(callee_ref, &args)
        }
    };
    let raw_tag = builder.inst_results(call)[0];
    let raw_payload = builder.inst_results(call)[1];
    let raw_result = boxed_value_from_tagged_parts(
        module, helpers, builder, ctx_val, raw_tag, raw_payload,
    );
    let check_ret = get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_return");
    let checked_ret_call = builder.ins().call(
        check_ret,
        &[
            ctx_val,
            fn_index,
            raw_result,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let checked_ret = builder.inst_results(checked_ret_call)[0];
    emit_return_on_null_value(builder, checked_ret);
    let ok_args = [BlockArg::Value(checked_ret)];
    builder.ins().jump(done_block, &ok_args);
    builder.seal_block(call_block);

    builder.switch_to_block(done_block);
    let result = builder.block_params(done_block)[0];
    builder.seal_block(done_block);
    Ok(JitValue::boxed(result))
}

fn compile_jit_cfg_base_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    base_idx: usize,
    arg_vals: &[JitValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let (_slot, args_ptr) = emit_tagged_stack_array(builder, arg_vals);
    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let call_base =
        get_helper_func_ref(module, helpers, builder, "rt_call_base_function_tagged");
    let call = builder.ins().call(
        call_base,
        &[
            ctx_val, idx_val, args_ptr, nargs_val, start_line_val, start_col_val,
            end_line_val, end_col_val,
        ],
    );
    let result = builder.inst_results(call)[0];
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn compile_jit_cfg_user_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    meta: JitFunctionMeta,
    arg_vals: &[JitValue],
    call_span: Span,
) -> Result<JitValue, String> {
    let nargs = arg_vals.len();
    if nargs != meta.num_params {
        return Err(format!(
            "wrong number of arguments in JIT CFG call: want={}, got={}",
            meta.num_params, nargs
        ));
    }

    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    let zero = builder.ins().iconst(PTR_TYPE, 0);
    let callee_ref = module.declare_func_in_func(meta.id, builder.func);

    if !meta.has_contract {
        let call = match meta.call_abi {
            JitCallAbi::Array => {
                let (_slot, args_ptr) = emit_tagged_stack_array(builder, arg_vals);
                let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
            }
            JitCallAbi::Reg1 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                builder
                    .ins()
                    .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
            }
            JitCallAbi::Reg2 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                builder.ins().call(
                    callee_ref,
                    &[ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero],
                )
            }
            JitCallAbi::Reg3 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                builder.ins().call(
                    callee_ref,
                    &[
                        ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
                    ],
                )
            }
            JitCallAbi::Reg4 => {
                let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
                let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
                let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
                let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
                builder.ins().call(
                    callee_ref,
                    &[
                        ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                        null_ptr, zero,
                    ],
                )
            }
        };
        let raw_tag = builder.inst_results(call)[0];
        let raw_payload = builder.inst_results(call)[1];
        let raw_result =
            boxed_value_from_tagged_parts(module, helpers, builder, ctx_val, raw_tag, raw_payload);
        emit_return_on_null_value(builder, raw_result);
        return Ok(JitValue::boxed(raw_result));
    }

    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let start_line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let start_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);
    let end_line_val = builder.ins().iconst(PTR_TYPE, call_span.end.line as i64);
    let end_col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.end.column + 1) as i64);
    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();

    let call_ok = match meta.call_abi {
        JitCallAbi::Array => {
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                boxed_arg_vals.len().max(1) as u32 * 8,
                3,
            ));
            for (i, value) in boxed_arg_vals.iter().enumerate() {
                builder.ins().stack_store(*value, slot, (i * 8) as i32);
            }
            let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    args_ptr,
                    nargs_val,
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg1 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call1");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg2 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call2");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg3 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call3");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
        JitCallAbi::Reg4 => {
            let check_call =
                get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call4");
            builder.ins().call(
                check_call,
                &[
                    ctx_val,
                    fn_index,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                    start_line_val,
                    start_col_val,
                    end_line_val,
                    end_col_val,
                ],
            )
        }
    };
    let call_ok_val = builder.inst_results(call_ok)[0];
    let call_ok_bool = builder.ins().icmp_imm(IntCC::NotEqual, call_ok_val, 0);
    let call_block = builder.create_block();
    let fail_block = builder.create_block();
    let done_block = builder.create_block();
    builder.append_block_param(done_block, PTR_TYPE);
    builder
        .ins()
        .brif(call_ok_bool, call_block, &[], fail_block, &[]);

    builder.switch_to_block(fail_block);
    builder.ins().jump(done_block, &[BlockArg::Value(null_ptr)]);
    builder.seal_block(fail_block);

    builder.switch_to_block(call_block);
    let call = match meta.call_abi {
        JitCallAbi::Array => {
            let (_slot, args_ptr) = emit_tagged_stack_array(builder, arg_vals);
            let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero])
        }
        JitCallAbi::Reg1 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            builder
                .ins()
                .call(callee_ref, &[ctx_val, tag0, payload0, null_ptr, zero])
        }
        JitCallAbi::Reg2 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            builder.ins().call(
                callee_ref,
                &[ctx_val, tag0, payload0, tag1, payload1, null_ptr, zero],
            )
        }
        JitCallAbi::Reg3 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            builder.ins().call(
                callee_ref,
                &[
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, null_ptr, zero,
                ],
            )
        }
        JitCallAbi::Reg4 => {
            let (tag0, payload0) = jit_value_to_tag_payload(builder, arg_vals[0]);
            let (tag1, payload1) = jit_value_to_tag_payload(builder, arg_vals[1]);
            let (tag2, payload2) = jit_value_to_tag_payload(builder, arg_vals[2]);
            let (tag3, payload3) = jit_value_to_tag_payload(builder, arg_vals[3]);
            builder.ins().call(
                callee_ref,
                &[
                    ctx_val, tag0, payload0, tag1, payload1, tag2, payload2, tag3, payload3,
                    null_ptr, zero,
                ],
            )
        }
    };
    let raw_tag = builder.inst_results(call)[0];
    let raw_payload = builder.inst_results(call)[1];
    let raw_result =
        boxed_value_from_tagged_parts(module, helpers, builder, ctx_val, raw_tag, raw_payload);
    let check_ret = get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_return");
    let checked_ret_call = builder.ins().call(
        check_ret,
        &[
            ctx_val,
            fn_index,
            raw_result,
            start_line_val,
            start_col_val,
            end_line_val,
            end_col_val,
        ],
    );
    let checked_ret = builder.inst_results(checked_ret_call)[0];
    emit_return_on_null_value(builder, checked_ret);
    builder.ins().jump(done_block, &[BlockArg::Value(checked_ret)]);
    builder.seal_block(call_block);

    builder.switch_to_block(done_block);
    let result = builder.block_params(done_block)[0];
    builder.seal_block(done_block);
    Ok(JitValue::boxed(result))
}

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
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn compile_generic_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    call_span: crate::diagnostics::position::Span,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let callee = compile_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        function,
        interner,
    )?;

    let callee = box_and_guard_jit_value(module, helpers, builder, ctx_val, callee);
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_boxed_array(builder, &arg_vals);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
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
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn compile_ir_generic_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    call_span: crate::diagnostics::position::Span,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let callee = compile_ir_expression(
        module,
        helpers,
        builder,
        function_compiler,
        scope,
        ctx_val,
        return_block,
        tail_call,
        function,
        interner,
    )?;

    let callee = box_and_guard_jit_value(module, helpers, builder, ctx_val, callee);
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(box_and_guard_jit_value(
            module, helpers, builder, ctx_val, val,
        ));
    }

    let nargs = arg_vals.len();
    let args_ptr = function_compiler.emit_boxed_array(builder, &arg_vals);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
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
    emit_return_on_null_value(builder, result);
    Ok(JitValue::boxed(result))
}

fn compile_ir_adt_constructor_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    function_compiler: &FunctionCompiler,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    name: Identifier,
    arity: usize,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<JitValue, String> {
    let name_str = interner.resolve(name);
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

    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let value = compile_ir_expression(
            module,
            helpers,
            builder,
            function_compiler,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        emit_return_on_null_jit_value(module, helpers, builder, ctx_val, value);
        arg_vals.push(value);
    }

    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();

    emit_push_gc_roots(module, helpers, builder, ctx_val, &boxed_arg_vals);

    let call = match arity {
        1 => {
            let make_adt1 = get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
            builder
                .ins()
                .call(make_adt1, &[ctx_val, name_ptr, name_len, boxed_arg_vals[0]])
        }
        2 => {
            let make_adt2 = get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
            builder.ins().call(
                make_adt2,
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
            let make_adt3 = get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
            builder.ins().call(
                make_adt3,
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
            let make_adt4 = get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
            builder.ins().call(
                make_adt4,
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
            let make_adt5 = get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
            builder.ins().call(
                make_adt5,
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
            let fields_ptr = function_compiler.emit_tagged_array(builder, &arg_vals);
            let arity_value = builder.ins().iconst(PTR_TYPE, arity as i64);
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

fn compile_function_literal(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    expr: &Expression,
    _interner: &Interner,
) -> Result<CraneliftValue, String> {
    let key = LiteralKey::from_expr(expr);
    let Some(meta) = scope.literal_functions.get(&key).copied() else {
        return Err("missing literal function metadata in JIT".to_string());
    };
    let captures = scope
        .literal_captures
        .get(&key)
        .cloned()
        .unwrap_or_default();

    let mut capture_vals: Vec<JitValue> = Vec::new();
    for sym in captures {
        if let Some(binding) = scope.locals.get(&sym).cloned() {
            let value = use_local(builder, binding);
            capture_vals.push(value);
            continue;
        }
        if let Some(fn_meta) = scope.functions.get(&sym).copied() {
            let make_jit_closure =
                get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let fn_idx = builder
                .ins()
                .iconst(PTR_TYPE, fn_meta.function_index as i64);
            let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
            let zero = builder.ins().iconst(PTR_TYPE, 0);
            let call = builder
                .ins()
                .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        if let Some(&idx) = scope.globals.get(&sym) {
            let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
            let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
            let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        if let Some(&base_idx) = scope.base_functions.get(&sym) {
            let make_base = get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
            let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
            let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        return Err("unsupported capture in JIT function literal".to_string());
    }

    let (_slot, captures_ptr) = emit_tagged_stack_array(builder, &capture_vals);
    let ncaptures = builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let make_jit_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
    let call = builder.ins().call(
        make_jit_closure,
        &[ctx_val, fn_idx, captures_ptr, ncaptures],
    );
    Ok(builder.inst_results(call)[0])
}

fn compile_ir_function_literal(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    parameters: &[Identifier],
    parameter_types: &[Option<TypeExpr>],
    return_type: &Option<TypeExpr>,
    effects: &[crate::syntax::effect_expr::EffectExpr],
    body: &Block,
    span: Span,
    _interner: &Interner,
) -> Result<CraneliftValue, String> {
    let key = LiteralKey::from_ir_function(parameters, body.span, span);
    let Some(meta) = scope.literal_functions.get(&key).copied() else {
        return Err("missing literal function metadata in JIT".to_string());
    };
    let captures = scope
        .literal_captures
        .get(&key)
        .cloned()
        .unwrap_or_default();

    let mut capture_vals: Vec<JitValue> = Vec::new();
    for sym in captures {
        if let Some(binding) = scope.locals.get(&sym).cloned() {
            let value = use_local(builder, binding);
            capture_vals.push(value);
            continue;
        }
        if let Some(fn_meta) = scope.functions.get(&sym).copied() {
            let make_jit_closure =
                get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let fn_idx = builder
                .ins()
                .iconst(PTR_TYPE, fn_meta.function_index as i64);
            let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
            let zero = builder.ins().iconst(PTR_TYPE, 0);
            let call = builder
                .ins()
                .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        if let Some(&idx) = scope.globals.get(&sym) {
            let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
            let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
            let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        if let Some(&base_idx) = scope.base_functions.get(&sym) {
            let make_base = get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
            let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
            let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
            capture_vals.push(JitValue::boxed(builder.inst_results(call)[0]));
            continue;
        }
        return Err("unsupported capture in JIT function literal".to_string());
    }

    let _ = (parameter_types, return_type, effects);
    let (_slot, captures_ptr) = emit_tagged_stack_array(builder, &capture_vals);
    let ncaptures = builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let make_jit_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
    let call = builder.ins().call(
        make_jit_closure,
        &[ctx_val, fn_idx, captures_ptr, ncaptures],
    );
    Ok(builder.inst_results(call)[0])
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

fn is_base_symbol(name: Identifier, interner: &Interner) -> bool {
    interner
        .try_resolve(name)
        .is_some_and(|name| name == "Base")
}

fn apply_base_directives(
    top_level_statements: &[Statement],
    scope: &mut Scope,
    interner: &Interner,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for stmt in top_level_statements {
        let Statement::Import {
            name,
            alias,
            except,
            ..
        } = stmt
        else {
            continue;
        };
        if !is_base_symbol(*name, interner) {
            continue;
        }
        if let Some(alias) = alias {
            return Err(format!(
                "`import Base as {}` is not allowed",
                interner.resolve(*alias)
            ));
        }
        for excluded in except {
            if !seen.insert(*excluded) {
                return Err(format!(
                    "duplicate Base exclusion `{}`",
                    interner.resolve(*excluded)
                ));
            }
            let excluded_name = interner.resolve(*excluded);
            if BaseModule::new().index_of(excluded_name).is_none() {
                return Err(format!("unknown Base member: {}", excluded_name));
            }
            scope.excluded_base_symbols.insert(*excluded);
            scope.base_functions.remove(excluded);
        }
    }
    Ok(())
}

fn apply_ir_base_directives(
    top_level_items: &[IrTopLevelItem],
    scope: &mut Scope,
    interner: &Interner,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for item in top_level_items {
        let IrTopLevelItem::Import {
            name,
            alias,
            except,
            ..
        } = item
        else {
            continue;
        };
        if !is_base_symbol(*name, interner) {
            continue;
        }
        if let Some(alias) = alias {
            return Err(format!(
                "`import Base as {}` is not allowed",
                interner.resolve(*alias)
            ));
        }
        for excluded in except {
            if !seen.insert(*excluded) {
                return Err(format!(
                    "duplicate Base exclusion `{}`",
                    interner.resolve(*excluded)
                ));
            }
            let excluded_name = interner.resolve(*excluded);
            if BaseModule::new().index_of(excluded_name).is_none() {
                return Err(format!("unknown Base member: {}", excluded_name));
            }
            scope.excluded_base_symbols.insert(*excluded);
            scope.base_functions.remove(excluded);
        }
    }
    Ok(())
}

fn ir_top_level_has_explicit_main_call(
    top_level_items: &[IrTopLevelItem],
    interner: &Interner,
) -> bool {
    top_level_items.iter().any(|item| {
        matches!(
            item,
            IrTopLevelItem::Expression {
                expression: Expression::Call { function, arguments, .. },
                ..
            } if matches!(function.as_ref(), Expression::Identifier { name, .. } if interner.resolve(*name) == "main")
                && arguments.is_empty()
        )
    })
}

fn collect_literal_function_specs(top_level_statements: &[Statement]) -> Vec<LiteralFunctionSpec> {
    let mut collector = LiteralCollector::new();
    collector.collect_statements(top_level_statements);
    collector.specs
}

fn collect_ir_literal_function_specs(top_level_items: &[IrTopLevelItem]) -> Vec<IrLiteralFunctionSpec> {
    let mut collector = IrLiteralCollector::new();
    collector.collect_items(top_level_items);
    collector.specs
}

struct LiteralCollector {
    scopes: Vec<HashSet<Identifier>>,
    specs: Vec<LiteralFunctionSpec>,
    seen: HashSet<LiteralKey>,
}

struct IrLiteralCollector {
    scopes: Vec<HashSet<Identifier>>,
    specs: Vec<IrLiteralFunctionSpec>,
    seen: HashSet<LiteralKey>,
}

impl LiteralCollector {
    fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
            specs: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn collect_statements(&mut self, top_level_statements: &[Statement]) {
        // Pre-bind top-level function names for recursion/references.
        for stmt in top_level_statements {
            if let Statement::Function { name, .. } = stmt {
                self.define(*name);
            }
        }
        for stmt in top_level_statements {
            self.collect_stmt(stmt);
        }
    }

    fn define(&mut self, ident: Identifier) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(ident);
        }
    }

    fn is_bound(&self, ident: Identifier) -> bool {
        self.scopes.iter().rev().any(|s| s.contains(&ident))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind_pattern_identifiers(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => self.define(*name),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.bind_pattern_identifiers(pattern),
            Pattern::Cons { head, tail, .. } => {
                self.bind_pattern_identifiers(head);
                self.bind_pattern_identifiers(tail);
            }
            Pattern::Tuple { elements, .. } => {
                for element in elements {
                    self.bind_pattern_identifiers(element);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for field in fields {
                    self.bind_pattern_identifiers(field);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Literal { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. } => {}
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let { name, value, .. } => {
                self.collect_expr(value);
                self.define(*name);
            }
            Statement::LetDestructure { pattern, value, .. } => {
                self.collect_expr(value);
                self.bind_pattern_identifiers(pattern);
            }
            Statement::Assign { value, .. } => self.collect_expr(value),
            Statement::Expression { expression, .. } => self.collect_expr(expression),
            Statement::Return { value, .. } => {
                if let Some(v) = value {
                    self.collect_expr(v);
                }
            }
            Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let expr = Expression::Function {
                    parameters: parameters.clone(),
                    parameter_types: parameter_types.clone(),
                    return_type: return_type.clone(),
                    effects: effects.clone(),
                    body: body.clone(),
                    span: stmt.span(),
                    id: ExprId::UNSET,
                };
                let key = LiteralKey::from_expr(&expr);
                if !self.seen.contains(&key) {
                    let mut captures: Vec<Identifier> = collect_free_vars(&expr)
                        .into_iter()
                        .filter(|sym| self.is_bound(*sym))
                        .collect();
                    // Recursive local functions should not capture themselves.
                    captures.retain(|sym| sym != name);
                    captures.sort_by_key(|s| s.as_u32());
                    self.specs.push(LiteralFunctionSpec {
                        key,
                        parameters: parameters.clone(),
                        parameter_types: parameter_types.clone(),
                        return_type: return_type.clone(),
                        body: body.clone(),
                        captures,
                        self_name: Some(*name),
                    });
                    self.seen.insert(key);
                }

                // Function name is bound in outer scope after declaration.
                self.define(*name);

                self.push_scope();
                // Recursive references resolve in function body.
                self.define(*name);
                for p in parameters {
                    self.define(*p);
                }
                for s in &body.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Statement::Module { body, .. } => {
                self.push_scope();
                // Pre-bind all module function names before processing bodies.
                // This mirrors collect_program's pre-binding for top-level functions and
                // ensures nested literal functions inside module functions can correctly
                // include sibling module functions in their capture sets.
                for s in &body.statements {
                    if let Statement::Function { name, .. } = s {
                        self.define(*name);
                    }
                }
                // Process module body: module-level functions are compiled as module
                // functions (not literal closures), so we must NOT register them as
                // literal specs. We only collect nested literal functions from their bodies.
                for s in &body.statements {
                    match s {
                        Statement::Function {
                            name,
                            parameters,
                            body,
                            ..
                        } => {
                            // Module-level function: push a scope with params and collect
                            // any nested literal functions defined inside the body.
                            self.push_scope();
                            self.define(*name); // allow self-recursion within the body
                            for p in parameters {
                                self.define(*p);
                            }
                            for inner in &body.statements {
                                self.collect_stmt(inner);
                            }
                            self.pop_scope();
                        }
                        _ => self.collect_stmt(s),
                    }
                }
                self.pop_scope();
            }
            Statement::Import { .. } => {}
            Statement::Data { .. } => {}
            Statement::EffectDecl { .. } => {}
        }
    }

    fn collect_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                body,
                ..
            } => {
                let key = LiteralKey::from_expr(expr);
                if !self.seen.contains(&key) {
                    let mut captures: Vec<Identifier> = collect_free_vars(expr)
                        .into_iter()
                        .filter(|sym| self.is_bound(*sym))
                        .collect();
                    captures.sort_by_key(|s| s.as_u32());
                    self.specs.push(LiteralFunctionSpec {
                        key,
                        parameters: parameters.clone(),
                        parameter_types: parameter_types.clone(),
                        return_type: return_type.clone(),
                        body: body.clone(),
                        captures,
                        self_name: None,
                    });
                    self.seen.insert(key);
                }

                self.push_scope();
                for p in parameters {
                    self.define(*p);
                }
                for s in &body.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Expression::Prefix { right, .. } => self.collect_expr(right),
            Expression::Infix { left, right, .. } => {
                self.collect_expr(left);
                self.collect_expr(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_expr(condition);
                self.push_scope();
                for s in &consequence.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
                if let Some(alt) = alternative {
                    self.push_scope();
                    for s in &alt.statements {
                        self.collect_stmt(s);
                    }
                    self.pop_scope();
                }
            }
            Expression::DoBlock { block, .. } => {
                self.push_scope();
                for s in &block.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_expr(function);
                for a in arguments {
                    self.collect_expr(a);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for e in elements {
                    self.collect_expr(e);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_expr(left);
                self.collect_expr(index);
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.collect_expr(k);
                    self.collect_expr(v);
                }
            }
            Expression::MemberAccess { object, .. } => self.collect_expr(object),
            Expression::TupleFieldAccess { object, .. } => self.collect_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern_identifiers(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.collect_expr(g);
                    }
                    self.collect_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.collect_expr(value),
            Expression::Cons { head, tail, .. } => {
                self.collect_expr(head);
                self.collect_expr(tail);
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let crate::syntax::expression::StringPart::Interpolation(expr) = part {
                        self.collect_expr(expr);
                    }
                }
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.collect_expr(arg);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_expr(expr);
                for arm in arms {
                    // Build the same synthetic Function expression used by compile_jit_handle,
                    // so each arm closure is pre-compiled as a literal function spec.
                    let mut params = vec![arm.resume_param];
                    params.extend_from_slice(&arm.params);
                    let arm_fn_expr = Expression::Function {
                        parameters: params.clone(),
                        parameter_types: vec![None; params.len()],
                        return_type: None,
                        effects: vec![],
                        body: crate::syntax::block::Block {
                            statements: vec![crate::syntax::statement::Statement::Expression {
                                expression: arm.body.clone(),
                                has_semicolon: false,
                                span: arm.body.span(),
                            }],
                            span: arm.body.span(),
                        },
                        span: arm.span,
                        id: ExprId::UNSET,
                    };
                    self.collect_expr(&arm_fn_expr);
                }
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::EmptyList { .. }
            | Expression::None { .. } => {}
        }
    }
}

impl IrLiteralCollector {
    fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
            specs: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn collect_items(&mut self, top_level_items: &[IrTopLevelItem]) {
        for item in top_level_items {
            if let IrTopLevelItem::Function { name, .. } = item {
                self.define(*name);
            }
        }
        for item in top_level_items {
            self.collect_item(item);
        }
    }

    fn define(&mut self, ident: Identifier) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(ident);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind_pattern_identifiers(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => self.define(*name),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.bind_pattern_identifiers(pattern),
            Pattern::Cons { head, tail, .. } => {
                self.bind_pattern_identifiers(head);
                self.bind_pattern_identifiers(tail);
            }
            Pattern::Tuple { elements, .. } => {
                for element in elements {
                    self.bind_pattern_identifiers(element);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for field in fields {
                    self.bind_pattern_identifiers(field);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Literal { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. } => {}
        }
    }

    fn bound_scope_set(&self) -> HashSet<Identifier> {
        self.scopes
            .iter()
            .flat_map(|scope| scope.iter().copied())
            .collect()
    }

    fn collect_capture_spec(
        &mut self,
        key: LiteralKey,
        parameters: &[Identifier],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        body: &Block,
        self_name: Option<Identifier>,
    ) {
        if self.seen.contains(&key) {
            return;
        }

        let outer_bound = self.bound_scope_set();
        let mut function_bound = HashSet::new();
        for param in parameters {
            function_bound.insert(*param);
        }
        if let Some(self_name) = self_name {
            function_bound.insert(self_name);
        }
        let mut captures = collect_ir_free_vars_in_block(body, &function_bound)
            .into_iter()
            .filter(|ident| outer_bound.contains(ident))
            .collect::<Vec<_>>();
        captures.sort_by_key(|s| s.as_u32());
        self.specs.push(IrLiteralFunctionSpec {
            key,
            parameters: parameters.to_vec(),
            parameter_types: parameter_types.to_vec(),
            return_type: return_type.clone(),
            body: body.clone(),
            captures,
            self_name,
        });
        self.seen.insert(key);
    }

    fn collect_item(&mut self, item: &IrTopLevelItem) {
        match item {
            IrTopLevelItem::Let { name, value, .. } => {
                self.collect_expr(value);
                self.define(*name);
            }
            IrTopLevelItem::LetDestructure { pattern, value, .. } => {
                self.collect_expr(value);
                self.bind_pattern_identifiers(pattern);
            }
            IrTopLevelItem::Assign { value, .. } => self.collect_expr(value),
            IrTopLevelItem::Expression { expression, .. } => self.collect_expr(expression),
            IrTopLevelItem::Return { value, .. } => {
                if let Some(v) = value {
                    self.collect_expr(v);
                }
            }
            IrTopLevelItem::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                body,
                span,
                ..
            } => {
                let key = LiteralKey::from_ir_function(parameters, body.span, *span);
                self.collect_capture_spec(
                    key,
                    parameters,
                    parameter_types,
                    return_type,
                    body,
                    Some(*name),
                );

                self.define(*name);
                self.push_scope();
                self.define(*name);
                for p in parameters {
                    self.define(*p);
                }
                for stmt in &body.statements {
                    self.collect_stmt_for_ir(stmt);
                }
                self.pop_scope();
            }
            IrTopLevelItem::Module { body, .. } => {
                self.push_scope();
                for item in body {
                    if let IrTopLevelItem::Function { name, .. } = item {
                        self.define(*name);
                    }
                }
                for item in body {
                    match item {
                        IrTopLevelItem::Function {
                            name,
                            parameters,
                            body,
                            ..
                        } => {
                            self.push_scope();
                            self.define(*name);
                            for p in parameters {
                                self.define(*p);
                            }
                            for stmt in &body.statements {
                                self.collect_stmt_for_ir(stmt);
                            }
                            self.pop_scope();
                        }
                        _ => self.collect_item(item),
                    }
                }
                self.pop_scope();
            }
            IrTopLevelItem::Import { .. }
            | IrTopLevelItem::Data { .. }
            | IrTopLevelItem::EffectDecl { .. } => {}
        }
    }

    fn collect_stmt_for_ir(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let { name, value, .. } => {
                self.collect_expr(value);
                self.define(*name);
            }
            Statement::LetDestructure { pattern, value, .. } => {
                self.collect_expr(value);
                self.bind_pattern_identifiers(pattern);
            }
            Statement::Assign { value, .. } => self.collect_expr(value),
            Statement::Expression { expression, .. } => self.collect_expr(expression),
            Statement::Return { value, .. } => {
                if let Some(v) = value {
                    self.collect_expr(v);
                }
            }
            Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                body,
                span,
                ..
            } => {
                let key = LiteralKey::from_ir_function(parameters, body.span, *span);
                self.collect_capture_spec(
                    key,
                    parameters,
                    parameter_types,
                    return_type,
                    body,
                    Some(*name),
                );
                self.define(*name);
                self.push_scope();
                self.define(*name);
                for p in parameters {
                    self.define(*p);
                }
                for s in &body.statements {
                    self.collect_stmt_for_ir(s);
                }
                self.pop_scope();
            }
            Statement::Module { body, .. } => {
                self.push_scope();
                for s in &body.statements {
                    if let Statement::Function { name, .. } = s {
                        self.define(*name);
                    }
                }
                for s in &body.statements {
                    self.collect_stmt_for_ir(s);
                }
                self.pop_scope();
            }
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. } => {}
        }
    }

    fn collect_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } => {
                let key = LiteralKey::from_ir_function(parameters, body.span, *span);
                self.collect_capture_spec(
                    key,
                    parameters,
                    parameter_types,
                    return_type,
                    body,
                    None,
                );

                let _ = effects;
                self.push_scope();
                for p in parameters {
                    self.define(*p);
                }
                for stmt in &body.statements {
                    self.collect_stmt_for_ir(stmt);
                }
                self.pop_scope();
            }
            Expression::Prefix { right, .. }
            | Expression::Some { value: right, .. }
            | Expression::Left { value: right, .. }
            | Expression::Right { value: right, .. } => self.collect_expr(right),
            Expression::Infix { left, right, .. }
            | Expression::Cons {
                head: left,
                tail: right,
                ..
            } => {
                self.collect_expr(left);
                self.collect_expr(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_expr(condition);
                self.push_scope();
                for stmt in &consequence.statements {
                    self.collect_stmt_for_ir(stmt);
                }
                self.pop_scope();
                if let Some(alt) = alternative {
                    self.push_scope();
                    for stmt in &alt.statements {
                        self.collect_stmt_for_ir(stmt);
                    }
                    self.pop_scope();
                }
            }
            Expression::DoBlock { block, .. } => {
                self.push_scope();
                for stmt in &block.statements {
                    self.collect_stmt_for_ir(stmt);
                }
                self.pop_scope();
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_expr(function);
                for arg in arguments {
                    self.collect_expr(arg);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.collect_expr(element);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_expr(left);
                self.collect_expr(index);
            }
            Expression::Hash { pairs, .. } => {
                for (key, value) in pairs {
                    self.collect_expr(key);
                    self.collect_expr(value);
                }
            }
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => self.collect_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern_identifiers(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.collect_expr(guard);
                    }
                    self.collect_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(expr) = part {
                        self.collect_expr(expr);
                    }
                }
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.collect_expr(arg);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_expr(expr);
                for arm in arms {
                    let body = Block {
                        statements: vec![Statement::Expression {
                            expression: arm.body.clone(),
                            has_semicolon: false,
                            span: arm.span,
                        }],
                        span: arm.span,
                    };
                    let key = LiteralKey::from_ir_function(
                        &[arm.resume_param],
                        body.span,
                        arm.span,
                    );
                    if !self.seen.contains(&key) {
                        let arm_expr = Expression::Function {
                            parameters: {
                                let mut params = vec![arm.resume_param];
                                params.extend_from_slice(&arm.params);
                                params
                            },
                            parameter_types: vec![None; arm.params.len() + 1],
                            return_type: None,
                            effects: vec![],
                            body: body.clone(),
                            span: arm.span,
                            id: ExprId::UNSET,
                        };
                        if let Expression::Function { parameters, parameter_types, return_type, body, .. } = arm_expr {
                            self.collect_capture_spec(
                                LiteralKey::from_ir_function(&parameters, body.span, arm.span),
                                &parameters,
                                &parameter_types,
                                &return_type,
                                &body,
                                None,
                            );
                        }
                    }
                }
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::EmptyList { .. }
            | Expression::None { .. } => {}
        }
    }
}

fn collect_ir_free_vars_in_block(
    block: &Block,
    bound: &HashSet<Identifier>,
) -> HashSet<Identifier> {
    let mut locals = bound.clone();
    let mut free = HashSet::new();
    for stmt in &block.statements {
        collect_ir_free_vars_in_stmt(stmt, &mut locals, &mut free);
    }
    free
}

fn collect_ir_free_vars_in_stmt(
    stmt: &Statement,
    bound: &mut HashSet<Identifier>,
    free: &mut HashSet<Identifier>,
) {
    match stmt {
        Statement::Let { name, value, .. } => {
            collect_ir_free_vars_in_expr(value, bound, free);
            bound.insert(*name);
        }
        Statement::LetDestructure { pattern, value, .. } => {
            collect_ir_free_vars_in_expr(value, bound, free);
            bind_ir_pattern_names_to_set(pattern, bound);
        }
        Statement::Assign { value, .. } => collect_ir_free_vars_in_expr(value, bound, free),
        Statement::Expression { expression, .. } => {
            collect_ir_free_vars_in_expr(expression, bound, free)
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                collect_ir_free_vars_in_expr(value, bound, free);
            }
        }
        Statement::Function {
            name,
            parameters,
            body,
            ..
        } => {
            let mut nested_bound = bound.clone();
            nested_bound.insert(*name);
            for param in parameters {
                nested_bound.insert(*param);
            }
            let nested_free = collect_ir_free_vars_in_block(body, &nested_bound);
            free.extend(nested_free.into_iter().filter(|ident| bound.contains(ident)));
            bound.insert(*name);
        }
        Statement::Module { body, .. } => {
            let mut nested_bound = bound.clone();
            for stmt in &body.statements {
                if let Statement::Function { name, .. } = stmt {
                    nested_bound.insert(*name);
                }
            }
            for stmt in &body.statements {
                collect_ir_free_vars_in_stmt(stmt, &mut nested_bound, free);
            }
        }
        Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. } => {}
    }
}

fn collect_ir_free_vars_in_expr(
    expr: &Expression,
    bound: &HashSet<Identifier>,
    free: &mut HashSet<Identifier>,
) {
    match expr {
        Expression::Identifier { name, .. } => {
            if !bound.contains(name) {
                free.insert(*name);
            }
        }
        Expression::Function {
            parameters, body, ..
        } => {
            let mut nested_bound = bound.clone();
            for param in parameters {
                nested_bound.insert(*param);
            }
            free.extend(
                collect_ir_free_vars_in_block(body, &nested_bound)
                    .into_iter()
                    .filter(|ident| bound.contains(ident)),
            );
        }
        Expression::Prefix { right, .. }
        | Expression::Some { value: right, .. }
        | Expression::Left { value: right, .. }
        | Expression::Right { value: right, .. } => {
            collect_ir_free_vars_in_expr(right, bound, free)
        }
        Expression::Infix { left, right, .. }
        | Expression::Cons {
            head: left,
            tail: right,
            ..
        } => {
            collect_ir_free_vars_in_expr(left, bound, free);
            collect_ir_free_vars_in_expr(right, bound, free);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_ir_free_vars_in_expr(condition, bound, free);
            free.extend(
                collect_ir_free_vars_in_block(consequence, bound)
                    .into_iter()
                    .filter(|ident| bound.contains(ident)),
            );
            if let Some(alternative) = alternative {
                free.extend(
                    collect_ir_free_vars_in_block(alternative, bound)
                        .into_iter()
                        .filter(|ident| bound.contains(ident)),
                );
            }
        }
        Expression::DoBlock { block, .. } => {
            free.extend(
                collect_ir_free_vars_in_block(block, bound)
                    .into_iter()
                    .filter(|ident| bound.contains(ident)),
            );
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_ir_free_vars_in_expr(function, bound, free);
            for arg in arguments {
                collect_ir_free_vars_in_expr(arg, bound, free);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for element in elements {
                collect_ir_free_vars_in_expr(element, bound, free);
            }
        }
        Expression::Index { left, index, .. } => {
            collect_ir_free_vars_in_expr(left, bound, free);
            collect_ir_free_vars_in_expr(index, bound, free);
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                collect_ir_free_vars_in_expr(key, bound, free);
                collect_ir_free_vars_in_expr(value, bound, free);
            }
        }
        Expression::MemberAccess { object, .. }
        | Expression::TupleFieldAccess { object, .. } => {
            collect_ir_free_vars_in_expr(object, bound, free)
        }
        Expression::Match { scrutinee, arms, .. } => {
            collect_ir_free_vars_in_expr(scrutinee, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                bind_ir_pattern_names_to_set(&arm.pattern, &mut arm_bound);
                if let Some(guard) = &arm.guard {
                    collect_ir_free_vars_in_expr(guard, &arm_bound, free);
                }
                collect_ir_free_vars_in_expr(&arm.body, &arm_bound, free);
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    collect_ir_free_vars_in_expr(expr, bound, free);
                }
            }
        }
        Expression::Perform { args, .. } => {
            for arg in args {
                collect_ir_free_vars_in_expr(arg, bound, free);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            collect_ir_free_vars_in_expr(expr, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                arm_bound.insert(arm.resume_param);
                for param in &arm.params {
                    arm_bound.insert(*param);
                }
                collect_ir_free_vars_in_expr(&arm.body, &arm_bound, free);
            }
        }
        Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
    }
}

fn bind_ir_pattern_names_to_set(pattern: &Pattern, out: &mut HashSet<Identifier>) {
    match pattern {
        Pattern::Identifier { name, .. } => {
            out.insert(*name);
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => bind_ir_pattern_names_to_set(pattern, out),
        Pattern::Cons { head, tail, .. } => {
            bind_ir_pattern_names_to_set(head, out);
            bind_ir_pattern_names_to_set(tail, out);
        }
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                bind_ir_pattern_names_to_set(element, out);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for field in fields {
                bind_ir_pattern_names_to_set(field, out);
            }
        }
        Pattern::Wildcard { .. }
        | Pattern::Literal { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

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

enum StmtOutcome {
    None,
    Value(JitValue),
    Returned,
}

enum BlockEval {
    Value(JitValue),
    Returned,
}

#[derive(Clone)]
struct TailCallContext {
    function_name: Option<Identifier>,
    loop_block: cranelift_codegen::ir::Block,
    params: Vec<(Identifier, Variable)>,
}

struct FunctionCompiler {
    boxed_array_slot: Option<StackSlot>,
    boxed_array_capacity: usize,
    tagged_array_slot: Option<StackSlot>,
    tagged_array_capacity: usize,
}

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
            boxed_array_capacity,
            tagged_array_slot,
            tagged_array_capacity,
        }
    }

    fn emit_boxed_array(
        &self,
        builder: &mut FunctionBuilder,
        values: &[CraneliftValue],
    ) -> CraneliftValue {
        debug_assert!(values.len() <= self.boxed_array_capacity);
        if values.is_empty() {
            return builder.ins().iconst(PTR_TYPE, 0);
        }
        let slot = self
            .boxed_array_slot
            .expect("boxed array slot must be preallocated");
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
        debug_assert!(values.len() <= self.tagged_array_capacity);
        if values.is_empty() {
            return builder.ins().iconst(PTR_TYPE, 0);
        }
        let slot = self
            .tagged_array_slot
            .expect("tagged array slot must be preallocated");
        for (i, value) in values.iter().enumerate() {
            let (tag, payload) = jit_value_to_tag_payload(builder, *value);
            builder.ins().stack_store(tag, slot, (i * 16) as i32);
            builder.ins().stack_store(payload, slot, (i * 16 + 8) as i32);
        }
        builder.ins().stack_addr(PTR_TYPE, slot, 0)
    }
}

fn note_boxed_array_usage(current_max: &mut usize, len: usize) {
    *current_max = (*current_max).max(len.max(1));
}

fn max_boxed_array_len_in_block(block: &Block) -> usize {
    max_boxed_array_len_in_statements(&block.statements)
}

fn max_boxed_array_len_in_ir_block(block: &Block) -> usize {
    max_boxed_array_len_in_statements(&block.statements)
}

fn note_tagged_array_usage(current_max: &mut usize, len: usize) {
    *current_max = (*current_max).max(len.max(1));
}

fn max_tagged_array_len_in_block(block: &Block) -> usize {
    max_tagged_array_len_in_statements(&block.statements)
}

fn max_tagged_array_len_in_ir_block(block: &Block) -> usize {
    max_tagged_array_len_in_statements(&block.statements)
}

fn max_tagged_array_len_in_statements(statements: &[Statement]) -> usize {
    let mut max_len = 0;
    for stmt in statements {
        scan_stmt_tagged_array_usage(stmt, &mut max_len);
    }
    max_len
}

fn max_tagged_array_len_in_ir_items(items: &[IrTopLevelItem]) -> usize {
    let mut max_len = 0;
    for item in items {
        scan_ir_item_tagged_array_usage(item, &mut max_len);
    }
    max_len
}

fn scan_stmt_tagged_array_usage(stmt: &Statement, max_len: &mut usize) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. } => scan_expr_tagged_array_usage(value, max_len),
        Statement::LetDestructure { value, .. } => scan_expr_tagged_array_usage(value, max_len),
        Statement::Expression { expression, .. } => scan_expr_tagged_array_usage(expression, max_len),
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                scan_expr_tagged_array_usage(value, max_len);
            }
        }
        Statement::Function { .. }
        | Statement::Import { .. }
        | Statement::Module { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. } => {}
    }
}

fn scan_ir_item_tagged_array_usage(item: &IrTopLevelItem, max_len: &mut usize) {
    match item {
        IrTopLevelItem::Let { value, .. }
        | IrTopLevelItem::Assign { value, .. } => scan_ir_expr_tagged_array_usage(value, max_len),
        IrTopLevelItem::LetDestructure { value, .. } => {
            scan_ir_expr_tagged_array_usage(value, max_len)
        }
        IrTopLevelItem::Expression { expression, .. } => {
            scan_ir_expr_tagged_array_usage(expression, max_len)
        }
        IrTopLevelItem::Return { value, .. } => {
            if let Some(value) = value {
                scan_ir_expr_tagged_array_usage(value, max_len);
            }
        }
        IrTopLevelItem::Function { .. }
        | IrTopLevelItem::Import { .. }
        | IrTopLevelItem::Module { .. }
        | IrTopLevelItem::Data { .. }
        | IrTopLevelItem::EffectDecl { .. } => {}
    }
}

fn scan_expr_tagged_array_usage(expr: &Expression, max_len: &mut usize) {
    match expr {
        Expression::Function { .. }
        | Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
        Expression::Prefix { right, .. } => scan_expr_tagged_array_usage(right, max_len),
        Expression::Infix { left, right, .. }
        | Expression::Cons {
            head: left,
            tail: right,
            ..
        } => {
            scan_expr_tagged_array_usage(left, max_len);
            scan_expr_tagged_array_usage(right, max_len);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            scan_expr_tagged_array_usage(condition, max_len);
            for stmt in &consequence.statements {
                scan_stmt_tagged_array_usage(stmt, max_len);
            }
            if let Some(alt) = alternative {
                for stmt in &alt.statements {
                    scan_stmt_tagged_array_usage(stmt, max_len);
                }
            }
        }
        Expression::DoBlock { block, .. } => {
            for stmt in &block.statements {
                scan_stmt_tagged_array_usage(stmt, max_len);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            note_tagged_array_usage(max_len, arguments.len());
            scan_expr_tagged_array_usage(function, max_len);
            for arg in arguments {
                scan_expr_tagged_array_usage(arg, max_len);
            }
        }
        Expression::TupleLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. } => {
            note_tagged_array_usage(max_len, elements.len());
            for element in elements {
                scan_expr_tagged_array_usage(element, max_len);
            }
        }
        Expression::ListLiteral { elements, .. } => {
            for element in elements {
                scan_expr_tagged_array_usage(element, max_len);
            }
        }
        Expression::Hash { pairs, .. } => {
            note_tagged_array_usage(max_len, pairs.len() * 2);
            for (key, value) in pairs {
                scan_expr_tagged_array_usage(key, max_len);
                scan_expr_tagged_array_usage(value, max_len);
            }
        }
        Expression::Index { left, index, .. } => {
            scan_expr_tagged_array_usage(left, max_len);
            scan_expr_tagged_array_usage(index, max_len);
        }
        Expression::MemberAccess { object, .. }
        | Expression::TupleFieldAccess { object, .. } => {
            scan_expr_tagged_array_usage(object, max_len);
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            scan_expr_tagged_array_usage(scrutinee, max_len);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    scan_expr_tagged_array_usage(guard, max_len);
                }
                scan_expr_tagged_array_usage(&arm.body, max_len);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => scan_expr_tagged_array_usage(value, max_len),
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let crate::syntax::expression::StringPart::Interpolation(expr) = part {
                    scan_expr_tagged_array_usage(expr, max_len);
                }
            }
        }
        Expression::Perform { args, .. } => {
            note_tagged_array_usage(max_len, args.len());
            for arg in args {
                scan_expr_tagged_array_usage(arg, max_len);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            scan_expr_tagged_array_usage(expr, max_len);
            for arm in arms {
                scan_expr_tagged_array_usage(&arm.body, max_len);
            }
        }
    }
}

fn scan_ir_expr_tagged_array_usage(expr: &Expression, max_len: &mut usize) {
    match expr {
        Expression::Function { .. }
        | Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
        Expression::Prefix { right, .. }
        | Expression::Some { value: right, .. }
        | Expression::Left { value: right, .. }
        | Expression::Right { value: right, .. } => {
            scan_ir_expr_tagged_array_usage(right, max_len)
        }
        Expression::Infix { left, right, .. }
        | Expression::Cons {
            head: left,
            tail: right,
            ..
        } => {
            scan_ir_expr_tagged_array_usage(left, max_len);
            scan_ir_expr_tagged_array_usage(right, max_len);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            scan_ir_expr_tagged_array_usage(condition, max_len);
            for stmt in &consequence.statements {
                scan_stmt_tagged_array_usage(stmt, max_len);
            }
            if let Some(alternative) = alternative {
                for stmt in &alternative.statements {
                    scan_stmt_tagged_array_usage(stmt, max_len);
                }
            }
        }
        Expression::DoBlock { block, .. } => {
            for stmt in &block.statements {
                scan_stmt_tagged_array_usage(stmt, max_len);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            note_tagged_array_usage(max_len, arguments.len());
            scan_ir_expr_tagged_array_usage(function, max_len);
            for arg in arguments {
                scan_ir_expr_tagged_array_usage(arg, max_len);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            note_tagged_array_usage(max_len, elements.len());
            for element in elements {
                scan_ir_expr_tagged_array_usage(element, max_len);
            }
        }
        Expression::Index { left, index, .. } => {
            scan_ir_expr_tagged_array_usage(left, max_len);
            scan_ir_expr_tagged_array_usage(index, max_len);
        }
        Expression::Hash { pairs, .. } => {
            note_tagged_array_usage(max_len, pairs.len() * 2);
            for (key, value) in pairs {
                scan_ir_expr_tagged_array_usage(key, max_len);
                scan_ir_expr_tagged_array_usage(value, max_len);
            }
        }
        Expression::MemberAccess { object, .. }
        | Expression::TupleFieldAccess { object, .. } => {
            scan_ir_expr_tagged_array_usage(object, max_len);
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            scan_ir_expr_tagged_array_usage(scrutinee, max_len);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    scan_ir_expr_tagged_array_usage(guard, max_len);
                }
                scan_ir_expr_tagged_array_usage(&arm.body, max_len);
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    scan_ir_expr_tagged_array_usage(expr, max_len);
                }
            }
        }
        Expression::Perform { args, .. } => {
            note_tagged_array_usage(max_len, args.len());
            for arg in args {
                scan_ir_expr_tagged_array_usage(arg, max_len);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            scan_ir_expr_tagged_array_usage(expr, max_len);
            for arm in arms {
                scan_ir_expr_tagged_array_usage(&arm.body, max_len);
            }
        }
    }
}

fn can_compile_jit_ir_cfg_function(function: &IrFunction) -> bool {
    fn supported_expr(
        expr: &IrExpr,
        kinds: &HashMap<IrVar, JitValueKind>,
        dest: Option<IrVar>,
    ) -> Option<JitValueKind> {
        let kind = match expr {
            IrExpr::Const(IrConst::Int(_)) => JitValueKind::Int,
            IrExpr::Const(IrConst::Float(_)) => JitValueKind::Float,
            IrExpr::Const(IrConst::Bool(_)) => JitValueKind::Bool,
            IrExpr::Const(IrConst::String(_)) => JitValueKind::Boxed,
            IrExpr::Const(IrConst::Unit) | IrExpr::None => JitValueKind::Boxed,
            IrExpr::Var(var) => *kinds.get(var)?,
            IrExpr::TagTest { .. } | IrExpr::ListTest { .. } | IrExpr::TupleArityTest { .. } => {
                JitValueKind::Bool
            }
            IrExpr::TagPayload { .. }
            | IrExpr::ListHead { .. }
            | IrExpr::ListTail { .. }
            | IrExpr::TupleFieldAccess { .. }
            | IrExpr::AdtField { .. } => JitValueKind::Boxed,
            IrExpr::AdtTagTest { .. } => JitValueKind::Bool,
            IrExpr::Binary(op, lhs, rhs) => {
                let lhs_kind = *kinds.get(lhs)?;
                let rhs_kind = *kinds.get(rhs)?;
                match op {
                    crate::cfg::IrBinaryOp::Add
                    | crate::cfg::IrBinaryOp::Sub
                    | crate::cfg::IrBinaryOp::Mul
                    | crate::cfg::IrBinaryOp::Div
                    | crate::cfg::IrBinaryOp::Mod
                        if lhs_kind == JitValueKind::Int && rhs_kind == JitValueKind::Int =>
                    {
                        JitValueKind::Int
                    }
                    crate::cfg::IrBinaryOp::Add
                    | crate::cfg::IrBinaryOp::Sub
                    | crate::cfg::IrBinaryOp::Mul
                    | crate::cfg::IrBinaryOp::Div
                        if lhs_kind == JitValueKind::Float && rhs_kind == JitValueKind::Float =>
                    {
                        JitValueKind::Float
                    }
                    crate::cfg::IrBinaryOp::IAdd
                    | crate::cfg::IrBinaryOp::ISub
                    | crate::cfg::IrBinaryOp::IMul
                    | crate::cfg::IrBinaryOp::IDiv
                    | crate::cfg::IrBinaryOp::IMod
                        if lhs_kind == JitValueKind::Int && rhs_kind == JitValueKind::Int =>
                    {
                        JitValueKind::Int
                    }
                    crate::cfg::IrBinaryOp::FAdd
                    | crate::cfg::IrBinaryOp::FSub
                    | crate::cfg::IrBinaryOp::FMul
                    | crate::cfg::IrBinaryOp::FDiv
                        if lhs_kind == JitValueKind::Float && rhs_kind == JitValueKind::Float =>
                    {
                        JitValueKind::Float
                    }
                    crate::cfg::IrBinaryOp::Eq
                    | crate::cfg::IrBinaryOp::NotEq
                    | crate::cfg::IrBinaryOp::Lt
                    | crate::cfg::IrBinaryOp::Gt
                    | crate::cfg::IrBinaryOp::Ge
                    | crate::cfg::IrBinaryOp::Le
                        if lhs_kind == rhs_kind
                            && matches!(
                                lhs_kind,
                                JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool
                            ) =>
                    {
                        JitValueKind::Bool
                    }
                    _ => return None,
                }
            }
            // Wrapper constructors
            IrExpr::EmptyList => JitValueKind::Boxed,
            IrExpr::Some(var) | IrExpr::Left(var) | IrExpr::Right(var) => {
                if !kinds.contains_key(var) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::Cons { head, tail } => {
                if !kinds.contains_key(head) || !kinds.contains_key(tail) {
                    return None;
                }
                JitValueKind::Boxed
            }
            // Data construction
            IrExpr::MakeTuple(vars) | IrExpr::MakeArray(vars) | IrExpr::MakeList(vars) => {
                if vars.iter().any(|v| !kinds.contains_key(v)) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::MakeHash(pairs) => {
                if pairs
                    .iter()
                    .any(|(k, v)| !kinds.contains_key(k) || !kinds.contains_key(v))
                {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::MakeAdt(_, vars) => {
                if vars.iter().any(|v| !kinds.contains_key(v)) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::MakeClosure(_, captures) => {
                if captures.iter().any(|v| !kinds.contains_key(v)) {
                    return None;
                }
                JitValueKind::Boxed
            }
            // Operations
            IrExpr::LoadName(_) => JitValueKind::Boxed,
            IrExpr::Prefix { right, .. } => {
                if !kinds.contains_key(right) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::Index { left, index } => {
                if !kinds.contains_key(left) || !kinds.contains_key(index) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::InterpolatedString(parts) => {
                for part in parts {
                    if let crate::cfg::IrStringPart::Interpolation(var) = part {
                        if !kinds.contains_key(var) {
                            return None;
                        }
                    }
                }
                JitValueKind::Boxed
            }
            IrExpr::Perform { args, .. } => {
                if args.iter().any(|v| !kinds.contains_key(v)) {
                    return None;
                }
                JitValueKind::Boxed
            }
            IrExpr::Handle { .. } => {
                // Handle as expression should not appear in CFG — it is
                // compiled as IrInstr::HandleScope at the instruction level.
                return None;
            }
            _ => return None,
        };
        if let Some(dest) = dest {
            let _ = dest;
        }
        Some(kind)
    }

    let Some(entry_index) = function
        .blocks
        .iter()
        .position(|block| block.id == function.entry)
    else {
        return false;
    };
    if entry_index != 0 {
        return false;
    }

    let block_indices: HashMap<_, _> = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id, index))
        .collect();

    let mut kinds = HashMap::new();
    for param in &function.params {
        // Use the IR-level type to give typed params an unboxed kind.
        // This enables typed arithmetic (IAdd/ISub etc.) on function parameters
        // without an explicit unboxing step in the eligibility analysis.
        let kind = match param.ty {
            crate::cfg::IrType::Int => JitValueKind::Int,
            crate::cfg::IrType::Bool => JitValueKind::Bool,
            _ => JitValueKind::Boxed,
        };
        kinds.insert(param.var, kind);
    }

    for block in &function.blocks {
        for param in &block.params {
            kinds.insert(param.var, JitValueKind::Boxed);
        }
        for instr in &block.instrs {
            match instr {
                IrInstr::Assign { dest, expr, .. } => {
                    let Some(kind) = supported_expr(expr, &kinds, Some(*dest)) else {
                        return false;
                    };
                    kinds.insert(*dest, kind);
                }
                IrInstr::HandleScope { dest, .. } => {
                    // HandleScope's dest receives the body result — always Boxed.
                    kinds.insert(*dest, JitValueKind::Boxed);
                }
                IrInstr::Call {
                    dest,
                    target,
                    args,
                    ..
                } => {
                    if let IrCallTarget::Var(var) = target
                        && !kinds.contains_key(var)
                    {
                        return false;
                    }
                    if args.iter().any(|var| !kinds.contains_key(var)) {
                        return false;
                    }
                    kinds.insert(*dest, JitValueKind::Boxed);
                }
            }
        }

        match &block.terminator {
            IrTerminator::Return(var, _) => {
                if !kinds.contains_key(var) {
                    return false;
                }
            }
                IrTerminator::Jump(target, args, _) => {
                    let Some(target_index) = block_indices.get(target).copied() else {
                        return false;
                    };
                    if function.blocks[target_index].params.len() != args.len() {
                        return false;
                    }
                    if args.iter().any(|var| !kinds.contains_key(var)) {
                        return false;
                    }
                    // For backward jumps, block params were already registered.
                    let _ = target_index;
            }
            IrTerminator::Branch {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if !kinds.contains_key(cond) {
                    return false;
                }
                if !block_indices.contains_key(then_block)
                    || !block_indices.contains_key(else_block)
                {
                    return false;
                }
            }
            IrTerminator::TailCall { callee, args, .. } => {
                if args.iter().any(|var| !kinds.contains_key(var)) {
                    return false;
                }
                match callee {
                    IrCallTarget::Named(name) => {
                        if function.name == Some(*name) && args.len() != function.params.len() {
                            return false;
                        }
                    }
                    IrCallTarget::Direct(function_id) => {
                        if *function_id == function.id && args.len() != function.params.len() {
                            return false;
                        }
                    }
                    IrCallTarget::Var(var) => {
                        if !kinds.contains_key(var) {
                            return false;
                        }
                    }
                }
            }
            IrTerminator::Unreachable(_) => {}
        }
    }

    // Last block must end with Return, TailCall, or Unreachable (dead arm).
    matches!(
        function.blocks.last().map(|block| &block.terminator),
        Some(IrTerminator::Return(..) | IrTerminator::TailCall { .. } | IrTerminator::Unreachable(_))
    )
}

fn max_boxed_array_len_in_statements(statements: &[Statement]) -> usize {
    let mut max_len = 0;
    for stmt in statements {
        scan_stmt_boxed_array_usage(stmt, &mut max_len);
    }
    max_len
}

fn max_boxed_array_len_in_ir_items(items: &[IrTopLevelItem]) -> usize {
    let mut max_len = 0;
    for item in items {
        scan_ir_item_boxed_array_usage(item, &mut max_len);
    }
    max_len
}

fn scan_stmt_boxed_array_usage(stmt: &Statement, max_len: &mut usize) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. } => scan_expr_boxed_array_usage(value, max_len),
        Statement::LetDestructure { value, .. } => scan_expr_boxed_array_usage(value, max_len),
        Statement::Expression { expression, .. } => scan_expr_boxed_array_usage(expression, max_len),
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                scan_expr_boxed_array_usage(value, max_len);
            }
        }
        Statement::Function { .. }
        | Statement::Import { .. }
        | Statement::Module { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. } => {}
    }
}

fn scan_ir_item_boxed_array_usage(item: &IrTopLevelItem, max_len: &mut usize) {
    match item {
        IrTopLevelItem::Let { value, .. }
        | IrTopLevelItem::Assign { value, .. } => scan_ir_expr_boxed_array_usage(value, max_len),
        IrTopLevelItem::LetDestructure { value, .. } => {
            scan_ir_expr_boxed_array_usage(value, max_len)
        }
        IrTopLevelItem::Expression { expression, .. } => {
            scan_ir_expr_boxed_array_usage(expression, max_len)
        }
        IrTopLevelItem::Return { value, .. } => {
            if let Some(value) = value {
                scan_ir_expr_boxed_array_usage(value, max_len);
            }
        }
        IrTopLevelItem::Function { .. }
        | IrTopLevelItem::Import { .. }
        | IrTopLevelItem::Module { .. }
        | IrTopLevelItem::Data { .. }
        | IrTopLevelItem::EffectDecl { .. } => {}
    }
}

fn scan_expr_boxed_array_usage(expr: &Expression, max_len: &mut usize) {
    match expr {
        Expression::Function { .. }
        | Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
        Expression::Prefix { right, .. } => scan_expr_boxed_array_usage(right, max_len),
        Expression::Infix { left, right, .. }
        | Expression::Cons {
            head: left,
            tail: right,
            ..
        } => {
            scan_expr_boxed_array_usage(left, max_len);
            scan_expr_boxed_array_usage(right, max_len);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            scan_expr_boxed_array_usage(condition, max_len);
            for stmt in &consequence.statements {
                scan_stmt_boxed_array_usage(stmt, max_len);
            }
            if let Some(alternative) = alternative {
                for stmt in &alternative.statements {
                    scan_stmt_boxed_array_usage(stmt, max_len);
                }
            }
        }
        Expression::DoBlock { block, .. } => {
            for stmt in &block.statements {
                scan_stmt_boxed_array_usage(stmt, max_len);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            note_boxed_array_usage(max_len, arguments.len());
            scan_expr_boxed_array_usage(function, max_len);
            for arg in arguments {
                scan_expr_boxed_array_usage(arg, max_len);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for element in elements {
                scan_expr_boxed_array_usage(element, max_len);
            }
        }
        Expression::Index { left, index, .. } => {
            scan_expr_boxed_array_usage(left, max_len);
            scan_expr_boxed_array_usage(index, max_len);
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                scan_expr_boxed_array_usage(key, max_len);
                scan_expr_boxed_array_usage(value, max_len);
            }
        }
        Expression::MemberAccess { object, .. }
        | Expression::TupleFieldAccess { object, .. } => {
            scan_expr_boxed_array_usage(object, max_len);
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            scan_expr_boxed_array_usage(scrutinee, max_len);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    scan_expr_boxed_array_usage(guard, max_len);
                }
                scan_expr_boxed_array_usage(&arm.body, max_len);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => scan_expr_boxed_array_usage(value, max_len),
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let crate::syntax::expression::StringPart::Interpolation(expr) = part {
                    scan_expr_boxed_array_usage(expr, max_len);
                }
            }
        }
        Expression::Perform { args, .. } => {
            note_boxed_array_usage(max_len, args.len());
            for arg in args {
                scan_expr_boxed_array_usage(arg, max_len);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            note_boxed_array_usage(max_len, arms.len());
            scan_expr_boxed_array_usage(expr, max_len);
        }
    }
}

fn scan_ir_expr_boxed_array_usage(expr: &Expression, max_len: &mut usize) {
    match expr {
        Expression::Function { .. }
        | Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
        Expression::Prefix { right, .. }
        | Expression::Some { value: right, .. }
        | Expression::Left { value: right, .. }
        | Expression::Right { value: right, .. } => {
            scan_ir_expr_boxed_array_usage(right, max_len)
        }
        Expression::Infix { left, right, .. }
        | Expression::Cons {
            head: left,
            tail: right,
            ..
        } => {
            scan_ir_expr_boxed_array_usage(left, max_len);
            scan_ir_expr_boxed_array_usage(right, max_len);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            scan_ir_expr_boxed_array_usage(condition, max_len);
            for stmt in &consequence.statements {
                scan_stmt_boxed_array_usage(stmt, max_len);
            }
            if let Some(alternative) = alternative {
                for stmt in &alternative.statements {
                    scan_stmt_boxed_array_usage(stmt, max_len);
                }
            }
        }
        Expression::DoBlock { block, .. } => {
            for stmt in &block.statements {
                scan_stmt_boxed_array_usage(stmt, max_len);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            note_boxed_array_usage(max_len, arguments.len());
            scan_ir_expr_boxed_array_usage(function, max_len);
            for arg in arguments {
                scan_ir_expr_boxed_array_usage(arg, max_len);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for element in elements {
                scan_ir_expr_boxed_array_usage(element, max_len);
            }
        }
        Expression::Index { left, index, .. } => {
            scan_ir_expr_boxed_array_usage(left, max_len);
            scan_ir_expr_boxed_array_usage(index, max_len);
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                scan_ir_expr_boxed_array_usage(key, max_len);
                scan_ir_expr_boxed_array_usage(value, max_len);
            }
        }
        Expression::MemberAccess { object, .. }
        | Expression::TupleFieldAccess { object, .. } => {
            scan_ir_expr_boxed_array_usage(object, max_len);
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            scan_ir_expr_boxed_array_usage(scrutinee, max_len);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    scan_ir_expr_boxed_array_usage(guard, max_len);
                }
                scan_ir_expr_boxed_array_usage(&arm.body, max_len);
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    scan_ir_expr_boxed_array_usage(expr, max_len);
                }
            }
        }
        Expression::Perform { args, .. } => {
            note_boxed_array_usage(max_len, args.len());
            for arg in args {
                scan_ir_expr_boxed_array_usage(arg, max_len);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            note_boxed_array_usage(max_len, arms.len());
            scan_ir_expr_boxed_array_usage(expr, max_len);
        }
    }
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
