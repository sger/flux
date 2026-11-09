use crate::llvm::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmFunction,
    LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator,
    LlvmType, LlvmValueKind, TlsModel,
};

/// Pointer-tag layout constants for the LLVM codegen layer.
/// Encoding:
///   Integer: `(raw << 1) | 1`  (LSB=1, 63-bit signed)
///   Pointer: raw heap address   (LSB=0, even, >= 12)
///   Sentinels: None=0, false=2, true=4, EmptyList=6, Uninit=8, Yield=10
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluxPtrTagLayout;

impl FluxPtrTagLayout {
    pub const FLUX_NONE: i64 = 0;
    pub const FLUX_FALSE: i64 = 2;
    pub const FLUX_TRUE: i64 = 4;
    pub const FLUX_EMPTY_LIST: i64 = 6;
    pub const FLUX_UNINIT: i64 = 8;
    pub const FLUX_YIELD_SENTINEL: i64 = 10;
    pub const FLUX_MIN_PTR: u64 = 12;
    pub const FLUX_OBJ_FLOAT: u8 = 0xF8;
    pub const FLUX_OBJ_THUNK: u8 = 0xF9;
}

/// Backward-compatible alias for downstream code that still references the old name.
pub type FluxNanboxLayout = FluxPtrTagLayout;

const PTRTAG_CONSTANTS: [(&str, i64); 6] = [
    ("flux.ptrtag.none", FluxPtrTagLayout::FLUX_NONE),
    ("flux.ptrtag.false", FluxPtrTagLayout::FLUX_FALSE),
    ("flux.ptrtag.true", FluxPtrTagLayout::FLUX_TRUE),
    ("flux.ptrtag.empty_list", FluxPtrTagLayout::FLUX_EMPTY_LIST),
    ("flux.ptrtag.uninit", FluxPtrTagLayout::FLUX_UNINIT),
    ("flux.ptrtag.yield_sentinel", FluxPtrTagLayout::FLUX_YIELD_SENTINEL),
];

pub fn flux_prelude_symbol(name: &str) -> GlobalId {
    GlobalId(name.to_string())
}

pub fn emit_prelude(module: &mut LlvmModule) {
    emit_ptrtag_constants(module);
    emit_tag_int(module);
    emit_untag_int(module);
    emit_is_ptr(module);
    emit_dup(module);
    emit_drop(module);
    emit_dup_inline(module);
    emit_drop_inline(module);
    emit_drop_reuse(module);
    emit_rc_is_unique(module);
    emit_is_thunk(module);
    emit_tag_thunk(module);
    emit_untag_thunk_ptr(module);
    emit_bump_alloc_inline(module);
}

pub fn emit_prelude_and_arith(module: &mut LlvmModule) {
    emit_prelude(module);
    super::emit_arith(module);
}

fn emit_ptrtag_constants(module: &mut LlvmModule) {
    for (name, value) in PTRTAG_CONSTANTS {
        if has_global(module, name) {
            continue;
        }
        module.globals.push(LlvmGlobal {
            linkage: Linkage::Private,
            name: flux_prelude_symbol(name),
            ty: LlvmType::i64(),
            is_constant: true,
            thread_local: None,
            value: Some(const_i64(value)),
            attrs: vec![],
        });
    }
}

fn emit_tag_int(module: &mut LlvmModule) {
    let name = "flux_tag_int";
    if has_function(module, name) {
        return;
    }
    // Pointer-tag int: (raw << 1) | 1
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("raw".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Binary {
                    dst: LlvmLocal("shifted".into()),
                    op: LlvmValueKind::Shl,
                    ty: LlvmType::i64(),
                    lhs: local("raw"),
                    rhs: const_i64_operand(1),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tagged".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i64(),
                    lhs: local("shifted"),
                    rhs: const_i64_operand(1),
                },
            ],
            term: LlvmTerminator::Ret { ty: LlvmType::i64(), value: local("tagged") },
        }],
    });
}

fn emit_untag_int(module: &mut LlvmModule) {
    let name = "flux_untag_int";
    if has_function(module, name) {
        return;
    }
    // Pointer-tag untag int: val >> 1 (arithmetic shift preserves sign)
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![LlvmInstr::Binary {
                dst: LlvmLocal("raw".into()),
                op: LlvmValueKind::AShr,
                ty: LlvmType::i64(),
                lhs: local("val"),
                rhs: const_i64_operand(1),
            }],
            term: LlvmTerminator::Ret { ty: LlvmType::i64(), value: local("raw") },
        }],
    });
}

fn emit_is_ptr(module: &mut LlvmModule) {
    let name = "flux_is_ptr";
    if has_function(module, name) {
        return;
    }
    // Pointer-tag: is_ptr iff bit0 == 0 AND val >= 12
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i1(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                // %bit0 = and i64 %val, 1
                LlvmInstr::Binary {
                    dst: LlvmLocal("bit0".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(1),
                },
                // %not_int = icmp eq i64 %bit0, 0
                LlvmInstr::Icmp {
                    dst: LlvmLocal("not_int".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("bit0"),
                    rhs: const_i64_operand(0),
                },
                // %above_sentinel = icmp sge i64 %val, 12
                // (signed comparison is fine — heap addresses are always positive)
                LlvmInstr::Icmp {
                    dst: LlvmLocal("above_sentinel".into()),
                    op: LlvmCmpOp::Sge,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxPtrTagLayout::FLUX_MIN_PTR as i64),
                },
                // %is_ptr = and i1 %not_int, %above_sentinel
                LlvmInstr::Binary {
                    dst: LlvmLocal("result".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i1(),
                    lhs: local("not_int"),
                    rhs: local("above_sentinel"),
                },
            ],
            term: LlvmTerminator::Ret { ty: LlvmType::i1(), value: local("result") },
        }],
    });
}

/// Declare `flux_dup(i64) -> void` as an external C function.
/// The implementation lives in `runtime/c/rc.c` (Aether RC).
fn emit_dup(module: &mut LlvmModule) {
    let name = "flux_dup";
    if has_function(module, name) || module.declarations.iter().any(|d| d.name.0 == name) {
        return;
    }
    module.declarations.push(crate::llvm::LlvmDecl {
        linkage: Linkage::External,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".into()],
    });
}

/// Declare `flux_drop(i64) -> void` as an external C function.
/// The implementation lives in `runtime/c/rc.c` (Aether RC).
fn emit_drop(module: &mut LlvmModule) {
    let name = "flux_drop";
    if has_function(module, name) || module.declarations.iter().any(|d| d.name.0 == name) {
        return;
    }
    module.declarations.push(crate::llvm::LlvmDecl {
        linkage: Linkage::External,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".into()],
    });
}

/// The runtime's sticky count (`FLUX_RC_STICKY` in `runtime/c/rc.c`): a
/// count at or past it never changes.
const FLUX_RC_STICKY: i128 = 0x4000_0000;

/// Emit `flux_dup_inline(i64 %val) -> void`: `flux_dup` with its common case,
/// a live count owned by one thread, done in place.
fn emit_dup_inline(module: &mut LlvmModule) {
    emit_rc_inline(module, "flux_dup_inline", "flux_dup", 0, 1);
}

/// Emit `flux_drop_inline(i64 %val) -> void`: `flux_drop` with its common
/// case, a count owned by one thread that stays above zero, done in place.
fn emit_drop_inline(module: &mut LlvmModule) {
    emit_rc_inline(module, "flux_drop_inline", "flux_drop", 1, -1);
}

/// A count above `floor` and below the sticky count belongs to one thread
/// (`rc.c`'s sign encoding), so it changes by `delta` with a plain store. A
/// sticky count is left alone. Anything else calls `slow`: a shared count
/// needs the runtime's atomics, and a drop to zero frees. A block the debug
/// heap has freed keeps count 0, so its check in `slow` still runs.
fn emit_rc_inline(module: &mut LlvmModule, name: &str, slow: &str, floor: i128, delta: i128) {
    if has_function(module, name) {
        return;
    }
    let i32_const = |value: i128| LlvmOperand::Const(LlvmConst::Int { bits: 32, value });
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("is_ptr".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i1(),
                    callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
                    args: vec![(LlvmType::i64(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_ptr"),
                    then_label: LabelId("check_rc".into()),
                    else_label: LabelId("done".into()),
                },
            },
            LlvmBlock {
                label: LabelId("check_rc".into()),
                instrs: {
                    let mut instrs = rc_extract_ptr_instrs();
                    instrs.push(LlvmInstr::Load {
                        dst: LlvmLocal("rc".into()),
                        ty: LlvmType::i32(),
                        ptr: local("rc_ptr"),
                        align: Some(4),
                    });
                    instrs.push(LlvmInstr::Icmp {
                        dst: LlvmLocal("local".into()),
                        op: LlvmCmpOp::Sgt,
                        ty: LlvmType::i32(),
                        lhs: local("rc"),
                        rhs: i32_const(floor),
                    });
                    instrs
                },
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("local"),
                    then_label: LabelId("check_sticky".into()),
                    else_label: LabelId("slow".into()),
                },
            },
            LlvmBlock {
                label: LabelId("check_sticky".into()),
                instrs: vec![LlvmInstr::Icmp {
                    dst: LlvmLocal("sticky".into()),
                    op: LlvmCmpOp::Sge,
                    ty: LlvmType::i32(),
                    lhs: local("rc"),
                    rhs: i32_const(FLUX_RC_STICKY),
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("sticky"),
                    then_label: LabelId("done".into()),
                    else_label: LabelId("update".into()),
                },
            },
            LlvmBlock {
                label: LabelId("update".into()),
                instrs: vec![
                    LlvmInstr::Binary {
                        dst: LlvmLocal("next".into()),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i32(),
                        lhs: local("rc"),
                        rhs: i32_const(delta),
                    },
                    LlvmInstr::Store {
                        ty: LlvmType::i32(),
                        value: local("next"),
                        ptr: local("rc_ptr"),
                        align: Some(4),
                    },
                ],
                term: LlvmTerminator::RetVoid,
            },
            LlvmBlock {
                label: LabelId("slow".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_prelude_symbol(slow)),
                    args: vec![(LlvmType::i64(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::RetVoid,
            },
            LlvmBlock {
                label: LabelId("done".into()),
                instrs: vec![],
                term: LlvmTerminator::RetVoid,
            },
        ],
    });
}

/// Emit `flux_drop_reuse(i64 %val, i32 %size) -> ptr`:
/// If the value's RC == 1 (unique), return the raw pointer for in-place
/// reuse.  Otherwise, drop the value and allocate a fresh block of `size`
/// bytes via flux_gc_alloc.
fn emit_drop_reuse(module: &mut LlvmModule) {
    let name = "flux_drop_reuse";
    if has_function(module, name) {
        return;
    }
    emit_gc_free_decl(module);
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i64(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into()), LlvmLocal("size".into())],
        attrs: helper_attrs(),
        blocks: vec![
            // entry: check if value is a pointer. Non-pointer values
            // (integers, booleans, etc.) cannot be reused — return null.
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("is_ptr".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i1(),
                    callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
                    args: vec![(LlvmType::i64(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_ptr"),
                    then_label: LabelId("check_rc".into()),
                    else_label: LabelId("not_ptr".into()),
                },
            },
            // not_ptr: return null (not reusable).
            LlvmBlock {
                label: LabelId("not_ptr".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::ptr(),
                    value: LlvmOperand::Const(LlvmConst::Null),
                },
            },
            // check_rc: extract pointer, load RC, check if unique.
            LlvmBlock {
                label: LabelId("check_rc".into()),
                instrs: {
                    let mut instrs = rc_extract_ptr_instrs();
                    instrs.push(LlvmInstr::Load {
                        dst: LlvmLocal("rc".into()),
                        ty: LlvmType::i32(),
                        ptr: local("rc_ptr"),
                        align: Some(4),
                    });
                    instrs.push(LlvmInstr::Icmp {
                        dst: LlvmLocal("unique".into()),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::i32(),
                        lhs: local("rc"),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }),
                    });
                    instrs
                },
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("unique"),
                    then_label: LabelId("reuse".into()),
                    else_label: LabelId("fresh".into()),
                },
            },
            // reuse: RC==1, return the pointer directly for in-place reuse.
            LlvmBlock {
                label: LabelId("reuse".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret { ty: LlvmType::ptr(), value: local("ptr") },
            },
            // fresh: RC>1, drop (decrement) and allocate fresh memory.
            LlvmBlock {
                label: LabelId("fresh".into()),
                instrs: vec![
                    LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(flux_prelude_symbol("flux_drop")),
                        args: vec![(LlvmType::i64(), local("val"))],
                        attrs: vec![],
                    },
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("new_mem".into())),
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::ptr(),
                        callee: LlvmOperand::Global(GlobalId("flux_bump_alloc_inline".into())),
                        args: vec![
                            (LlvmType::i32(), local("size")),
                            (
                                LlvmType::i32(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                            ),
                            (
                                LlvmType::i32(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                            ),
                        ],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::Ret { ty: LlvmType::ptr(), value: local("new_mem") },
            },
        ],
    });
}

/// Emit `flux_rc_is_unique(i64 %val) -> i1`:
/// Returns true only if the value is a heap pointer with RC == 1.
/// Used by DropSpecialized to branch on uniqueness.
pub fn emit_rc_is_unique(module: &mut LlvmModule) {
    let name = "flux_rc_is_unique";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i1(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("is_ptr".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i1(),
                    callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
                    args: vec![(LlvmType::i64(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_ptr"),
                    then_label: LabelId("check_rc".into()),
                    else_label: LabelId("not_unique".into()),
                },
            },
            LlvmBlock {
                label: LabelId("check_rc".into()),
                instrs: {
                    let mut instrs = rc_extract_ptr_instrs();
                    instrs.push(LlvmInstr::Load {
                        dst: LlvmLocal("rc".into()),
                        ty: LlvmType::i32(),
                        ptr: local("rc_ptr"),
                        align: Some(4),
                    });
                    instrs.push(LlvmInstr::Icmp {
                        dst: LlvmLocal("is_one".into()),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::i32(),
                        lhs: local("rc"),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }),
                    });
                    instrs
                },
                term: LlvmTerminator::Ret { ty: LlvmType::i1(), value: local("is_one") },
            },
            LlvmBlock {
                label: LabelId("not_unique".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret { ty: LlvmType::i1(), value: const_i1_operand(false) },
            },
        ],
    });
}

/// Declare `flux_gc_free(ptr) -> void` and `flux_gc_alloc(i32) -> ptr`
/// as external C functions (from the Flux C runtime).
fn emit_gc_free_decl(module: &mut LlvmModule) {
    let free_name = "flux_gc_free";
    if !has_function(module, free_name)
        && !module.declarations.iter().any(|d| d.name.0 == free_name)
    {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: flux_prelude_symbol(free_name),
            sig: LlvmFunctionSig {
                ret: LlvmType::Void,
                params: vec![LlvmType::ptr()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        });
    }
    let alloc_name = "flux_gc_alloc";
    if !has_function(module, alloc_name)
        && !module.declarations.iter().any(|d| d.name.0 == alloc_name)
    {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: flux_prelude_symbol(alloc_name),
            sig: LlvmFunctionSig {
                ret: LlvmType::ptr(),
                params: vec![LlvmType::i32()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        });
    }

    // flux_gc_alloc_header(i32 size, i32 scan_fsize, i32 obj_tag) → ptr
    let alloc_hdr_name = "flux_gc_alloc_header";
    if !has_function(module, alloc_hdr_name)
        && !module.declarations.iter().any(|d| d.name.0 == alloc_hdr_name)
    {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: flux_prelude_symbol(alloc_hdr_name),
            sig: LlvmFunctionSig {
                ret: LlvmType::ptr(),
                params: vec![LlvmType::i32(), LlvmType::i32(), LlvmType::i32()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        });
    }
}

// ── Phase 7b: Bump allocator ─────────────────────────────────────────

const FLUX_HEADER_SIZE: i64 = 8;
/// The largest payload the runtime recycles through a free list
/// (`FLUX_SMALL_MAX` in `runtime/c/rc.c`).
const FLUX_SMALL_MAX: i32 = 256;
/// The class-byte bit of a block from the heap's chunks (`FLUX_FROM_CHUNK`).
const FLUX_FROM_CHUNK: i32 = 0x80;
/// `FluxHeap` in `runtime/c/rc.c`: the bump pointer, its limit, then one
/// free list per size class, 0 to 32. Only these words are named here.
const FLUX_HEAP_NAME: &str = "flux_heap";
const FLUX_HEAP_WORDS: u64 = 35;
const FLUX_HEAP_LIMIT: i64 = 8;
const FLUX_HEAP_FREE: i64 = 16;
/// The class-byte bits that name the size class (`FLUX_CLASS_MASK`).
const FLUX_CLASS_MASK: i128 = 0x3f;
const THREADLOCAL_ADDRESS: &str = "llvm.threadlocal.address.p0";

/// Emit external global declarations for the heap's bump pointers and
/// the `flux_bump_alloc_inline` helper function.
fn emit_bump_alloc_inline(module: &mut LlvmModule) {
    // ── External thread-local global: @flux_heap ──
    // All zero on every thread but the heap's owner, so neither the pop nor
    // the bump succeeds there ("Heap" in runtime/c/rc.c). The runtime is
    // linked into the executable, so its offset is fixed on ELF.
    if !has_global(module, FLUX_HEAP_NAME) {
        module.globals.push(LlvmGlobal {
            linkage: Linkage::External,
            name: GlobalId(FLUX_HEAP_NAME.into()),
            ty: LlvmType::Array { len: FLUX_HEAP_WORDS, element: Box::new(LlvmType::Ptr) },
            is_constant: false,
            thread_local: Some(TlsModel::InitialExec),
            value: None,
            attrs: vec![],
        });
    }
    if !module.declarations.iter().any(|d| d.name.0 == THREADLOCAL_ADDRESS) {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId(THREADLOCAL_ADDRESS.into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::Ptr,
                params: vec![LlvmType::Ptr],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        });
    }

    // ── Declare the C runtime fallbacks, zeroing and not ─────────
    for slow_name in ["flux_bump_alloc_slow", "flux_bump_alloc_slow_filled"] {
        if !module.declarations.iter().any(|d| d.name.0 == slow_name) {
            module.declarations.push(crate::llvm::LlvmDecl {
                linkage: Linkage::External,
                name: GlobalId(slow_name.into()),
                sig: LlvmFunctionSig {
                    ret: LlvmType::ptr(),
                    params: vec![LlvmType::i32(), LlvmType::i32(), LlvmType::i32()],
                    varargs: false,
                    call_conv: CallConv::Ccc,
                },
                attrs: vec!["nounwind".into()],
            });
        }
    }

    // ── Declare @llvm.memset.p0.i64 intrinsic ────────────────────
    let memset_name = "llvm.memset.p0.i64";
    if !module.declarations.iter().any(|d| d.name.0 == memset_name) {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId(memset_name.into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::Void,
                params: vec![
                    LlvmType::Ptr,
                    LlvmType::Integer(8),
                    LlvmType::Integer(64),
                    LlvmType::Integer(1),
                ],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nocallback".into(), "nounwind".into()],
        });
    }

    emit_bump_alloc_fn(module, "flux_bump_alloc_inline", true);
    // For callers that write every payload word before anything reads the
    // object; the zeroing would be overwritten at once.
    emit_bump_alloc_fn(module, "flux_bump_alloc_filled", false);
}

/// Defines `fn_name`, the inline allocator, zeroing the payload when `zero`
/// is set, on both paths.
fn emit_bump_alloc_fn(module: &mut LlvmModule, fn_name: &str, zero: bool) {
    if has_function(module, fn_name) {
        return;
    }
    let slow_name = if zero { "flux_bump_alloc_slow" } else { "flux_bump_alloc_slow_filled" };
    let memset_name = "llvm.memset.p0.i64";

    // Signature: fastcc ptr @flux_bump_alloc_inline(i32 %size, i32 %scan, i32 %tag)
    //
    // entry:
    //   %aligned = add i32 %size, 7
    //   %masked  = and i32 %aligned, -8
    //   %total_i32 = add i32 %masked, 8   ; + FLUX_HEADER_SIZE
    //   %total   = zext i32 %total_i32 to i64
    //   %small = icmp ule i32 %masked, 256
    //   %class_all = lshr i32 %masked, 3
    //   %class32 = select i1 %small, i32 %class_all, i32 0
    //   %has_class = icmp ne i32 %class32, 0
    //   %chunk_class = or i32 %class32, 128
    //   %class8 = trunc i32 %chunk_class to i8
    //   %heap = call ptr @llvm.threadlocal.address.p0(ptr @flux_heap)
    //   br i1 %has_class, label %pop_check, label %slow
    //
    // pop_check:                          ; free[class], the runtime's free_list_pop
    //   %class64 = zext i32 %class32 to i64
    //   %free = getelementptr i8, ptr %heap, i64 16
    //   %slot = getelementptr ptr, ptr %free, i64 %class64
    //   %head = load ptr, ptr %slot
    //   %has_head = icmp ne ptr %head, null
    //   br i1 %has_head, label %pop, label %bump_check
    //
    // pop:                                ; a freed block links on in its first payload word
    //   %next_ptr = getelementptr i8, ptr %head, i64 8
    //   %next = load ptr, ptr %next_ptr
    //   store ptr %next, ptr %slot
    //   br label %init
    //
    // bump_check:
    //   %hp      = load ptr, ptr %heap
    //   %new_hp  = getelementptr i8, ptr %hp, i64 %total
    //   %lim_ptr = getelementptr i8, ptr %heap, i64 8
    //   %lim     = load ptr, ptr %lim_ptr
    //   %in_chunk = icmp ule ptr %new_hp, %lim
    //   br i1 %in_chunk, label %bump, label %slow
    //
    // bump:
    //   store ptr %new_hp, ptr %heap
    //   br label %init
    //
    // init:
    //   %block = phi ptr [ %head, %pop ], [ %hp, %bump ]
    //   ; header at %block: refcount=1, scan_fsize, obj_tag, progress=0, size_class
    //   ...
    //   %payload = getelementptr i8, ptr %block, i64 8
    //   call void @llvm.memset.p0.i64(ptr %payload, i8 0, i64 %aligned64, i1 false) ; if zero
    //   ret ptr %payload
    //
    // slow:
    //   %p = call ccc ptr @flux_bump_alloc_slow(i32 %size, i32 %scan, i32 %tag)
    //   ret ptr %p                        ; @flux_bump_alloc_slow_filled if !zero

    let i32_ty = LlvmType::i32();
    let i64_ty = LlvmType::i64();
    let ptr_ty = LlvmType::Ptr;
    let i8_ty = LlvmType::Integer(8);
    let i1_ty = LlvmType::i1();
    let i32_const = |value: i128| LlvmOperand::Const(LlvmConst::Int { bits: 32, value });
    let byte_at = |dst: &str, base: &str, offset: i64| LlvmInstr::GetElementPtr {
        dst: LlvmLocal(dst.into()),
        inbounds: false,
        element_ty: LlvmType::Integer(8),
        base: local(base),
        indices: vec![(LlvmType::i64(), const_i64_operand(offset))],
    };
    let label = |name: &str| LabelId(name.into());

    let mut init = vec![
        LlvmInstr::Phi {
            dst: LlvmLocal("block".into()),
            ty: ptr_ty.clone(),
            incoming: vec![(local("head"), label("pop")), (local("hp"), label("bump"))],
        },
        // store i32 1, ptr %block  (refcount = 1)
        LlvmInstr::Store {
            ty: i32_ty.clone(),
            value: i32_const(1),
            ptr: local("block"),
            align: Some(4),
        },
        LlvmInstr::Cast {
            dst: LlvmLocal("scan8".into()),
            op: LlvmValueKind::Trunc,
            from_ty: i32_ty.clone(),
            operand: local("scan"),
            to_ty: i8_ty.clone(),
        },
        byte_at("sf_ptr", "block", 4),
        LlvmInstr::Store {
            ty: i8_ty.clone(),
            value: local("scan8"),
            ptr: local("sf_ptr"),
            align: Some(1),
        },
        LlvmInstr::Cast {
            dst: LlvmLocal("tag8".into()),
            op: LlvmValueKind::Trunc,
            from_ty: i32_ty.clone(),
            operand: local("tag"),
            to_ty: i8_ty.clone(),
        },
        byte_at("ot_ptr", "block", 5),
        LlvmInstr::Store {
            ty: i8_ty.clone(),
            value: local("tag8"),
            ptr: local("ot_ptr"),
            align: Some(1),
        },
        // progress = 0
        byte_at("prog_ptr", "block", 6),
        LlvmInstr::Store {
            ty: i8_ty.clone(),
            value: LlvmOperand::Const(LlvmConst::Int { bits: 8, value: 0 }),
            ptr: local("prog_ptr"),
            align: Some(1),
        },
        // size_class, marked as a chunk block
        byte_at("class_ptr", "block", 7),
        LlvmInstr::Store {
            ty: i8_ty.clone(),
            value: local("class8"),
            ptr: local("class_ptr"),
            align: Some(1),
        },
        byte_at("payload", "block", FLUX_HEADER_SIZE),
    ];
    if zero {
        init.extend([
            LlvmInstr::Cast {
                dst: LlvmLocal("aligned64".into()),
                op: LlvmValueKind::ZExt,
                from_ty: i32_ty.clone(),
                operand: local("masked"),
                to_ty: i64_ty.clone(),
            },
            LlvmInstr::Call {
                dst: None,
                tail: false,
                call_conv: Some(CallConv::Ccc),
                ret_ty: LlvmType::Void,
                callee: LlvmOperand::Global(GlobalId(memset_name.into())),
                args: vec![
                    (ptr_ty.clone(), local("payload")),
                    (i8_ty.clone(), LlvmOperand::Const(LlvmConst::Int { bits: 8, value: 0 })),
                    (i64_ty.clone(), local("aligned64")),
                    (i1_ty.clone(), LlvmOperand::Const(LlvmConst::Int { bits: 1, value: 0 })),
                ],
                attrs: vec![],
            },
        ]);
    }

    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: GlobalId(fn_name.into()),
        sig: LlvmFunctionSig {
            ret: ptr_ty.clone(),
            params: vec![i32_ty.clone(), i32_ty.clone(), i32_ty.clone()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("size".into()), LlvmLocal("scan".into()), LlvmLocal("tag".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: label("entry"),
                instrs: vec![
                    LlvmInstr::Binary {
                        dst: LlvmLocal("aligned".into()),
                        op: LlvmValueKind::Add,
                        ty: i32_ty.clone(),
                        lhs: local("size"),
                        rhs: i32_const(7),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("masked".into()),
                        op: LlvmValueKind::And,
                        ty: i32_ty.clone(),
                        lhs: local("aligned"),
                        rhs: i32_const(-8i32 as i128),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("total_i32".into()),
                        op: LlvmValueKind::Add,
                        ty: i32_ty.clone(),
                        lhs: local("masked"),
                        rhs: i32_const(FLUX_HEADER_SIZE as i128),
                    },
                    LlvmInstr::Cast {
                        dst: LlvmLocal("total".into()),
                        op: LlvmValueKind::ZExt,
                        from_ty: i32_ty.clone(),
                        operand: local("total_i32"),
                        to_ty: i64_ty.clone(),
                    },
                    // The runtime's size_class_of. Only a block with a class
                    // is kept on a list or carved from a chunk; the size is a
                    // constant at every call, so this folds away.
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("small".into()),
                        op: LlvmCmpOp::Ule,
                        ty: i32_ty.clone(),
                        lhs: local("masked"),
                        rhs: i32_const(FLUX_SMALL_MAX as i128),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("class_all".into()),
                        op: LlvmValueKind::LShr,
                        ty: i32_ty.clone(),
                        lhs: local("masked"),
                        rhs: i32_const(3),
                    },
                    LlvmInstr::Select {
                        dst: LlvmLocal("class32".into()),
                        cond_ty: i1_ty.clone(),
                        cond: local("small"),
                        value_ty: i32_ty.clone(),
                        then_value: local("class_all"),
                        else_value: i32_const(0),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("has_class".into()),
                        op: LlvmCmpOp::Ne,
                        ty: i32_ty.clone(),
                        lhs: local("class32"),
                        rhs: i32_const(0),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("chunk_class".into()),
                        op: LlvmValueKind::Or,
                        ty: i32_ty.clone(),
                        lhs: local("class32"),
                        rhs: i32_const(FLUX_FROM_CHUNK as i128),
                    },
                    LlvmInstr::Cast {
                        dst: LlvmLocal("class8".into()),
                        op: LlvmValueKind::Trunc,
                        from_ty: i32_ty.clone(),
                        operand: local("chunk_class"),
                        to_ty: i8_ty.clone(),
                    },
                    // Once per function after inlining: a native function
                    // never changes thread mid-call, since a fiber suspends by
                    // returning its continuation (0162).
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("heap".into())),
                        tail: false,
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: ptr_ty.clone(),
                        callee: LlvmOperand::Global(GlobalId(THREADLOCAL_ADDRESS.into())),
                        args: vec![(
                            ptr_ty.clone(),
                            LlvmOperand::Global(GlobalId(FLUX_HEAP_NAME.into())),
                        )],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: i1_ty.clone(),
                    cond: local("has_class"),
                    then_label: label("pop_check"),
                    else_label: label("slow"),
                },
            },
            // Once the first chunks are carved, nearly every block is a freed
            // one, so the class's list comes before the bump.
            LlvmBlock {
                label: label("pop_check"),
                instrs: vec![
                    LlvmInstr::Cast {
                        dst: LlvmLocal("class64".into()),
                        op: LlvmValueKind::ZExt,
                        from_ty: i32_ty.clone(),
                        operand: local("class32"),
                        to_ty: i64_ty.clone(),
                    },
                    byte_at("free", "heap", FLUX_HEAP_FREE),
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("slot".into()),
                        inbounds: false,
                        element_ty: ptr_ty.clone(),
                        base: local("free"),
                        indices: vec![(i64_ty.clone(), local("class64"))],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("head".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("slot"),
                        align: Some(8),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("has_head".into()),
                        op: LlvmCmpOp::Ne,
                        ty: ptr_ty.clone(),
                        lhs: local("head"),
                        rhs: LlvmOperand::Const(LlvmConst::Null),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: i1_ty.clone(),
                    cond: local("has_head"),
                    then_label: label("pop"),
                    else_label: label("bump_check"),
                },
            },
            // A freed block links to the next in its first payload word.
            LlvmBlock {
                label: label("pop"),
                instrs: vec![
                    byte_at("next_ptr", "head", FLUX_HEADER_SIZE),
                    LlvmInstr::Load {
                        dst: LlvmLocal("next".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("next_ptr"),
                        align: Some(8),
                    },
                    LlvmInstr::Store {
                        ty: ptr_ty.clone(),
                        value: local("next"),
                        ptr: local("slot"),
                        align: Some(8),
                    },
                ],
                term: LlvmTerminator::Br { target: label("init") },
            },
            LlvmBlock {
                label: label("bump_check"),
                instrs: vec![
                    LlvmInstr::Load {
                        dst: LlvmLocal("hp".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("heap"),
                        align: Some(8),
                    },
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("new_hp".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), local("total"))],
                    },
                    byte_at("lim_ptr", "heap", FLUX_HEAP_LIMIT),
                    LlvmInstr::Load {
                        dst: LlvmLocal("lim".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("lim_ptr"),
                        align: Some(8),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("in_chunk".into()),
                        op: LlvmCmpOp::Ule,
                        ty: ptr_ty.clone(),
                        lhs: local("new_hp"),
                        rhs: local("lim"),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: i1_ty.clone(),
                    cond: local("in_chunk"),
                    then_label: label("bump"),
                    else_label: label("slow"),
                },
            },
            LlvmBlock {
                label: label("bump"),
                instrs: vec![LlvmInstr::Store {
                    ty: ptr_ty.clone(),
                    value: local("new_hp"),
                    ptr: local("heap"),
                    align: Some(8),
                }],
                term: LlvmTerminator::Br { target: label("init") },
            },
            LlvmBlock {
                label: label("init"),
                instrs: init,
                term: LlvmTerminator::Ret { ty: ptr_ty.clone(), value: local("payload") },
            },
            // The C runtime: the remote frees, a new chunk, or malloc.
            LlvmBlock {
                label: label("slow"),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("slow_ptr".into())),
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: ptr_ty.clone(),
                    callee: LlvmOperand::Global(GlobalId(slow_name.into())),
                    args: vec![
                        (i32_ty.clone(), local("size")),
                        (i32_ty.clone(), local("scan")),
                        (i32_ty.clone(), local("tag")),
                    ],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret { ty: ptr_ty.clone(), value: local("slow_ptr") },
            },
        ],
    });
}

/// `FluxHeapOwner` in rc.c, set in `arena_init`: the owner's TPIDRRO_EL0,
/// then its heap's address at offset 8.
const FLUX_HEAP_OWNER: &str = "flux_heap_owner";
const FLUX_HEAP_OWNER_HEAP: i64 = 8;

/// Emit `flux_free_shell_inline(i64 %val) -> void`, for macOS arm64 only:
/// `flux_free_shell` with its common case done in place. A walk that takes
/// each cell apart frees one per node, and nearly all of them are chunk
/// blocks freed by the heap's owner, which go back on their class's list
/// (rc.c's free_list_push). It finds the owner by its TPIDRRO_EL0 and the
/// heap through a plain global, since reaching `flux_heap` there is a thunk
/// call that LLVM hoists into every call of the function (0189, P1-3).
/// Every other block calls the runtime: a malloc block goes to free(), a
/// worker's free goes on the remote stack, and the debug heap and ASan
/// builds have no owner, so the debug heap still checks the count there.
pub fn emit_free_shell_inline(module: &mut LlvmModule) {
    let name = "flux_free_shell_inline";
    if has_function(module, name) {
        return;
    }
    let slow = "flux_free_shell";
    if !module.declarations.iter().any(|d| d.name.0 == slow) {
        module.declarations.push(crate::llvm::LlvmDecl {
            linkage: Linkage::External,
            name: flux_prelude_symbol(slow),
            sig: LlvmFunctionSig {
                ret: LlvmType::Void,
                params: vec![LlvmType::i64()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        });
    }
    if !has_global(module, FLUX_HEAP_OWNER) {
        module.globals.push(LlvmGlobal {
            linkage: Linkage::External,
            name: GlobalId(FLUX_HEAP_OWNER.into()),
            ty: LlvmType::Array { len: 2, element: Box::new(LlvmType::i64()) },
            is_constant: false,
            thread_local: None,
            value: None,
            attrs: vec![],
        });
    }
    let ptr_ty = LlvmType::Ptr;
    let i8_ty = LlvmType::Integer(8);
    let i64_ty = LlvmType::i64();
    let label = |name: &str| LabelId(name.into());
    let byte_at = |dst: &str, base: &str, offset: i64| LlvmInstr::GetElementPtr {
        dst: LlvmLocal(dst.into()),
        inbounds: false,
        element_ty: LlvmType::Integer(8),
        base: local(base),
        indices: vec![(LlvmType::i64(), const_i64_operand(offset))],
    };
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![i64_ty.clone()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: label("entry"),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("is_ptr".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i1(),
                    callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
                    args: vec![(i64_ty.clone(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_ptr"),
                    then_label: label("check_class"),
                    else_label: label("done"),
                },
            },
            // The class byte is the header's last; FLUX_FROM_CHUNK is its
            // top bit.
            LlvmBlock {
                label: label("check_class"),
                instrs: vec![
                    LlvmInstr::Cast {
                        dst: LlvmLocal("ptr".into()),
                        op: LlvmValueKind::IntToPtr,
                        from_ty: i64_ty.clone(),
                        operand: local("val"),
                        to_ty: ptr_ty.clone(),
                    },
                    byte_at("class_ptr", "ptr", -1),
                    LlvmInstr::Load {
                        dst: LlvmLocal("class_byte".into()),
                        ty: i8_ty.clone(),
                        ptr: local("class_ptr"),
                        align: Some(1),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("from_chunk".into()),
                        op: LlvmCmpOp::Slt,
                        ty: i8_ty.clone(),
                        lhs: local("class_byte"),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 8, value: 0 }),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("from_chunk"),
                    then_label: label("check_owner"),
                    else_label: label("slow"),
                },
            },
            // The owner global is zero when no thread owns the heap, and no
            // thread's TPIDRRO_EL0 is zero.
            LlvmBlock {
                label: label("check_owner"),
                instrs: vec![
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("thread".into())),
                        tail: false,
                        call_conv: None,
                        ret_ty: i64_ty.clone(),
                        callee: LlvmOperand::InlineAsm {
                            template: "mrs $0, TPIDRRO_EL0".into(),
                            constraints: "=r".into(),
                        },
                        args: vec![],
                        attrs: vec![],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("owner".into()),
                        ty: i64_ty.clone(),
                        ptr: LlvmOperand::Global(GlobalId(FLUX_HEAP_OWNER.into())),
                        align: Some(8),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("is_owner".into()),
                        op: LlvmCmpOp::Eq,
                        ty: i64_ty.clone(),
                        lhs: local("thread"),
                        rhs: local("owner"),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_owner"),
                    then_label: label("push"),
                    else_label: label("slow"),
                },
            },
            // The list holds headers and links through the first payload word.
            LlvmBlock {
                label: label("push"),
                instrs: vec![
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("heap_ptr".into()),
                        inbounds: false,
                        element_ty: LlvmType::Integer(8),
                        base: LlvmOperand::Global(GlobalId(FLUX_HEAP_OWNER.into())),
                        indices: vec![(LlvmType::i64(), const_i64_operand(FLUX_HEAP_OWNER_HEAP))],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("heap".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("heap_ptr"),
                        align: Some(8),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("class_bits".into()),
                        op: LlvmValueKind::And,
                        ty: i8_ty.clone(),
                        lhs: local("class_byte"),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 8, value: FLUX_CLASS_MASK }),
                    },
                    LlvmInstr::Cast {
                        dst: LlvmLocal("class64".into()),
                        op: LlvmValueKind::ZExt,
                        from_ty: i8_ty.clone(),
                        operand: local("class_bits"),
                        to_ty: i64_ty.clone(),
                    },
                    byte_at("free", "heap", FLUX_HEAP_FREE),
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("slot".into()),
                        inbounds: false,
                        element_ty: ptr_ty.clone(),
                        base: local("free"),
                        indices: vec![(i64_ty.clone(), local("class64"))],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("head".into()),
                        ty: ptr_ty.clone(),
                        ptr: local("slot"),
                        align: Some(8),
                    },
                    LlvmInstr::Store {
                        ty: ptr_ty.clone(),
                        value: local("head"),
                        ptr: local("ptr"),
                        align: Some(8),
                    },
                    byte_at("block", "ptr", -FLUX_HEADER_SIZE),
                    LlvmInstr::Store {
                        ty: ptr_ty.clone(),
                        value: local("block"),
                        ptr: local("slot"),
                        align: Some(8),
                    },
                ],
                term: LlvmTerminator::RetVoid,
            },
            LlvmBlock {
                label: label("slow"),
                instrs: vec![LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_prelude_symbol(slow)),
                    args: vec![(i64_ty.clone(), local("val"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::RetVoid,
            },
            LlvmBlock { label: label("done"), instrs: vec![], term: LlvmTerminator::RetVoid },
        ],
    });
}

/// Shared instruction sequence: extract the raw pointer and RC word
/// address from a pointer-tagged heap value in `%val`.
///
/// With pointer tagging, the value IS the raw pointer (no masking/shifting
/// needed). Produces locals: `%ptr`, `%rc_ptr`.
fn rc_extract_ptr_instrs() -> Vec<LlvmInstr> {
    vec![
        // %ptr = inttoptr i64 %val to ptr
        LlvmInstr::Cast {
            dst: LlvmLocal("ptr".into()),
            op: LlvmValueKind::IntToPtr,
            from_ty: LlvmType::i64(),
            operand: local("val"),
            to_ty: LlvmType::ptr(),
        },
        // FluxHeader is at ptr - 8.  Its first field is i32 refcount.
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("rc_ptr".into()),
            inbounds: false,
            element_ty: LlvmType::i8(),
            base: local("ptr"),
            indices: vec![(LlvmType::i64(), const_i64_operand(-8))],
        },
    ]
}

pub fn linkage_internal() -> Linkage {
    Linkage::Internal
}

pub fn unary_i64_sig() -> LlvmFunctionSig {
    LlvmFunctionSig {
        ret: LlvmType::i64(),
        params: vec![LlvmType::i64()],
        varargs: false,
        call_conv: CallConv::Fastcc,
    }
}

// ── Thunk helpers (Phase 2 mutual TCO) ───────────────────────────────────────

/// `flux_is_thunk(i64 %val) -> i1`: check if value is a heap pointer
/// with obj_tag == FLUX_OBJ_THUNK (0xF9) in the FluxHeader.
fn emit_is_thunk(module: &mut LlvmModule) {
    let name = "flux_is_thunk";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i1(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![
                    // First check if it's a pointer at all
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("is_ptr".into())),
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::i1(),
                        callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_ptr")),
                        args: vec![(LlvmType::i64(), local("val"))],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_ptr"),
                    then_label: LabelId("check_tag".into()),
                    else_label: LabelId("not_thunk".into()),
                },
            },
            LlvmBlock {
                label: LabelId("check_tag".into()),
                instrs: vec![
                    // %ptr = inttoptr i64 %val to ptr
                    LlvmInstr::Cast {
                        dst: LlvmLocal("ptr".into()),
                        op: LlvmValueKind::IntToPtr,
                        from_ty: LlvmType::i64(),
                        operand: local("val"),
                        to_ty: LlvmType::ptr(),
                    },
                    // obj_tag is at ptr - 3 (offset 5 in the 8-byte header before payload)
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("obj_tag_ptr".into()),
                        inbounds: false,
                        element_ty: LlvmType::i8(),
                        base: local("ptr"),
                        indices: vec![(LlvmType::i64(), const_i64_operand(-3))],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("obj_tag".into()),
                        ty: LlvmType::Integer(8),
                        ptr: local("obj_tag_ptr"),
                        align: Some(1),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("is_thunk_tag".into()),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::Integer(8),
                        lhs: local("obj_tag"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 8,
                            value: FluxPtrTagLayout::FLUX_OBJ_THUNK as i128,
                        }),
                    },
                ],
                term: LlvmTerminator::Ret { ty: LlvmType::i1(), value: local("is_thunk_tag") },
            },
            LlvmBlock {
                label: LabelId("not_thunk".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret { ty: LlvmType::i1(), value: const_i1_operand(false) },
            },
        ],
    });
}

/// `flux_tag_thunk(ptr %ptr) -> i64`: convert heap pointer to tagged value.
/// With pointer tagging, this is just `ptrtoint ptr to i64`.
fn emit_tag_thunk(module: &mut LlvmModule) {
    let name = "flux_tag_thunk";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::ptr()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("ptr".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![LlvmInstr::Cast {
                dst: LlvmLocal("tagged".into()),
                op: LlvmValueKind::PtrToInt,
                from_ty: LlvmType::ptr(),
                operand: local("ptr"),
                to_ty: LlvmType::i64(),
            }],
            term: LlvmTerminator::Ret { ty: LlvmType::i64(), value: local("tagged") },
        }],
    });
}

/// `flux_untag_thunk_ptr(i64 %val) -> ptr`: extract raw pointer from a thunk-tagged value.
/// With pointer tagging, this is just `inttoptr i64 to ptr`.
fn emit_untag_thunk_ptr(module: &mut LlvmModule) {
    let name = "flux_untag_thunk_ptr";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![LlvmInstr::Cast {
                dst: LlvmLocal("ptr".into()),
                op: LlvmValueKind::IntToPtr,
                from_ty: LlvmType::i64(),
                operand: local("val"),
                to_ty: LlvmType::ptr(),
            }],
            term: LlvmTerminator::Ret { ty: LlvmType::ptr(), value: local("ptr") },
        }],
    });
}

pub fn helper_attrs() -> Vec<String> {
    vec!["alwaysinline".into()]
}

pub fn has_function(module: &LlvmModule, name: &str) -> bool {
    module.functions.iter().any(|f| f.name.0 == name)
}

fn has_global(module: &LlvmModule, name: &str) -> bool {
    module.globals.iter().any(|g| g.name.0 == name)
}

fn local(name: &str) -> LlvmOperand {
    LlvmOperand::Local(LlvmLocal(name.into()))
}

fn const_i64(value: i64) -> LlvmConst {
    LlvmConst::Int { bits: 64, value: value.into() }
}

fn const_i64_operand(value: i64) -> LlvmOperand {
    LlvmOperand::Const(const_i64(value))
}

fn const_i1_operand(value: bool) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int { bits: 1, value: i128::from(value as i8) })
}

#[cfg(test)]
mod tests {
    use crate::llvm::{LlvmModule, render_module};

    use super::*;

    #[test]
    fn ptrtag_layout_constants() {
        assert_eq!(FluxPtrTagLayout::FLUX_NONE, 0);
        assert_eq!(FluxPtrTagLayout::FLUX_FALSE, 2);
        assert_eq!(FluxPtrTagLayout::FLUX_TRUE, 4);
        assert_eq!(FluxPtrTagLayout::FLUX_EMPTY_LIST, 6);
        assert_eq!(FluxPtrTagLayout::FLUX_UNINIT, 8);
        assert_eq!(FluxPtrTagLayout::FLUX_YIELD_SENTINEL, 10);
        assert_eq!(FluxPtrTagLayout::FLUX_MIN_PTR, 12);
    }

    #[test]
    fn emits_tag_int_and_untag_int_helpers() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(rendered.contains("@flux.ptrtag.none = private constant i64 0"));
        assert!(
            rendered.contains("define internal fastcc i64 @flux_tag_int(i64 %raw) alwaysinline")
        );
        // Pointer-tag: shl + or 1
        assert!(rendered.contains("%shifted = shl i64 %raw, 1"));
        assert!(rendered.contains("%tagged = or i64 %shifted, 1"));
        // Untag: ashr by 1
        assert!(rendered.contains("%raw = ashr i64 %val, 1"));
    }

    #[test]
    fn emits_is_ptr_and_aether_rc_helpers() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(rendered.contains("define internal fastcc i1 @flux_is_ptr(i64 %val) alwaysinline"));
        // Pointer-tag is_ptr: bit0 == 0 && val >= 12
        assert!(rendered.contains("%bit0 = and i64 %val, 1"));
        assert!(rendered.contains("%not_int = icmp eq i64 %bit0, 0"));
        assert!(rendered.contains("%above_sentinel = icmp sge i64 %val, 12"));
        // flux_dup: external C declaration (Aether RC in rc.c)
        assert!(rendered.contains("declare ccc void @flux_dup(i64) nounwind"));
        // flux_drop: external C declaration (Aether RC in rc.c)
        assert!(rendered.contains("declare ccc void @flux_drop(i64) nounwind"));
        // flux_drop_reuse: returns ptr for reuse if unique, else allocs fresh
        assert!(rendered.contains(
            "define internal fastcc ptr @flux_drop_reuse(i64 %val, i32 %size) alwaysinline"
        ));
        // flux_rc_is_unique: checks if RC == 1
        assert!(
            rendered
                .contains("define internal fastcc i1 @flux_rc_is_unique(i64 %val) alwaysinline")
        );
    }

    #[test]
    fn bump_alloc_records_the_size_class() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(rendered.contains("store i8 0, ptr %prog_ptr"));
        assert!(rendered.contains("%small = icmp ule i32 %masked, 256"));
        assert!(rendered.contains("%class_all = lshr i32 %masked, 3"));
        assert!(rendered.contains("%class32 = select i1 %small, i32 %class_all, i32 0"));
        assert!(rendered.contains("%class_ptr = getelementptr i8, ptr %block, i64 7"));
        assert!(rendered.contains("store i8 %class8, ptr %class_ptr"));
        assert!(!rendered.contains("store i16 0"));
    }

    /// Only a block with a size class is taken from a list or bumped from a
    /// chunk, since only it can go back on a free list, and it is marked as a
    /// chunk block (`FLUX_FROM_CHUNK` in `runtime/c/rc.c`).
    #[test]
    fn bump_alloc_takes_only_small_blocks_from_a_chunk() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        let filled = function_body(&rendered, "flux_bump_alloc_filled");
        assert!(filled.contains("%has_class = icmp ne i32 %class32, 0"), "{filled}");
        assert!(filled.contains("br i1 %has_class, label %pop_check, label %slow"), "{filled}");
        assert!(filled.contains("%chunk_class = or i32 %class32, 128"), "{filled}");
        assert!(filled.contains("%class8 = trunc i32 %chunk_class to i8"), "{filled}");
    }

    /// Each thread has its own heap, all zero where it owns none, so an
    /// allocation asks no one which thread it is on. The bump pointers and
    /// the class lists share one thread-local, so a function finds it once
    /// after inlining; a block comes off its class's list before the bump,
    /// since once the first chunks are used nearly every block is a freed one.
    #[test]
    fn bump_alloc_pops_its_class_list_before_bumping() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(
            rendered.contains("@flux_heap = external thread_local(initialexec) global [35 x ptr]"),
            "{rendered}"
        );
        assert!(!rendered.contains("@flux_arena_hp"), "{rendered}");
        assert!(!rendered.contains("flux_can_use_bump_arena"), "{rendered}");
        for name in ["flux_bump_alloc_inline", "flux_bump_alloc_filled"] {
            let body = function_body(&rendered, name);
            let finds = body.matches("@llvm.threadlocal.address.p0(ptr @flux_heap)").count();
            assert_eq!(finds, 1, "{body}");
            let pop = body.find("%head = load ptr, ptr %slot").expect(name);
            let bump = body.find("%hp = load ptr, ptr %heap").expect(name);
            assert!(pop < bump, "{body}");
            assert!(body.contains("%block = phi ptr [ %head, %pop ], [ %hp, %bump ]"), "{body}");
        }
    }

    /// A caller that writes every payload word gets a block nobody zeroed, on
    /// the slow path too.
    #[test]
    fn bump_alloc_filled_leaves_the_payload_to_its_caller() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);

        let zeroed = function_body(&rendered, "flux_bump_alloc_inline");
        assert!(zeroed.contains("call ccc void @llvm.memset.p0.i64("), "{zeroed}");
        assert!(zeroed.contains("call ccc ptr @flux_bump_alloc_slow("), "{zeroed}");

        let filled = function_body(&rendered, "flux_bump_alloc_filled");
        assert!(
            filled.starts_with(
                "define internal fastcc ptr @flux_bump_alloc_filled(i32 %size, i32 %scan, i32 %tag)"
            ),
            "{filled}"
        );
        assert!(!filled.contains("memset"), "{filled}");
        assert!(filled.contains("store i8 %class8, ptr %class_ptr"), "{filled}");
        assert!(filled.contains("call ccc ptr @flux_bump_alloc_slow_filled("), "{filled}");
    }

    /// The body of `name` in `rendered`, from its `define` to its closing brace.
    fn function_body<'a>(rendered: &'a str, name: &str) -> &'a str {
        let needle = format!("@{name}(");
        let start = rendered
            .match_indices("define ")
            .map(|(at, _)| at)
            .find(|&at| rendered[at..].lines().next().is_some_and(|line| line.contains(&needle)))
            .unwrap_or_else(|| panic!("no `{name}` in:\n{rendered}"));
        let end = start + rendered[start..].find("\n}").expect("an unclosed function");
        &rendered[start..end]
    }

    #[test]
    fn inline_rc_updates_only_a_thread_local_live_count() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);

        let dup = function_body(&rendered, "flux_dup_inline");
        assert!(
            dup.starts_with("define internal fastcc void @flux_dup_inline(i64 %val) alwaysinline")
        );
        assert!(dup.contains("%local = icmp sgt i32 %rc, 0"), "{dup}");
        assert!(dup.contains("%sticky = icmp sge i32 %rc, 1073741824"), "{dup}");
        assert!(dup.contains("%next = add i32 %rc, 1"), "{dup}");
        assert!(dup.contains("call ccc void @flux_dup(i64 %val)"), "{dup}");

        let drop = function_body(&rendered, "flux_drop_inline");
        assert!(
            drop.starts_with(
                "define internal fastcc void @flux_drop_inline(i64 %val) alwaysinline"
            )
        );
        // A count of 1 is the last reference: the runtime frees it.
        assert!(drop.contains("%local = icmp sgt i32 %rc, 1"), "{drop}");
        assert!(drop.contains("%sticky = icmp sge i32 %rc, 1073741824"), "{drop}");
        assert!(drop.contains("%next = add i32 %rc, -1"), "{drop}");
        assert!(drop.contains("call ccc void @flux_drop(i64 %val)"), "{drop}");
    }
}
