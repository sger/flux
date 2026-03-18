//! Thin safe wrapper over `llvm-sys` for the Flux LLVM backend.
//!
//! Only the ~30 LLVM C API functions needed by the compiler are wrapped here.
//! Owned LLVM resources implement `Drop` for automatic cleanup.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Once;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::execution_engine::*;
use llvm_sys::prelude::*;
use llvm_sys::target;

// ── Initialization ───────────────────────────────────────────────────────────

static INIT: Once = Once::new();

/// Initialize LLVM targets (called once per process).
pub fn ensure_initialized() {
    INIT.call_once(|| unsafe {
        target::LLVM_InitializeNativeTarget();
        target::LLVM_InitializeNativeAsmPrinter();
        target::LLVM_InitializeNativeAsmParser();
        LLVMLinkInMCJIT();
    });
}

// ── Owned wrappers ───────────────────────────────────────────────────────────

/// Owned LLVM context. Drop calls `LLVMContextDispose`.
pub struct LlvmCtx {
    raw: LLVMContextRef,
}

impl LlvmCtx {
    pub fn new() -> Self {
        Self {
            raw: unsafe { LLVMContextCreate() },
        }
    }

    pub fn raw(&self) -> LLVMContextRef {
        self.raw
    }

    // ── Type helpers ─────────────────────────────────────────────────────

    pub fn i64_type(&self) -> LLVMTypeRef {
        unsafe { LLVMInt64TypeInContext(self.raw) }
    }

    pub fn i1_type(&self) -> LLVMTypeRef {
        unsafe { LLVMInt1TypeInContext(self.raw) }
    }

    pub fn ptr_type(&self) -> LLVMTypeRef {
        // Opaque pointer (LLVM 15+).
        unsafe { LLVMPointerTypeInContext(self.raw, 0) }
    }

    pub fn void_type(&self) -> LLVMTypeRef {
        unsafe { LLVMVoidTypeInContext(self.raw) }
    }

    pub fn struct_type(&self, fields: &[LLVMTypeRef], packed: bool) -> LLVMTypeRef {
        unsafe {
            LLVMStructTypeInContext(
                self.raw,
                fields.as_ptr() as *mut _,
                fields.len() as u32,
                packed as LLVMBool,
            )
        }
    }

    // ── Block creation ───────────────────────────────────────────────────

    pub fn append_basic_block(&self, func: LLVMValueRef, name: &str) -> LLVMBasicBlockRef {
        let c_name = CString::new(name).unwrap();
        unsafe { LLVMAppendBasicBlockInContext(self.raw, func, c_name.as_ptr()) }
    }
}

impl Drop for LlvmCtx {
    fn drop(&mut self) {
        unsafe { LLVMContextDispose(self.raw) };
    }
}

/// Owned LLVM module. Ownership may be transferred to an execution engine.
pub struct LlvmModule {
    raw: LLVMModuleRef,
    owned: bool,
}

impl LlvmModule {
    pub fn new(ctx: &LlvmCtx, name: &str) -> Self {
        let c_name = CString::new(name).unwrap();
        Self {
            raw: unsafe { LLVMModuleCreateWithNameInContext(c_name.as_ptr(), ctx.raw()) },
            owned: true,
        }
    }

    pub fn raw(&self) -> LLVMModuleRef {
        self.raw
    }

    /// Transfer ownership of the module to an execution engine.
    /// After this call the module must not be disposed separately.
    pub(crate) fn take_raw(&mut self) -> LLVMModuleRef {
        self.owned = false;
        self.raw
    }

    // ── Function creation ────────────────────────────────────────────────

    pub fn add_function(&self, name: &str, fn_type: LLVMTypeRef) -> LLVMValueRef {
        let c_name = CString::new(name).unwrap();
        unsafe { LLVMAddFunction(self.raw, c_name.as_ptr(), fn_type) }
    }

    // ── Verification ─────────────────────────────────────────────────────

    pub fn verify(&self) -> Result<(), String> {
        let mut err_msg: *mut i8 = ptr::null_mut();
        let failed = unsafe {
            LLVMVerifyModule(
                self.raw,
                LLVMVerifierFailureAction::LLVMReturnStatusAction,
                &mut err_msg,
            )
        };
        if failed != 0 {
            let msg = if err_msg.is_null() {
                "unknown verification error".to_string()
            } else {
                let s = unsafe { CStr::from_ptr(err_msg) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { LLVMDisposeMessage(err_msg) };
                s
            };
            Err(msg)
        } else {
            if !err_msg.is_null() {
                unsafe { LLVMDisposeMessage(err_msg) };
            }
            Ok(())
        }
    }

    /// Dump the LLVM IR as a string (for debugging).
    #[allow(dead_code)]
    pub fn dump_to_string(&self) -> String {
        let raw = unsafe { LLVMPrintModuleToString(self.raw) };
        let s = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { LLVMDisposeMessage(raw) };
        s
    }
}

impl Drop for LlvmModule {
    fn drop(&mut self) {
        if self.owned {
            unsafe { LLVMDisposeModule(self.raw) };
        }
    }
}

/// Owned LLVM IR builder.
pub struct LlvmBuilder {
    raw: LLVMBuilderRef,
}

impl LlvmBuilder {
    pub fn new(ctx: &LlvmCtx) -> Self {
        Self {
            raw: unsafe { LLVMCreateBuilderInContext(ctx.raw()) },
        }
    }

    /// Raw pointer for direct llvm-sys calls not yet wrapped.
    pub fn raw_ptr(&self) -> LLVMBuilderRef {
        self.raw
    }

    /// Get the current insertion block.
    pub fn get_insert_block(&self) -> LLVMBasicBlockRef {
        unsafe { LLVMGetInsertBlock(self.raw) }
    }

    pub fn position_at_end(&self, block: LLVMBasicBlockRef) {
        unsafe { LLVMPositionBuilderAtEnd(self.raw, block) };
    }

    // ── Terminators ──────────────────────────────────────────────────────

    pub fn build_ret(&self, value: LLVMValueRef) -> LLVMValueRef {
        unsafe { LLVMBuildRet(self.raw, value) }
    }

    pub fn build_br(&self, dest: LLVMBasicBlockRef) -> LLVMValueRef {
        unsafe { LLVMBuildBr(self.raw, dest) }
    }

    pub fn build_cond_br(
        &self,
        cond: LLVMValueRef,
        then_block: LLVMBasicBlockRef,
        else_block: LLVMBasicBlockRef,
    ) -> LLVMValueRef {
        unsafe { LLVMBuildCondBr(self.raw, cond, then_block, else_block) }
    }

    // ── Integer arithmetic ───────────────────────────────────────────────

    pub fn build_add(&self, lhs: LLVMValueRef, rhs: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildAdd(self.raw, lhs, rhs, c.as_ptr()) }
    }

    pub fn build_sub(&self, lhs: LLVMValueRef, rhs: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildSub(self.raw, lhs, rhs, c.as_ptr()) }
    }

    pub fn build_mul(&self, lhs: LLVMValueRef, rhs: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildMul(self.raw, lhs, rhs, c.as_ptr()) }
    }

    pub fn build_sdiv(&self, lhs: LLVMValueRef, rhs: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildSDiv(self.raw, lhs, rhs, c.as_ptr()) }
    }

    pub fn build_srem(&self, lhs: LLVMValueRef, rhs: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildSRem(self.raw, lhs, rhs, c.as_ptr()) }
    }

    // ── Comparisons ──────────────────────────────────────────────────────

    pub fn build_icmp(
        &self,
        op: llvm_sys::LLVMIntPredicate,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
        name: &str,
    ) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildICmp(self.raw, op, lhs, rhs, c.as_ptr()) }
    }

    // ── Struct / aggregate ───────────────────────────────────────────────

    pub fn build_extract_value(&self, agg: LLVMValueRef, index: u32, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildExtractValue(self.raw, agg, index, c.as_ptr()) }
    }

    pub fn build_insert_value(
        &self,
        agg: LLVMValueRef,
        val: LLVMValueRef,
        index: u32,
        name: &str,
    ) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildInsertValue(self.raw, agg, val, index, c.as_ptr()) }
    }

    // ── Memory ───────────────────────────────────────────────────────────

    pub fn build_alloca(&self, ty: LLVMTypeRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildAlloca(self.raw, ty, c.as_ptr()) }
    }

    pub fn build_store(&self, val: LLVMValueRef, ptr: LLVMValueRef) -> LLVMValueRef {
        unsafe { LLVMBuildStore(self.raw, val, ptr) }
    }

    pub fn build_load(&self, ty: LLVMTypeRef, ptr: LLVMValueRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildLoad2(self.raw, ty, ptr, c.as_ptr()) }
    }

    // ── Calls ────────────────────────────────────────────────────────────

    pub fn build_call(
        &self,
        fn_type: LLVMTypeRef,
        func: LLVMValueRef,
        args: &mut [LLVMValueRef],
        name: &str,
    ) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe {
            LLVMBuildCall2(
                self.raw,
                fn_type,
                func,
                args.as_mut_ptr(),
                args.len() as u32,
                c.as_ptr(),
            )
        }
    }

    // ── Phi ──────────────────────────────────────────────────────────────

    pub fn build_phi(&self, ty: LLVMTypeRef, name: &str) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildPhi(self.raw, ty, c.as_ptr()) }
    }

    // ── Casts ────────────────────────────────────────────────────────────

    pub fn build_int_to_ptr(
        &self,
        val: LLVMValueRef,
        dest_ty: LLVMTypeRef,
        name: &str,
    ) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildIntToPtr(self.raw, val, dest_ty, c.as_ptr()) }
    }

    pub fn build_ptr_to_int(
        &self,
        val: LLVMValueRef,
        dest_ty: LLVMTypeRef,
        name: &str,
    ) -> LLVMValueRef {
        let c = CString::new(name).unwrap();
        unsafe { LLVMBuildPtrToInt(self.raw, val, dest_ty, c.as_ptr()) }
    }
}

impl Drop for LlvmBuilder {
    fn drop(&mut self) {
        unsafe { LLVMDisposeBuilder(self.raw) };
    }
}

/// Owned LLVM execution engine (MCJIT).
pub struct LlvmExecutionEngine {
    raw: LLVMExecutionEngineRef,
}

impl LlvmExecutionEngine {
    /// Create an execution engine from a module. Takes ownership of the module.
    pub fn new(module: &mut LlvmModule) -> Result<Self, String> {
        let mut engine: LLVMExecutionEngineRef = ptr::null_mut();
        let mut err_msg: *mut i8 = ptr::null_mut();
        let failed = unsafe {
            LLVMCreateExecutionEngineForModule(&mut engine, module.take_raw(), &mut err_msg)
        };
        if failed != 0 {
            let msg = if err_msg.is_null() {
                "failed to create execution engine".to_string()
            } else {
                let s = unsafe { CStr::from_ptr(err_msg) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { LLVMDisposeMessage(err_msg) };
                s
            };
            Err(msg)
        } else {
            Ok(Self { raw: engine })
        }
    }

    /// Map a symbol name to a function pointer in the current process.
    pub fn add_global_mapping(&self, value: LLVMValueRef, addr: *const u8) {
        unsafe {
            LLVMAddGlobalMapping(self.raw, value, addr as *mut _);
        }
    }

    /// Get the address of a compiled function by name.
    pub fn get_function_address(&self, name: &str) -> Option<u64> {
        let c_name = CString::new(name).unwrap();
        let addr = unsafe { LLVMGetFunctionAddress(self.raw, c_name.as_ptr()) };
        if addr == 0 { None } else { Some(addr) }
    }
}

impl Drop for LlvmExecutionEngine {
    fn drop(&mut self) {
        unsafe { LLVMDisposeExecutionEngine(self.raw) };
    }
}

// ── Utility functions ────────────────────────────────────────────────────────

/// Create an LLVM function type.
pub fn function_type(ret: LLVMTypeRef, params: &[LLVMTypeRef], is_var_arg: bool) -> LLVMTypeRef {
    unsafe {
        LLVMFunctionType(
            ret,
            params.as_ptr() as *mut _,
            params.len() as u32,
            is_var_arg as LLVMBool,
        )
    }
}

/// Create an i64 constant.
pub fn const_i64(ty: LLVMTypeRef, value: i64) -> LLVMValueRef {
    unsafe { LLVMConstInt(ty, value as u64, 1) } // 1 = sign-extend
}

/// Get a function parameter by index.
pub fn get_param(func: LLVMValueRef, index: u32) -> LLVMValueRef {
    unsafe { LLVMGetParam(func, index) }
}

/// Add incoming edges to a phi node.
pub fn add_incoming(phi: LLVMValueRef, values: &[LLVMValueRef], blocks: &[LLVMBasicBlockRef]) {
    debug_assert_eq!(values.len(), blocks.len());
    unsafe {
        LLVMAddIncoming(
            phi,
            values.as_ptr() as *mut _,
            blocks.as_ptr() as *mut _,
            values.len() as u32,
        );
    }
}

/// Get the undef value for a type.
pub fn get_undef(ty: LLVMTypeRef) -> LLVMValueRef {
    unsafe { LLVMGetUndef(ty) }
}

/// Create a global constant string in the module and return a pointer to it.
pub fn create_global_string(
    module: &LlvmModule,
    ctx: &LlvmCtx,
    name: &str,
    data: &[u8],
) -> LLVMValueRef {
    let array_ty = unsafe {
        llvm_sys::core::LLVMArrayType2(LLVMInt8TypeInContext(ctx.raw()), data.len() as u64)
    };
    let c_name = CString::new(name).unwrap();
    let global = unsafe { LLVMAddGlobal(module.raw(), array_ty, c_name.as_ptr()) };
    let init = unsafe {
        llvm_sys::core::LLVMConstStringInContext(
            ctx.raw(),
            data.as_ptr() as *const _,
            data.len() as u32,
            1, // don't null-terminate
        )
    };
    unsafe {
        LLVMSetInitializer(global, init);
        LLVMSetGlobalConstant(global, 1);
        llvm_sys::core::LLVMSetLinkage(global, llvm_sys::LLVMLinkage::LLVMPrivateLinkage);
    }
    global
}

/// Create a constant null pointer.
pub fn const_null(ty: LLVMTypeRef) -> LLVMValueRef {
    unsafe { LLVMConstNull(ty) }
}

// ── Target machine and AOT emission ─────────────────────────────────────────

/// Get the default target triple for the host.
pub fn get_default_target_triple() -> String {
    let raw = unsafe { llvm_sys::target_machine::LLVMGetDefaultTargetTriple() };
    let s = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    unsafe { LLVMDisposeMessage(raw) };
    s
}

/// Look up an LLVM target by triple.
pub fn get_target_from_triple(
    triple: &str,
) -> Result<llvm_sys::target_machine::LLVMTargetRef, String> {
    let c_triple = CString::new(triple).unwrap();
    let mut target_ref: llvm_sys::target_machine::LLVMTargetRef = ptr::null_mut();
    let mut err_msg: *mut i8 = ptr::null_mut();
    let failed = unsafe {
        llvm_sys::target_machine::LLVMGetTargetFromTriple(
            c_triple.as_ptr(),
            &mut target_ref,
            &mut err_msg,
        )
    };
    if failed != 0 {
        let msg = if err_msg.is_null() {
            "unknown target".to_string()
        } else {
            let s = unsafe { CStr::from_ptr(err_msg) }
                .to_string_lossy()
                .into_owned();
            unsafe { LLVMDisposeMessage(err_msg) };
            s
        };
        Err(msg)
    } else {
        Ok(target_ref)
    }
}

/// Owned LLVM target machine for code generation.
pub struct LlvmTargetMachine {
    raw: llvm_sys::target_machine::LLVMTargetMachineRef,
}

impl LlvmTargetMachine {
    /// Create a target machine for the host.
    pub fn for_host(opt_level: u32) -> Result<Self, String> {
        use llvm_sys::target_machine::*;

        let triple = get_default_target_triple();
        let target = get_target_from_triple(&triple)?;
        let c_triple = CString::new(triple).unwrap();
        let c_cpu = CString::new("generic").unwrap();
        let c_features = CString::new("").unwrap();

        let level = match opt_level {
            0 => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
            1 => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
            2 => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
            _ => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
        };

        let raw = unsafe {
            LLVMCreateTargetMachine(
                target,
                c_triple.as_ptr(),
                c_cpu.as_ptr(),
                c_features.as_ptr(),
                level,
                LLVMRelocMode::LLVMRelocPIC,
                LLVMCodeModel::LLVMCodeModelDefault,
            )
        };
        if raw.is_null() {
            Err("failed to create target machine".to_string())
        } else {
            Ok(Self { raw })
        }
    }

    /// Get the data layout string for this target.
    pub fn data_layout(&self) -> String {
        unsafe {
            let td = llvm_sys::target_machine::LLVMCreateTargetDataLayout(self.raw);
            let raw = llvm_sys::target::LLVMCopyStringRepOfTargetData(td);
            let s = CStr::from_ptr(raw).to_string_lossy().into_owned();
            LLVMDisposeMessage(raw);
            llvm_sys::target::LLVMDisposeTargetData(td);
            s
        }
    }

    /// Emit the module to an object file.
    pub fn emit_object_file(&self, module: &LlvmModule, path: &str) -> Result<(), String> {
        self.emit_to_file(
            module,
            path,
            llvm_sys::target_machine::LLVMCodeGenFileType::LLVMObjectFile,
        )
    }

    /// Emit the module to an assembly file.
    pub fn emit_asm_file(&self, module: &LlvmModule, path: &str) -> Result<(), String> {
        self.emit_to_file(
            module,
            path,
            llvm_sys::target_machine::LLVMCodeGenFileType::LLVMAssemblyFile,
        )
    }

    fn emit_to_file(
        &self,
        module: &LlvmModule,
        path: &str,
        file_type: llvm_sys::target_machine::LLVMCodeGenFileType,
    ) -> Result<(), String> {
        let c_path = CString::new(path).unwrap();
        let mut err_msg: *mut i8 = ptr::null_mut();
        let failed = unsafe {
            llvm_sys::target_machine::LLVMTargetMachineEmitToFile(
                self.raw,
                module.raw(),
                c_path.as_ptr() as *mut _,
                file_type,
                &mut err_msg,
            )
        };
        if failed != 0 {
            let msg = if err_msg.is_null() {
                "unknown emission error".to_string()
            } else {
                let s = unsafe { CStr::from_ptr(err_msg) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { LLVMDisposeMessage(err_msg) };
                s
            };
            Err(msg)
        } else {
            Ok(())
        }
    }
}

impl Drop for LlvmTargetMachine {
    fn drop(&mut self) {
        unsafe { llvm_sys::target_machine::LLVMDisposeTargetMachine(self.raw) };
    }
}

/// Set the target triple on a module.
pub fn set_module_target(module: &LlvmModule, triple: &str) {
    let c_triple = CString::new(triple).unwrap();
    unsafe { LLVMSetTarget(module.raw(), c_triple.as_ptr()) };
}

/// Set the data layout on a module.
pub fn set_module_data_layout(module: &LlvmModule, layout: &str) {
    let c_layout = CString::new(layout).unwrap();
    unsafe { LLVMSetDataLayout(module.raw(), c_layout.as_ptr()) };
}

/// Run the LLVM optimization pipeline on a module.
///
/// Uses the new pass builder API (LLVM 15+). Pass strings like
/// `"default<O0>"`, `"default<O1>"`, `"default<O2>"`, `"default<O3>"`.
pub fn run_optimization_passes(
    module: &LlvmModule,
    target_machine: &LlvmTargetMachine,
    passes: &str,
) -> Result<(), String> {
    use llvm_sys::error::LLVMGetErrorMessage;
    use llvm_sys::transforms::pass_builder::*;

    let c_passes = CString::new(passes).unwrap();
    let options = unsafe { LLVMCreatePassBuilderOptions() };
    let err =
        unsafe { LLVMRunPasses(module.raw(), c_passes.as_ptr(), target_machine.raw, options) };
    unsafe { LLVMDisposePassBuilderOptions(options) };
    if err.is_null() {
        Ok(())
    } else {
        let msg_ptr = unsafe { LLVMGetErrorMessage(err) };
        let msg = if msg_ptr.is_null() {
            "unknown pass error".to_string()
        } else {
            let s = unsafe { CStr::from_ptr(msg_ptr) }
                .to_string_lossy()
                .into_owned();
            unsafe { llvm_sys::error::LLVMDisposeErrorMessage(msg_ptr) };
            s
        };
        Err(msg)
    }
}

/// Function-level attribute index (applies to the function itself, not params/return).
pub const FUNCTION_INDEX: u32 = u32::MAX;

/// Add an LLVM enum attribute to a function at the given index.
/// Index: `FUNCTION_INDEX` for function-level, `0` for return value, `1+` for params.
pub fn add_function_attribute(
    ctx: &LlvmCtx,
    func: LLVMValueRef,
    attr_name: &str,
    index: u32,
) {
    let c_name = CString::new(attr_name).unwrap();
    unsafe {
        let kind = LLVMGetEnumAttributeKindForName(c_name.as_ptr(), attr_name.len());
        if kind > 0 {
            let attr = LLVMCreateEnumAttribute(ctx.raw(), kind, 0);
            LLVMAddAttributeAtIndex(func, index, attr);
        }
    }
}

/// Add an LLVM string attribute (e.g. `memory(read)`) to a function.
pub fn add_function_string_attribute(
    ctx: &LlvmCtx,
    func: LLVMValueRef,
    key: &str,
    value: &str,
    index: u32,
) {
    let c_key = CString::new(key).unwrap();
    let c_val = CString::new(value).unwrap();
    unsafe {
        let attr = LLVMCreateStringAttribute(
            ctx.raw(),
            c_key.as_ptr(),
            key.len() as u32,
            c_val.as_ptr(),
            value.len() as u32,
        );
        LLVMAddAttributeAtIndex(func, index, attr);
    }
}

// ── TBAA (Type-Based Alias Analysis) metadata ────────────────────────────────

/// Create a TBAA root node: `!{!"name"}`.
pub fn create_tbaa_root(ctx: &LlvmCtx, name: &str) -> LLVMValueRef {
    unsafe {
        let md_str = LLVMMDStringInContext2(ctx.raw(), name.as_ptr().cast(), name.len());
        let node = LLVMMDNodeInContext2(ctx.raw(), [md_str].as_mut_ptr(), 1);
        LLVMMetadataAsValue(ctx.raw(), node)
    }
}

/// Create a TBAA type node: `!{!"name", !parent}`.
pub fn create_tbaa_node(ctx: &LlvmCtx, name: &str, parent: LLVMValueRef) -> LLVMValueRef {
    unsafe {
        let md_str = LLVMMDStringInContext2(ctx.raw(), name.as_ptr().cast(), name.len());
        let parent_md = LLVMValueAsMetadata(parent);
        let node = LLVMMDNodeInContext2(ctx.raw(), [md_str, parent_md].as_mut_ptr(), 2);
        LLVMMetadataAsValue(ctx.raw(), node)
    }
}

/// Create a TBAA access tag: `!{!type_node, !type_node, i64 0}`.
/// Used to annotate individual load/store instructions.
pub fn create_tbaa_access_tag(ctx: &LlvmCtx, type_node: LLVMValueRef) -> LLVMValueRef {
    unsafe {
        let type_md = LLVMValueAsMetadata(type_node);
        let i64_ty = LLVMInt64TypeInContext(ctx.raw());
        let zero = LLVMConstInt(i64_ty, 0, 0);
        let zero_md = LLVMValueAsMetadata(zero);
        let node = LLVMMDNodeInContext2(ctx.raw(), [type_md, type_md, zero_md].as_mut_ptr(), 3);
        LLVMMetadataAsValue(ctx.raw(), node)
    }
}

/// Attach TBAA metadata to a load or store instruction.
pub fn set_tbaa(instr: LLVMValueRef, tbaa_tag: LLVMValueRef) {
    let kind = unsafe { LLVMGetMDKindID(c"tbaa".as_ptr(), 4) };
    unsafe { LLVMSetMetadata(instr, kind, tbaa_tag) };
}
