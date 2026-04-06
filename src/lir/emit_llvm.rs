//! LIR → LLVM IR emitter (Proposal 0132 Phase 7).
//!
//! Walks a `LirProgram` and produces LLVM IR text by building an `LlvmModule`
//! (reusing the existing IR representation from `core_to_llvm/ir/`) and
//! rendering it via `render_module`.
//!
//! Since LIR is already a flat CFG with SSA variables, the translation is
//! nearly 1:1: each `LirInstr` → one or more `LlvmInstr`, each
//! `LirTerminator` → an `LlvmTerminator`, each `LirBlock` → an `LlvmBlock`.

use std::collections::HashSet;

use crate::core::CorePrimOp;
use crate::core_to_llvm::codegen::builtins;
use crate::core_to_llvm::ir::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmDecl, LlvmFunction,
    LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator,
    LlvmType, LlvmValueKind, render_module,
};
use crate::lir::*;

// ── Pointer-tag constants (must match runtime/c/flux_rt.h and prelude.rs) ───

const FLUX_NONE: i64 = 0;
const FLUX_FALSE: i64 = 2;
const FLUX_TRUE: i64 = 4;
const FLUX_EMPTY_LIST: i64 = 6;
const FLUX_MIN_PTR: i64 = 12;
#[allow(dead_code)]
const FLUX_UNINIT: i64 = 8;
#[allow(dead_code)]
const FLUX_YIELD_SENTINEL: i64 = 10;

/// Object tag for floats in the heap header (must match FLUX_OBJ_FLOAT in flux_rt.h).
#[allow(dead_code)]
const FLUX_OBJ_FLOAT: i64 = 0xF8;

// ADT constructor tag IDs (must match lir/lower.rs and core_to_llvm/codegen/adt.rs)
const SOME_TAG: i32 = 1;
const LEFT_TAG: i32 = 2;
const RIGHT_TAG: i32 = 3;
const CONS_TAG: i32 = 4;

// ── Public entry point ──────────────────────────────────────────────────────

/// Emit an `LlvmModule` from a `LirProgram`.
/// The caller can inject target triple / data layout before rendering.
pub fn emit_llvm_module(program: &LirProgram) -> LlvmModule {
    emit_llvm_module_with_options(program, true, true)
}

/// Emit an `LlvmModule` from a `LirProgram` with configurable export of the
/// runtime-facing closure trampoline and user constructor-name helper.
pub fn emit_llvm_module_with_options(
    program: &LirProgram,
    export_runtime_trampoline: bool,
    export_user_ctor_name_helper: bool,
) -> LlvmModule {
    let mut module = LlvmModule {
        source_filename: Some("flux_lir".to_string()),
        target_triple: None,
        data_layout: None,
        type_defs: Vec::new(),
        globals: Vec::new(),
        declarations: Vec::new(),
        functions: Vec::new(),
    };

    // Emit prelude (pointer-tag helpers, dup/drop declarations, arithmetic).
    crate::core_to_llvm::codegen::emit_prelude_and_arith(&mut module);
    // Emit ADT/tuple/closure support types and helpers.
    let empty_metadata = crate::core_to_llvm::codegen::AdtMetadata::default();
    crate::core_to_llvm::codegen::emit_adt_support(&mut module, &empty_metadata);
    crate::core_to_llvm::codegen::emit_closure_support(&mut module);
    let support_function_count = module.functions.len();

    // Track which C runtime functions need external declarations.
    let mut needed_decls: HashSet<String> = HashSet::new();
    let mut needed_user_fastcc_decls: HashSet<(String, usize)> = HashSet::new();
    let mut needed_closure_entry_decls: HashSet<String> = HashSet::new();
    let mut string_globals: Vec<(GlobalId, String)> = Vec::new();

    // Build set of worker-eligible functions (Phase 10: Worker/Wrapper unboxing).
    let worker_eligible: HashSet<LirFuncId> = program
        .functions
        .iter()
        .filter(|f| is_worker_eligible(f))
        .map(|f| f.id)
        .collect();

    // Emit each LIR function.
    let mut closure_wrappers_needed: HashSet<LirFuncId> = HashSet::new();
    for func in &program.functions {
        let is_main = func.qualified_name == "main";
        let mut emitter = FnEmitter {
            program,
            func,
            is_main,
            next_tmp: 0,
            needed_decls: &mut needed_decls,
            needed_user_fastcc_decls: &mut needed_user_fastcc_decls,
            needed_closure_entry_decls: &mut needed_closure_entry_decls,
            string_globals: &mut string_globals,
            current_instrs: Vec::new(),
            extra_blocks: Vec::new(),
            closure_wrappers_needed: &mut closure_wrappers_needed,
            worker_mode: false,
            worker_eligible: &worker_eligible,
        };
        let llvm_func = emitter.emit_function();
        module.functions.push(llvm_func);
    }

    // Phase 10: Emit worker functions for eligible all-int functions.
    for func in &program.functions {
        if is_worker_eligible(func) {
            let mut emitter = FnEmitter {
                program,
                func,
                is_main: false,
                next_tmp: 0,
                needed_decls: &mut needed_decls,
                needed_user_fastcc_decls: &mut needed_user_fastcc_decls,
                needed_closure_entry_decls: &mut needed_closure_entry_decls,
                string_globals: &mut string_globals,
                current_instrs: Vec::new(),
                extra_blocks: Vec::new(),
                closure_wrappers_needed: &mut closure_wrappers_needed,
                worker_mode: true,
                worker_eligible: &worker_eligible,
            };
            let worker_func = emitter.emit_function();
            module.functions.push(worker_func);
        }
    }

    // Emit closure entry wrappers for direct functions used as higher-order values.
    // These thin wrappers convert (i64 closure_raw, ptr args, i32 nargs) → direct call.
    for func in &program.functions {
        if func.capture_vars.is_empty() && func.qualified_name != "main" {
            let wrapper = emit_closure_wrapper(func, Linkage::External);
            module.functions.push(wrapper);
        }
    }
    for &func_id in &closure_wrappers_needed {
        if let Some(target) = program.func_by_id(func_id)
            && !target.capture_vars.is_empty()
        {
            let wrapper = emit_closure_wrapper(target, Linkage::Internal);
            module.functions.push(wrapper);
        }
    }

    // Add string globals.
    for (gid, s) in &string_globals {
        module.globals.push(LlvmGlobal {
            linkage: Linkage::Private,
            name: gid.clone(),
            ty: LlvmType::Array {
                len: s.len() as u64,
                element: Box::new(LlvmType::i8()),
            },
            is_constant: true,
            value: Some(LlvmConst::Array {
                element_ty: LlvmType::i8(),
                elements: s
                    .bytes()
                    .map(|b| LlvmConst::Int {
                        bits: 8,
                        value: b as i128,
                    })
                    .collect(),
            }),
            attrs: Vec::new(),
        });
    }

    if export_user_ctor_name_helper {
        emit_user_ctor_name_helper(&mut module, program);
    }

    // Add external declarations for C runtime functions.
    for name in &needed_decls {
        if module.declarations.iter().any(|d| d.name.0 == *name)
            || module.functions.iter().any(|f| f.name.0 == *name)
        {
            continue;
        }
        // Known C runtime functions with fixed signatures (checked first
        // because builtins table may map to a different function name).
        if let Some(decl) = known_c_decl(name) {
            module.declarations.push(decl);
            continue;
        }
        // Try builtins table.
        let flux_name = name.strip_prefix("flux_").unwrap_or(name);
        if let Some(mapping) = builtins::find_builtin(flux_name) {
            builtins::ensure_builtin_declared(&mut module, mapping);
            continue;
        }
        // Generic fallback: all-i64 params based on common arities.
        let arity = guess_arity(name);
        module.declarations.push(LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId(name.clone()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: (0..arity).map(|_| LlvmType::i64()).collect(),
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".to_string()],
        });
    }

    for (name, arity) in &needed_user_fastcc_decls {
        if module.declarations.iter().any(|d| d.name.0 == *name)
            || module.functions.iter().any(|f| f.name.0 == *name)
        {
            continue;
        }
        module.declarations.push(LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId(name.clone()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: (0..*arity).map(|_| LlvmType::i64()).collect(),
                varargs: false,
                call_conv: CallConv::Fastcc,
            },
            attrs: vec!["nounwind".to_string()],
        });
    }

    for name in &needed_closure_entry_decls {
        if module.declarations.iter().any(|d| d.name.0 == *name)
            || module.functions.iter().any(|f| f.name.0 == *name)
        {
            continue;
        }
        module.declarations.push(LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId(name.clone()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: vec![LlvmType::i64(), LlvmType::Ptr, LlvmType::i32()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".to_string()],
        });
    }

    if !export_runtime_trampoline {
        for func in module.functions.iter_mut().take(support_function_count) {
            func.linkage = Linkage::Internal;
        }
    }

    module
}

fn emit_user_ctor_name_helper(module: &mut LlvmModule, program: &LirProgram) {
    let mut ctor_entries: Vec<(i32, String)> = program
        .constructor_tags
        .iter()
        .filter_map(|(name, &tag)| (tag > CONS_TAG).then_some((tag, name.clone())))
        .collect();
    ctor_entries.sort_by_key(|(tag, _)| *tag);

    if ctor_entries.is_empty() {
        return;
    }

    let helper_name = GlobalId("flux_user_ctor_name".to_string());
    if module.functions.iter().any(|func| func.name == helper_name) {
        return;
    }

    let tag_param = LlvmLocal("ctor_tag".to_string());
    let default_label = LabelId("ctor_name.default".to_string());
    let mut cases: Vec<(LlvmConst, LabelId)> = Vec::new();
    let mut blocks: Vec<LlvmBlock> = Vec::new();

    for (index, (tag, name)) in ctor_entries.iter().enumerate() {
        let bytes = {
            let mut raw = name.clone().into_bytes();
            raw.push(0);
            raw
        };
        let global = GlobalId(format!("flux_ctor_name.{index}"));
        module.globals.push(LlvmGlobal {
            linkage: Linkage::Private,
            name: global.clone(),
            ty: LlvmType::Array {
                len: bytes.len() as u64,
                element: Box::new(LlvmType::i8()),
            },
            is_constant: true,
            value: Some(LlvmConst::Array {
                element_ty: LlvmType::i8(),
                elements: bytes
                    .into_iter()
                    .map(|byte| LlvmConst::Int {
                        bits: 8,
                        value: byte as i128,
                    })
                    .collect(),
            }),
            attrs: Vec::new(),
        });

        let case_label = LabelId(format!("ctor_name.case.{index}"));
        let ptr_local = LlvmLocal(format!("ctor_name.ptr.{index}"));
        blocks.push(LlvmBlock {
            label: case_label.clone(),
            instrs: vec![LlvmInstr::GetElementPtr {
                dst: ptr_local.clone(),
                inbounds: true,
                element_ty: LlvmType::Array {
                    len: (name.len() + 1) as u64,
                    element: Box::new(LlvmType::i8()),
                },
                base: LlvmOperand::Global(global),
                indices: vec![
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                ],
            }],
            term: LlvmTerminator::Ret {
                ty: LlvmType::Ptr,
                value: LlvmOperand::Local(ptr_local),
            },
        });
        cases.push((
            LlvmConst::Int {
                bits: 32,
                value: *tag as i128,
            },
            case_label,
        ));
    }

    let entry_label = LabelId("ctor_name.entry".to_string());
    blocks.insert(
        0,
        LlvmBlock {
            label: entry_label,
            instrs: Vec::new(),
            term: LlvmTerminator::Switch {
                ty: LlvmType::i32(),
                scrutinee: LlvmOperand::Local(tag_param.clone()),
                default: default_label.clone(),
                cases,
            },
        },
    );
    blocks.push(LlvmBlock {
        label: default_label,
        instrs: Vec::new(),
        term: LlvmTerminator::Ret {
            ty: LlvmType::Ptr,
            value: LlvmOperand::Const(LlvmConst::Null),
        },
    });

    module.functions.push(LlvmFunction {
        linkage: Linkage::External,
        name: helper_name,
        sig: LlvmFunctionSig {
            ret: LlvmType::Ptr,
            params: vec![LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        params: vec![tag_param],
        attrs: vec!["nounwind".to_string()],
        blocks,
    });
}

/// Emit LLVM IR text from a `LirProgram`.
pub fn emit_llvm_ir(program: &LirProgram) -> String {
    render_module(&emit_llvm_module(program))
}

/// Emit a thin closure-convention wrapper for a direct-convention function.
/// The wrapper has signature `(i64, ptr, i32) → i64` and unpacks args from
/// the args array, then calls the direct function with individual params.
fn emit_closure_wrapper(func: &LirFunction, linkage: Linkage) -> LlvmFunction {
    let direct_name = format!("flux_{}", func.qualified_name);
    let wrapper_name = format!("flux_{}.closure_entry", func.qualified_name);

    let closure_param = LlvmLocal("closure_raw".into());
    let args_param = LlvmLocal("args_ptr".into());
    let nargs_param = LlvmLocal("nargs".into());

    let mut instrs = Vec::new();

    // Unpack parameters from args array.
    let mut call_args = Vec::new();
    for (i, _param) in func.params.iter().enumerate() {
        let gep = LlvmLocal(format!("arg.{i}.ptr"));
        instrs.push(LlvmInstr::GetElementPtr {
            dst: gep.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(args_param.clone()),
            indices: vec![(
                LlvmType::i32(),
                LlvmOperand::Const(LlvmConst::Int {
                    bits: 32,
                    value: i as i128,
                }),
            )],
        });
        let val = LlvmLocal(format!("arg.{i}"));
        instrs.push(LlvmInstr::Load {
            dst: val.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(gep),
            align: Some(8),
        });
        call_args.push((LlvmType::i64(), LlvmOperand::Local(val)));
    }

    // Call the direct function.
    let result = LlvmLocal("result".into());
    instrs.push(LlvmInstr::Call {
        dst: Some(result.clone()),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::i64(),
        callee: LlvmOperand::Global(GlobalId(direct_name)),
        args: call_args,
        attrs: Vec::new(),
    });

    LlvmFunction {
        linkage,
        name: GlobalId(wrapper_name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::i64(), LlvmType::Ptr, LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        params: vec![closure_param, args_param, nargs_param],
        attrs: vec!["nounwind".to_string()],
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs,
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(result),
            },
        }],
    }
}

// ── Worker/Wrapper eligibility (Proposal 0140 Phase 10) ────────────────────

/// A function is eligible for worker/wrapper splitting when all parameters
/// and the return value are `IntRep`, it has zero captures, and it is not main.
fn is_worker_eligible(func: &LirFunction) -> bool {
    if func.capture_vars.is_empty()
        && func.qualified_name != "main"
        && !func.param_reps.is_empty()
        && func
            .param_reps
            .iter()
            .all(|r| matches!(r, crate::core::FluxRep::IntRep))
        && matches!(func.result_rep, crate::core::FluxRep::IntRep)
    {
        return true;
    }

    // Fallback: infer param types from LIR body.
    // A function is worker-eligible if it has zero captures, is not main,
    // has parameters, returns Int, and every parameter is immediately
    // consumed by UntagInt in the entry block (indicating integer type).
    if func.capture_vars.is_empty()
        && func.qualified_name != "main"
        && !func.params.is_empty()
        && matches!(func.result_rep, crate::core::FluxRep::IntRep)
    {
        let entry = &func.blocks[0];
        let param_set: HashSet<u32> = func.params.iter().map(|v| v.0).collect();
        let mut untagged_params: HashSet<u32> = HashSet::new();
        for instr in &entry.instrs {
            if let LirInstr::UntagInt { val, .. } = instr
                && param_set.contains(&val.0)
            {
                untagged_params.insert(val.0);
            }
        }
        return untagged_params.len() == func.params.len();
    }

    false
}

// ── Per-function emitter ────────────────────────────────────────────────────

struct FnEmitter<'a> {
    program: &'a LirProgram,
    func: &'a LirFunction,
    is_main: bool,
    next_tmp: u32,
    needed_decls: &'a mut HashSet<String>,
    needed_user_fastcc_decls: &'a mut HashSet<(String, usize)>,
    needed_closure_entry_decls: &'a mut HashSet<String>,
    string_globals: &'a mut Vec<(GlobalId, String)>,
    current_instrs: Vec<LlvmInstr>,
    /// Extra blocks emitted by MatchCtor for field extraction.
    extra_blocks: Vec<LlvmBlock>,
    /// Set of LirFuncIds that need closure entry wrappers
    /// (top-level direct functions used as higher-order values via MakeClosure).
    closure_wrappers_needed: &'a mut HashSet<LirFuncId>,
    /// When true, emit the worker variant (raw i64, no tag/untag overhead).
    worker_mode: bool,
    /// Set of LirFuncIds eligible for worker/wrapper splitting.
    worker_eligible: &'a HashSet<LirFuncId>,
}

impl<'a> FnEmitter<'a> {
    // ── Naming helpers ──────────────────────────────────────────────

    fn var(&self, v: LirVar) -> LlvmOperand {
        LlvmOperand::Local(LlvmLocal(format!("v{}", v.0)))
    }

    fn var_local(&self, v: LirVar) -> LlvmLocal {
        LlvmLocal(format!("v{}", v.0))
    }

    fn label(&self, b: BlockId) -> LabelId {
        LabelId(format!("bb{}", b.0))
    }

    fn tmp(&mut self) -> LlvmLocal {
        let t = LlvmLocal(format!("t{}", self.next_tmp));
        self.next_tmp += 1;
        t
    }

    fn func_name(&self) -> GlobalId {
        if self.is_main {
            GlobalId("flux_main".to_string())
        } else if self.worker_mode {
            GlobalId(format!("flux_{}$w", self.func.qualified_name))
        } else {
            GlobalId(format!("flux_{}", self.func.qualified_name))
        }
    }

    fn emit(&mut self, instr: LlvmInstr) {
        self.current_instrs.push(instr);
    }

    fn i64_const(&self, value: i64) -> LlvmOperand {
        LlvmOperand::Const(LlvmConst::Int {
            bits: 64,
            value: value as i128,
        })
    }

    fn i32_const(&self, value: i32) -> LlvmOperand {
        LlvmOperand::Const(LlvmConst::Int {
            bits: 32,
            value: value as i128,
        })
    }

    /// Emit a call to a C runtime function.
    /// Automatically uses `fastcc` for LLVM prelude helpers (arithmetic, NaN-box,
    /// ADT constructors) and `ccc` for actual C runtime functions.
    fn call_c(
        &mut self,
        dst: Option<LlvmLocal>,
        name: &str,
        args: Vec<(LlvmType, LlvmOperand)>,
        ret: LlvmType,
    ) {
        let conv = if is_fastcc_prelude_helper(name) {
            CallConv::Fastcc
        } else {
            self.needed_decls.insert(name.to_string());
            CallConv::Ccc
        };
        self.emit(LlvmInstr::Call {
            dst,
            tail: false,
            call_conv: Some(conv),
            ret_ty: ret,
            callee: LlvmOperand::Global(GlobalId(name.to_string())),
            args,
            attrs: Vec::new(),
        });
    }

    /// Emit a call to a prelude helper (fastcc convention).
    fn call_fastcc(
        &mut self,
        dst: Option<LlvmLocal>,
        name: &str,
        args: Vec<(LlvmType, LlvmOperand)>,
        ret: LlvmType,
    ) {
        if !is_fastcc_prelude_helper(name) {
            self.needed_user_fastcc_decls
                .insert((name.to_string(), args.len()));
        }
        self.emit(LlvmInstr::Call {
            dst,
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: ret,
            callee: LlvmOperand::Global(GlobalId(name.to_string())),
            args,
            attrs: Vec::new(),
        });
    }

    fn call_extern_fastcc_user(
        &mut self,
        dst: Option<LlvmLocal>,
        name: &str,
        args: Vec<(LlvmType, LlvmOperand)>,
        ret: LlvmType,
    ) {
        self.needed_user_fastcc_decls
            .insert((name.to_string(), args.len()));
        self.emit(LlvmInstr::Call {
            dst,
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: ret,
            callee: LlvmOperand::Global(GlobalId(name.to_string())),
            args,
            attrs: Vec::new(),
        });
    }

    // ── Function emission ───────────────────────────────────────────

    fn emit_function(&mut self) -> LlvmFunction {
        // Pre-pass: compute reachable blocks. After tail call promotion,
        // some continuation blocks become orphaned. Emitting them would
        // produce invalid LLVM SSA (undefined phi values).
        let reachable = {
            let mut visited = HashSet::new();
            let mut worklist = vec![0u32]; // entry block
            while let Some(idx) = worklist.pop() {
                if !visited.insert(idx) {
                    continue;
                }
                let Some(block) = self.func.blocks.get(idx as usize) else {
                    continue;
                };
                let successors: Vec<u32> = match &block.terminator {
                    LirTerminator::Jump(t) => vec![t.0],
                    LirTerminator::Branch {
                        then_block,
                        else_block,
                        ..
                    } => vec![then_block.0, else_block.0],
                    LirTerminator::Switch {
                        cases, default, ..
                    } => {
                        let mut v: Vec<_> = cases.iter().map(|(_, t)| t.0).collect();
                        v.push(default.0);
                        v
                    }
                    LirTerminator::Call { cont, .. } => vec![cont.0],
                    LirTerminator::MatchCtor {
                        arms, default, ..
                    } => {
                        let mut v: Vec<_> = arms.iter().map(|a| a.target.0).collect();
                        v.push(default.0);
                        v
                    }
                    // Return, TailCall, Unreachable have no successors.
                    _ => vec![],
                };
                worklist.extend(successors);
            }
            visited
        };

        // Pre-pass: collect phi-node information.
        // Block params in LIR are written via Copy instructions in predecessor
        // blocks.  In LLVM SSA, these become phi nodes at the target block.
        let mut phi_info: std::collections::HashMap<u32, Vec<(LirVar, BlockId)>> =
            std::collections::HashMap::new();
        // Collect: for each block param variable, find (source_value, source_block).
        let block_param_vars: HashSet<u32> = self
            .func
            .blocks
            .iter()
            .flat_map(|b| b.params.iter().map(|v| v.0))
            .collect();

        for block in &self.func.blocks {
            // Skip unreachable blocks — their Copy instructions must not
            // contribute phi edges (orphaned by tail call promotion).
            if !reachable.contains(&block.id.0) {
                continue;
            }
            for instr in &block.instrs {
                if let LirInstr::Copy { dst, src } = instr
                    && block_param_vars.contains(&dst.0)
                {
                    phi_info.entry(dst.0).or_default().push((*src, block.id));
                }
            }
        }

        let mut blocks = Vec::new();

        for block in &self.func.blocks {
            // Skip unreachable blocks entirely.
            if !reachable.contains(&block.id.0) {
                continue;
            }

            self.current_instrs.clear();

            // Emit phi nodes for block parameters.
            for param in &block.params {
                if let Some(incoming) = phi_info.get(&param.0) {
                    self.emit(LlvmInstr::Phi {
                        dst: self.var_local(*param),
                        ty: LlvmType::i64(),
                        incoming: incoming
                            .iter()
                            .map(|(src, blk)| (self.var(*src), self.label(*blk)))
                            .collect(),
                    });
                }
            }

            // Emit instructions (skip Copy to block params — handled by phi).
            for instr in &block.instrs {
                if let LirInstr::Copy { dst, .. } = instr
                    && block_param_vars.contains(&dst.0)
                {
                    continue; // Skip — this is a phi-bridge Copy.
                }
                self.emit_instr(instr);
            }

            // Emit terminator.
            let term = self.emit_terminator(&block.terminator);

            blocks.push(LlvmBlock {
                label: self.label(block.id),
                instrs: std::mem::take(&mut self.current_instrs),
                term,
            });
            // Append any extra blocks emitted by MatchCtor.
            blocks.append(&mut self.extra_blocks);
        }

        if self.is_main {
            // Main: no params, returns i64, ccc (called from C main()).
            LlvmFunction {
                linkage: Linkage::External,
                name: self.func_name(),
                sig: LlvmFunctionSig {
                    ret: LlvmType::i64(),
                    params: Vec::new(),
                    varargs: false,
                    call_conv: CallConv::Ccc,
                },
                params: Vec::new(),
                attrs: vec!["nounwind".to_string()],
                blocks,
            }
        } else if self.func.capture_vars.is_empty() {
            // ── Direct calling convention (GHC-style known calls) ──────
            // Top-level function with no captures: takes individual i64 params.
            // No closure preamble needed — params are used directly.
            let param_locals: Vec<LlvmLocal> = self
                .func
                .params
                .iter()
                .map(|p| self.var_local(*p))
                .collect();
            let param_types: Vec<LlvmType> =
                self.func.params.iter().map(|_| LlvmType::i64()).collect();

            LlvmFunction {
                linkage: Linkage::External,
                name: self.func_name(),
                sig: LlvmFunctionSig {
                    ret: LlvmType::i64(),
                    params: param_types,
                    varargs: false,
                    call_conv: CallConv::Fastcc,
                },
                params: param_locals,
                attrs: vec!["nounwind".to_string()],
                blocks,
            }
        } else {
            // ── Closure calling convention ─────────────────────────────
            // Function with captures: signature (i64 closure_raw, ptr args_ptr, i32 nargs).
            // Insert an entry preamble that unpacks captures and args.
            let mut entry_instrs = Vec::new();
            let closure_param = LlvmLocal("closure_raw".into());
            let args_param = LlvmLocal("args_ptr".into());
            let _nargs_param = LlvmLocal("nargs".into());

            // Unpack captures from closure payload.
            if !self.func.capture_vars.is_empty() {
                let clo_ptr = LlvmLocal("clo.ptr".into());
                entry_instrs.push(LlvmInstr::Call {
                    dst: Some(clo_ptr.clone()),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Ptr,
                    callee: LlvmOperand::Global(GlobalId("flux_untag_boxed_ptr".into())),
                    args: vec![(LlvmType::i64(), LlvmOperand::Local(closure_param.clone()))],
                    attrs: Vec::new(),
                });
                // Captures start at FluxClosure payload field.
                // FluxClosure = {ptr, i32, i32, i32, i32(pad), [0 x i64]}
                // field 5 is the payload (after padding for i64 alignment).
                let cap_base = LlvmLocal("cap.base".into());
                entry_instrs.push(LlvmInstr::GetElementPtr {
                    dst: cap_base.clone(),
                    inbounds: true,
                    element_ty: LlvmType::Named("FluxClosure".into()),
                    base: LlvmOperand::Local(clo_ptr),
                    indices: vec![
                        (
                            LlvmType::i32(),
                            LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                        ),
                        (
                            LlvmType::i32(),
                            LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 5 }),
                        ),
                        (
                            LlvmType::i32(),
                            LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                        ),
                    ],
                });
                for (i, cap_var) in self.func.capture_vars.iter().enumerate() {
                    let cap_gep = LlvmLocal(format!("cap.{i}.ptr"));
                    entry_instrs.push(LlvmInstr::GetElementPtr {
                        dst: cap_gep.clone(),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: LlvmOperand::Local(cap_base.clone()),
                        indices: vec![(
                            LlvmType::i32(),
                            LlvmOperand::Const(LlvmConst::Int {
                                bits: 32,
                                value: i as i128,
                            }),
                        )],
                    });
                    entry_instrs.push(LlvmInstr::Load {
                        dst: self.var_local(*cap_var),
                        ty: LlvmType::i64(),
                        ptr: LlvmOperand::Local(cap_gep),
                        align: Some(8),
                    });
                }
            }

            // Unpack parameters from args array.
            for (i, param) in self.func.params.iter().enumerate() {
                let arg_gep = LlvmLocal(format!("arg.{i}.ptr"));
                entry_instrs.push(LlvmInstr::GetElementPtr {
                    dst: arg_gep.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(args_param.clone()),
                    indices: vec![(
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: i as i128,
                        }),
                    )],
                });
                entry_instrs.push(LlvmInstr::Load {
                    dst: self.var_local(*param),
                    ty: LlvmType::i64(),
                    ptr: LlvmOperand::Local(arg_gep),
                    align: Some(8),
                });
            }

            // Prepend entry instructions to the first block.
            if !blocks.is_empty() {
                let first = &mut blocks[0];
                let mut merged = entry_instrs;
                merged.append(&mut first.instrs);
                first.instrs = merged;
            }

            // Closure entry functions use ccc (C calling convention) so
            // they're compatible with the C runtime's flux_call_closure_c
            // trampoline and safe under opt -O1 (no fastcc register
            // mismatches across indirect calls).
            LlvmFunction {
                linkage: Linkage::Internal,
                name: self.func_name(),
                sig: LlvmFunctionSig {
                    ret: LlvmType::i64(),
                    params: vec![LlvmType::i64(), LlvmType::Ptr, LlvmType::i32()],
                    varargs: false,
                    call_conv: CallConv::Ccc,
                },
                params: vec![closure_param, args_param, _nargs_param],
                attrs: vec!["nounwind".to_string()],
                blocks,
            }
        }
    }

    // ── Instruction emission ────────────────────────────────────────

    fn emit_instr(&mut self, instr: &LirInstr) {
        match instr {
            LirInstr::Const { dst, value } => self.emit_const(*dst, value),
            LirInstr::Copy { dst, src } => {
                // SSA identity: %dst = add i64 %src, 0
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.var(*src),
                    rhs: self.i64_const(0),
                });
            }

            LirInstr::GetGlobal { dst, global_idx } => {
                // Call C runtime to load from globals table.
                self.call_c(
                    Some(self.var_local(*dst)),
                    "flux_get_global",
                    vec![(LlvmType::i64(), self.i64_const(*global_idx as i64))],
                    LlvmType::i64(),
                );
            }

            LirInstr::TupleGet { dst, tuple, index } => {
                // Inline tuple field access (Phase 4, Proposal 0140):
                // inttoptr → GEP %FluxTuple field 5 (payload) → GEP [i64] index → load i64
                let ptr_tmp = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: ptr_tmp.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*tuple),
                    to_ty: LlvmType::Ptr,
                });
                let payload_ptr = self.tmp();
                self.emit(LlvmInstr::GetElementPtr {
                    dst: payload_ptr.clone(),
                    inbounds: true,
                    element_ty: LlvmType::Named("FluxTuple".into()),
                    base: LlvmOperand::Local(ptr_tmp),
                    indices: vec![
                        (LlvmType::i32(), self.i32_const(0)),
                        (LlvmType::i32(), self.i32_const(5)), // payload field
                        (LlvmType::i32(), self.i32_const(0)),
                    ],
                });
                let field_ptr = self.tmp();
                self.emit(LlvmInstr::GetElementPtr {
                    dst: field_ptr.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(payload_ptr),
                    indices: vec![(LlvmType::i32(), self.i32_const(*index as i32))],
                });
                self.emit(LlvmInstr::Load {
                    dst: self.var_local(*dst),
                    ty: LlvmType::i64(),
                    ptr: LlvmOperand::Local(field_ptr),
                    align: Some(8),
                });
            }

            // ── Integer arithmetic (raw, untagged) ──────────────────
            LirInstr::IAdd { dst, a, b } => self.emit_binop(*dst, LlvmValueKind::Add, *a, *b),
            LirInstr::ISub { dst, a, b } => self.emit_binop(*dst, LlvmValueKind::Sub, *a, *b),
            LirInstr::IMul { dst, a, b } => self.emit_binop(*dst, LlvmValueKind::Mul, *a, *b),
            LirInstr::IDiv { dst, a, b } => self.emit_binop(*dst, LlvmValueKind::SDiv, *a, *b),
            LirInstr::IRem { dst, a, b } => self.emit_binop(*dst, LlvmValueKind::SRem, *a, *b),

            LirInstr::ICmp { dst, op, a, b } => {
                let cmp_op = match op {
                    CmpOp::Eq => LlvmCmpOp::Eq,
                    CmpOp::Ne => LlvmCmpOp::Ne,
                    CmpOp::Slt => LlvmCmpOp::Slt,
                    CmpOp::Sle => LlvmCmpOp::Sle,
                    CmpOp::Sgt => LlvmCmpOp::Sgt,
                    CmpOp::Sge => LlvmCmpOp::Sge,
                };
                let cmp_tmp = self.tmp();
                self.emit(LlvmInstr::Icmp {
                    dst: cmp_tmp.clone(),
                    op: cmp_op,
                    ty: LlvmType::i64(),
                    lhs: self.var(*a),
                    rhs: self.var(*b),
                });
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::ZExt,
                    from_ty: LlvmType::i1(),
                    operand: LlvmOperand::Local(cmp_tmp),
                    to_ty: LlvmType::i64(),
                });
            }

            // ── Pointer-tag tag/untag ──────────────────────────────
            LirInstr::TagInt { dst, raw } => {
                if self.worker_mode {
                    // Worker mode: identity copy, no tagging needed.
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i64(),
                        lhs: self.var(*raw),
                        rhs: self.i64_const(0),
                    });
                } else {
                    // Pointer-tag int: (raw << 1) | 1
                    let shifted = self.tmp();
                    self.emit(LlvmInstr::Binary {
                        dst: shifted.clone(),
                        op: LlvmValueKind::Shl,
                        ty: LlvmType::i64(),
                        lhs: self.var(*raw),
                        rhs: self.i64_const(1),
                    });
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Or,
                        ty: LlvmType::i64(),
                        lhs: LlvmOperand::Local(shifted),
                        rhs: self.i64_const(1),
                    });
                }
            }
            LirInstr::UntagInt { dst, val } => {
                if self.worker_mode {
                    // Worker mode: identity copy, no untagging needed.
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i64(),
                        lhs: self.var(*val),
                        rhs: self.i64_const(0),
                    });
                } else {
                    // Pointer-tag untag int: val >> 1 (arithmetic shift)
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::AShr,
                        ty: LlvmType::i64(),
                        lhs: self.var(*val),
                        rhs: self.i64_const(1),
                    });
                }
            }
            LirInstr::TagFloat { dst, raw } => {
                // Floats are heap-boxed: call flux_box_float.
                // raw is i64 (IEEE 754 bits), bitcast to double first.
                let dbl_tmp = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: dbl_tmp.clone(),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*raw),
                    to_ty: LlvmType::Double,
                });
                self.call_c(
                    Some(self.var_local(*dst)),
                    "flux_box_float_rt",
                    vec![(LlvmType::Double, LlvmOperand::Local(dbl_tmp))],
                    LlvmType::i64(),
                );
            }
            LirInstr::UntagFloat { dst, val } => {
                // Floats are heap-boxed: call flux_unbox_float_rt.
                let dbl_tmp = self.tmp();
                self.call_c(
                    Some(dbl_tmp.clone()),
                    "flux_unbox_float_rt",
                    vec![(LlvmType::i64(), self.var(*val))],
                    LlvmType::Double,
                );
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::Double,
                    operand: LlvmOperand::Local(dbl_tmp),
                    to_ty: LlvmType::i64(),
                });
            }
            LirInstr::TagBool { dst, raw } => {
                if self.worker_mode {
                    // Worker mode: identity copy, raw 0/1 stays as 0/1.
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i64(),
                        lhs: self.var(*raw),
                        rhs: self.i64_const(0),
                    });
                } else {
                    // Pointer-tag bool: false=2, true=4.
                    // raw is i64 0 or 1: result = raw * 2 + 2 = (raw << 1) + 2
                    let shifted = self.tmp();
                    self.emit(LlvmInstr::Binary {
                        dst: shifted.clone(),
                        op: LlvmValueKind::Shl,
                        ty: LlvmType::i64(),
                        lhs: self.var(*raw),
                        rhs: self.i64_const(1),
                    });
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i64(),
                        lhs: LlvmOperand::Local(shifted),
                        rhs: self.i64_const(FLUX_FALSE),
                    });
                }
            }
            LirInstr::UntagBool { dst, val } => {
                if self.worker_mode {
                    // Worker mode: identity copy, no untagging needed.
                    self.emit(LlvmInstr::Binary {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i64(),
                        lhs: self.var(*val),
                        rhs: self.i64_const(0),
                    });
                } else {
                    // Pointer-tag untag bool: compare with FLUX_TRUE (4)
                    let cmp_tmp = self.tmp();
                    self.emit(LlvmInstr::Icmp {
                        dst: cmp_tmp.clone(),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::i64(),
                        lhs: self.var(*val),
                        rhs: self.i64_const(FLUX_TRUE),
                    });
                    self.emit(LlvmInstr::Cast {
                        dst: self.var_local(*dst),
                        op: LlvmValueKind::ZExt,
                        from_ty: LlvmType::i1(),
                        operand: LlvmOperand::Local(cmp_tmp),
                        to_ty: LlvmType::i64(),
                    });
                }
            }
            LirInstr::TagPtr { dst, ptr } => {
                // Pointer-tag: raw pointer value IS the tagged value (even, >= 12).
                // Just copy the i64 directly.
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.var(*ptr),
                    rhs: self.i64_const(0),
                });
            }
            LirInstr::UntagPtr { dst, val } => {
                // Pointer-tag: the tagged value IS the raw pointer.
                // Just copy the i64 directly.
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.var(*val),
                    rhs: self.i64_const(0),
                });
            }
            LirInstr::GetTag { dst, val } => {
                // With pointer tagging, GetTag extracts the ADT constructor tag
                // from the heap object. The value is a pointer; load the i32
                // ctor_tag from offset 0 of the pointed-to struct.
                let ptr_tmp = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: ptr_tmp.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*val),
                    to_ty: LlvmType::Ptr,
                });
                let i32_tmp = self.tmp();
                self.emit(LlvmInstr::Load {
                    dst: i32_tmp.clone(),
                    ty: LlvmType::i32(),
                    ptr: LlvmOperand::Local(ptr_tmp),
                    align: Some(4),
                });
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::ZExt,
                    from_ty: LlvmType::i32(),
                    operand: LlvmOperand::Local(i32_tmp),
                    to_ty: LlvmType::i64(),
                });
            }

            // ── Memory operations ───────────────────────────────────
            LirInstr::Load { dst, ptr, offset } => {
                let raw_ptr = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: raw_ptr.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*ptr),
                    to_ty: LlvmType::Ptr,
                });
                if *offset != 0 {
                    let gep = self.tmp();
                    self.emit(LlvmInstr::GetElementPtr {
                        dst: gep.clone(),
                        inbounds: true,
                        element_ty: LlvmType::i8(),
                        base: LlvmOperand::Local(raw_ptr),
                        indices: vec![(LlvmType::i32(), self.i32_const(*offset))],
                    });
                    self.emit(LlvmInstr::Load {
                        dst: self.var_local(*dst),
                        ty: LlvmType::i64(),
                        ptr: LlvmOperand::Local(gep),
                        align: Some(8),
                    });
                } else {
                    self.emit(LlvmInstr::Load {
                        dst: self.var_local(*dst),
                        ty: LlvmType::i64(),
                        ptr: LlvmOperand::Local(raw_ptr),
                        align: Some(8),
                    });
                }
            }
            LirInstr::Store { ptr, offset, val } => {
                let raw_ptr = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: raw_ptr.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*ptr),
                    to_ty: LlvmType::Ptr,
                });
                // ADT header fields (ctor_tag at offset 0, field_count at offset 4)
                // are i32; payload fields (offset >= 8) are i64.
                let (store_ty, align) = if *offset < 8 {
                    (LlvmType::i32(), 4)
                } else {
                    (LlvmType::i64(), 8)
                };
                // For i32 stores, truncate the i64 value.
                let store_val = if *offset < 8 {
                    let trunc = self.tmp();
                    self.emit(LlvmInstr::Cast {
                        dst: trunc.clone(),
                        op: LlvmValueKind::Trunc,
                        from_ty: LlvmType::i64(),
                        operand: self.var(*val),
                        to_ty: LlvmType::i32(),
                    });
                    LlvmOperand::Local(trunc)
                } else {
                    self.var(*val)
                };
                if *offset != 0 {
                    let gep = self.tmp();
                    self.emit(LlvmInstr::GetElementPtr {
                        dst: gep.clone(),
                        inbounds: true,
                        element_ty: LlvmType::i8(),
                        base: LlvmOperand::Local(raw_ptr),
                        indices: vec![(LlvmType::i32(), self.i32_const(*offset))],
                    });
                    self.emit(LlvmInstr::Store {
                        ty: store_ty,
                        value: store_val,
                        ptr: LlvmOperand::Local(gep),
                        align: Some(align),
                    });
                } else {
                    self.emit(LlvmInstr::Store {
                        ty: store_ty,
                        value: store_val,
                        ptr: LlvmOperand::Local(raw_ptr),
                        align: Some(align),
                    });
                }
            }
            LirInstr::StoreI32 { ptr, offset, value } => {
                let raw_ptr = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: raw_ptr.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*ptr),
                    to_ty: LlvmType::Ptr,
                });
                let store_val = LlvmOperand::Const(LlvmConst::Int {
                    bits: 32,
                    value: (*value) as i128,
                });
                if *offset != 0 {
                    let gep = self.tmp();
                    self.emit(LlvmInstr::GetElementPtr {
                        dst: gep.clone(),
                        inbounds: true,
                        element_ty: LlvmType::i8(),
                        base: LlvmOperand::Local(raw_ptr),
                        indices: vec![(LlvmType::i32(), self.i32_const(*offset))],
                    });
                    self.emit(LlvmInstr::Store {
                        ty: LlvmType::i32(),
                        value: store_val,
                        ptr: LlvmOperand::Local(gep),
                        align: Some(4),
                    });
                } else {
                    self.emit(LlvmInstr::Store {
                        ty: LlvmType::i32(),
                        value: store_val,
                        ptr: LlvmOperand::Local(raw_ptr),
                        align: Some(4),
                    });
                }
            }
            LirInstr::Alloc { dst, size, .. } => {
                let ptr_tmp = self.tmp();
                self.call_fastcc(
                    Some(ptr_tmp.clone()),
                    "flux_bump_alloc_inline",
                    vec![
                        (LlvmType::i32(), self.i32_const(*size as i32)),
                        (
                            LlvmType::i32(),
                            self.i32_const(instr.scan_fields_val() as i32),
                        ),
                        (LlvmType::i32(), self.i32_const(instr.obj_tag_val() as i32)),
                    ],
                    LlvmType::Ptr,
                );
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::PtrToInt,
                    from_ty: LlvmType::Ptr,
                    operand: LlvmOperand::Local(ptr_tmp),
                    to_ty: LlvmType::i64(),
                });
            }

            // ── PrimCall (CorePrimOp → C runtime) ───────────────────
            LirInstr::PrimCall { dst, op, args } => self.emit_primcall(dst, op, args),

            // ── Aether RC ───────────────────────────────────────────
            LirInstr::Dup { val } => {
                // In worker mode, all values are raw integers — skip RC ops.
                if !self.worker_mode {
                    self.call_c(
                        None,
                        "flux_dup",
                        vec![(LlvmType::i64(), self.var(*val))],
                        LlvmType::Void,
                    );
                }
            }
            LirInstr::Drop { val } => {
                if !self.worker_mode {
                    self.call_c(
                        None,
                        "flux_drop",
                        vec![(LlvmType::i64(), self.var(*val))],
                        LlvmType::Void,
                    );
                }
            }
            LirInstr::IsUnique { dst, val } => {
                let bool_tmp = self.tmp();
                self.call_c(
                    Some(bool_tmp.clone()),
                    "flux_rc_is_unique",
                    vec![(LlvmType::i64(), self.var(*val))],
                    LlvmType::i1(),
                );
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::ZExt,
                    from_ty: LlvmType::i1(),
                    operand: LlvmOperand::Local(bool_tmp),
                    to_ty: LlvmType::i64(),
                });
            }
            LirInstr::DropReuse { dst, val, size } => {
                let ptr_tmp = self.tmp();
                self.call_c(
                    Some(ptr_tmp.clone()),
                    "flux_drop_reuse",
                    vec![
                        (LlvmType::i64(), self.var(*val)),
                        (LlvmType::i32(), self.i32_const(*size as i32)),
                    ],
                    LlvmType::Ptr,
                );
                self.emit(LlvmInstr::Cast {
                    dst: self.var_local(*dst),
                    op: LlvmValueKind::PtrToInt,
                    from_ty: LlvmType::Ptr,
                    operand: LlvmOperand::Local(ptr_tmp),
                    to_ty: LlvmType::i64(),
                });
            }

            // ── Closures ────────────────────────────────────────────
            LirInstr::MakeClosure {
                dst,
                func_id,
                captures,
            } => {
                self.emit_make_closure(*dst, *func_id, captures);
            }
            LirInstr::MakeExternClosure { dst, symbol, arity } => {
                self.emit_make_extern_closure(*dst, symbol, *arity);
            }

            // ── Collections ─────────────────────────────────────────
            LirInstr::MakeArray { dst, elements } => {
                self.emit_make_collection(*dst, elements, "flux_array_new");
            }
            LirInstr::MakeTuple { dst, elements } => {
                // flux_make_tuple is emitted by emit_adt_support (fastcc LLVM helper).
                self.emit_make_collection_fastcc(*dst, elements, "flux_make_tuple");
            }
            LirInstr::MakeHash { dst, pairs } => {
                self.emit_make_hash(*dst, pairs);
            }
            LirInstr::MakeList { dst, elements } => {
                self.emit_make_list(*dst, elements);
            }
            LirInstr::Interpolate { dst, parts } => {
                self.emit_interpolate(*dst, parts);
            }

            // ── Constructor creation ────────────────────────────────
            LirInstr::MakeCtor {
                dst,
                ctor_tag,
                ctor_name: _,
                fields,
            } => {
                self.emit_make_ctor(*dst, *ctor_tag, fields);
            }
        }
    }

    // ── Instruction helpers ─────────────────────────────────────────

    fn emit_binop(&mut self, dst: LirVar, op: LlvmValueKind, a: LirVar, b: LirVar) {
        self.emit(LlvmInstr::Binary {
            dst: self.var_local(dst),
            op,
            ty: LlvmType::i64(),
            lhs: self.var(a),
            rhs: self.var(b),
        });
    }

    fn emit_const(&mut self, dst: LirVar, value: &LirConst) {
        match value {
            LirConst::Int(n) => {
                // In worker mode, emit raw value; otherwise pointer-tag: (n << 1) | 1.
                let value = if self.worker_mode { *n } else { (*n << 1) | 1 };
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.i64_const(value),
                    rhs: self.i64_const(0),
                });
            }
            LirConst::Float(f) => {
                // Floats are heap-boxed: call flux_box_float_rt at runtime.
                let bits = f.to_bits() as i64;
                let dbl_tmp = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: dbl_tmp.clone(),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::i64(),
                    operand: self.i64_const(bits),
                    to_ty: LlvmType::Double,
                });
                self.call_c(
                    Some(self.var_local(dst)),
                    "flux_box_float_rt",
                    vec![(LlvmType::Double, LlvmOperand::Local(dbl_tmp))],
                    LlvmType::i64(),
                );
            }
            LirConst::Bool(b) => {
                // In worker mode, emit raw 0/1; otherwise tagged constants.
                let value = if self.worker_mode {
                    if *b { 1 } else { 0 }
                } else if *b {
                    FLUX_TRUE
                } else {
                    FLUX_FALSE
                };
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.i64_const(value),
                    rhs: self.i64_const(0),
                });
            }
            LirConst::String(s) => {
                let idx = self.string_globals.len();
                let gid = GlobalId(format!("flux.str.{}", idx));
                self.string_globals.push((gid.clone(), s.clone()));
                // Call flux_string_new(ptr, i32 len) → i64
                self.call_c(
                    Some(self.var_local(dst)),
                    "flux_string_new",
                    vec![
                        (LlvmType::Ptr, LlvmOperand::Global(gid)),
                        (LlvmType::i32(), self.i32_const(s.len() as i32)),
                    ],
                    LlvmType::i64(),
                );
            }
            LirConst::None => {
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.i64_const(FLUX_NONE),
                    rhs: self.i64_const(0),
                });
            }
            LirConst::EmptyList => {
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.i64_const(FLUX_EMPTY_LIST),
                    rhs: self.i64_const(0),
                });
            }
            LirConst::Tagged(n) => {
                self.emit(LlvmInstr::Binary {
                    dst: self.var_local(dst),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i64(),
                    lhs: self.i64_const(*n),
                    rhs: self.i64_const(0),
                });
            }
        }
    }

    fn emit_primcall(&mut self, dst: &Option<LirVar>, op: &CorePrimOp, args: &[LirVar]) {
        let llvm_args: Vec<(LlvmType, LlvmOperand)> = args
            .iter()
            .map(|a| (LlvmType::i64(), self.var(*a)))
            .collect();
        let dst_local = dst.map(|d| self.var_local(d));

        // Map CorePrimOp to C runtime function name.
        let c_name = primop_c_name(op);
        let ret_ty = if dst.is_some() {
            LlvmType::i64()
        } else {
            LlvmType::Void
        };

        self.call_c(dst_local, &c_name, llvm_args, ret_ty);
    }

    fn emit_make_closure(&mut self, dst: LirVar, func_id: LirFuncId, captures: &[LirVar]) {
        let target = self
            .program
            .func_by_id(func_id)
            .expect("MakeClosure references unknown LirFuncId");
        let arity = target.params.len();

        // For direct-convention functions (no captures), the closure must
        // reference the closure-convention WRAPPER, not the direct function.
        // The wrapper unpacks args and calls the direct function.
        let fn_ptr_name = if target.capture_vars.is_empty() && target.qualified_name != "main" {
            self.closure_wrappers_needed.insert(func_id);
            format!("flux_{}.closure_entry", target.qualified_name)
        } else if target.qualified_name == "main" {
            "flux_main".to_string()
        } else {
            format!("flux_{}", target.qualified_name)
        };

        // flux_make_closure(ptr fn_ptr, i32 arity, ptr captures, i32 cap_count,
        //                   ptr applied, i32 applied_count) → i64
        // Called with fastcc (it's an LLVM IR helper from emit_closure_support).
        if captures.is_empty() {
            self.call_fastcc(
                Some(self.var_local(dst)),
                "flux_make_closure",
                vec![
                    (LlvmType::Ptr, LlvmOperand::Global(GlobalId(fn_ptr_name))),
                    (LlvmType::i32(), self.i32_const(arity as i32)),
                    (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                    (LlvmType::i32(), self.i32_const(0)),
                    (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                    (LlvmType::i32(), self.i32_const(0)),
                ],
                LlvmType::i64(),
            );
        } else {
            let arr = self.tmp();
            self.emit(LlvmInstr::Alloca {
                dst: arr.clone(),
                ty: LlvmType::Array {
                    len: captures.len() as u64,
                    element: Box::new(LlvmType::i64()),
                },
                count: None,
                align: Some(8),
            });
            for (i, cap) in captures.iter().enumerate() {
                let gep = self.tmp();
                self.emit(LlvmInstr::GetElementPtr {
                    dst: gep.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(arr.clone()),
                    indices: vec![(LlvmType::i32(), self.i32_const(i as i32))],
                });
                self.emit(LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: self.var(*cap),
                    ptr: LlvmOperand::Local(gep),
                    align: Some(8),
                });
            }
            self.call_fastcc(
                Some(self.var_local(dst)),
                "flux_make_closure",
                vec![
                    (
                        LlvmType::Ptr,
                        LlvmOperand::Global(GlobalId(format!("flux_{}", target.qualified_name))),
                    ),
                    (LlvmType::i32(), self.i32_const(arity as i32)),
                    (LlvmType::Ptr, LlvmOperand::Local(arr)),
                    (LlvmType::i32(), self.i32_const(captures.len() as i32)),
                    (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                    (LlvmType::i32(), self.i32_const(0)),
                ],
                LlvmType::i64(),
            );
        }
    }

    fn emit_make_extern_closure(&mut self, dst: LirVar, symbol: &str, arity: usize) {
        let wrapper_name = format!("{symbol}.closure_entry");
        self.needed_closure_entry_decls.insert(wrapper_name.clone());
        self.call_fastcc(
            Some(self.var_local(dst)),
            "flux_make_closure",
            vec![
                (LlvmType::Ptr, LlvmOperand::Global(GlobalId(wrapper_name))),
                (LlvmType::i32(), self.i32_const(arity as i32)),
                (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                (LlvmType::i32(), self.i32_const(0)),
                (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                (LlvmType::i32(), self.i32_const(0)),
            ],
            LlvmType::i64(),
        );
    }

    fn emit_make_collection_fastcc(&mut self, dst: LirVar, elements: &[LirVar], func: &str) {
        self.emit_make_collection_inner(dst, elements, func, true);
    }

    fn emit_make_collection(&mut self, dst: LirVar, elements: &[LirVar], c_func: &str) {
        self.emit_make_collection_inner(dst, elements, c_func, false);
    }

    fn emit_make_collection_inner(
        &mut self,
        dst: LirVar,
        elements: &[LirVar],
        c_func: &str,
        use_fastcc: bool,
    ) {
        let call = if use_fastcc {
            FnEmitter::call_fastcc
        } else {
            FnEmitter::call_c
        };
        if elements.is_empty() {
            call(
                self,
                Some(self.var_local(dst)),
                c_func,
                vec![
                    (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                    (LlvmType::i32(), self.i32_const(0)),
                ],
                LlvmType::i64(),
            );
        } else {
            let arr = self.tmp();
            self.emit(LlvmInstr::Alloca {
                dst: arr.clone(),
                ty: LlvmType::Array {
                    len: elements.len() as u64,
                    element: Box::new(LlvmType::i64()),
                },
                count: None,
                align: Some(8),
            });
            for (i, elem) in elements.iter().enumerate() {
                let gep = self.tmp();
                self.emit(LlvmInstr::GetElementPtr {
                    dst: gep.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(arr.clone()),
                    indices: vec![(LlvmType::i32(), self.i32_const(i as i32))],
                });
                self.emit(LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: self.var(*elem),
                    ptr: LlvmOperand::Local(gep),
                    align: Some(8),
                });
            }
            call(
                self,
                Some(self.var_local(dst)),
                c_func,
                vec![
                    (LlvmType::Ptr, LlvmOperand::Local(arr)),
                    (LlvmType::i32(), self.i32_const(elements.len() as i32)),
                ],
                LlvmType::i64(),
            );
        }
    }

    fn emit_interpolate(&mut self, dst: LirVar, parts: &[LirVar]) {
        // String interpolation: use interpolation-friendly formatting for each part.
        if parts.is_empty() {
            self.call_c(
                Some(self.var_local(dst)),
                "flux_string_new",
                vec![
                    (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                    (LlvmType::i32(), self.i32_const(0)),
                ],
                LlvmType::i64(),
            );
            return;
        }
        let mut current = self.tmp();
        self.call_c(
            Some(current.clone()),
            "flux_to_string_value",
            vec![(LlvmType::i64(), self.var(parts[0]))],
            LlvmType::i64(),
        );
        for &part in &parts[1..] {
            let part_str = self.tmp();
            self.call_c(
                Some(part_str.clone()),
                "flux_to_string_value",
                vec![(LlvmType::i64(), self.var(part))],
                LlvmType::i64(),
            );
            let next = self.tmp();
            self.call_c(
                Some(next.clone()),
                "flux_string_concat",
                vec![
                    (LlvmType::i64(), LlvmOperand::Local(current)),
                    (LlvmType::i64(), LlvmOperand::Local(part_str)),
                ],
                LlvmType::i64(),
            );
            current = next;
        }
        self.emit(LlvmInstr::Binary {
            dst: self.var_local(dst),
            op: LlvmValueKind::Add,
            ty: LlvmType::i64(),
            lhs: LlvmOperand::Local(current),
            rhs: self.i64_const(0),
        });
    }

    fn emit_make_hash(&mut self, dst: LirVar, pairs: &[LirVar]) {
        // Build HAMT from interleaved key-value pairs.
        // Start with flux_hamt_empty(), then flux_hamt_set(map, key, val) for each pair.
        let mut current = self.tmp();
        self.call_c(
            Some(current.clone()),
            "flux_hamt_empty",
            Vec::new(),
            LlvmType::i64(),
        );
        for chunk in pairs.chunks(2) {
            if chunk.len() == 2 {
                let next = self.tmp();
                self.call_c(
                    Some(next.clone()),
                    "flux_hamt_set",
                    vec![
                        (LlvmType::i64(), LlvmOperand::Local(current)),
                        (LlvmType::i64(), self.var(chunk[0])),
                        (LlvmType::i64(), self.var(chunk[1])),
                    ],
                    LlvmType::i64(),
                );
                current = next;
            }
        }
        // Copy result to dst.
        self.emit(LlvmInstr::Binary {
            dst: self.var_local(dst),
            op: LlvmValueKind::Add,
            ty: LlvmType::i64(),
            lhs: LlvmOperand::Local(current),
            rhs: self.i64_const(0),
        });
    }

    fn emit_make_list(&mut self, dst: LirVar, elements: &[LirVar]) {
        // Build cons list right-to-left: Cons(e_n-1, Cons(e_n, EmptyList))
        let empty = FLUX_EMPTY_LIST;
        let current = self.i64_const(empty);
        // Need a variable to hold the current tail.
        let mut current_local: Option<LlvmLocal> = None;

        for elem in elements.iter().rev() {
            let result = self.tmp();
            let tail_op = if let Some(ref loc) = current_local {
                LlvmOperand::Local(loc.clone())
            } else {
                current.clone()
            };
            self.call_fastcc(
                Some(result.clone()),
                "flux_make_cons",
                vec![
                    (LlvmType::i64(), self.var(*elem)),
                    (LlvmType::i64(), tail_op),
                ],
                LlvmType::i64(),
            );
            current_local = Some(result);
        }

        if let Some(loc) = current_local {
            // Copy to dst variable.
            self.emit(LlvmInstr::Binary {
                dst: self.var_local(dst),
                op: LlvmValueKind::Add,
                ty: LlvmType::i64(),
                lhs: LlvmOperand::Local(loc),
                rhs: self.i64_const(0),
            });
        } else {
            // Empty list.
            self.emit(LlvmInstr::Binary {
                dst: self.var_local(dst),
                op: LlvmValueKind::Add,
                ty: LlvmType::i64(),
                lhs: current,
                rhs: self.i64_const(0),
            });
        }
    }

    fn emit_make_ctor(&mut self, dst: LirVar, ctor_tag: i32, fields: &[LirVar]) {
        // Built-in constructors with dedicated C runtime functions
        // that use flux_gc_alloc_header (correct RC metadata).
        match (ctor_tag, fields.len()) {
            (1, 1) => {
                // Some(val)
                self.call_c(
                    Some(self.var_local(dst)),
                    "flux_wrap_some",
                    vec![(LlvmType::i64(), self.var(fields[0]))],
                    LlvmType::i64(),
                );
                return;
            }
            (2, 1) => {
                // Left(val)
                self.call_c(
                    Some(self.var_local(dst)),
                    "flux_make_left",
                    vec![(LlvmType::i64(), self.var(fields[0]))],
                    LlvmType::i64(),
                );
                return;
            }
            (3, 1) => {
                // Right(val)
                self.call_c(
                    Some(self.var_local(dst)),
                    "flux_make_right",
                    vec![(LlvmType::i64(), self.var(fields[0]))],
                    LlvmType::i64(),
                );
                return;
            }
            (4, 2) => {
                // Cons(head, tail)
                self.call_fastcc(
                    Some(self.var_local(dst)),
                    "flux_make_cons",
                    vec![
                        (LlvmType::i64(), self.var(fields[0])),
                        (LlvmType::i64(), self.var(fields[1])),
                    ],
                    LlvmType::i64(),
                );
                return;
            }
            _ => {}
        }
        {
            // User-defined ADTs: use flux_make_adt LLVM helper.
            // flux_make_adt(ptr fields, i32 field_count, i32 ctor_tag) → i64
            let arr = self.tmp();
            if fields.is_empty() {
                self.call_fastcc(
                    Some(self.var_local(dst)),
                    "flux_make_adt",
                    vec![
                        (LlvmType::Ptr, LlvmOperand::Const(LlvmConst::Null)),
                        (LlvmType::i32(), self.i32_const(0)),
                        (LlvmType::i32(), self.i32_const(ctor_tag)),
                    ],
                    LlvmType::i64(),
                );
            } else {
                self.emit(LlvmInstr::Alloca {
                    dst: arr.clone(),
                    ty: LlvmType::Array {
                        len: fields.len() as u64,
                        element: Box::new(LlvmType::i64()),
                    },
                    count: None,
                    align: Some(8),
                });
                for (i, field) in fields.iter().enumerate() {
                    let gep = self.tmp();
                    self.emit(LlvmInstr::GetElementPtr {
                        dst: gep.clone(),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: LlvmOperand::Local(arr.clone()),
                        indices: vec![(LlvmType::i32(), self.i32_const(i as i32))],
                    });
                    self.emit(LlvmInstr::Store {
                        ty: LlvmType::i64(),
                        value: self.var(*field),
                        ptr: LlvmOperand::Local(gep),
                        align: Some(8),
                    });
                }
                self.call_fastcc(
                    Some(self.var_local(dst)),
                    "flux_make_adt",
                    vec![
                        (LlvmType::Ptr, LlvmOperand::Local(arr)),
                        (LlvmType::i32(), self.i32_const(fields.len() as i32)),
                        (LlvmType::i32(), self.i32_const(ctor_tag)),
                    ],
                    LlvmType::i64(),
                );
            }
        }
    }

    // ── Terminator emission ─────────────────────────────────────────

    fn emit_terminator(&mut self, term: &LirTerminator) -> LlvmTerminator {
        match term {
            LirTerminator::Return(val) => LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: self.var(*val),
            },

            LirTerminator::Jump(target) => LlvmTerminator::Br {
                target: self.label(*target),
            },

            LirTerminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let cond_i1 = self.tmp();
                self.emit(LlvmInstr::Cast {
                    dst: cond_i1.clone(),
                    op: LlvmValueKind::Trunc,
                    from_ty: LlvmType::i64(),
                    operand: self.var(*cond),
                    to_ty: LlvmType::i1(),
                });
                LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: LlvmOperand::Local(cond_i1),
                    then_label: self.label(*then_block),
                    else_label: self.label(*else_block),
                }
            }

            LirTerminator::Switch {
                scrutinee,
                cases,
                default,
            } => LlvmTerminator::Switch {
                ty: LlvmType::i64(),
                scrutinee: self.var(*scrutinee),
                default: self.label(*default),
                cases: cases
                    .iter()
                    .map(|(val, target)| {
                        (
                            LlvmConst::Int {
                                bits: 64,
                                value: *val as i128,
                            },
                            self.label(*target),
                        )
                    })
                    .collect(),
            },

            LirTerminator::Call {
                dst,
                func,
                args,
                cont,
                kind,
                yield_cont: _,
            } => {
                match kind {
                    CallKind::Direct { func_id } => {
                        // Direct call: call @flux_<name>(i64 %a0, i64 %a1, ...)
                        let target = self
                            .program
                            .func_by_id(*func_id)
                            .expect("Direct call references unknown LirFuncId");
                        // In worker mode, if the callee is also worker-eligible,
                        // call the worker variant directly to bypass tag/untag.
                        let use_worker = self.worker_mode && self.worker_eligible.contains(func_id);
                        let target_name = if use_worker {
                            format!("flux_{}$w", target.qualified_name)
                        } else {
                            format!("flux_{}", target.qualified_name)
                        };
                        let call_args: Vec<(LlvmType, LlvmOperand)> = args
                            .iter()
                            .map(|a| (LlvmType::i64(), self.var(*a)))
                            .collect();
                        self.call_fastcc(
                            Some(self.var_local(*dst)),
                            &target_name,
                            call_args,
                            LlvmType::i64(),
                        );
                    }
                    CallKind::DirectExtern { symbol } => {
                        let call_args: Vec<(LlvmType, LlvmOperand)> = args
                            .iter()
                            .map(|a| (LlvmType::i64(), self.var(*a)))
                            .collect();
                        self.call_extern_fastcc_user(
                            Some(self.var_local(*dst)),
                            symbol,
                            call_args,
                            LlvmType::i64(),
                        );
                    }
                    CallKind::Indirect => {
                        // Closure dispatch: flux_call_closure(closure, args_array, nargs)
                        let llvm_args = self.build_call_args(args);
                        self.call_fastcc(
                            Some(self.var_local(*dst)),
                            "flux_call_closure",
                            vec![
                                (LlvmType::i64(), self.var(*func)),
                                (LlvmType::Ptr, LlvmOperand::Local(llvm_args.0)),
                                (LlvmType::i32(), self.i32_const(args.len() as i32)),
                            ],
                            LlvmType::i64(),
                        );
                    }
                }
                // TODO(0134): yield check emission when yield_cont is Some
                LlvmTerminator::Br {
                    target: self.label(*cont),
                }
            }

            LirTerminator::TailCall { func, args, kind } => {
                let result = self.tmp();
                match kind {
                    CallKind::Direct { func_id } => {
                        let target = self
                            .program
                            .func_by_id(*func_id)
                            .expect("Direct tail call references unknown LirFuncId");
                        let use_worker = self.worker_mode && self.worker_eligible.contains(func_id);
                        let target_name = if use_worker {
                            format!("flux_{}$w", target.qualified_name)
                        } else {
                            format!("flux_{}", target.qualified_name)
                        };
                        let call_args: Vec<(LlvmType, LlvmOperand)> = args
                            .iter()
                            .map(|a| (LlvmType::i64(), self.var(*a)))
                            .collect();
                        if !is_fastcc_prelude_helper(&target_name) {
                            self.needed_user_fastcc_decls
                                .insert((target_name.clone(), call_args.len()));
                        }
                        self.emit(LlvmInstr::Call {
                            dst: Some(result.clone()),
                            tail: true,
                            call_conv: Some(CallConv::Fastcc),
                            ret_ty: LlvmType::i64(),
                            callee: LlvmOperand::Global(GlobalId(target_name)),
                            args: call_args,
                            attrs: Vec::new(),
                        });
                    }
                    CallKind::DirectExtern { symbol } => {
                        let call_args: Vec<(LlvmType, LlvmOperand)> = args
                            .iter()
                            .map(|a| (LlvmType::i64(), self.var(*a)))
                            .collect();
                        self.needed_user_fastcc_decls
                            .insert((symbol.clone(), call_args.len()));
                        self.emit(LlvmInstr::Call {
                            dst: Some(result.clone()),
                            tail: true,
                            call_conv: Some(CallConv::Fastcc),
                            ret_ty: LlvmType::i64(),
                            callee: LlvmOperand::Global(GlobalId(symbol.clone())),
                            args: call_args,
                            attrs: Vec::new(),
                        });
                    }
                    CallKind::Indirect => {
                        // Indirect calls go through flux_call_closure which has
                        // a different prototype — do NOT mark as tail call.
                        let llvm_args = self.build_call_args(args);
                        self.call_fastcc(
                            Some(result.clone()),
                            "flux_call_closure",
                            vec![
                                (LlvmType::i64(), self.var(*func)),
                                (LlvmType::Ptr, LlvmOperand::Local(llvm_args.0)),
                                (LlvmType::i32(), self.i32_const(args.len() as i32)),
                            ],
                            LlvmType::i64(),
                        );
                    }
                }
                LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: LlvmOperand::Local(result),
                }
            }

            LirTerminator::MatchCtor {
                scrutinee,
                arms,
                default,
            } => self.emit_match_ctor(*scrutinee, arms, *default),

            LirTerminator::Unreachable => LlvmTerminator::Unreachable,
        }
    }

    fn build_call_args(&mut self, args: &[LirVar]) -> (LlvmLocal, usize) {
        let arr = self.tmp();
        if args.is_empty() {
            self.emit(LlvmInstr::Alloca {
                dst: arr.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
        } else {
            self.emit(LlvmInstr::Alloca {
                dst: arr.clone(),
                ty: LlvmType::Array {
                    len: args.len() as u64,
                    element: Box::new(LlvmType::i64()),
                },
                count: None,
                align: Some(8),
            });
            for (i, arg) in args.iter().enumerate() {
                let gep = self.tmp();
                self.emit(LlvmInstr::GetElementPtr {
                    dst: gep.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(arr.clone()),
                    indices: vec![(LlvmType::i32(), self.i32_const(i as i32))],
                });
                self.emit(LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: self.var(*arg),
                    ptr: LlvmOperand::Local(gep),
                    align: Some(8),
                });
            }
        }
        (arr, args.len())
    }

    fn emit_match_ctor(
        &mut self,
        scrutinee: LirVar,
        arms: &[CtorArm],
        default: BlockId,
    ) -> LlvmTerminator {
        let mut none_arm: Option<&CtorArm> = None;
        let mut empty_list_arm: Option<&CtorArm> = None;
        let mut boxed_arms: Vec<&CtorArm> = Vec::new();

        for arm in arms {
            match &arm.tag {
                CtorTag::None => none_arm = Some(arm),
                CtorTag::EmptyList => empty_list_arm = Some(arm),
                _ => boxed_arms.push(arm),
            }
        }

        // With pointer tagging, sentinels are small even integers:
        //   None=0, false=2, true=4, EmptyList=6, Uninit=8, Yield=10
        // Pointers are even values >= 12.
        // We switch directly on the i64 value for sentinel arms,
        // then fall through to boxed dispatch for heap pointers.

        let mut switch_cases: Vec<(LlvmConst, LabelId)> = Vec::new();

        // For None/EmptyList: compare against sentinel values directly.
        if let Some(arm) = none_arm {
            switch_cases.push((
                LlvmConst::Int {
                    bits: 64,
                    value: FLUX_NONE as i128,
                },
                self.label(arm.target),
            ));
        }
        if let Some(arm) = empty_list_arm {
            switch_cases.push((
                LlvmConst::Int {
                    bits: 64,
                    value: FLUX_EMPTY_LIST as i128,
                },
                self.label(arm.target),
            ));
        }

        if boxed_arms.is_empty() {
            // Only sentinel arms — switch directly on the scrutinee value.
            return LlvmTerminator::Switch {
                ty: LlvmType::i64(),
                scrutinee: self.var(scrutinee),
                default: self.label(default),
                cases: switch_cases,
            };
        }

        // Separate tuple arms from ADT arms — they use different memory layouts.
        let mut tuple_arm: Option<&CtorArm> = None;
        let mut adt_arms: Vec<&CtorArm> = Vec::new();
        for arm in &boxed_arms {
            if matches!(&arm.tag, CtorTag::Tuple) {
                tuple_arm = Some(arm);
            } else {
                adt_arms.push(arm);
            }
        }

        // ADT tag extraction is deferred to the adt_dispatch block
        // (only reached when we've confirmed the value is a heap pointer).
        let adt_tag = self.tmp();

        // Boxed dispatch block label.
        let boxed_label = LabelId(format!("match.boxed.{}", self.next_tmp));
        self.next_tmp += 1;

        // Helper: emit an extraction block for field binders.
        // Inline field access (Phase 4, Proposal 0140): inttoptr → GEP payload → GEP index → load.
        let emit_extract_block = |this: &mut Self, arm: &CtorArm, is_tuple: bool| -> LabelId {
            if arm.field_binders.is_empty() {
                return this.label(arm.target);
            }
            let extract_label =
                LabelId(format!("match.extract.{}.{}", arm.target.0, this.next_tmp));
            this.next_tmp += 1;

            let mut extract_instrs = Vec::new();

            // Convert scrutinee i64 to ptr once per extraction block.
            let base_ptr = LlvmLocal(format!("ext.ptr.{}", arm.target.0));
            extract_instrs.push(LlvmInstr::Cast {
                dst: base_ptr.clone(),
                op: LlvmValueKind::IntToPtr,
                from_ty: LlvmType::i64(),
                operand: this.var(scrutinee),
                to_ty: LlvmType::Ptr,
            });

            // GEP to payload base once.
            let (struct_ty, payload_field) = if is_tuple {
                (LlvmType::Named("FluxTuple".into()), 5)
            } else {
                (LlvmType::Named("FluxAdt".into()), 2)
            };
            let payload_ptr = LlvmLocal(format!("ext.payload.{}", arm.target.0));
            extract_instrs.push(LlvmInstr::GetElementPtr {
                dst: payload_ptr.clone(),
                inbounds: true,
                element_ty: struct_ty,
                base: LlvmOperand::Local(base_ptr),
                indices: vec![
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: payload_field,
                        }),
                    ),
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                ],
            });

            for (i, binder) in arm.field_binders.iter().enumerate() {
                // GEP to field[i].
                let field_ptr = LlvmLocal(format!("ext.{}.{}", arm.target.0, i));
                extract_instrs.push(LlvmInstr::GetElementPtr {
                    dst: field_ptr.clone(),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: LlvmOperand::Local(payload_ptr.clone()),
                    indices: vec![(
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: i as i128,
                        }),
                    )],
                });
                extract_instrs.push(LlvmInstr::Load {
                    dst: this.var_local(*binder),
                    ty: LlvmType::i64(),
                    ptr: LlvmOperand::Local(field_ptr),
                    align: Some(8),
                });
                extract_instrs.push(LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(GlobalId("flux_dup".to_string())),
                    args: vec![(LlvmType::i64(), this.var(*binder))],
                    attrs: Vec::new(),
                });
            }

            this.extra_blocks.push(LlvmBlock {
                label: extract_label.clone(),
                instrs: extract_instrs,
                term: LlvmTerminator::Br {
                    target: this.label(arm.target),
                },
            });
            extract_label
        };

        // Build ADT dispatch cases.
        let mut adt_cases: Vec<(LlvmConst, LabelId)> = Vec::new();
        for arm in &adt_arms {
            let tag_val = match &arm.tag {
                CtorTag::Some => SOME_TAG,
                CtorTag::Left => LEFT_TAG,
                CtorTag::Right => RIGHT_TAG,
                CtorTag::Cons => CONS_TAG,
                CtorTag::Named(name) => self
                    .program
                    .constructor_tags
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(5),
                _ => continue,
            };
            let target = emit_extract_block(self, arm, false);
            adt_cases.push((
                LlvmConst::Int {
                    bits: 32,
                    value: tag_val as i128,
                },
                target,
            ));
        }

        // The ADT dispatch block: extract ctor_tag THEN switch.
        // Inline ADT tag extraction (Phase 4, Proposal 0140):
        // inttoptr → GEP %FluxAdt field 0 → load i32
        let adt_dispatch_label = LabelId(format!("match.adt.{}", self.next_tmp));
        self.next_tmp += 1;
        let adt_ptr = LlvmLocal(format!("match.adtptr.{}", self.next_tmp));
        self.next_tmp += 1;
        let adt_tag_ptr = LlvmLocal(format!("match.adttagptr.{}", self.next_tmp));
        self.next_tmp += 1;
        let adt_tag_instrs = vec![
            LlvmInstr::Cast {
                dst: adt_ptr.clone(),
                op: LlvmValueKind::IntToPtr,
                from_ty: LlvmType::i64(),
                operand: self.var(scrutinee),
                to_ty: LlvmType::Ptr,
            },
            LlvmInstr::GetElementPtr {
                dst: adt_tag_ptr.clone(),
                inbounds: true,
                element_ty: LlvmType::Named("FluxAdt".into()),
                base: LlvmOperand::Local(adt_ptr),
                indices: vec![
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                    (
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                    ),
                ],
            },
            LlvmInstr::Load {
                dst: adt_tag.clone(),
                ty: LlvmType::i32(),
                ptr: LlvmOperand::Local(adt_tag_ptr),
                align: Some(4),
            },
        ];
        self.extra_blocks.push(LlvmBlock {
            label: adt_dispatch_label.clone(),
            instrs: adt_tag_instrs,
            term: LlvmTerminator::Switch {
                ty: LlvmType::i32(),
                scrutinee: LlvmOperand::Local(adt_tag),
                default: self.label(default),
                cases: adt_cases,
            },
        });

        // Boxed dispatch: if there's a Tuple arm, check obj_tag first.
        // Tuples have FluxHeader obj_tag = 0xF3 (FLUX_OBJ_TUPLE), while ADTs
        // have 0xF2. We read the obj_tag to distinguish them before reading ctor_tag.
        if let Some(t_arm) = tuple_arm {
            let tuple_target = emit_extract_block(self, t_arm, true);

            // Build instructions for the boxed block inline.
            // With pointer tagging, the value IS the pointer — just inttoptr.
            let ptr_tmp = LlvmLocal(format!("match.ptr.{}", self.next_tmp));
            self.next_tmp += 1;
            let obj_tag_ptr = LlvmLocal(format!("match.otptr.{}", self.next_tmp));
            self.next_tmp += 1;
            let obj_tag_tmp = LlvmLocal(format!("match.ot.{}", self.next_tmp));
            self.next_tmp += 1;
            let is_tuple = LlvmLocal(format!("match.istup.{}", self.next_tmp));
            self.next_tmp += 1;

            let boxed_instrs = vec![
                // Convert i64 to ptr (pointer IS the tagged value).
                LlvmInstr::Cast {
                    dst: ptr_tmp.clone(),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: self.var(scrutinee),
                    to_ty: LlvmType::Ptr,
                },
                // GEP to ptr - 3 (obj_tag field in FluxHeader).
                LlvmInstr::GetElementPtr {
                    dst: obj_tag_ptr.clone(),
                    inbounds: false,
                    element_ty: LlvmType::i8(),
                    base: LlvmOperand::Local(ptr_tmp),
                    indices: vec![(
                        LlvmType::i32(),
                        LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: -3_i32 as i128,
                        }),
                    )],
                },
                // Load obj_tag byte.
                LlvmInstr::Load {
                    dst: obj_tag_tmp.clone(),
                    ty: LlvmType::i8(),
                    ptr: LlvmOperand::Local(obj_tag_ptr),
                    align: Some(1),
                },
                // Compare with FLUX_OBJ_TUPLE (0xF3).
                LlvmInstr::Icmp {
                    dst: is_tuple.clone(),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i8(),
                    lhs: LlvmOperand::Local(obj_tag_tmp),
                    rhs: LlvmOperand::Const(LlvmConst::Int {
                        bits: 8,
                        value: 0xF3,
                    }),
                },
            ];

            self.extra_blocks.push(LlvmBlock {
                label: boxed_label.clone(),
                instrs: boxed_instrs,
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: LlvmOperand::Local(is_tuple),
                    then_label: tuple_target,
                    else_label: adt_dispatch_label,
                },
            });
        } else {
            // No tuple arm — boxed dispatch is the ADT dispatch directly.
            self.extra_blocks.push(LlvmBlock {
                label: boxed_label.clone(),
                instrs: Vec::new(),
                term: LlvmTerminator::Br {
                    target: adt_dispatch_label,
                },
            });
        }

        // If there are sentinel arms, we need a two-level dispatch:
        // switch on the value for sentinels, default to boxed dispatch.
        if !switch_cases.is_empty() {
            LlvmTerminator::Switch {
                ty: LlvmType::i64(),
                scrutinee: self.var(scrutinee),
                default: boxed_label,
                cases: switch_cases,
            }
        } else {
            // No explicit sentinel arms, but with pointer tagging the
            // scrutinee could be a sentinel (None=0, EmptyList=6, etc.)
            // handled by the default arm.  Guard: if scrutinee < 12
            // (i.e. it's a sentinel, not a heap pointer), jump to default.
            let guard_ok = LlvmLocal(format!("match.ptrck.{}", self.next_tmp));
            self.next_tmp += 1;

            self.emit(LlvmInstr::Icmp {
                dst: guard_ok.clone(),
                op: LlvmCmpOp::Ule,
                ty: LlvmType::i64(),
                lhs: LlvmOperand::Const(LlvmConst::Int {
                    bits: 64,
                    value: FLUX_MIN_PTR as i128,
                }),
                rhs: self.var(scrutinee),
            });

            LlvmTerminator::CondBr {
                cond_ty: LlvmType::i1(),
                cond: LlvmOperand::Local(guard_ok),
                then_label: boxed_label,
                else_label: self.label(default),
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Helper to extract scan_fields from an Alloc instruction.
impl LirInstr {
    fn scan_fields_val(&self) -> u8 {
        if let LirInstr::Alloc { scan_fields, .. } = self {
            *scan_fields
        } else {
            0
        }
    }

    fn obj_tag_val(&self) -> u8 {
        if let LirInstr::Alloc { obj_tag, .. } = self {
            *obj_tag
        } else {
            0
        }
    }
}

/// Returns true if the function is a fastcc LLVM prelude helper (emitted by
/// `core_to_llvm/codegen/prelude.rs` or `adt.rs`), as opposed to a true C
/// runtime function that uses the C calling convention.
fn is_fastcc_prelude_helper(name: &str) -> bool {
    matches!(
        name,
        // NaN-box tag/untag
        "flux_tag_int" | "flux_untag_int" | "flux_is_ptr"
        | "flux_tag_thunk" | "flux_untag_thunk_ptr" | "flux_is_thunk"
        | "flux_tag_boxed_ptr" | "flux_untag_boxed_ptr"
        // Integer arithmetic
        | "flux_iadd" | "flux_isub" | "flux_imul" | "flux_idiv" | "flux_imod" | "flux_ineg"
        // Float arithmetic
        | "flux_fadd" | "flux_fsub" | "flux_fmul" | "flux_fdiv" | "flux_fmod" | "flux_fneg"
        // Polymorphic arithmetic (mixed int/float)
        | "flux_add" | "flux_sub" | "flux_mul" | "flux_div" | "flux_mod" | "flux_neg"
        // Logic
        | "flux_not" | "flux_and" | "flux_or"
        // RC helpers
        | "flux_drop_reuse" | "flux_rc_is_unique"
        // Bump allocator
        | "flux_bump_alloc_inline"
        // ADT/Tuple/Closure construction
        | "flux_make_adt" | "flux_make_cons" | "flux_make_tuple"
        | "flux_tuple_len"
        | "flux_copy_i64s"
        | "flux_make_closure" | "flux_call_closure"
    )
}

/// Map a `CorePrimOp` to its C runtime function name.
fn primop_c_name(op: &CorePrimOp) -> String {
    // First try the promoted primop name → builtin mapping.
    let flux_name = match op {
        CorePrimOp::Print => "print",
        CorePrimOp::Println => "println",
        CorePrimOp::ToString => "to_string",
        CorePrimOp::ReadFile => "read_file",
        CorePrimOp::WriteFile => "write_file",
        CorePrimOp::ReadStdin => "read_stdin",
        CorePrimOp::ReadLines => "read_lines",
        CorePrimOp::StringLength => "string_length",
        CorePrimOp::StringConcat => "string_concat",
        CorePrimOp::StringSlice => "string_slice",
        CorePrimOp::Split => "split",
        CorePrimOp::Join => "join",
        CorePrimOp::Trim => "trim",
        CorePrimOp::Upper => "upper",
        CorePrimOp::Lower => "lower",
        CorePrimOp::StartsWith => "starts_with",
        CorePrimOp::EndsWith => "ends_with",
        CorePrimOp::Replace => "replace",
        CorePrimOp::Substring => "substring",
        CorePrimOp::Chars => "chars",
        CorePrimOp::StrContains => "str_contains",
        CorePrimOp::ArrayLen => "array_len",
        CorePrimOp::ArrayGet => "array_get",
        CorePrimOp::ArraySet => "array_set",
        CorePrimOp::ArrayPush => "push",
        CorePrimOp::ArrayConcat => "concat",
        CorePrimOp::ArraySlice => "slice",
        CorePrimOp::HamtGet => "get",
        CorePrimOp::HamtSet => "put",
        CorePrimOp::HamtDelete => "delete",
        CorePrimOp::HamtKeys => "keys",
        CorePrimOp::HamtValues => "values",
        CorePrimOp::HamtMerge => "merge",
        CorePrimOp::HamtSize => "size",
        CorePrimOp::HamtContains => "has_key",
        CorePrimOp::TypeOf => "type_of",
        CorePrimOp::IsInt => "is_int_val",
        CorePrimOp::IsFloat => "is_float_val",
        CorePrimOp::IsString => "is_string_val",
        CorePrimOp::IsBool => "is_bool_val",
        CorePrimOp::IsArray => "is_array",
        CorePrimOp::IsNone => "is_none_val",
        CorePrimOp::IsSome => "is_some",
        CorePrimOp::IsList => "is_list",
        CorePrimOp::IsMap => "is_map",
        CorePrimOp::CmpEq => "cmp_eq",
        CorePrimOp::CmpNe => "cmp_ne",
        CorePrimOp::Panic => "panic",
        CorePrimOp::ClockNow => "now_ms",
        CorePrimOp::Time => "time",
        CorePrimOp::Try => "try",
        CorePrimOp::AssertThrows => "assert_throws",
        CorePrimOp::ParseInt => "parse_int",
        CorePrimOp::ParseInts => "parse_ints",
        CorePrimOp::SplitInts => "split_ints",
        CorePrimOp::ToList => "to_list",
        CorePrimOp::ToArray => "to_array",
        CorePrimOp::Abs => "abs",
        CorePrimOp::Min => "min",
        CorePrimOp::Max => "max",
        CorePrimOp::Len => "len",
        // Arithmetic — dispatch to runtime
        CorePrimOp::Add | CorePrimOp::Concat => return "flux_rt_add".to_string(),
        CorePrimOp::Sub => return "flux_rt_sub".to_string(),
        CorePrimOp::Mul => return "flux_rt_mul".to_string(),
        CorePrimOp::Div => return "flux_rt_div".to_string(),
        CorePrimOp::Mod => return "flux_rt_mod".to_string(),
        CorePrimOp::Neg => return "flux_rt_neg".to_string(),
        CorePrimOp::Eq => return "flux_rt_eq".to_string(),
        CorePrimOp::NEq => return "flux_rt_neq".to_string(),
        CorePrimOp::Lt => return "flux_rt_lt".to_string(),
        CorePrimOp::Le => return "flux_rt_le".to_string(),
        CorePrimOp::Gt => return "flux_rt_gt".to_string(),
        CorePrimOp::Ge => return "flux_rt_ge".to_string(),
        CorePrimOp::Not => return "flux_not".to_string(),
        CorePrimOp::And => return "flux_and".to_string(),
        CorePrimOp::Or => return "flux_or".to_string(),
        // Typed int ops (already untagged by LIR lowerer)
        CorePrimOp::IAdd => return "flux_iadd".to_string(),
        CorePrimOp::ISub => return "flux_isub".to_string(),
        CorePrimOp::IMul => return "flux_imul".to_string(),
        CorePrimOp::IDiv => return "flux_idiv".to_string(),
        CorePrimOp::IMod => return "flux_imod".to_string(),
        CorePrimOp::ICmpEq => return "flux_rt_eq".to_string(),
        CorePrimOp::ICmpNe => return "flux_rt_neq".to_string(),
        CorePrimOp::ICmpLt => return "flux_rt_lt".to_string(),
        CorePrimOp::ICmpLe => return "flux_rt_le".to_string(),
        CorePrimOp::ICmpGt => return "flux_rt_gt".to_string(),
        CorePrimOp::ICmpGe => return "flux_rt_ge".to_string(),
        // Float ops
        CorePrimOp::FAdd => return "flux_fadd".to_string(),
        CorePrimOp::FSub => return "flux_fsub".to_string(),
        CorePrimOp::FMul => return "flux_fmul".to_string(),
        CorePrimOp::FDiv => return "flux_fdiv".to_string(),
        CorePrimOp::FCmpEq => return "flux_rt_eq".to_string(),
        CorePrimOp::FCmpNe => return "flux_rt_neq".to_string(),
        CorePrimOp::FCmpLt => return "flux_rt_lt".to_string(),
        CorePrimOp::FCmpLe => return "flux_rt_le".to_string(),
        CorePrimOp::FCmpGt => return "flux_rt_gt".to_string(),
        CorePrimOp::FCmpGe => return "flux_rt_ge".to_string(),
        // Collection construction
        CorePrimOp::MakeArray => return "flux_make_array".to_string(),
        CorePrimOp::MakeTuple => return "flux_make_tuple".to_string(),
        CorePrimOp::MakeHash => return "flux_make_hash".to_string(),
        CorePrimOp::MakeList => return "flux_make_list".to_string(),
        CorePrimOp::Interpolate => return "flux_to_string".to_string(), // simplified: single-arg toString
        CorePrimOp::Index => return "flux_rt_index".to_string(),
        // Collection helpers (promoted for native)
        CorePrimOp::Reverse => return "flux_reverse".to_string(),
        CorePrimOp::Contains => return "flux_contains".to_string(),
        CorePrimOp::Sort => return "flux_sort_default".to_string(),
        CorePrimOp::SortBy => return "flux_ho_sort_by".to_string(),
        CorePrimOp::HoMap => return "flux_ho_map".to_string(),
        CorePrimOp::HoFilter => return "flux_ho_filter".to_string(),
        CorePrimOp::HoFold => return "flux_ho_fold".to_string(),
        CorePrimOp::HoAny => return "flux_ho_any".to_string(),
        CorePrimOp::HoAll => return "flux_ho_all".to_string(),
        CorePrimOp::HoEach => return "flux_ho_each".to_string(),
        CorePrimOp::HoFind => return "flux_ho_find".to_string(),
        CorePrimOp::HoCount => return "flux_ho_count".to_string(),
        CorePrimOp::Zip => return "flux_zip".to_string(),
        CorePrimOp::Flatten => return "flux_flatten".to_string(),
        CorePrimOp::HoFlatMap => return "flux_ho_flat_map".to_string(),
        // Effect handlers (Koka-style yield model)
        CorePrimOp::EvvGet => return "flux_evv_get".to_string(),
        CorePrimOp::EvvSet => return "flux_evv_set".to_string(),
        CorePrimOp::FreshMarker => return "flux_fresh_marker".to_string(),
        CorePrimOp::EvvInsert => return "flux_evv_insert".to_string(),
        CorePrimOp::YieldTo => return "flux_yield_to".to_string(),
        CorePrimOp::YieldExtend => return "flux_yield_extend".to_string(),
        CorePrimOp::YieldPrompt => return "flux_yield_prompt".to_string(),
        CorePrimOp::IsYielding => return "flux_is_yielding".to_string(),
        CorePrimOp::PerformDirect => return "flux_perform_direct".to_string(),
    };

    // Look up in builtins table for the C name.
    if let Some(mapping) = builtins::find_builtin(flux_name) {
        mapping.c_name.to_string()
    } else {
        format!("flux_{flux_name}")
    }
}

/// Return a proper declaration for known C runtime functions.
fn known_c_decl(name: &str) -> Option<LlvmDecl> {
    let (ret, params) = match name {
        "flux_string_new" => (LlvmType::i64(), vec![LlvmType::Ptr, LlvmType::i32()]),
        "flux_gc_alloc_header" => (
            LlvmType::Ptr,
            vec![LlvmType::i32(), LlvmType::i32(), LlvmType::i32()],
        ),
        "flux_get_global" => (LlvmType::i64(), vec![LlvmType::i64()]),
        "flux_wrap_some" | "flux_make_left" | "flux_make_right" => {
            (LlvmType::i64(), vec![LlvmType::i64()])
        }
        "flux_make_cons" => (LlvmType::i64(), vec![LlvmType::i64(), LlvmType::i64()]),
        "flux_make_array" | "flux_make_tuple" | "flux_make_hash" | "flux_interpolate" => {
            (LlvmType::i64(), vec![LlvmType::Ptr, LlvmType::i32()])
        }
        "flux_make_adt" => (
            LlvmType::i64(),
            vec![LlvmType::Ptr, LlvmType::i32(), LlvmType::i32()],
        ),
        "flux_rc_is_unique" => (LlvmType::i1(), vec![LlvmType::i64()]),
        "flux_drop_reuse" => (LlvmType::Ptr, vec![LlvmType::i64(), LlvmType::i32()]),
        // Collection helpers
        "flux_reverse" | "flux_sort_default" | "flux_flatten" => {
            (LlvmType::i64(), vec![LlvmType::i64()])
        }
        "flux_contains" | "flux_ho_sort_by" | "flux_ho_map" | "flux_ho_filter" | "flux_ho_any"
        | "flux_ho_all" | "flux_ho_each" | "flux_ho_find" | "flux_ho_count"
        | "flux_ho_flat_map" | "flux_zip" => {
            (LlvmType::i64(), vec![LlvmType::i64(), LlvmType::i64()])
        }
        // Effect handlers (Koka-style yield model)
        "flux_evv_get" => (LlvmType::i64(), vec![]),
        "flux_evv_set" => (LlvmType::Void, vec![LlvmType::i64()]),
        "flux_fresh_marker" => (LlvmType::i64(), vec![]),
        "flux_evv_insert" => (
            LlvmType::i64(),
            vec![
                LlvmType::i64(),
                LlvmType::i64(),
                LlvmType::i64(),
                LlvmType::i64(),
            ],
        ),
        "flux_yield_to" => (
            LlvmType::i64(),
            vec![LlvmType::i64(), LlvmType::i64(), LlvmType::i64()],
        ),
        "flux_yield_extend" => (LlvmType::i64(), vec![LlvmType::i64()]),
        "flux_yield_prompt" => (
            LlvmType::i64(),
            vec![LlvmType::i64(), LlvmType::i64(), LlvmType::i64()],
        ),
        "flux_is_yielding" => (LlvmType::i32(), vec![]),
        "flux_perform_direct" => (
            LlvmType::i64(),
            vec![
                LlvmType::i64(),
                LlvmType::i64(),
                LlvmType::i64(),
                LlvmType::i64(),
            ],
        ),
        // Float boxing/unboxing wrappers (Phase 9 pointer tagging)
        "flux_box_float_rt" => (LlvmType::i64(), vec![LlvmType::Double]),
        "flux_unbox_float_rt" => (LlvmType::Double, vec![LlvmType::i64()]),
        _ => return None,
    };
    Some(LlvmDecl {
        linkage: Linkage::External,
        name: GlobalId(name.to_string()),
        sig: LlvmFunctionSig {
            ret,
            params,
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".to_string()],
    })
}

/// Guess the arity of a C runtime function by name pattern.
fn guess_arity(name: &str) -> usize {
    if name.contains("print")
        || name.contains("neg")
        || name.contains("not")
        || name.contains("is_")
        || name.contains("type_of")
        || name.contains("to_string")
        || name.contains("trim")
        || name.contains("upper")
        || name.contains("lower")
        || name.contains("chars")
        || name.contains("to_list")
        || name.contains("to_array")
        || name.contains("abs")
        || name.contains("len")
        || name.contains("panic")
        || name.contains("read_stdin")
        || name.contains("now_ms")
        || name.contains("time")
    {
        1
    } else if name.contains("add")
        || name.contains("sub")
        || name.contains("mul")
        || name.contains("div")
        || name.contains("mod")
        || name.contains("eq")
        || name.contains("neq")
        || name.contains("lt")
        || name.contains("le")
        || name.contains("gt")
        || name.contains("ge")
        || name.contains("and")
        || name.contains("or")
        || name.contains("concat")
        || name.contains("split")
        || name.contains("join")
        || name.contains("index")
        || name.contains("contains")
        || name.contains("push")
        || name.contains("min")
        || name.contains("max")
    {
        2
    } else if name.contains("slice") || name.contains("replace") || name.contains("substring") {
        3
    } else {
        1 // default fallback
    }
}

/// Generate an empty `flux_main` stub function: `define i64 @flux_main() { ret i64 0 }`.
/// Used when compiling module-only .flx files with `--native` that have no `fn main()`.
pub fn flux_main_stub() -> LlvmFunction {
    LlvmFunction {
        linkage: Linkage::External,
        name: GlobalId("flux_main".to_string()),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        params: vec![],
        attrs: vec![],
        blocks: vec![LlvmBlock {
            label: LabelId("entry".to_string()),
            instrs: vec![],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 0 }),
            },
        }],
    }
}
