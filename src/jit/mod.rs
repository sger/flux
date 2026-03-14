//! Cranelift JIT backend for Flux.
//!
//! Compiles Flux AST directly to native machine code via Cranelift IR.
//! Sits alongside the bytecode VM and is activated with `--jit`.

pub mod compiler;
pub mod context;
pub mod runtime_helpers;
pub mod value_arena;

use crate::runtime::value::Value;
use crate::syntax::{interner::Interner, program::Program};
use crate::bytecode::compiler::Compiler;
use crate::cfg::{IrPassContext, lower_program_to_ir, patch_function_ids_from_core, run_ir_pass_pipeline};

use compiler::JitCompiler;
use context::{JitCallAbi, JitContext, JitTaggedValue, JitThunk, JIT_TAG_PTR, JIT_TAG_THUNK};

/// Runtime options for JIT execution.
#[derive(Default)]
pub struct JitOptions {
    pub no_gc: bool,
    pub gc_threshold: Option<usize>,
    pub source_file: Option<String>,
    pub source_text: Option<String>,
}

/// Compiled JIT program ready to execute.
/// Keeps the `JitCompiler` alive so the code memory backing `main_ptr` is valid.
pub struct JitCompiledProgram {
    _compiler: JitCompiler,
    main_ptr: *const u8,
    pub ctx: JitContext,
}

// SAFETY: The function pointer points into memory owned by `_compiler` (the
// JITModule), which travels with this struct. Single-threaded use only.
unsafe impl Send for JitCompiledProgram {}

/// Compile a Flux program to native code via Cranelift. Does not execute.
/// Returns a `JitCompiledProgram` ready to pass to `jit_execute`.
pub fn jit_compile(
    program: &Program,
    interner: &Interner,
    options: &JitOptions,
) -> Result<JitCompiledProgram, String> {
    let mut hm_compiler = Compiler::new_with_interner(
        options.source_file.clone().unwrap_or_default(),
        interner.clone(),
    );
    let hm_expr_types = hm_compiler.infer_expr_types_for_program(program);

    // ── Core IR pipeline (primary) ───────────────────────────────────────────
    let mut core = crate::nary::lower_ast::lower_program_ast(program, &hm_expr_types);
    crate::nary::passes::run_core_passes(&mut core);
    let core_ir = crate::nary::to_ir::lower_core_to_ir(&core);

    // ── Structured-IR pipeline (metadata) ────────────────────────────────────
    // Needed by JIT infrastructure: imports, ADT definitions, literal function
    // specs, base directives, contracts, and structured bodies for fallback.
    let mut ir_program =
        lower_program_to_ir(program, &hm_expr_types).map_err(diagnostic_to_string)?;

    // Route function CFG through Core IR.
    let old_functions = std::mem::take(&mut ir_program.functions);
    let kept_old_ids = patch_function_ids_from_core(
        &mut ir_program.top_level_items,
        &core_ir.top_level_items,
    );
    ir_program.functions = core_ir.functions;
    for old_id in kept_old_ids {
        if let Some(old_fn) = old_functions.iter().find(|f| f.id == old_id) {
            ir_program.functions.push(old_fn.clone());
        }
    }
    ir_program.core = Some(core);

    run_ir_pass_pipeline(&mut ir_program, &IrPassContext).map_err(diagnostic_to_string)?;

    let mut compiler = JitCompiler::new(hm_expr_types)?;
    let main_id = compiler.compile_ir_program(&ir_program, interner)?;
    compiler.finalize();

    let main_ptr = compiler.get_func_ptr(main_id);

    let mut ctx = JitContext::new();
    ctx.set_jit_functions(compiler.jit_function_entries());
    ctx.set_named_functions(compiler.named_functions());
    ctx.set_source_context(options.source_file.clone(), options.source_text.clone());
    ctx.identity_fn_index = compiler.identity_fn_index;

    if options.no_gc {
        ctx.gc_heap.set_enabled(false);
    }
    if let Some(threshold) = options.gc_threshold {
        ctx.gc_heap.set_threshold(threshold);
    }

    Ok(JitCompiledProgram {
        _compiler: compiler,
        main_ptr,
        ctx,
    })
}

fn diagnostic_to_string(diag: crate::diagnostics::Diagnostic) -> String {
    match (diag.title().is_empty(), diag.message()) {
        (false, Some(message)) => format!("{}: {}", diag.title(), message),
        (false, None) => diag.title().to_string(),
        (true, Some(message)) => message.to_string(),
        (true, None) => "Flux IR pipeline failed".to_string(),
    }
}

/// Execute a previously compiled JIT program.
pub fn jit_execute(mut compiled: JitCompiledProgram) -> Result<(Value, JitContext), String> {
    let mut result: JitTaggedValue = unsafe {
        let func: unsafe extern "C" fn(*mut JitContext) -> JitTaggedValue =
            std::mem::transmute(compiled.main_ptr);
        func(&mut compiled.ctx as *mut JitContext)
    };

    // Trampoline: re-invoke while the callee requests a mutual tail call.
    // Each iteration unwinds the JIT call frame before re-entering the target,
    // so mutual recursion does not grow the native stack.
    while result.tag == JIT_TAG_THUNK {
        let thunk = compiled
            .ctx
            .pending_thunk
            .take()
            .ok_or_else(|| "JIT_TAG_THUNK returned without pending_thunk".to_string())?;
        result = unsafe { invoke_jit_thunk(&mut compiled.ctx, &thunk) };
    }

    if result.tag == JIT_TAG_PTR && result.as_ptr().is_null() {
        return Err(compiled
            .ctx
            .take_error()
            .unwrap_or_else(|| "unknown JIT error".to_string()));
    }

    let result = compiled
        .ctx
        .clone_from_tagged(result)
        .ok_or_else(|| "unknown JIT error".to_string())?;
    Ok((result, compiled.ctx))
}

/// Dispatch a single trampoline step: call a JIT function via the ABI stored
/// in its `JitFunctionEntry`, then return the raw tagged result for the
/// outer trampoline loop to inspect.
///
/// # Safety
/// The function pointer must be valid and the context must outlive the call.
unsafe fn invoke_jit_thunk(ctx: &mut JitContext, thunk: &JitThunk) -> JitTaggedValue {
    // Extract the function pointer and ABI before borrowing `ctx` mutably.
    let (fn_ptr, call_abi) = {
        let entry = &ctx.jit_functions[thunk.fn_index];
        (entry.ptr, entry.call_abi)
    };
    let ctx_ptr = ctx as *mut JitContext;
    let null_ptr: *mut Value = std::ptr::null_mut();
    let zero: i64 = 0;

    unsafe {
        match call_abi {
            JitCallAbi::Array => {
                type F = unsafe extern "C" fn(
                    *mut JitContext,
                    *const JitTaggedValue,
                    i64,
                    *mut Value,
                    i64,
                ) -> JitTaggedValue;
                let f: F = std::mem::transmute(fn_ptr);
                f(ctx_ptr, thunk.args.as_ptr(), thunk.args.len() as i64, null_ptr, zero)
            }
            JitCallAbi::Reg1 => {
                let (t0, p0) = thunk_arg(&thunk.args, 0);
                type F = unsafe extern "C" fn(
                    *mut JitContext,
                    i64,
                    i64,
                    *mut Value,
                    i64,
                ) -> JitTaggedValue;
                let f: F = std::mem::transmute(fn_ptr);
                f(ctx_ptr, t0, p0, null_ptr, zero)
            }
            JitCallAbi::Reg2 => {
                let (t0, p0) = thunk_arg(&thunk.args, 0);
                let (t1, p1) = thunk_arg(&thunk.args, 1);
                type F = unsafe extern "C" fn(
                    *mut JitContext,
                    i64,
                    i64,
                    i64,
                    i64,
                    *mut Value,
                    i64,
                ) -> JitTaggedValue;
                let f: F = std::mem::transmute(fn_ptr);
                f(ctx_ptr, t0, p0, t1, p1, null_ptr, zero)
            }
            JitCallAbi::Reg3 => {
                let (t0, p0) = thunk_arg(&thunk.args, 0);
                let (t1, p1) = thunk_arg(&thunk.args, 1);
                let (t2, p2) = thunk_arg(&thunk.args, 2);
                type F = unsafe extern "C" fn(
                    *mut JitContext,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    *mut Value,
                    i64,
                ) -> JitTaggedValue;
                let f: F = std::mem::transmute(fn_ptr);
                f(ctx_ptr, t0, p0, t1, p1, t2, p2, null_ptr, zero)
            }
            JitCallAbi::Reg4 => {
                let (t0, p0) = thunk_arg(&thunk.args, 0);
                let (t1, p1) = thunk_arg(&thunk.args, 1);
                let (t2, p2) = thunk_arg(&thunk.args, 2);
                let (t3, p3) = thunk_arg(&thunk.args, 3);
                type F = unsafe extern "C" fn(
                    *mut JitContext,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    i64,
                    *mut Value,
                    i64,
                ) -> JitTaggedValue;
                let f: F = std::mem::transmute(fn_ptr);
                f(ctx_ptr, t0, p0, t1, p1, t2, p2, t3, p3, null_ptr, zero)
            }
        }
    }
}

fn thunk_arg(args: &[JitTaggedValue], idx: usize) -> (i64, i64) {
    args.get(idx).map_or((0, 0), |a| (a.tag, a.payload))
}

/// High-level entry point: compile and run a Flux program via JIT.
/// Returns the result value and the JIT context (for telemetry/diagnostics).
pub fn jit_compile_and_run(
    program: &Program,
    interner: &Interner,
    options: &JitOptions,
) -> Result<(Value, JitContext), String> {
    let compiled = jit_compile(program, interner, options)?;
    jit_execute(compiled)
}
