//! Translates `IrProgram` (CFG IR) into LLVM IR and creates a JIT-compiled
//! entry point.
//!
//! Phase 1 covers the minimal subset needed for fibonacci:
//! - Integer/boolean constants, variables, binary arithmetic/comparison
//! - Direct and named function calls (via runtime helpers)
//! - Jump, Branch, Return terminators

mod binary_ops;
mod calls;
mod entry;
mod expressions;
mod function;
mod helpers;
mod symbols;

use std::collections::HashMap;

use crate::cfg::IrProgram;
use crate::syntax::interner::Interner;

use super::context::LlvmCompilerContext;
use super::wrapper;

use entry::{compile_entry_wrapper, compile_identity_function};
use function::compile_function;
use symbols::{
    collect_adt_constructors, collect_module_functions, declare_runtime_helpers,
    declare_user_functions, resolve_all_runtime_symbols,
};

/// Compile an IR program into LLVM IR inside the given context.
///
/// After this call, `ctx.finalize()` must be called to create the execution
/// engine, followed by `resolve_all_runtime_symbols()` to bind the `rt_*`
/// helpers.
pub fn compile_program(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
    opt_level: u32,
) -> Result<(), String> {
    // 1. Declare runtime helpers as external functions
    declare_runtime_helpers(ctx);

    // 2. Collect ADT constructor arities from all data declarations (including inside modules)
    let mut adt_constructors: HashMap<crate::syntax::Identifier, usize> = HashMap::new();
    collect_adt_constructors(&program.top_level_items, &mut adt_constructors);

    // 2b. Collect module functions: (module_name, fn_name) → function index
    let mut module_functions: HashMap<
        (crate::syntax::Identifier, crate::syntax::Identifier),
        usize,
    > = HashMap::new();
    let mut module_names: Vec<crate::syntax::Identifier> = Vec::new();
    collect_module_functions(
        &program.top_level_items,
        None,
        program,
        &mut module_functions,
        &mut module_names,
    );

    // 3. Forward-declare all user functions
    declare_user_functions(ctx, program, interner);

    // 3. Compile each function body
    for (idx, function) in program.functions.iter().enumerate() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!(
                "[llvm] compiling function {} ({}) params={} captures={} blocks={}",
                idx,
                function
                    .name
                    .map(|n| interner.resolve(n).to_string())
                    .unwrap_or_else(|| "anon".to_string()),
                function.params.len(),
                function.captures.len(),
                function.blocks.len()
            );
        }
        compile_function(
            ctx,
            program,
            function,
            idx,
            interner,
            &adt_constructors,
            &module_functions,
            &module_names,
        )?;
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] function {} compiled OK", idx);
        }
    }

    // 4. Create entry wrapper: __flux_entry(ctx) → {i64, i64}
    compile_entry_wrapper(ctx, program, interner)?;

    // 5. Compile identity function (used as `resume` in effect handlers)
    compile_identity_function(ctx)?;

    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm] entry wrapper compiled OK");
    }

    // 5. Verify
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!(
            "=== LLVM IR ===\n{}\n===============",
            ctx.module.dump_to_string()
        );
    }
    if let Err(err) = ctx.module.verify() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] verification failed: {}", err);
        }
        return Err(err);
    }

    // 7. Optimization passes are skipped for JIT mode.
    // MCJIT on ARM64 has issues with optimized struct returns.
    // For optimized code, use AOT emission (--emit-obj -O).
    let _ = opt_level;

    // 8. Finalize (create execution engine for JIT)
    ctx.finalize()?;

    // 9. Resolve runtime symbols
    resolve_all_runtime_symbols(ctx);

    Ok(())
}

fn run_opt_passes(ctx: &LlvmCompilerContext, opt_level: u32) -> Result<(), String> {
    if opt_level == 0 {
        return Ok(());
    }
    let tm = wrapper::LlvmTargetMachine::for_host(opt_level)?;
    let triple = wrapper::get_default_target_triple();
    wrapper::set_module_target(&ctx.module, &triple);
    wrapper::set_module_data_layout(&ctx.module, &tm.data_layout());
    let passes = match opt_level {
        1 => "default<O1>",
        2 => "default<O2>",
        _ => "default<O3>",
    };
    wrapper::run_optimization_passes(&ctx.module, &tm, passes)
}

/// Compile an IR program into LLVM IR without creating an execution engine.
/// Used for AOT emission (object file / assembly).
/// Optimization passes are applied when `opt_level > 0`.
pub fn compile_program_ir_only(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
    opt_level: u32,
) -> Result<(), String> {
    declare_runtime_helpers(ctx);

    let mut adt_constructors: HashMap<crate::syntax::Identifier, usize> = HashMap::new();
    collect_adt_constructors(&program.top_level_items, &mut adt_constructors);

    let mut module_functions: HashMap<
        (crate::syntax::Identifier, crate::syntax::Identifier),
        usize,
    > = HashMap::new();
    let mut module_names: Vec<crate::syntax::Identifier> = Vec::new();
    collect_module_functions(
        &program.top_level_items,
        None,
        program,
        &mut module_functions,
        &mut module_names,
    );

    declare_user_functions(ctx, program, interner);

    for (idx, function) in program.functions.iter().enumerate() {
        compile_function(
            ctx,
            program,
            function,
            idx,
            interner,
            &adt_constructors,
            &module_functions,
            &module_names,
        )?;
    }

    compile_entry_wrapper(ctx, program, interner)?;
    compile_identity_function(ctx)?;

    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!(
            "=== LLVM IR ===\n{}\n===============",
            ctx.module.dump_to_string()
        );
    }
    if let Err(err) = ctx.module.verify() {
        return Err(err);
    }

    // Run optimization passes for AOT
    run_opt_passes(ctx, opt_level)?;

    Ok(())
}
