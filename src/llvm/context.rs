//! LLVM compilation context holding the LLVM module, builder, cached types,
//! and declared helper/user function references.

use std::collections::HashMap;

use llvm_sys::prelude::*;

use super::wrapper::{self, LlvmBuilder, LlvmCtx, LlvmExecutionEngine, LlvmModule};

/// Holds all LLVM state needed during compilation and execution.
///
/// Field order matters for drop: the execution engine must be dropped before
/// the builder and context, because it holds references to their memory.
pub struct LlvmCompilerContext {
    // Drop order: engine first, then builder, then module, then context.
    pub(crate) engine: Option<LlvmExecutionEngine>,
    pub(crate) builder: LlvmBuilder,
    pub(crate) module: LlvmModule,
    pub(crate) llvm_ctx: LlvmCtx,

    // Cached LLVM types
    pub(crate) i64_type: LLVMTypeRef,
    pub(crate) ptr_type: LLVMTypeRef,
    /// `{i64, i64}` — matches `JitTaggedValue { tag, payload }`.
    pub(crate) tagged_value_type: LLVMTypeRef,
    pub(crate) void_type: LLVMTypeRef,

    /// Runtime helper functions declared as external symbols.
    /// Key is the helper name (e.g. `"rt_add"`), value is the LLVM function ref.
    pub(crate) helpers: HashMap<&'static str, (LLVMValueRef, LLVMTypeRef)>,

    /// User functions from the IR program (FunctionId index → LLVM function ref + type).
    pub(crate) functions: HashMap<usize, (LLVMValueRef, LLVMTypeRef)>,
}

impl LlvmCompilerContext {
    pub fn new() -> Self {
        wrapper::ensure_initialized();

        let llvm_ctx = LlvmCtx::new();
        let module = LlvmModule::new(&llvm_ctx, "flux_module");
        let builder = LlvmBuilder::new(&llvm_ctx);

        let i64_type = llvm_ctx.i64_type();
        let ptr_type = llvm_ctx.ptr_type();
        let tagged_value_type = llvm_ctx.struct_type(&[i64_type, i64_type], false);
        let void_type = llvm_ctx.void_type();

        // Set module target triple and data layout for the host upfront.
        // This is required for MCJIT and for optimization passes to work correctly.
        let triple = wrapper::get_default_target_triple();
        wrapper::set_module_target(&module, &triple);
        if let Ok(tm) = wrapper::LlvmTargetMachine::for_host(0) {
            wrapper::set_module_data_layout(&module, &tm.data_layout());
        }

        Self {
            engine: None,
            builder,
            module,
            llvm_ctx,
            i64_type,
            ptr_type,
            tagged_value_type,
            void_type,
            helpers: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    /// The standard user function type used by the Array ABI calling convention:
    /// `fn(ctx: ptr, args: ptr, nargs: i64, captures: ptr, ncaptures: i64) -> {i64, i64}`
    pub fn user_function_type(&self) -> LLVMTypeRef {
        wrapper::function_type(
            self.tagged_value_type,
            &[
                self.ptr_type, // ctx
                self.ptr_type, // args
                self.i64_type, // nargs
                self.ptr_type, // captures
                self.i64_type, // ncaptures
            ],
            false,
        )
    }

    /// The entry wrapper type: `fn(ctx: ptr) -> {i64, i64}`
    pub fn entry_function_type(&self) -> LLVMTypeRef {
        wrapper::function_type(self.tagged_value_type, &[self.ptr_type], false)
    }

    /// Finalize: create the execution engine and resolve runtime symbols.
    pub fn finalize(&mut self) -> Result<(), String> {
        let engine = LlvmExecutionEngine::new(&mut self.module)?;
        self.engine = Some(engine);
        Ok(())
    }

    /// Map a declared external function to its actual address in the process.
    pub fn resolve_symbol(&self, name: &str, addr: *const u8) {
        if let Some(engine) = &self.engine {
            if let Some((func_ref, _)) = self.helpers.get(name) {
                engine.add_global_mapping(*func_ref, addr);
            }
        }
    }

    /// Get the address of a compiled function by name.
    pub fn get_function_address(&self, name: &str) -> Option<u64> {
        self.engine.as_ref()?.get_function_address(name)
    }

    /// Finalize for AOT: set target triple/data layout and emit to object file.
    pub fn emit_object_file(&self, path: &str, opt_level: u32) -> Result<(), String> {
        let tm = wrapper::LlvmTargetMachine::for_host(opt_level)?;
        let triple = wrapper::get_default_target_triple();
        wrapper::set_module_target(&self.module, &triple);
        wrapper::set_module_data_layout(&self.module, &tm.data_layout());
        tm.emit_object_file(&self.module, path)
    }

    /// Emit assembly text file.
    pub fn emit_asm_file(&self, path: &str, opt_level: u32) -> Result<(), String> {
        let tm = wrapper::LlvmTargetMachine::for_host(opt_level)?;
        let triple = wrapper::get_default_target_triple();
        wrapper::set_module_target(&self.module, &triple);
        wrapper::set_module_data_layout(&self.module, &tm.data_layout());
        tm.emit_asm_file(&self.module, path)
    }
}
