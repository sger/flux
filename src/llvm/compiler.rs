//! Translates `IrProgram` (CFG IR) into LLVM IR and creates a JIT-compiled
//! entry point.
//!
//! Phase 1 covers the minimal subset needed for fibonacci:
//! - Integer/boolean constants, variables, binary arithmetic/comparison
//! - Direct and named function calls (via runtime helpers)
//! - Jump, Branch, Return terminators

use std::collections::HashMap;

use llvm_sys::prelude::*;
use llvm_sys::LLVMIntPredicate;

use crate::cfg::{
    BlockId, IrBinaryOp, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr,
    IrProgram, IrTerminator, IrVar,
};
use crate::jit::context::{JIT_TAG_BOOL, JIT_TAG_INT, JIT_TAG_PTR};
use crate::jit::runtime_helpers::rt_symbols;
use crate::syntax::interner::Interner;

use super::context::LlvmCompilerContext;
use super::wrapper::{self, function_type};

/// Compile an IR program into LLVM IR inside the given context.
///
/// After this call, `ctx.finalize()` must be called to create the execution
/// engine, followed by `resolve_all_runtime_symbols()` to bind the `rt_*`
/// helpers.
pub fn compile_program(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) -> Result<(), String> {
    // 1. Declare runtime helpers as external functions
    declare_runtime_helpers(ctx);

    // 2. Forward-declare all user functions
    declare_user_functions(ctx, program, interner);

    // 3. Compile each function body
    for (idx, function) in program.functions.iter().enumerate() {
        compile_function(ctx, program, function, idx, interner)?;
    }

    // 4. Create entry wrapper: flux_main(ctx) → {i64, i64}
    compile_entry_wrapper(ctx, program)?;

    // 5. Verify
    ctx.module.verify()?;

    // 6. Finalize (create execution engine)
    ctx.finalize()?;

    // 7. Resolve runtime symbols
    resolve_all_runtime_symbols(ctx);

    Ok(())
}

// ── Runtime helper declaration ───────────────────────────────────────────────

/// Declare all `rt_*` functions as external symbols in the LLVM module.
fn declare_runtime_helpers(ctx: &mut LlvmCompilerContext) {
    let i64_ty = ctx.i64_type;
    let ptr_ty = ctx.ptr_type;
    let tv_ty = ctx.tagged_value_type;
    let void_ty = ctx.void_type;

    // Helper signature descriptors: (name, return_type, param_types)
    let helpers: Vec<(&str, LLVMTypeRef, Vec<LLVMTypeRef>)> = vec![
        // Arithmetic: (ctx, tv, tv) -> tv  — but the C ABI flattens JitTaggedValue
        // rt_add(ctx: ptr, a_tag: i64, a_payload: i64, b_tag: i64, b_payload: i64) -> {i64, i64}
        ("rt_add", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_sub", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_mul", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_div", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_mod", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        // Comparison: same signature
        ("rt_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_not_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_greater_than", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_less_than_or_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_greater_than_or_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        // Unary
        ("rt_negate", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        ("rt_not", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // Truthiness: (ctx, tag, payload) -> i64 (0 or 1)
        ("rt_is_truthy", i64_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // Value constructors
        ("rt_make_integer", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_float", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_bool", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_none", tv_ty, vec![ptr_ty]),
        // Force box: (ctx, tag, payload) -> {i64, i64}
        ("rt_force_boxed", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // String: (ctx, ptr, len) -> *mut Value (returned as ptr)
        ("rt_make_string", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Base function call: (ctx, idx, args_ptr, nargs, sl, sc, el, ec) -> ptr
        ("rt_call_base_function_tagged", ptr_ty, vec![
            ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
        ]),
        // Generic value call: (ctx, callee, args_ptr, nargs, sl, sc, el, ec) -> ptr
        ("rt_call_value", ptr_ty, vec![
            ptr_ty, ptr_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
        ]),
        // Global access
        ("rt_get_global", ptr_ty, vec![ptr_ty, i64_ty]),
        ("rt_set_global", void_ty, vec![ptr_ty, i64_ty, ptr_ty]),
        // Closure creation: (ctx, fn_index, captures_ptr, ncaptures) -> ptr
        ("rt_make_jit_closure", ptr_ty, vec![ptr_ty, i64_ty, ptr_ty, i64_ty]),
        // Array/tuple
        ("rt_make_array", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        ("rt_make_tuple", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Print / IO (called through base functions, not directly)
    ];

    for (name, ret_ty, param_tys) in helpers {
        let fn_type = function_type(ret_ty, &param_tys, false);
        let func = ctx.module.add_function(name, fn_type);
        ctx.helpers.insert(name, (func, fn_type));
    }
}

/// Bind each declared helper to its actual function pointer.
fn resolve_all_runtime_symbols(ctx: &LlvmCompilerContext) {
    let symbols = rt_symbols();
    for (name, ptr) in &symbols {
        ctx.resolve_symbol(name, *ptr);
    }
}

// ── User function declaration ────────────────────────────────────────────────

fn declare_user_functions(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) {
    let fn_type = ctx.user_function_type();
    for (idx, function) in program.functions.iter().enumerate() {
        let name = match function.name {
            Some(id) => format!("flux_{}", interner.resolve(id)),
            None => format!("flux_anon_{}", idx),
        };
        let func = ctx.module.add_function(&name, fn_type);
        ctx.functions.insert(idx, (func, fn_type));
    }
}

// ── Function compilation ─────────────────────────────────────────────────────

fn compile_function(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    fn_index: usize,
    interner: &Interner,
) -> Result<(), String> {
    let (func_ref, _) = ctx.functions[&fn_index];

    // Get function parameters (Array ABI)
    let ctx_val = wrapper::get_param(func_ref, 0); // *mut JitContext
    let args_ptr = wrapper::get_param(func_ref, 1); // *const JitTaggedValue
    let _nargs = wrapper::get_param(func_ref, 2); // i64
    let captures_ptr = wrapper::get_param(func_ref, 3); // *const JitTaggedValue
    let _ncaptures = wrapper::get_param(func_ref, 4); // i64

    // Create LLVM basic blocks for each IR block
    let mut block_map: HashMap<BlockId, LLVMBasicBlockRef> = HashMap::new();
    for block in &function.blocks {
        let llvm_block = ctx
            .llvm_ctx
            .append_basic_block(func_ref, &format!("b{}", block.id.0));
        block_map.insert(block.id, llvm_block);
    }

    // Variable environment: IrVar → LLVMValueRef (each is a {i64, i64} tagged value)
    let mut env: HashMap<IrVar, LLVMValueRef> = HashMap::new();

    // Load function parameters into env.
    // In the Array ABI, captures occupy the first N params, then explicit args follow.
    let capture_count = function.captures.len();
    let entry_block = block_map[&function.entry];
    ctx.builder.position_at_end(entry_block);

    // Load captures from captures_ptr (first capture_count params)
    for (i, param) in function.params.iter().take(capture_count).enumerate() {
        let tagged = load_tagged_from_ptr(ctx, captures_ptr, i, "cap");
        env.insert(param.var, tagged);
    }

    // Load explicit args from args_ptr (remaining params)
    for (i, param) in function.params.iter().skip(capture_count).enumerate() {
        let tagged = load_tagged_from_ptr(ctx, args_ptr, i, "arg");
        env.insert(param.var, tagged);
    }

    // Compile each block
    for block in &function.blocks {
        let llvm_block = block_map[&block.id];
        // Only position if this isn't the entry block (already positioned)
        if block.id != function.entry {
            ctx.builder.position_at_end(llvm_block);
        }

        compile_block(ctx, program, function, block, &block_map, &mut env, ctx_val, interner)?;
    }

    Ok(())
}

fn compile_block(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    block: &IrBlock,
    block_map: &HashMap<BlockId, LLVMBasicBlockRef>,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
) -> Result<(), String> {
    // Compile instructions
    for instr in &block.instrs {
        match instr {
            IrInstr::Assign { dest, expr, .. } => {
                let value = compile_expr(ctx, program, expr, env, ctx_val, interner)?;
                env.insert(*dest, value);
            }
            IrInstr::Call {
                dest,
                target,
                args,
                ..
            } => {
                let value =
                    compile_call(ctx, program, function, target, args, env, ctx_val, interner)?;
                env.insert(*dest, value);
            }
            IrInstr::HandleScope { .. } => {
                // Phase 1: effect handlers not yet supported
                return Err("HandleScope not yet supported in LLVM backend".to_string());
            }
        }
    }

    // Compile terminator
    match &block.terminator {
        IrTerminator::Return(var, _) => {
            let value = get_var(env, *var)?;
            ctx.builder.build_ret(value);
        }
        IrTerminator::Jump(target, args, _) => {
            // For Phase 1 with alloca-based vars, we handle block params
            // by storing into the target block's param vars before jumping.
            let target_block_ir = function
                .blocks
                .iter()
                .find(|b| b.id == *target)
                .ok_or_else(|| format!("missing target block {:?}", target))?;

            for (param, arg) in target_block_ir.params.iter().zip(args.iter()) {
                let val = get_var(env, *arg)?;
                env.insert(param.var, val);
            }

            let llvm_target = block_map[target];
            ctx.builder.build_br(llvm_target);
        }
        IrTerminator::Branch {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let cond_val = get_var(env, *cond)?;
            // Extract the payload (boolean value) and compare to 0
            let payload = ctx
                .builder
                .build_extract_value(cond_val, 1, "cond_payload");
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let cond_bool = ctx.builder.build_icmp(
                LLVMIntPredicate::LLVMIntNE,
                payload,
                zero,
                "cond_bool",
            );
            let llvm_then = block_map[then_block];
            let llvm_else = block_map[else_block];
            ctx.builder.build_cond_br(cond_bool, llvm_then, llvm_else);
        }
        IrTerminator::TailCall {
            callee, args, ..
        } => {
            // Phase 1: compile as a regular call + return
            let value = compile_call(ctx, program, function, callee, args, env, ctx_val, interner)?;
            ctx.builder.build_ret(value);
        }
        IrTerminator::Unreachable(_) => {
            // Emit unreachable
            unsafe {
                llvm_sys::core::LLVMBuildUnreachable(ctx.builder.raw_ptr());
            }
        }
    }

    Ok(())
}

// ── Expression compilation ───────────────────────────────────────────────────

fn compile_expr(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    expr: &IrExpr,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
) -> Result<LLVMValueRef, String> {
    match expr {
        IrExpr::Const(IrConst::Int(n)) => Ok(build_int_tagged(ctx, *n)),
        IrExpr::Const(IrConst::Bool(b)) => Ok(build_bool_tagged(ctx, *b)),
        IrExpr::Const(IrConst::Unit) => {
            // Unit is represented as None
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "unit");
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
        IrExpr::Const(IrConst::String(_s)) => {
            // Phase 1: string constants via rt_make_string
            // For simplicity, we embed the string bytes as a global constant.
            // This requires more LLVM API surface; for now return a None placeholder.
            // TODO: implement string constants properly
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "str_placeholder");
            Ok(result)
        }
        IrExpr::Var(var) => get_var(env, *var),
        IrExpr::Binary(op, lhs, rhs) => {
            let lhs_val = get_var(env, *lhs)?;
            let rhs_val = get_var(env, *rhs)?;
            compile_binary(ctx, *op, lhs_val, rhs_val, ctx_val)
        }
        IrExpr::None => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "none");
            Ok(result)
        }
        IrExpr::LoadName(name) => {
            // Check if it's a global
            if let Some(idx) = program
                .globals
                .iter()
                .position(|g| *g == *name)
            {
                let (func, fn_ty) = get_helper(ctx, "rt_get_global")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx
                    .builder
                    .build_call(fn_ty, func, &mut [ctx_val, idx_val], "global_ptr");
                // Convert *mut Value → tagged value via rt_force_boxed is wrong here,
                // the result is already a *mut Value pointer, wrap as PTR tag
                Ok(build_ptr_tagged(ctx, ptr_result))
            } else {
                // Check if it's a known base function
                let name_str = interner.resolve(*name);
                if let Some(idx) = crate::runtime::base::get_base_function_index(name_str) {
                    Ok(build_int_tagged(ctx, idx as i64))
                } else {
                    Err(format!(
                        "LLVM backend: unresolved name '{}'",
                        name_str
                    ))
                }
            }
        }
        IrExpr::MakeClosure(fn_id, captures) => {
            let fn_index = program
                .functions
                .iter()
                .position(|f| f.id == *fn_id)
                .ok_or_else(|| format!("missing function {:?}", fn_id))?;

            if captures.is_empty() {
                let (func, fn_ty) = get_helper(ctx, "rt_make_jit_closure")?;
                let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
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
                // TODO: allocate captures array on stack, fill, pass to rt_make_jit_closure
                Err("LLVM backend: closures with captures not yet supported".to_string())
            }
        }
        _ => Err(format!(
            "LLVM backend: unsupported expression {:?}",
            std::mem::discriminant(expr)
        )),
    }
}

// ── Binary operations ────────────────────────────────────────────────────────

fn compile_binary(
    ctx: &LlvmCompilerContext,
    op: IrBinaryOp,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    // For typed integer operations, inline the arithmetic
    match op {
        IrBinaryOp::IAdd | IrBinaryOp::ISub | IrBinaryOp::IMul | IrBinaryOp::IDiv | IrBinaryOp::IMod => {
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
        IrBinaryOp::Lt | IrBinaryOp::Gt | IrBinaryOp::Le | IrBinaryOp::Ge | IrBinaryOp::Eq | IrBinaryOp::NotEq => {
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
        // Polymorphic arithmetic — delegate to runtime
        IrBinaryOp::Add | IrBinaryOp::Sub | IrBinaryOp::Mul | IrBinaryOp::Div | IrBinaryOp::Mod => {
            let helper_name = match op {
                IrBinaryOp::Add => "rt_add",
                IrBinaryOp::Sub => "rt_sub",
                IrBinaryOp::Mul => "rt_mul",
                IrBinaryOp::Div => "rt_div",
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
        }
        _ => Err(format!("LLVM backend: unsupported binary op {:?}", op)),
    }
}

// ── Call compilation ─────────────────────────────────────────────────────────

fn compile_call(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    _function: &IrFunction,
    target: &IrCallTarget,
    args: &[IrVar],
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
) -> Result<LLVMValueRef, String> {
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
            if let Some(fn_index) = program
                .functions
                .iter()
                .position(|f| f.name == Some(*name))
            {
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

            // Check if it's a base function
            if let Some(base_idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (func, fn_ty) = get_helper(ctx, "rt_call_base_function_tagged")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, base_idx as i64);
                let args_array = build_tagged_args_array(ctx, args, env)?;
                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let zero = wrapper::const_i64(ctx.i64_type, 0);

                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, idx_val, args_array, nargs, zero, zero, zero, zero],
                    "base_call",
                );
                // Result is *mut Value; wrap as PTR tag
                return Ok(build_ptr_tagged(ctx, result));
            }

            Err(format!(
                "LLVM backend: unresolved named callee '{}'",
                name_str
            ))
        }
        IrCallTarget::Var(_var) => {
            Err("LLVM backend: indirect calls via Var not yet supported".to_string())
        }
    }
}

// ── Entry wrapper ────────────────────────────────────────────────────────────

fn compile_entry_wrapper(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
) -> Result<(), String> {
    let entry_fn_type = ctx.entry_function_type();
    let entry_wrapper = ctx.module.add_function("flux_main", entry_fn_type);

    let block = ctx.llvm_ctx.append_basic_block(entry_wrapper, "entry");
    ctx.builder.position_at_end(block);

    let ctx_param = wrapper::get_param(entry_wrapper, 0);

    // Find the entry function
    let entry_idx = program
        .functions
        .iter()
        .position(|f| f.id == program.entry)
        .ok_or_else(|| "missing entry function".to_string())?;

    let (entry_func, entry_func_ty) = ctx.functions[&entry_idx];

    // Call with empty args
    let null_args = wrapper::const_null(ctx.ptr_type);
    let zero = wrapper::const_i64(ctx.i64_type, 0);

    let result = ctx.builder.build_call(
        entry_func_ty,
        entry_func,
        &mut [ctx_param, null_args, zero, null_args, zero],
        "main_result",
    );

    ctx.builder.build_ret(result);

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Load a `{i64, i64}` tagged value from a pointer to an array of i64 pairs.
/// Index `i` means the pair at `ptr[i*2]` (tag) and `ptr[i*2+1]` (payload).
fn load_tagged_from_ptr(
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

    build_tagged_value(ctx, tag, payload)
}

fn get_var(env: &HashMap<IrVar, LLVMValueRef>, var: IrVar) -> Result<LLVMValueRef, String> {
    env.get(&var)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undefined variable {:?}", var))
}

fn get_helper(
    ctx: &LlvmCompilerContext,
    name: &str,
) -> Result<(LLVMValueRef, LLVMTypeRef), String> {
    ctx.helpers
        .get(name)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undeclared helper '{}'", name))
}

/// Build a `{i64, i64}` tagged value from tag and payload.
fn build_tagged_value(
    ctx: &LlvmCompilerContext,
    tag: LLVMValueRef,
    payload: LLVMValueRef,
) -> LLVMValueRef {
    let undef = wrapper::get_undef(ctx.tagged_value_type);
    let v1 = ctx.builder.build_insert_value(undef, tag, 0, "tv_tag");
    ctx.builder.build_insert_value(v1, payload, 1, "tv")
}

/// Build a tagged integer constant.
fn build_int_tagged(ctx: &LlvmCompilerContext, value: impl IntoI64OrValue) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_INT);
    let payload = value.to_llvm_value(ctx.i64_type);
    build_tagged_value(ctx, tag, payload)
}

/// Build a tagged boolean constant.
fn build_bool_tagged(ctx: &LlvmCompilerContext, value: bool) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_BOOL);
    let payload = wrapper::const_i64(ctx.i64_type, value as i64);
    build_tagged_value(ctx, tag, payload)
}

/// Build a tagged PTR value from a pointer.
fn build_ptr_tagged(ctx: &LlvmCompilerContext, ptr: LLVMValueRef) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_PTR);
    let payload = ctx
        .builder
        .build_ptr_to_int(ptr, ctx.i64_type, "ptr_as_int");
    build_tagged_value(ctx, tag, payload)
}

/// Build an array of tagged values on the stack for passing to function calls.
/// Returns a pointer to the first element (or null if empty).
fn build_tagged_args_array(
    ctx: &LlvmCompilerContext,
    args: &[IrVar],
    env: &HashMap<IrVar, LLVMValueRef>,
) -> Result<LLVMValueRef, String> {
    if args.is_empty() {
        return Ok(wrapper::const_null(ctx.ptr_type));
    }

    // Allocate space for args.len() * 2 i64s on the stack
    let array_size = wrapper::const_i64(ctx.i64_type, (args.len() * 2) as i64);
    let array_ty = unsafe {
        llvm_sys::core::LLVMArrayType2(ctx.i64_type, (args.len() * 2) as u64)
    };
    let alloca = ctx.builder.build_alloca(array_ty, "args_buf");

    for (i, arg) in args.iter().enumerate() {
        let val = get_var(env, *arg)?;
        let tag = ctx.builder.build_extract_value(val, 0, "tag");
        let payload = ctx.builder.build_extract_value(val, 1, "payload");

        let tag_offset = wrapper::const_i64(ctx.i64_type, (i * 2) as i64);
        let tag_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                ctx.i64_type,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), tag_offset].as_mut_ptr(),
                2,
                c"arg_tag_slot".as_ptr(),
            )
        };
        ctx.builder.build_store(tag, tag_ptr);

        let payload_offset = wrapper::const_i64(ctx.i64_type, (i * 2 + 1) as i64);
        let payload_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                ctx.i64_type,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), payload_offset].as_mut_ptr(),
                2,
                c"arg_payload_slot".as_ptr(),
            )
        };
        ctx.builder.build_store(payload, payload_ptr);
    }

    // Cast to ptr
    let _ = array_size; // used for sizing
    Ok(alloca)
}

/// Trait to accept both i64 constants and LLVM values for `build_int_tagged`.
trait IntoI64OrValue {
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
