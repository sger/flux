//! LLVM backend for Flux.
//!
//! Compiles Flux programs through Flux Core and backend IR to native machine
//! code via LLVM. Sits alongside the bytecode VM and Cranelift JIT, and is
//! activated with `--llvm`.

pub mod compiler;
pub mod context;
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub mod wrapper;

use crate::bytecode::compiler::Compiler;
use crate::cfg::{IrPassContext, lower_program_to_ir, run_ir_pass_pipeline};
use crate::diagnostics::Diagnostic;
use crate::runtime::native_context::{JIT_TAG_PTR, JIT_TAG_THUNK, JitContext, JitTaggedValue};
use crate::runtime::value::Value;
use crate::syntax::{interner::Interner, program::Program};

use context::LlvmCompilerContext;
use std::fmt;

#[derive(Debug, Clone)]
pub enum LlvmError {
    Compile(Box<Diagnostic>),
    Runtime(Box<Diagnostic>),
    Internal(String),
}

impl fmt::Display for LlvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(diag) | Self::Runtime(diag) => {
                let rendered = crate::diagnostics::render_diagnostics(
                    std::slice::from_ref(diag.as_ref()),
                    None,
                    None,
                );
                write!(f, "{}", rendered)
            }
            Self::Internal(message) => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for LlvmError {}

pub type LlvmResult<T> = Result<T, LlvmError>;

/// Runtime options for LLVM execution.
#[derive(Default)]
pub struct LlvmOptions {
    pub source_file: Option<String>,
    pub source_text: Option<String>,
    /// LLVM optimization level: 0 (none), 1 (basic), 2 (default), 3 (aggressive).
    pub opt_level: u32,
}

/// Compiled LLVM program ready to execute.
/// Keeps the `LlvmCompilerContext` alive so the code memory backing `main_ptr` is valid.
pub struct LlvmCompiledProgram {
    _context: LlvmCompilerContext,
    main_ptr: *const u8,
    pub ctx: JitContext,
}

// SAFETY: The function pointer points into memory owned by `_context` (the LLVM
// execution engine), which travels with this struct. Single-threaded use only.
unsafe impl Send for LlvmCompiledProgram {}

/// Compile a Flux program to native code via LLVM. Does not execute.
pub fn llvm_compile(
    program: &Program,
    interner: &Interner,
    options: &LlvmOptions,
) -> LlvmResult<LlvmCompiledProgram> {
    let mut hm_compiler = Compiler::new_with_interner(
        options.source_file.clone().unwrap_or_default(),
        interner.clone(),
    );
    let hm_expr_types = hm_compiler.infer_expr_types_for_program(program);
    let mut ir_program = lower_program_to_ir(program, &hm_expr_types)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;
    run_ir_pass_pipeline(&mut ir_program, &IrPassContext)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;

    let mut llvm_ctx = LlvmCompilerContext::new();

    compiler::compile_program(&mut llvm_ctx, &ir_program, interner, options.opt_level)
        .map_err(LlvmError::Internal)?;

    let main_ptr = llvm_ctx
        .get_function_address("__flux_entry")
        .ok_or_else(|| LlvmError::Internal("__flux_entry not found".to_string()))?
        as *const u8;

    let mut jit_ctx = JitContext::new();
    jit_ctx.set_source_context(options.source_file.clone(), options.source_text.clone());

    // Register user function entries so rt_call_jit_function can dispatch
    let mut entries = Vec::new();
    for (idx, func) in ir_program.functions.iter().enumerate() {
        let fn_addr = llvm_ctx
            .get_function_address(&format!(
                "flux_{}",
                func.name
                    .map(|n| interner.resolve(n).to_string())
                    .unwrap_or_else(|| format!("anon_{}", idx))
            ))
            .unwrap_or(0) as *const u8;

        let explicit_arity = func.params.len().saturating_sub(func.captures.len());
        let contract = crate::runtime::function_contract::runtime_contract_from_annotations(
            &func.parameter_types,
            &func.return_type_annotation,
            &func.effects,
            interner,
        );
        entries.push(crate::runtime::native_context::JitFunctionEntry {
            ptr: fn_addr,
            num_params: explicit_arity,
            call_abi: crate::runtime::native_context::JitCallAbi::Array,
            contract,
            return_span: func.return_type_annotation.as_ref().map(|_| func.body_span),
        });
    }

    // Register the identity function (used as `resume` in effect handlers)
    let identity_fn_index = entries.len();
    let identity_addr = llvm_ctx
        .get_function_address("__flux_identity")
        .unwrap_or(0) as *const u8;
    entries.push(crate::runtime::native_context::JitFunctionEntry {
        ptr: identity_addr,
        num_params: 1,
        call_abi: crate::runtime::native_context::JitCallAbi::Array,
        contract: None,
        return_span: None,
    });
    jit_ctx.set_jit_functions(entries);
    jit_ctx.identity_fn_index = identity_fn_index;

    Ok(LlvmCompiledProgram {
        _context: llvm_ctx,
        main_ptr,
        ctx: jit_ctx,
    })
}

/// Execute a previously compiled LLVM program.
pub fn llvm_execute(mut compiled: LlvmCompiledProgram) -> LlvmResult<(Value, JitContext)> {
    let mut result: JitTaggedValue = unsafe {
        let func: unsafe extern "C" fn(*mut JitContext) -> JitTaggedValue =
            std::mem::transmute(compiled.main_ptr);
        func(&mut compiled.ctx as *mut JitContext)
    };

    // Trampoline: re-invoke while the callee requests a mutual tail call.
    while result.tag == JIT_TAG_THUNK {
        let thunk = compiled.ctx.pending_thunk.take().ok_or_else(|| {
            LlvmError::Internal("JIT_TAG_THUNK returned without pending_thunk".to_string())
        })?;
        result =
            unsafe { crate::runtime::native_context::invoke_jit_thunk(&mut compiled.ctx, &thunk) };
    }

    if result.tag == JIT_TAG_PTR && result.as_ptr().is_null() {
        if let Some(diag) = compiled.ctx.take_runtime_error() {
            return Err(LlvmError::Runtime(Box::new(diag)));
        }
        return Err(LlvmError::Internal(
            compiled
                .ctx
                .take_internal_error()
                .unwrap_or_else(|| "unknown LLVM error".to_string()),
        ));
    }

    // Check for runtime errors even when the result tag is not null-ptr
    // (e.g. rt_add returns None on type mismatch but sets ctx.error)
    if let Some(diag) = compiled.ctx.take_runtime_error() {
        return Err(LlvmError::Runtime(Box::new(diag)));
    }
    if let Some(err) = compiled.ctx.take_internal_error() {
        return Err(LlvmError::Internal(err));
    }

    let result = compiled
        .ctx
        .clone_from_tagged(result)
        .ok_or_else(|| LlvmError::Internal("unknown LLVM error".to_string()))?;
    Ok((result, compiled.ctx))
}

/// Convenience: compile and execute in one call.
pub fn llvm_compile_and_run(
    program: &Program,
    interner: &Interner,
    options: &LlvmOptions,
) -> LlvmResult<(Value, JitContext)> {
    let compiled = llvm_compile(program, interner, options)?;
    llvm_execute(compiled)
}

/// Compile a Flux program and emit an object file (AOT compilation).
pub fn llvm_emit_object(
    program: &Program,
    interner: &Interner,
    options: &LlvmOptions,
    output_path: &str,
    opt_level: u32,
) -> LlvmResult<()> {
    let mut hm_compiler = Compiler::new_with_interner(
        options.source_file.clone().unwrap_or_default(),
        interner.clone(),
    );
    let hm_expr_types = hm_compiler.infer_expr_types_for_program(program);
    let mut ir_program = lower_program_to_ir(program, &hm_expr_types)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;
    run_ir_pass_pipeline(&mut ir_program, &IrPassContext)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;

    let mut llvm_ctx = LlvmCompilerContext::new();

    // Compile to LLVM IR (without creating execution engine)
    compiler::compile_program_ir_only(&mut llvm_ctx, &ir_program, interner, opt_level)
        .map_err(LlvmError::Internal)?;

    // Emit to object file
    llvm_ctx
        .emit_object_file(output_path, opt_level)
        .map_err(LlvmError::Internal)
}

/// Compile a Flux program and emit an assembly file.
pub fn llvm_emit_asm(
    program: &Program,
    interner: &Interner,
    options: &LlvmOptions,
    output_path: &str,
    opt_level: u32,
) -> LlvmResult<()> {
    let mut hm_compiler = Compiler::new_with_interner(
        options.source_file.clone().unwrap_or_default(),
        interner.clone(),
    );
    let hm_expr_types = hm_compiler.infer_expr_types_for_program(program);
    let mut ir_program = lower_program_to_ir(program, &hm_expr_types)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;
    run_ir_pass_pipeline(&mut ir_program, &IrPassContext)
        .map_err(|diag| LlvmError::Compile(Box::new(diag)))?;

    let mut llvm_ctx = LlvmCompilerContext::new();

    compiler::compile_program_ir_only(&mut llvm_ctx, &ir_program, interner, opt_level)
        .map_err(LlvmError::Internal)?;

    llvm_ctx
        .emit_asm_file(output_path, opt_level)
        .map_err(LlvmError::Internal)
}
