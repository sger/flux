use super::*;

#[derive(Clone)]
pub(super) struct LocalBinding {
    pub(super) var: Variable,
    pub(super) kind: JitValueKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JitValueKind {
    Boxed,
    Int,
    Float,
    Bool,
}

#[derive(Clone, Copy)]
pub(super) struct JitValue {
    pub(super) value: CraneliftValue,
    pub(super) kind: JitValueKind,
}

impl JitValue {
    pub(super) fn boxed(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Boxed,
        }
    }

    pub(super) fn int(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Int,
        }
    }

    pub(super) fn float(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Float,
        }
    }

    pub(super) fn bool(value: CraneliftValue) -> Self {
        Self {
            value,
            kind: JitValueKind::Bool,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub(super) struct Scope {
    /// Maps interned identifier → Cranelift Variable
    pub(super) locals: HashMap<Identifier, LocalBinding>,
    /// Maps interned identifier → global slot index
    pub(super) globals: HashMap<Identifier, usize>,
    /// Maps interned identifier → base index
    pub(super) base_functions: HashMap<Identifier, usize>,
    /// Base names excluded from unqualified lookup via `import Base except [...]`.
    pub(super) excluded_base_symbols: HashSet<Identifier>,
    /// Maps interned identifier → JIT function metadata.
    pub(super) functions: HashMap<Identifier, JitFunctionMeta>,
    /// Maps (module name, member name) -> JIT function metadata.
    pub(super) module_functions: HashMap<(Identifier, Identifier), JitFunctionMeta>,
    /// Imported module names visible in current scope.
    pub(super) imported_modules: HashSet<Identifier>,
    /// Import aliases: alias -> module name.
    pub(super) import_aliases: HashMap<Identifier, Identifier>,
    /// Maps ADT constructor name (interned) → arity. Used to route constructor calls.
    pub(super) adt_constructors: HashMap<Identifier, usize>,
    /// Maps ADT constructor name (interned) → owning ADT name.
    pub(super) adt_constructor_owner: HashMap<Identifier, Identifier>,
    /// Maps ADT name → constructor names.
    pub(super) adt_variants: HashMap<Identifier, Vec<Identifier>>,
    /// HM expression types used by the retired AST JIT path.
    pub(super) hm_expr_types: Rc<HashMap<ExprId, InferType>>,
}

impl Scope {
    pub(super) fn new(hm_expr_types: Rc<HashMap<ExprId, InferType>>) -> Self {
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

pub(super) fn resolve_module_name(
    scope: &Scope,
    interner: &Interner,
    name: Identifier,
) -> Option<Identifier> {
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
pub(super) fn declare_local(builder: &mut FunctionBuilder, kind: JitValueKind) -> Variable {
    let ty = match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    };
    builder.declare_var(ty)
}

#[allow(dead_code)]
pub(super) fn bind_local(
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
pub(super) fn use_local(builder: &mut FunctionBuilder, binding: LocalBinding) -> JitValue {
    let value = builder.use_var(binding.var);
    match binding.kind {
        JitValueKind::Boxed => JitValue::boxed(value),
        JitValueKind::Int => JitValue::int(value),
        JitValueKind::Float => JitValue::float(value),
        JitValueKind::Bool => JitValue::bool(value),
    }
}

pub(super) fn jit_value_tag(builder: &mut FunctionBuilder, kind: JitValueKind) -> CraneliftValue {
    let tag = match kind {
        JitValueKind::Boxed => JIT_TAG_PTR,
        JitValueKind::Int => JIT_TAG_INT,
        JitValueKind::Float => JIT_TAG_FLOAT,
        JitValueKind::Bool => JIT_TAG_BOOL,
    };
    builder.ins().iconst(types::I64, tag)
}

pub(super) fn box_jit_value(
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

pub(super) fn box_and_guard_jit_value(
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

pub(super) fn boxed_value_from_tagged_parts(
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

pub(super) fn emit_return_null_tagged(builder: &mut FunctionBuilder) {
    let null_tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    builder.ins().return_(&[null_tag, null_ptr]);
}

pub(super) fn jit_value_to_tag_payload(
    builder: &mut FunctionBuilder,
    value: JitValue,
) -> (CraneliftValue, CraneliftValue) {
    (jit_value_tag(builder, value.kind), value.value)
}

#[allow(dead_code)]
pub(super) fn append_return_block_params(
    builder: &mut FunctionBuilder,
    block: cranelift_codegen::ir::Block,
) {
    builder.append_block_param(block, types::I64);
    builder.append_block_param(block, PTR_TYPE);
}

#[allow(dead_code)]
pub(super) fn jump_with_jit_value(
    builder: &mut FunctionBuilder,
    block: cranelift_codegen::ir::Block,
    value: JitValue,
) {
    let (tag, payload) = jit_value_to_tag_payload(builder, value);
    let args = [BlockArg::Value(tag), BlockArg::Value(payload)];
    builder.ins().jump(block, &args);
}

pub(super) fn emit_tagged_stack_array(
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

pub(super) fn emit_boxed_stack_array(
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
pub(super) fn emit_return_on_null_jit_value(
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
pub(super) fn compile_truthiness_condition(
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
pub(super) fn jit_value_type(kind: JitValueKind) -> types::Type {
    match kind {
        JitValueKind::Boxed => PTR_TYPE,
        JitValueKind::Int | JitValueKind::Float | JitValueKind::Bool => types::I64,
    }
}

#[allow(dead_code)]
pub(super) fn merged_jit_value_kind(left: JitValue, right: JitValue) -> JitValueKind {
    if left.kind == right.kind {
        left.kind
    } else {
        JitValueKind::Boxed
    }
}

#[allow(dead_code)]
pub(super) fn live_branch_locals(
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
pub(super) fn bind_branch_block_params(
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
pub(super) fn expr_has_known_bool_type(scope: &Scope, expr: &Expression) -> bool {
    matches!(
        scope.hm_expr_types.get(&expr.expr_id()),
        Some(InferType::Con(TypeConstructor::Bool))
    )
}

pub(super) fn emit_error_check_and_return(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    span: Span,
) {
    let has_error = get_helper_func_ref(module, helpers, builder, "rt_has_error");
    let call = builder.ins().call(has_error, &[ctx_val]);
    let err_flag = builder.inst_results(call)[0];
    let is_err = builder.ins().icmp_imm(IntCC::NotEqual, err_flag, 0);

    let err_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_err, err_block, &[], continue_block, &[]);

    builder.switch_to_block(err_block);
    emit_render_error_with_span(module, helpers, builder, ctx_val, span);
    emit_return_null_tagged(builder);
    builder.seal_block(err_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

/// After a runtime helper that may set `ctx.error`, emit a call to
/// `rt_render_error_with_span` so the raw error is rendered as a structured
/// diagnostic with source location.  This produces VM-parity error output.
#[allow(dead_code)]
pub(super) fn emit_render_error_with_span(
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
pub(super) fn emit_return_on_null_value(builder: &mut FunctionBuilder, value_ptr: CraneliftValue) {
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
pub(super) fn emit_return_on_null_with_render(
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
pub(super) fn emit_push_gc_roots(
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

pub(super) fn emit_pop_gc_roots(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
) {
    let pop = get_helper_func_ref(module, helpers, builder, "rt_pop_gc_roots");
    builder.ins().call(pop, &[ctx_val]);
}
