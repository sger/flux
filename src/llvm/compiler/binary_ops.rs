//! Binary operation compilation for the LLVM backend.

use llvm_sys::LLVMIntPredicate;
use llvm_sys::prelude::*;

use crate::cfg::IrBinaryOp;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper;
use super::helpers::{build_bool_tagged, build_int_tagged, get_helper};

pub(super) fn compile_binary(
    ctx: &LlvmCompilerContext,
    op: IrBinaryOp,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    // For typed integer operations, delegate to the runtime helpers for
    // correctness: gradual typing may produce IAdd even when one operand is
    // None, so we need the runtime type check in rt_add/rt_sub/etc.
    match op {
        IrBinaryOp::Lt
        | IrBinaryOp::Gt
        | IrBinaryOp::Le
        | IrBinaryOp::Ge
        | IrBinaryOp::Eq
        | IrBinaryOp::NotEq => {
            // Use runtime helpers for polymorphic comparisons.
            // Note: there is no rt_less_than; for Lt we swap args and use rt_greater_than.
            let (helper_name, swap_args) = match op {
                IrBinaryOp::Lt => ("rt_greater_than", true), // a < b  ≡  b > a
                IrBinaryOp::Le => ("rt_less_than_or_equal", false),
                IrBinaryOp::Gt => ("rt_greater_than", false),
                IrBinaryOp::Ge => ("rt_greater_than_or_equal", false),
                IrBinaryOp::Eq => ("rt_equal", false),
                IrBinaryOp::NotEq => ("rt_not_equal", false),
                _ => unreachable!(),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let (a, b) = if swap_args { (rhs, lhs) } else { (lhs, rhs) };
            let a_tag = ctx.builder.build_extract_value(a, 0, "a_tag");
            let a_payload = ctx.builder.build_extract_value(a, 1, "a_payload");
            let b_tag = ctx.builder.build_extract_value(b, 0, "b_tag");
            let b_payload = ctx.builder.build_extract_value(b, 1, "b_payload");
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, a_tag, a_payload, b_tag, b_payload],
                "cmp",
            );
            Ok(result)
        }
        // Logical And/Or — extract payloads and use bitwise AND/OR on i64
        IrBinaryOp::And => {
            let (is_truthy, is_truthy_ty) = get_helper(ctx, "rt_is_truthy")?;
            let l_tag = ctx.builder.build_extract_value(lhs, 0, "l_tag");
            let l_payload = ctx.builder.build_extract_value(lhs, 1, "l_payload");
            let r_tag = ctx.builder.build_extract_value(rhs, 0, "r_tag");
            let r_payload = ctx.builder.build_extract_value(rhs, 1, "r_payload");
            let l_truthy = ctx.builder.build_call(
                is_truthy_ty,
                is_truthy,
                &mut [ctx_val, l_tag, l_payload],
                "l_truthy",
            );
            let r_truthy = ctx.builder.build_call(
                is_truthy_ty,
                is_truthy,
                &mut [ctx_val, r_tag, r_payload],
                "r_truthy",
            );
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let l_bool =
                ctx.builder
                    .build_icmp(LLVMIntPredicate::LLVMIntNE, l_truthy, zero, "l_bool");
            let r_bool =
                ctx.builder
                    .build_icmp(LLVMIntPredicate::LLVMIntNE, r_truthy, zero, "r_bool");
            let and_result = unsafe {
                let c = std::ffi::CString::new("and").unwrap();
                llvm_sys::core::LLVMBuildAnd(ctx.builder.raw_ptr(), l_bool, r_bool, c.as_ptr())
            };
            let result_i64 = unsafe {
                let c = std::ffi::CString::new("and_ext").unwrap();
                llvm_sys::core::LLVMBuildZExt(
                    ctx.builder.raw_ptr(),
                    and_result,
                    ctx.i64_type,
                    c.as_ptr(),
                )
            };
            Ok(build_bool_tagged(ctx, result_i64))
        }
        IrBinaryOp::Or => {
            let (is_truthy, is_truthy_ty) = get_helper(ctx, "rt_is_truthy")?;
            let l_tag = ctx.builder.build_extract_value(lhs, 0, "l_tag");
            let l_payload = ctx.builder.build_extract_value(lhs, 1, "l_payload");
            let r_tag = ctx.builder.build_extract_value(rhs, 0, "r_tag");
            let r_payload = ctx.builder.build_extract_value(rhs, 1, "r_payload");
            let l_truthy = ctx.builder.build_call(
                is_truthy_ty,
                is_truthy,
                &mut [ctx_val, l_tag, l_payload],
                "l_truthy",
            );
            let r_truthy = ctx.builder.build_call(
                is_truthy_ty,
                is_truthy,
                &mut [ctx_val, r_tag, r_payload],
                "r_truthy",
            );
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let l_bool =
                ctx.builder
                    .build_icmp(LLVMIntPredicate::LLVMIntNE, l_truthy, zero, "l_bool");
            let r_bool =
                ctx.builder
                    .build_icmp(LLVMIntPredicate::LLVMIntNE, r_truthy, zero, "r_bool");
            let or_result = unsafe {
                let c = std::ffi::CString::new("or").unwrap();
                llvm_sys::core::LLVMBuildOr(ctx.builder.raw_ptr(), l_bool, r_bool, c.as_ptr())
            };
            let result_i64 = unsafe {
                let c = std::ffi::CString::new("or_ext").unwrap();
                llvm_sys::core::LLVMBuildZExt(
                    ctx.builder.raw_ptr(),
                    or_result,
                    ctx.i64_type,
                    c.as_ptr(),
                )
            };
            Ok(build_bool_tagged(ctx, result_i64))
        }
        // Typed integer arithmetic — inlined (both operands guaranteed Int by Core IR)
        IrBinaryOp::IAdd
        | IrBinaryOp::ISub
        | IrBinaryOp::IMul
        | IrBinaryOp::IDiv
        | IrBinaryOp::IMod => {
            let lhs_payload = ctx.builder.build_extract_value(lhs, 1, "lhs_p");
            let rhs_payload = ctx.builder.build_extract_value(rhs, 1, "rhs_p");
            let result = match op {
                IrBinaryOp::IAdd => ctx.builder.build_add(lhs_payload, rhs_payload, "iadd"),
                IrBinaryOp::ISub => ctx.builder.build_sub(lhs_payload, rhs_payload, "isub"),
                IrBinaryOp::IMul => ctx.builder.build_mul(lhs_payload, rhs_payload, "imul"),
                IrBinaryOp::IDiv => ctx.builder.build_sdiv(lhs_payload, rhs_payload, "idiv"),
                IrBinaryOp::IMod => ctx.builder.build_srem(lhs_payload, rhs_payload, "imod"),
                _ => unreachable!(),
            };
            Ok(build_int_tagged(ctx, result))
        }
        // Polymorphic arithmetic — delegate to runtime for type safety
        IrBinaryOp::Add
        | IrBinaryOp::Sub
        | IrBinaryOp::Mul
        | IrBinaryOp::Div
        | IrBinaryOp::Mod
        | IrBinaryOp::FAdd
        | IrBinaryOp::FSub
        | IrBinaryOp::FMul
        | IrBinaryOp::FDiv => {
            let helper_name = match op {
                IrBinaryOp::Add | IrBinaryOp::FAdd => "rt_add",
                IrBinaryOp::Sub | IrBinaryOp::FSub => "rt_sub",
                IrBinaryOp::Mul | IrBinaryOp::FMul => "rt_mul",
                IrBinaryOp::Div | IrBinaryOp::FDiv => "rt_div",
                IrBinaryOp::Mod => "rt_mod",
                _ => unreachable!(),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let l_tag = ctx.builder.build_extract_value(lhs, 0, "l_tag");
            let l_payload = ctx.builder.build_extract_value(lhs, 1, "l_payload");
            let r_tag = ctx.builder.build_extract_value(rhs, 0, "r_tag");
            let r_payload = ctx.builder.build_extract_value(rhs, 1, "r_payload");
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, l_tag, l_payload, r_tag, r_payload],
                "binop",
            );
            Ok(result)
        } // All IrBinaryOp variants are covered above.
    }
}
