//! Utility functions for the LLVM compiler: tagged value construction, variable
//! access, null checks, error checks, and argument array building.

use std::collections::HashMap;

use llvm_sys::LLVMIntPredicate;
use llvm_sys::prelude::*;

use crate::cfg::IrVar;
use crate::runtime::native_context::JIT_TAG_PTR;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper::{self};

/// Load a `{i64, i64}` tagged value from a pointer to an array of i64 pairs.
/// Index `i` means the pair at `ptr[i*2]` (tag) and `ptr[i*2+1]` (payload).
pub(super) fn load_tagged_from_ptr(
    ctx: &LlvmCompilerContext,
    base_ptr: LLVMValueRef,
    index: usize,
    prefix: &str,
) -> LLVMValueRef {
    let tag_offset = wrapper::const_i64(ctx.i64_type, (index * 2) as i64);
    let tag_ptr = unsafe {
        llvm_sys::core::LLVMBuildGEP2(
            ctx.builder.raw_ptr(),
            ctx.i64_type,
            base_ptr,
            [tag_offset].as_mut_ptr(),
            1,
            c"tag_ptr".as_ptr(),
        )
    };
    let tag = ctx
        .builder
        .build_load(ctx.i64_type, tag_ptr, &format!("{}_tag", prefix));
    wrapper::set_tbaa(tag, ctx.tbaa_args);

    let payload_offset = wrapper::const_i64(ctx.i64_type, (index * 2 + 1) as i64);
    let payload_ptr = unsafe {
        llvm_sys::core::LLVMBuildGEP2(
            ctx.builder.raw_ptr(),
            ctx.i64_type,
            base_ptr,
            [payload_offset].as_mut_ptr(),
            1,
            c"payload_ptr".as_ptr(),
        )
    };
    let payload = ctx
        .builder
        .build_load(ctx.i64_type, payload_ptr, &format!("{}_payload", prefix));
    wrapper::set_tbaa(payload, ctx.tbaa_args);

    build_tagged_value(ctx, tag, payload)
}

/// If the destination variable is a global binding, emit `rt_set_global` to persist
/// the value in the JitContext globals array.
pub(super) fn emit_set_global_if_bound(
    ctx: &LlvmCompilerContext,
    dest: IrVar,
    value: LLVMValueRef,
    ctx_val: LLVMValueRef,
    global_binding_indices: &HashMap<IrVar, usize>,
) -> Result<(), String> {
    if let Some(&global_idx) = global_binding_indices.get(&dest) {
        // Force-box the tagged value to get a *mut Value
        let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
        let tag = ctx.builder.build_extract_value(value, 0, "gb_tag");
        let payload = ctx.builder.build_extract_value(value, 1, "gb_payload");
        let boxed = ctx.builder.build_call(
            force_boxed_ty,
            force_boxed,
            &mut [ctx_val, tag, payload],
            "gb_boxed",
        );
        let ptr_int = ctx.builder.build_extract_value(boxed, 1, "gb_ptr_int");
        let ptr = ctx
            .builder
            .build_int_to_ptr(ptr_int, ctx.ptr_type, "gb_ptr");

        let (set_global, set_global_ty) = get_helper(ctx, "rt_set_global")?;
        let idx_val = wrapper::const_i64(ctx.i64_type, global_idx as i64);
        ctx.builder
            .build_call(set_global_ty, set_global, &mut [ctx_val, idx_val, ptr], "");
    }
    Ok(())
}

/// After a fallible operation (e.g. rt_add), check if ctx.error is set.
/// If so, render the error with span info and return a null-tagged value
/// to propagate the error upward. Builder is left at the continue block.
pub(super) fn emit_error_check_and_return(
    ctx: &LlvmCompilerContext,
    ctx_val: LLVMValueRef,
    func_ref: LLVMValueRef,
    span: &crate::diagnostics::position::Span,
) -> Result<(), String> {
    let (has_error, has_error_ty) = get_helper(ctx, "rt_has_error")?;
    let err_flag = ctx
        .builder
        .build_call(has_error_ty, has_error, &mut [ctx_val], "has_err");
    let is_err = ctx.builder.build_icmp(
        LLVMIntPredicate::LLVMIntNE,
        err_flag,
        wrapper::const_i64(ctx.i64_type, 0),
        "is_err",
    );
    let err_block = ctx.llvm_ctx.append_basic_block(func_ref, "rt_err");
    let continue_block = ctx.llvm_ctx.append_basic_block(func_ref, "no_err");
    ctx.builder.build_cond_br(is_err, err_block, continue_block);

    ctx.builder.position_at_end(err_block);
    let (render, render_ty) = get_helper(ctx, "rt_render_error_with_span")?;
    let sl = wrapper::const_i64(ctx.i64_type, span.start.line as i64);
    let sc = wrapper::const_i64(ctx.i64_type, (span.start.column + 1) as i64);
    let el = wrapper::const_i64(ctx.i64_type, span.end.line as i64);
    let ec = wrapper::const_i64(ctx.i64_type, (span.end.column + 1) as i64);
    ctx.builder
        .build_call(render_ty, render, &mut [ctx_val, sl, sc, el, ec], "");
    let null_tagged = build_tagged_value(
        ctx,
        wrapper::const_i64(ctx.i64_type, JIT_TAG_PTR),
        wrapper::const_i64(ctx.i64_type, 0),
    );
    ctx.builder.build_ret(null_tagged);

    ctx.builder.position_at_end(continue_block);
    Ok(())
}

/// Check if a `*mut Value` pointer is null (runtime error). If null, return
/// `{JIT_TAG_PTR, 0}` from the current function. If non-null, continue in a
/// new block. The builder is left positioned at the continue block.
pub(super) fn emit_null_check(
    ctx: &LlvmCompilerContext,
    func_ref: LLVMValueRef,
    ptr_result: LLVMValueRef,
) -> LLVMValueRef {
    let is_null = ctx.builder.build_icmp(
        LLVMIntPredicate::LLVMIntEQ,
        ptr_result,
        wrapper::const_null(ctx.ptr_type),
        "is_null",
    );
    let null_block = ctx.llvm_ctx.append_basic_block(func_ref, "null_err");
    let continue_block = ctx.llvm_ctx.append_basic_block(func_ref, "continue");
    ctx.builder
        .build_cond_br(is_null, null_block, continue_block);

    ctx.builder.position_at_end(null_block);
    let null_tagged = build_tagged_value(
        ctx,
        wrapper::const_i64(ctx.i64_type, JIT_TAG_PTR),
        wrapper::const_i64(ctx.i64_type, 0),
    );
    ctx.builder.build_ret(null_tagged);

    // Builder now at continue block — caller resumes here
    ctx.builder.position_at_end(continue_block);
    ptr_result
}

/// Convert a `*mut Value` pointer back to a properly tagged `{i64, i64}` value.
/// Calls `rt_unbox_to_tagged` which reads the actual Value discriminant and
/// produces the correct tag (INT for integers, BOOL for booleans, PTR for heap types).
pub(super) fn unbox_ptr_result(
    ctx: &LlvmCompilerContext,
    ptr_result: LLVMValueRef,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    let (unbox, unbox_ty) = get_helper(ctx, "rt_unbox_to_tagged")?;
    Ok(ctx
        .builder
        .build_call(unbox_ty, unbox, &mut [ctx_val, ptr_result], "unboxed"))
}

/// Force-box a tagged value to get a `*mut Value` pointer.
/// Calls `rt_force_boxed` and extracts the pointer from the result.
pub(super) fn force_box_to_ptr(
    ctx: &LlvmCompilerContext,
    env: &HashMap<IrVar, LLVMValueRef>,
    var: IrVar,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    let val = get_var(env, var)?;
    let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
    let tag = ctx.builder.build_extract_value(val, 0, "fb_tag");
    let payload = ctx.builder.build_extract_value(val, 1, "fb_payload");
    let boxed = ctx.builder.build_call(
        force_boxed_ty,
        force_boxed,
        &mut [ctx_val, tag, payload],
        "fb_boxed",
    );
    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "fb_ptr_int");
    Ok(ctx
        .builder
        .build_int_to_ptr(ptr_int, ctx.ptr_type, "fb_ptr"))
}

pub(super) fn get_var(
    env: &HashMap<IrVar, LLVMValueRef>,
    var: IrVar,
) -> Result<LLVMValueRef, String> {
    env.get(&var)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undefined variable {:?}", var))
}

pub(super) fn get_helper(
    ctx: &LlvmCompilerContext,
    name: &str,
) -> Result<(LLVMValueRef, LLVMTypeRef), String> {
    ctx.helpers
        .get(name)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undeclared helper '{}'", name))
}

/// Build a `{i64, i64}` tagged value from tag and payload.
pub(super) fn build_tagged_value(
    ctx: &LlvmCompilerContext,
    tag: LLVMValueRef,
    payload: LLVMValueRef,
) -> LLVMValueRef {
    let undef = wrapper::get_undef(ctx.tagged_value_type);
    let v1 = ctx.builder.build_insert_value(undef, tag, 0, "tv_tag");
    ctx.builder.build_insert_value(v1, payload, 1, "tv")
}

/// Build a tagged integer constant.
pub(super) fn build_int_tagged(
    ctx: &LlvmCompilerContext,
    value: impl IntoI64OrValue,
) -> LLVMValueRef {
    use crate::runtime::native_context::JIT_TAG_INT;
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_INT);
    let payload = value.to_llvm_value(ctx.i64_type);
    build_tagged_value(ctx, tag, payload)
}

/// Build a tagged boolean constant.
pub(super) fn build_bool_tagged(
    ctx: &LlvmCompilerContext,
    value: impl IntoBoolOrValue,
) -> LLVMValueRef {
    use crate::runtime::native_context::JIT_TAG_BOOL;
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_BOOL);
    let payload = value.to_bool_payload(ctx.i64_type);
    build_tagged_value(ctx, tag, payload)
}

pub(super) trait IntoBoolOrValue {
    fn to_bool_payload(self, i64_type: LLVMTypeRef) -> LLVMValueRef;
}

impl IntoBoolOrValue for bool {
    fn to_bool_payload(self, i64_type: LLVMTypeRef) -> LLVMValueRef {
        wrapper::const_i64(i64_type, self as i64)
    }
}

impl IntoBoolOrValue for LLVMValueRef {
    fn to_bool_payload(self, _i64_type: LLVMTypeRef) -> LLVMValueRef {
        self // already an i64 value (0 or 1)
    }
}

/// Build a tagged PTR value from a pointer.
pub(super) fn build_ptr_tagged(ctx: &LlvmCompilerContext, ptr: LLVMValueRef) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_PTR);
    let payload = ctx
        .builder
        .build_ptr_to_int(ptr, ctx.i64_type, "ptr_as_int");
    build_tagged_value(ctx, tag, payload)
}

/// Build an array of tagged values on the stack for passing to function calls.
/// Returns a pointer to the first element (or null if empty).
pub(super) fn build_tagged_args_array(
    ctx: &LlvmCompilerContext,
    args: &[IrVar],
    env: &HashMap<IrVar, LLVMValueRef>,
) -> Result<LLVMValueRef, String> {
    if args.is_empty() {
        return Ok(wrapper::const_null(ctx.ptr_type));
    }

    // Allocate space for args.len() * 2 i64s on the stack as a flat i64 array.
    let total_slots = args.len() * 2;
    let array_ty = unsafe { llvm_sys::core::LLVMArrayType2(ctx.i64_type, total_slots as u64) };
    let alloca = ctx.builder.build_alloca(array_ty, "args_buf");

    for (i, arg) in args.iter().enumerate() {
        let val = get_var(env, *arg)?;
        let tag = ctx.builder.build_extract_value(val, 0, "tag");
        let payload = ctx.builder.build_extract_value(val, 1, "payload");

        // GEP with [array_ty, 0, offset] to index into [N x i64] array
        let tag_offset = wrapper::const_i64(ctx.i64_type, (i * 2) as i64);
        let tag_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                array_ty,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), tag_offset].as_mut_ptr(),
                2,
                c"arg_tag_slot".as_ptr(),
            )
        };
        let tag_store = ctx.builder.build_store(tag, tag_ptr);
        wrapper::set_tbaa(tag_store, ctx.tbaa_args);

        let payload_offset = wrapper::const_i64(ctx.i64_type, (i * 2 + 1) as i64);
        let payload_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                array_ty,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), payload_offset].as_mut_ptr(),
                2,
                c"arg_payload_slot".as_ptr(),
            )
        };
        let payload_store = ctx.builder.build_store(payload, payload_ptr);
        wrapper::set_tbaa(payload_store, ctx.tbaa_args);
    }

    Ok(alloca)
}

/// Trait to accept both i64 constants and LLVM values for `build_int_tagged`.
pub(super) trait IntoI64OrValue {
    fn to_llvm_value(self, i64_type: LLVMTypeRef) -> LLVMValueRef;
}

impl IntoI64OrValue for i64 {
    fn to_llvm_value(self, i64_type: LLVMTypeRef) -> LLVMValueRef {
        wrapper::const_i64(i64_type, self)
    }
}

impl IntoI64OrValue for LLVMValueRef {
    fn to_llvm_value(self, _i64_type: LLVMTypeRef) -> LLVMValueRef {
        self
    }
}
