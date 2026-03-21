use super::*;

pub(super) fn compile_jit_cfg_user_function_call(
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

pub(super) fn emit_jit_cfg_user_function_tailcall(
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
pub(super) fn compile_jit_cfg_base_function_call(
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
pub(super) fn compile_jit_cfg_primop_call(
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
pub(super) fn compile_jit_cfg_generic_call(
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
