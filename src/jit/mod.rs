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

use compiler::JitCompiler;
use context::JitContext;

/// Runtime options for JIT execution.
#[derive(Default)]
pub struct JitOptions {
    pub no_gc: bool,
    pub gc_threshold: Option<usize>,
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
    let mut compiler = JitCompiler::new()?;
    let main_id = compiler.compile_program(program, interner)?;
    compiler.finalize();

    let main_ptr = compiler.get_func_ptr(main_id);

    let mut ctx = JitContext::new();
    ctx.set_jit_functions(compiler.jit_function_entries());
    ctx.set_named_functions(compiler.named_functions());

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

/// Execute a previously compiled JIT program.
pub fn jit_execute(mut compiled: JitCompiledProgram) -> Result<(Value, JitContext), String> {
    let result_ptr: *mut Value = unsafe {
        let func: unsafe extern "C" fn(*mut JitContext) -> *mut Value =
            std::mem::transmute(compiled.main_ptr);
        func(&mut compiled.ctx as *mut JitContext)
    };

    if result_ptr.is_null() {
        return Err(compiled
            .ctx
            .take_error()
            .unwrap_or_else(|| "unknown JIT error".to_string()));
    }

    let result = unsafe { (*result_ptr).clone() };
    Ok((result, compiled.ctx))
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
