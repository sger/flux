//! Call compilation: translates `IrCallTarget` into LLVM IR call instructions.

use std::collections::HashMap;

use llvm_sys::prelude::*;

use crate::cfg::{IrCallTarget, IrFunction, IrProgram, IrVar};
use crate::syntax::interner::Interner;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper;
use super::helpers::{
    build_ptr_tagged, build_tagged_args_array, emit_null_check, get_helper, get_var,
    unbox_ptr_result,
};

#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn compile_call(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    _function: &IrFunction,
    target: &IrCallTarget,
    args: &[IrVar],
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    func_ref: LLVMValueRef,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
    call_span: Option<crate::diagnostics::position::Span>,
) -> Result<LLVMValueRef, String> {
    // Extract span coordinates for error reporting
    let (sl, sc, el, ec) = match call_span {
        Some(span) => (
            wrapper::const_i64(ctx.i64_type, span.start.line as i64),
            wrapper::const_i64(ctx.i64_type, (span.start.column + 1) as i64),
            wrapper::const_i64(ctx.i64_type, span.end.line as i64),
            wrapper::const_i64(ctx.i64_type, (span.end.column + 1) as i64),
        ),
        None => {
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            (zero, zero, zero, zero)
        }
    };

    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm]     compile_call target={:?}", target);
    }
    match target {
        IrCallTarget::Direct(fn_id) => {
            let fn_index = program
                .functions
                .iter()
                .position(|f| f.id == *fn_id)
                .ok_or_else(|| format!("missing direct callee {:?}", fn_id))?;
            let (callee, callee_ty) = ctx.functions[&fn_index];

            // Build args array on the stack as consecutive (tag, payload) i64s
            let args_array = build_tagged_args_array(ctx, args, env)?;
            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
            let null_captures = wrapper::const_null(ctx.ptr_type);
            let zero_captures = wrapper::const_i64(ctx.i64_type, 0);

            let result = ctx.builder.build_call(
                callee_ty,
                callee,
                &mut [ctx_val, args_array, nargs, null_captures, zero_captures],
                "direct_call",
            );
            Ok(result)
        }
        IrCallTarget::Named(name) => {
            let name_str = interner.resolve(*name);

            // Check if it's a user function
            if let Some(fn_index) = program.functions.iter().position(|f| f.name == Some(*name)) {
                let ir_func = &program.functions[fn_index];
                let has_contract = ir_func.parameter_types.iter().any(|t| t.is_some())
                    || ir_func.return_type_annotation.is_some();

                if has_contract {
                    // Route through rt_call_jit_function for contract checking
                    let (func, fn_ty) = get_helper(ctx, "rt_call_jit_function")?;
                    let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
                    let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);

                    // Build boxed *mut Value args array
                    let args_ptr = if args.is_empty() {
                        wrapper::const_null(ctx.ptr_type)
                    } else {
                        let array_ty = unsafe {
                            llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64)
                        };
                        let alloca = ctx.builder.build_alloca(array_ty, "contract_args");
                        for (i, arg) in args.iter().enumerate() {
                            let val = get_var(env, *arg)?;
                            let tag = ctx.builder.build_extract_value(val, 0, "ca_tag");
                            let payload = ctx.builder.build_extract_value(val, 1, "ca_payload");
                            let boxed = ctx.builder.build_call(
                                force_boxed_ty,
                                force_boxed,
                                &mut [ctx_val, tag, payload],
                                "ca_boxed",
                            );
                            let ptr_int = ctx.builder.build_extract_value(boxed, 1, "ca_ptr_int");
                            let ptr = ctx
                                .builder
                                .build_int_to_ptr(ptr_int, ctx.ptr_type, "ca_ptr");
                            let slot = unsafe {
                                llvm_sys::core::LLVMBuildGEP2(
                                    ctx.builder.raw_ptr(),
                                    array_ty,
                                    alloca,
                                    [
                                        wrapper::const_i64(ctx.i64_type, 0),
                                        wrapper::const_i64(ctx.i64_type, i as i64),
                                    ]
                                    .as_mut_ptr(),
                                    2,
                                    c"ca_slot".as_ptr(),
                                )
                            };
                            let s = ctx.builder.build_store(ptr, slot);
                            wrapper::set_tbaa(s, ctx.tbaa_args);
                        }
                        alloca
                    };

                    let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                    let result = ctx.builder.build_call(
                        fn_ty,
                        func,
                        &mut [ctx_val, fn_idx_val, args_ptr, nargs, sl, sc, el, ec],
                        "contract_call",
                    );
                    let result = emit_null_check(ctx, func_ref, result);
                    return unbox_ptr_result(ctx, result, ctx_val);
                }

                // No contract — direct call (fast path)
                let (callee, callee_ty) = ctx.functions[&fn_index];
                let args_array = build_tagged_args_array(ctx, args, env)?;
                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let null_captures = wrapper::const_null(ctx.ptr_type);
                let zero_captures = wrapper::const_i64(ctx.i64_type, 0);

                let result = ctx.builder.build_call(
                    callee_ty,
                    callee,
                    &mut [ctx_val, args_array, nargs, null_captures, zero_captures],
                    "named_call",
                );
                return Ok(result);
            }

            // Check if it's a primop
            if let Some(primop) = crate::primop::resolve_primop_call(name_str, args.len()) {
                let (func, fn_ty) = get_helper(ctx, "rt_call_primop")?;
                let primop_id = wrapper::const_i64(ctx.i64_type, primop as i64);
                let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;

                // Build boxed *mut Value args array
                let args_ptr = if args.is_empty() {
                    wrapper::const_null(ctx.ptr_type)
                } else {
                    let array_ty =
                        unsafe { llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64) };
                    let alloca = ctx.builder.build_alloca(array_ty, "primop_args");
                    for (i, arg) in args.iter().enumerate() {
                        let val = get_var(env, *arg)?;
                        let tag = ctx.builder.build_extract_value(val, 0, "po_tag");
                        let payload = ctx.builder.build_extract_value(val, 1, "po_payload");
                        let boxed = ctx.builder.build_call(
                            force_boxed_ty,
                            force_boxed,
                            &mut [ctx_val, tag, payload],
                            "po_boxed",
                        );
                        let ptr_int = ctx.builder.build_extract_value(boxed, 1, "po_ptr_int");
                        let ptr = ctx
                            .builder
                            .build_int_to_ptr(ptr_int, ctx.ptr_type, "po_ptr");
                        let slot = unsafe {
                            llvm_sys::core::LLVMBuildGEP2(
                                ctx.builder.raw_ptr(),
                                array_ty,
                                alloca,
                                [
                                    wrapper::const_i64(ctx.i64_type, 0),
                                    wrapper::const_i64(ctx.i64_type, i as i64),
                                ]
                                .as_mut_ptr(),
                                2,
                                c"po_slot".as_ptr(),
                            )
                        };
                        let s = ctx.builder.build_store(ptr, slot);
                        wrapper::set_tbaa(s, ctx.tbaa_args);
                    }
                    alloca
                };

                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, primop_id, args_ptr, nargs, sl, sc, el, ec],
                    "primop_call",
                );
                let result = emit_null_check(ctx, func_ref, result);
                return unbox_ptr_result(ctx, result, ctx_val);
            }

            // Check if it's a base function
            if let Some(base_idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (func, fn_ty) = get_helper(ctx, "rt_call_base_function_tagged")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, base_idx as i64);
                let args_array = build_tagged_args_array(ctx, args, env)?;
                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);

                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, idx_val, args_array, nargs, sl, sc, el, ec],
                    "base_call",
                );
                let result = emit_null_check(ctx, func_ref, result);
                return unbox_ptr_result(ctx, result, ctx_val);
            }

            // Check if it's an ADT constructor
            if let Some(&arity) = adt_constructors.get(name)
                && arity == args.len()
            {
                // Build ADT via rt_make_adt
                let (func, fn_ty) = get_helper(ctx, "rt_make_adt")?;
                let name_bytes = name_str.as_bytes();
                let global = wrapper::create_global_string(
                    &ctx.module,
                    &ctx.llvm_ctx,
                    &format!(".adt.{}", name_str),
                    name_bytes,
                );
                let name_len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let fields_buf = build_tagged_args_array(ctx, args, env)?;
                let nfields = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, global, name_len, fields_buf, nfields],
                    "adt_ctor",
                );
                return Ok(build_ptr_tagged(ctx, result));
            }

            Err(format!(
                "LLVM backend: unresolved named callee '{}'",
                name_str
            ))
        }
        IrCallTarget::Var(var) => {
            // Indirect call: the callee is a value in the env (closure, base fn, etc.)
            // Force-box it and call via rt_call_value
            let callee_val = get_var(env, *var)?;
            let callee_tag = ctx.builder.build_extract_value(callee_val, 0, "callee_tag");
            let callee_payload = ctx
                .builder
                .build_extract_value(callee_val, 1, "callee_payload");

            // rt_force_boxed converts a tagged value to *mut Value
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
            let boxed_tv = ctx.builder.build_call(
                force_boxed_ty,
                force_boxed,
                &mut [ctx_val, callee_tag, callee_payload],
                "callee_boxed",
            );
            // The result is {tag=JIT_TAG_PTR, payload=ptr_as_i64}
            let callee_ptr_int = ctx
                .builder
                .build_extract_value(boxed_tv, 1, "callee_ptr_int");
            let callee_ptr =
                ctx.builder
                    .build_int_to_ptr(callee_ptr_int, ctx.ptr_type, "callee_ptr");

            // Build args array and call rt_call_value
            let (rt_call_value, rt_call_value_ty) = get_helper(ctx, "rt_call_value")?;

            // rt_call_value expects *mut Value args, not JitTaggedValue args.
            // For now, force-box each arg individually.
            // TODO: optimize this for hot paths
            let args_ptrs = if args.is_empty() {
                wrapper::const_null(ctx.ptr_type)
            } else {
                let array_ty =
                    unsafe { llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64) };
                let alloca = ctx.builder.build_alloca(array_ty, "var_args_buf");
                for (i, arg) in args.iter().enumerate() {
                    let arg_val = get_var(env, *arg)?;
                    let arg_tag = ctx.builder.build_extract_value(arg_val, 0, "varg_tag");
                    let arg_payload = ctx.builder.build_extract_value(arg_val, 1, "varg_payload");
                    let boxed = ctx.builder.build_call(
                        force_boxed_ty,
                        force_boxed,
                        &mut [ctx_val, arg_tag, arg_payload],
                        "varg_boxed",
                    );
                    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "varg_ptr_int");
                    let ptr = ctx
                        .builder
                        .build_int_to_ptr(ptr_int, ctx.ptr_type, "varg_ptr");
                    let slot_ptr = unsafe {
                        llvm_sys::core::LLVMBuildGEP2(
                            ctx.builder.raw_ptr(),
                            array_ty,
                            alloca,
                            [
                                wrapper::const_i64(ctx.i64_type, 0),
                                wrapper::const_i64(ctx.i64_type, i as i64),
                            ]
                            .as_mut_ptr(),
                            2,
                            c"varg_slot".as_ptr(),
                        )
                    };
                    let s = ctx.builder.build_store(ptr, slot_ptr);
                    wrapper::set_tbaa(s, ctx.tbaa_args);
                }
                alloca
            };

            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
            let result = ctx.builder.build_call(
                rt_call_value_ty,
                rt_call_value,
                &mut [ctx_val, callee_ptr, args_ptrs, nargs, sl, sc, el, ec],
                "var_call",
            );
            let result = emit_null_check(ctx, func_ref, result);
            unbox_ptr_result(ctx, result, ctx_val)
        }
    }
}
