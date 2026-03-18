//! Expression compilation: translates `IrExpr` nodes into LLVM IR values.

use std::collections::HashMap;

use llvm_sys::prelude::*;

use crate::cfg::{IrConst, IrExpr, IrListTest, IrProgram, IrTagTest, IrVar};
use crate::syntax::interner::Interner;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper::{self};
use super::binary_ops::compile_binary;
use super::helpers::{
    build_bool_tagged, build_int_tagged, build_ptr_tagged, build_tagged_args_array,
    emit_null_check, force_box_to_ptr, get_helper, get_var, unbox_ptr_result,
};

#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn compile_expr(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    expr: &IrExpr,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    func_ref: LLVMValueRef,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
    module_functions: &HashMap<(crate::syntax::Identifier, crate::syntax::Identifier), usize>,
    module_names: &[crate::syntax::Identifier],
    module_env: &HashMap<IrVar, crate::syntax::Identifier>,
) -> Result<LLVMValueRef, String> {
    match expr {
        IrExpr::Const(IrConst::Int(n)) => Ok(build_int_tagged(ctx, *n)),
        IrExpr::Const(IrConst::Bool(b)) => Ok(build_bool_tagged(ctx, *b)),
        IrExpr::Const(IrConst::Unit) => {
            // Unit is represented as None
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val], "unit");
            Ok(result)
        }
        IrExpr::Const(IrConst::Float(f)) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_float")?;
            let bits = wrapper::const_i64(ctx.i64_type, f.to_bits() as i64);
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, bits], "float");
            Ok(result)
        }
        IrExpr::Const(IrConst::String(s)) => {
            // Embed string bytes as a global constant, then call rt_make_string
            let bytes = s.as_bytes();
            let global_name = format!(".str.{}", s.len());
            let global =
                wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, &global_name, bytes);
            let (make_string, make_string_ty) = get_helper(ctx, "rt_make_string")?;
            let len_val = wrapper::const_i64(ctx.i64_type, bytes.len() as i64);
            let ptr_result = ctx.builder.build_call(
                make_string_ty,
                make_string,
                &mut [ctx_val, global, len_val],
                "str",
            );
            Ok(build_ptr_tagged(ctx, ptr_result))
        }
        IrExpr::Var(var) => get_var(env, *var),
        IrExpr::Binary(op, lhs, rhs) => {
            let lhs_val = get_var(env, *lhs)?;
            let rhs_val = get_var(env, *rhs)?;
            compile_binary(ctx, *op, lhs_val, rhs_val, ctx_val)
        }
        IrExpr::None => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val], "none");
            Ok(result)
        }
        IrExpr::LoadName(name) => {
            let name_str = interner.resolve(*name);

            // 1. Check if it's a user function → create a JitClosure
            if let Some(fn_index) = program.functions.iter().position(|f| f.name == Some(*name)) {
                let (make_closure, make_closure_ty) = get_helper(ctx, "rt_make_jit_closure")?;
                let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
                let null_ptr = wrapper::const_null(ctx.ptr_type);
                let zero = wrapper::const_i64(ctx.i64_type, 0);
                let ptr_result = ctx.builder.build_call(
                    make_closure_ty,
                    make_closure,
                    &mut [ctx_val, fn_idx_val, null_ptr, zero],
                    "user_fn",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            // 2. Check if it's a global variable (globals shadow base functions)
            if let Some(idx) = program.globals.iter().position(|g| *g == *name) {
                let (func, fn_ty) = get_helper(ctx, "rt_get_global")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result =
                    ctx.builder
                        .build_call(fn_ty, func, &mut [ctx_val, idx_val], "global_ptr");
                return unbox_ptr_result(ctx, ptr_result, ctx_val);
            }

            // 3. Check if it's a base function
            if let Some(idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (make_base_fn, make_base_fn_ty) = get_helper(ctx, "rt_make_base_function")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx.builder.build_call(
                    make_base_fn_ty,
                    make_base_fn,
                    &mut [ctx_val, idx_val],
                    "base_fn",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            // 4. Check if it's a unit ADT constructor (0-arity)
            if let Some(&arity) = adt_constructors.get(name)
                && arity == 0
            {
                let (intern_adt, intern_adt_ty) = get_helper(ctx, "rt_intern_unit_adt")?;
                let name_bytes = name_str.as_bytes();
                let global = wrapper::create_global_string(
                    &ctx.module,
                    &ctx.llvm_ctx,
                    &format!(".adt.{}", name_str),
                    name_bytes,
                );
                let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let ptr_result = ctx.builder.build_call(
                    intern_adt_ty,
                    intern_adt,
                    &mut [ctx_val, global, len],
                    "unit_adt",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }
            // Non-zero arity constructors are handled via Named calls / MakeAdt

            // 5. Check if it's a module name or qualified module path
            let is_module_ref = name_str == "Base"
                || module_names.contains(name)
                || module_names
                    .iter()
                    .any(|m| name_str.starts_with(&format!("{}.", interner.resolve(*m))));
            if is_module_ref {
                // Module reference — return None tagged value (used only as target for MemberAccess)
                let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
                return Ok(ctx
                    .builder
                    .build_call(fn_ty, func, &mut [ctx_val], "module_ref"));
            }

            // 6. Check if it's a qualified module name with function (e.g., "Module.function")
            if name_str.contains('.') {
                let parts: Vec<&str> = name_str.splitn(2, '.').collect();
                if parts.len() == 2 {
                    let mod_part = parts[0];
                    let member_part = parts[1];

                    // Check if it's a qualified Base function (e.g., "Base.len")
                    if mod_part == "Base"
                        && let Some(idx) =
                            crate::runtime::base::get_base_function_index(member_part)
                    {
                        let (make_base_fn, make_base_fn_ty) =
                            get_helper(ctx, "rt_make_base_function")?;
                        let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                        let ptr_result = ctx.builder.build_call(
                            make_base_fn_ty,
                            make_base_fn,
                            &mut [ctx_val, idx_val],
                            "qualified_base_fn",
                        );
                        return Ok(build_ptr_tagged(ctx, ptr_result));
                    }

                    // Look up in module_functions by resolving the string parts back to Identifiers
                    for (&(mod_id, fn_id), &fn_index) in module_functions.iter() {
                        if interner.resolve(mod_id) == mod_part
                            && interner.resolve(fn_id) == member_part
                        {
                            let (make_closure, make_closure_ty) =
                                get_helper(ctx, "rt_make_jit_closure")?;
                            let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
                            let null_ptr = wrapper::const_null(ctx.ptr_type);
                            let zero = wrapper::const_i64(ctx.i64_type, 0);
                            let ptr_result = ctx.builder.build_call(
                                make_closure_ty,
                                make_closure,
                                &mut [ctx_val, fn_idx_val, null_ptr, zero],
                                "qualified_fn",
                            );
                            return Ok(build_ptr_tagged(ctx, ptr_result));
                        }
                    }
                    // Check if it's a qualified ADT constructor
                    for (&ctor_name, &arity) in adt_constructors.iter() {
                        if interner.resolve(ctor_name) == member_part
                            && arity == 0
                        {
                            let (intern_adt, intern_adt_ty) =
                                get_helper(ctx, "rt_intern_unit_adt")?;
                            let ctor_bytes = member_part.as_bytes();
                            let global = wrapper::create_global_string(
                                &ctx.module,
                                &ctx.llvm_ctx,
                                &format!(".adt.{}", member_part),
                                ctor_bytes,
                            );
                            let len = wrapper::const_i64(ctx.i64_type, ctor_bytes.len() as i64);
                            let ptr_result = ctx.builder.build_call(
                                intern_adt_ty,
                                intern_adt,
                                &mut [ctx_val, global, len],
                                "qualified_unit_adt",
                            );
                            return Ok(build_ptr_tagged(ctx, ptr_result));
                        }
                    }
                }
            }

            Err(format!("LLVM backend: unresolved name '{}'", name_str))
        }
        IrExpr::MakeClosure(fn_id, captures) => {
            let fn_index = program
                .functions
                .iter()
                .position(|f| f.id == *fn_id)
                .ok_or_else(|| format!("missing function {:?}", fn_id))?;

            let (func, fn_ty) = get_helper(ctx, "rt_make_jit_closure")?;
            let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);

            if captures.is_empty() {
                let null_ptr = wrapper::const_null(ctx.ptr_type);
                let zero = wrapper::const_i64(ctx.i64_type, 0);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, fn_idx_val, null_ptr, zero],
                    "closure",
                );
                Ok(build_ptr_tagged(ctx, result))
            } else {
                // Build captures as a tagged value array (consecutive {tag, payload} i64 pairs)
                let captures_buf = build_tagged_args_array(ctx, captures, env)?;
                let ncaptures = wrapper::const_i64(ctx.i64_type, captures.len() as i64);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, fn_idx_val, captures_buf, ncaptures],
                    "closure",
                );
                Ok(build_ptr_tagged(ctx, result))
            }
        }
        IrExpr::Prefix { operator, right } => {
            let right_val = get_var(env, *right)?;
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => {
                    return Err(format!(
                        "LLVM backend: unsupported prefix operator '{}'",
                        operator
                    ));
                }
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let tag = ctx.builder.build_extract_value(right_val, 0, "pfx_tag");
            let payload = ctx.builder.build_extract_value(right_val, 1, "pfx_payload");
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, tag, payload], "prefix");
            Ok(result)
        }
        IrExpr::InterpolatedString(parts) => {
            // Build each part as a string, then concatenate via rt_string_concat.
            // For now, convert each part to a boxed Value and concatenate.
            let (make_string, make_string_ty) = get_helper(ctx, "rt_make_string")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;

            // Helper: rt_to_string and rt_string_concat
            let to_string = ctx.helpers.get("rt_to_string");
            let string_concat = ctx.helpers.get("rt_string_concat");

            if to_string.is_none() || string_concat.is_none() {
                return Err("LLVM backend: rt_to_string/rt_string_concat not declared".to_string());
            }
            let (to_string_fn, to_string_ty) = *to_string.unwrap();
            let (concat_fn, concat_ty) = *string_concat.unwrap();

            // Start with empty string
            let empty_global =
                wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, ".str.empty", b"");
            let zero_len = wrapper::const_i64(ctx.i64_type, 0);
            let mut accum = ctx.builder.build_call(
                make_string_ty,
                make_string,
                &mut [ctx_val, empty_global, zero_len],
                "interp_base",
            );

            for part in parts {
                let part_ptr = match part {
                    crate::cfg::IrStringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let global = wrapper::create_global_string(
                            &ctx.module,
                            &ctx.llvm_ctx,
                            &format!(".str.interp.{}", bytes.len()),
                            bytes,
                        );
                        let len = wrapper::const_i64(ctx.i64_type, bytes.len() as i64);
                        ctx.builder.build_call(
                            make_string_ty,
                            make_string,
                            &mut [ctx_val, global, len],
                            "lit_part",
                        )
                    }
                    crate::cfg::IrStringPart::Interpolation(var) => {
                        let val = get_var(env, *var)?;
                        let tag = ctx.builder.build_extract_value(val, 0, "interp_tag");
                        let payload = ctx.builder.build_extract_value(val, 1, "interp_payload");
                        let boxed = ctx.builder.build_call(
                            force_boxed_ty,
                            force_boxed,
                            &mut [ctx_val, tag, payload],
                            "interp_boxed",
                        );
                        let ptr_int = ctx.builder.build_extract_value(boxed, 1, "interp_ptr_int");
                        let ptr = ctx
                            .builder
                            .build_int_to_ptr(ptr_int, ctx.ptr_type, "interp_ptr");
                        // Convert to string
                        ctx.builder.build_call(
                            to_string_ty,
                            to_string_fn,
                            &mut [ctx_val, ptr],
                            "interp_str",
                        )
                    }
                };
                // Concatenate
                accum = ctx.builder.build_call(
                    concat_ty,
                    concat_fn,
                    &mut [ctx_val, accum, part_ptr],
                    "interp_cat",
                );
            }

            Ok(build_ptr_tagged(ctx, accum))
        }
        IrExpr::EmptyList => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_empty_list")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "empty_list");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeArray(vars) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_array")?;
            let args_buf = build_tagged_args_array(ctx, vars, env)?;
            let len = wrapper::const_i64(ctx.i64_type, vars.len() as i64);
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, args_buf, len], "array");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeTuple(vars) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_tuple")?;
            let args_buf = build_tagged_args_array(ctx, vars, env)?;
            let len = wrapper::const_i64(ctx.i64_type, vars.len() as i64);
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, args_buf, len], "tuple");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeHash(pairs) => {
            // rt_make_hash expects interleaved [k0, v0, k1, v1, ...] tagged values
            let (func, fn_ty) = get_helper(ctx, "rt_make_hash")?;
            let flat: Vec<IrVar> = pairs.iter().flat_map(|(k, v)| [*k, *v]).collect();
            let args_buf = build_tagged_args_array(ctx, &flat, env)?;
            let npairs = wrapper::const_i64(ctx.i64_type, pairs.len() as i64);
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, args_buf, npairs], "hash");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeList(vars) => {
            // Build a cons list right-to-left: cons(last, cons(... cons(first, empty)))
            let (make_cons, make_cons_ty) = get_helper(ctx, "rt_make_cons")?;
            let (make_empty, make_empty_ty) = get_helper(ctx, "rt_make_empty_list")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
            let mut tail =
                ctx.builder
                    .build_call(make_empty_ty, make_empty, &mut [ctx_val], "list_tail");
            for var in vars.iter().rev() {
                let val = get_var(env, *var)?;
                let tag = ctx.builder.build_extract_value(val, 0, "le_tag");
                let payload = ctx.builder.build_extract_value(val, 1, "le_payload");
                let boxed = ctx.builder.build_call(
                    force_boxed_ty,
                    force_boxed,
                    &mut [ctx_val, tag, payload],
                    "le_boxed",
                );
                let ptr_int = ctx.builder.build_extract_value(boxed, 1, "le_ptr_int");
                let head = ctx
                    .builder
                    .build_int_to_ptr(ptr_int, ctx.ptr_type, "le_ptr");
                tail = ctx.builder.build_call(
                    make_cons_ty,
                    make_cons,
                    &mut [ctx_val, head, tail],
                    "list_cons",
                );
            }
            Ok(build_ptr_tagged(ctx, tail))
        }
        IrExpr::Index { left, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_index")?;
            let left_ptr = force_box_to_ptr(ctx, env, *left, ctx_val)?;
            let idx_ptr = force_box_to_ptr(ctx, env, *index, ctx_val)?;
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, left_ptr, idx_ptr], "index");
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::TupleFieldAccess { object, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_tuple_get")?;
            let obj_ptr = force_box_to_ptr(ctx, env, *object, ctx_val)?;
            let idx_val = wrapper::const_i64(ctx.i64_type, *index as i64);
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, obj_ptr, idx_val], "tuple_get");
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::TupleArityTest { value, arity } => {
            let (func, fn_ty) = get_helper(ctx, "rt_tuple_len_eq")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let arity_val = wrapper::const_i64(ctx.i64_type, *arity as i64);
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, val_ptr, arity_val],
                "tuple_arity",
            );
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::MakeAdt(constructor, fields) => {
            let name_str = interner.resolve(*constructor);
            let name_bytes = name_str.as_bytes();
            if fields.is_empty() {
                // Unit ADT — use rt_intern_unit_adt for deduplication
                let (func, fn_ty) = get_helper(ctx, "rt_intern_unit_adt")?;
                let global = wrapper::create_global_string(
                    &ctx.module,
                    &ctx.llvm_ctx,
                    &format!(".adt.{}", name_str),
                    name_bytes,
                );
                let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let result =
                    ctx.builder
                        .build_call(fn_ty, func, &mut [ctx_val, global, len], "unit_adt");
                Ok(build_ptr_tagged(ctx, result))
            } else {
                let (func, fn_ty) = get_helper(ctx, "rt_make_adt")?;
                let global = wrapper::create_global_string(
                    &ctx.module,
                    &ctx.llvm_ctx,
                    &format!(".adt.{}", name_str),
                    name_bytes,
                );
                let name_len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let fields_buf = build_tagged_args_array(ctx, fields, env)?;
                let nfields = wrapper::const_i64(ctx.i64_type, fields.len() as i64);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, global, name_len, fields_buf, nfields],
                    "adt",
                );
                Ok(build_ptr_tagged(ctx, result))
            }
        }
        IrExpr::AdtTagTest { value, constructor } => {
            let (func, fn_ty) = get_helper(ctx, "rt_is_adt_constructor")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let name_str = interner.resolve(*constructor);
            let name_bytes = name_str.as_bytes();
            let global = wrapper::create_global_string(
                &ctx.module,
                &ctx.llvm_ctx,
                &format!(".adt.{}", name_str),
                name_bytes,
            );
            let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, val_ptr, global, len],
                "adt_test",
            );
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::AdtField { value, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_adt_field_or_none")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let idx_val = wrapper::const_i64(ctx.i64_type, *index as i64);
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, val_ptr, idx_val], "adt_field");
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::TagTest { value, tag } => {
            let helper_name = match tag {
                IrTagTest::None => "rt_is_none",
                IrTagTest::Some => "rt_is_some",
                IrTagTest::Left => "rt_is_left",
                IrTagTest::Right => "rt_is_right",
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, val_ptr], "tag_test");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::TagPayload { value, tag } => {
            let helper_name = match tag {
                IrTagTest::Some => "rt_unwrap_some",
                IrTagTest::Left => "rt_unwrap_left",
                IrTagTest::Right => "rt_unwrap_right",
                IrTagTest::None => return Err("LLVM backend: cannot unwrap None".to_string()),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, val_ptr], "tag_payload");
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::ListTest { value, tag } => {
            let helper_name = match tag {
                IrListTest::Empty => "rt_is_empty_list",
                IrListTest::Cons => "rt_is_cons",
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_test");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::ListHead { value } => {
            let (func, fn_ty) = get_helper(ctx, "rt_cons_head")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_head");
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::ListTail { value } => {
            let (func, fn_ty) = get_helper(ctx, "rt_cons_tail")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_tail");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Some(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_some")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, ptr], "some");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Left(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_left")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, ptr], "left");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Right(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_right")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, ptr], "right");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Cons { head, tail } => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_cons")?;
            let head_ptr = force_box_to_ptr(ctx, env, *head, ctx_val)?;
            let tail_ptr = force_box_to_ptr(ctx, env, *tail, ctx_val)?;
            let result =
                ctx.builder
                    .build_call(fn_ty, func, &mut [ctx_val, head_ptr, tail_ptr], "cons");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MemberAccess {
            object,
            member,
            module_name,
        } => {
            let name_str = interner.resolve(*member);

            // Tier 1: Resolve via module_env or module_name
            let resolved_module = module_name.or_else(|| module_env.get(object).copied());
            if let Some(mod_name) = resolved_module {
                // Check module functions: (mod_name, member) → function index
                if let Some(&fn_index) = module_functions.get(&(mod_name, *member)) {
                    let (make_closure, make_closure_ty) = get_helper(ctx, "rt_make_jit_closure")?;
                    let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
                    let null_ptr = wrapper::const_null(ctx.ptr_type);
                    let zero = wrapper::const_i64(ctx.i64_type, 0);
                    let ptr_result = ctx.builder.build_call(
                        make_closure_ty,
                        make_closure,
                        &mut [ctx_val, fn_idx_val, null_ptr, zero],
                        "module_fn",
                    );
                    return Ok(build_ptr_tagged(ctx, ptr_result));
                }
                // Check ADT constructors from the module (unit ADTs)
                if let Some(&arity) = adt_constructors.get(member)
                    && arity == 0
                {
                    let (intern_adt, intern_adt_ty) = get_helper(ctx, "rt_intern_unit_adt")?;
                    let name_bytes = name_str.as_bytes();
                    let global = wrapper::create_global_string(
                        &ctx.module,
                        &ctx.llvm_ctx,
                        &format!(".adt.{}", name_str),
                        name_bytes,
                    );
                    let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                    let ptr_result = ctx.builder.build_call(
                        intern_adt_ty,
                        intern_adt,
                        &mut [ctx_val, global, len],
                        "module_unit_adt",
                    );
                    return Ok(build_ptr_tagged(ctx, ptr_result));
                }
            }

            // Tier 2: Base module functions
            if let Some(idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (make_base_fn, make_base_fn_ty) = get_helper(ctx, "rt_make_base_function")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx.builder.build_call(
                    make_base_fn_ty,
                    make_base_fn,
                    &mut [ctx_val, idx_val],
                    "member_base_fn",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            // Tier 3: Dynamic member access via rt_index (fallback)
            let obj_ptr = force_box_to_ptr(ctx, env, *object, ctx_val)?;
            let member_str_bytes = name_str.as_bytes();
            let (make_string, make_string_ty) = get_helper(ctx, "rt_make_string")?;
            let member_global = wrapper::create_global_string(
                &ctx.module,
                &ctx.llvm_ctx,
                &format!(".member.{}", name_str),
                member_str_bytes,
            );
            let member_len = wrapper::const_i64(ctx.i64_type, member_str_bytes.len() as i64);
            let member_val = ctx.builder.build_call(
                make_string_ty,
                make_string,
                &mut [ctx_val, member_global, member_len],
                "member_key",
            );
            let (index_fn, index_ty) = get_helper(ctx, "rt_index")?;
            let indexed = ctx.builder.build_call(
                index_ty,
                index_fn,
                &mut [ctx_val, obj_ptr, member_val],
                "member_access",
            );
            let indexed = emit_null_check(ctx, func_ref, indexed);
            // Unwrap Some wrapper from rt_index result
            let (unwrap_some, unwrap_some_ty) = get_helper(ctx, "rt_unwrap_some")?;
            let result = ctx.builder.build_call(
                unwrap_some_ty,
                unwrap_some,
                &mut [ctx_val, indexed],
                "member_unwrap",
            );
            let result = emit_null_check(ctx, func_ref, result);
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::Perform {
            effect,
            operation,
            args,
        } => {
            let (func, fn_ty) = get_helper(ctx, "rt_perform")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;

            let effect_id = wrapper::const_i64(ctx.i64_type, effect.as_u32() as i64);
            let op_id = wrapper::const_i64(ctx.i64_type, operation.as_u32() as i64);

            // Build boxed args array (*mut Value pointers)
            let args_ptr = if args.is_empty() {
                wrapper::const_null(ctx.ptr_type)
            } else {
                let array_ty =
                    unsafe { llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64) };
                let alloca = ctx.builder.build_alloca(array_ty, "perform_args");
                for (i, arg) in args.iter().enumerate() {
                    let val = get_var(env, *arg)?;
                    let tag = ctx.builder.build_extract_value(val, 0, "pa_tag");
                    let payload = ctx.builder.build_extract_value(val, 1, "pa_payload");
                    let boxed = ctx.builder.build_call(
                        force_boxed_ty,
                        force_boxed,
                        &mut [ctx_val, tag, payload],
                        "pa_boxed",
                    );
                    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "pa_ptr_int");
                    let ptr = ctx
                        .builder
                        .build_int_to_ptr(ptr_int, ctx.ptr_type, "pa_ptr");
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
                            c"pa_slot".as_ptr(),
                        )
                    };
                    let s = ctx.builder.build_store(ptr, slot);
                    wrapper::set_tbaa(s, ctx.tbaa_args);
                }
                alloca
            };
            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);

            // Effect and operation name strings for error messages
            let effect_name = interner.resolve(*effect);
            let op_name = interner.resolve(*operation);
            let effect_global = wrapper::create_global_string(
                &ctx.module,
                &ctx.llvm_ctx,
                &format!(".effect.{}", effect_name),
                effect_name.as_bytes(),
            );
            let op_global = wrapper::create_global_string(
                &ctx.module,
                &ctx.llvm_ctx,
                &format!(".op.{}", op_name),
                op_name.as_bytes(),
            );
            let effect_len = wrapper::const_i64(ctx.i64_type, effect_name.len() as i64);
            let op_len = wrapper::const_i64(ctx.i64_type, op_name.len() as i64);
            let zero = wrapper::const_i64(ctx.i64_type, 0);

            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [
                    ctx_val,
                    effect_id,
                    op_id,
                    args_ptr,
                    nargs,
                    effect_global,
                    effect_len,
                    op_global,
                    op_len,
                    zero,
                    zero,
                ],
                "perform",
            );
            let result = emit_null_check(ctx, func_ref, result);
            unbox_ptr_result(ctx, result, ctx_val)
        }
        IrExpr::Handle { .. } => Err(
            "LLVM backend: Handle expression not supported (use HandleScope instruction)"
                .to_string(),
        ),
    }
}
