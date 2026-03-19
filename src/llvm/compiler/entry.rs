//! Entry wrapper and identity function compilation.

use crate::cfg::IrProgram;
use crate::syntax::interner::Interner;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper;
use super::helpers::load_tagged_from_ptr;

// ── Entry wrapper ────────────────────────────────────────────────────────────

pub(super) fn compile_entry_wrapper(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) -> Result<(), String> {
    let entry_fn_type = ctx.entry_function_type();
    let entry_wrapper = ctx.module.add_function("__flux_entry", entry_fn_type);

    let block = ctx.llvm_ctx.append_basic_block(entry_wrapper, "entry");
    ctx.builder.position_at_end(block);

    let ctx_param = wrapper::get_param(entry_wrapper, 0);
    let null_args = wrapper::const_null(ctx.ptr_type);
    let zero = wrapper::const_i64(ctx.i64_type, 0);

    // 1. Call the IR entry function (sets up globals, creates closures)
    let entry_idx = program
        .functions
        .iter()
        .position(|f| f.id == program.entry)
        .ok_or_else(|| "missing entry function".to_string())?;

    let (entry_func, entry_func_ty) = ctx.functions[&entry_idx];
    let entry_result = ctx.builder.build_call(
        entry_func_ty,
        entry_func,
        &mut [ctx_param, null_args, zero, null_args, zero],
        "entry_result",
    );

    // 2. If there's a user-defined `main` function, call it too
    //    (mirrors the Cranelift JIT's entry wrapper pattern)
    let main_idx = program
        .functions
        .iter()
        .position(|f| f.name.is_some() && interner.resolve(f.name.unwrap()) == "main");

    let result = if let Some(main_idx) = main_idx {
        if main_idx != entry_idx {
            let (main_func, main_func_ty) = ctx.functions[&main_idx];
            ctx.builder.build_call(
                main_func_ty,
                main_func,
                &mut [ctx_param, null_args, zero, null_args, zero],
                "main_result",
            )
        } else {
            entry_result
        }
    } else {
        entry_result
    };

    ctx.builder.build_ret(result);

    Ok(())
}

// ── Identity function (effect handler resume) ────────────────────────────────

/// Compile `__flux_identity(ctx, args, nargs, captures, ncaptures) -> {i64, i64}`
/// that returns its first argument. Used as the `resume` value in effect handlers.
/// Returns the function index in `ctx.functions`.
pub(super) fn compile_identity_function(ctx: &mut LlvmCompilerContext) -> Result<usize, String> {
    let fn_type = ctx.user_function_type();
    let func = ctx.module.add_function("__flux_identity", fn_type);
    let idx = ctx.functions.len();
    ctx.functions.insert(idx, (func, fn_type));

    let block = ctx.llvm_ctx.append_basic_block(func, "entry");
    ctx.builder.position_at_end(block);

    // Load first arg from args_ptr[0] (tag at offset 0, payload at offset 1)
    let args_ptr = wrapper::get_param(func, 1);
    let tagged = load_tagged_from_ptr(ctx, args_ptr, 0, "id_arg");
    ctx.builder.build_ret(tagged);

    Ok(idx)
}
