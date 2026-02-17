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

/// High-level entry point: compile and run a Flux program via JIT.
pub fn jit_compile_and_run(program: &Program, interner: &Interner) -> Result<Value, String> {
    let mut compiler = JitCompiler::new()?;
    let main_id = compiler.compile_program(program, interner)?;
    compiler.finalize();

    let main_ptr = compiler.get_func_ptr(main_id);

    // Create JIT execution context
    let mut ctx = JitContext::new();

    // Call the compiled main function: fn(ctx: *mut JitContext) -> *mut Value
    let result_ptr: *mut Value = unsafe {
        let func: unsafe extern "C" fn(*mut JitContext) -> *mut Value =
            std::mem::transmute(main_ptr);
        func(&mut ctx as *mut JitContext)
    };

    // Check for errors
    if result_ptr.is_null() {
        return Err(ctx.take_error().unwrap_or_else(|| "unknown JIT error".to_string()));
    }

    // Clone the result out of the arena before it's dropped
    let result = unsafe { (*result_ptr).clone() };
    Ok(result)
}
