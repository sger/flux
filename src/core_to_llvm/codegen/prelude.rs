use crate::core_to_llvm::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmFunction,
    LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator,
    LlvmType, LlvmValueKind,
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
    (
        "flux.ptrtag.yield_sentinel",
        FluxPtrTagLayout::FLUX_YIELD_SENTINEL,
    ),
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("tagged"),
            },
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("raw"),
            },
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::i1(),
                value: local("result"),
            },
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
    module.declarations.push(crate::core_to_llvm::LlvmDecl {
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
    module.declarations.push(crate::core_to_llvm::LlvmDecl {
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
                term: LlvmTerminator::Ret {
                    ty: LlvmType::ptr(),
                    value: local("ptr"),
                },
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
                term: LlvmTerminator::Ret {
                    ty: LlvmType::ptr(),
                    value: local("new_mem"),
                },
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
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i1(),
                    value: local("is_one"),
                },
            },
            LlvmBlock {
                label: LabelId("not_unique".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i1(),
                    value: const_i1_operand(false),
                },
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
        module.declarations.push(crate::core_to_llvm::LlvmDecl {
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
        module.declarations.push(crate::core_to_llvm::LlvmDecl {
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
        && !module
            .declarations
            .iter()
            .any(|d| d.name.0 == alloc_hdr_name)
    {
        module.declarations.push(crate::core_to_llvm::LlvmDecl {
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

/// Emit external global declarations for the bump arena pointers and
/// the `flux_bump_alloc_inline` helper function.
fn emit_bump_alloc_inline(module: &mut LlvmModule) {
    // ── External globals: @flux_arena_hp, @flux_arena_limit ──────
    let hp_name = "flux_arena_hp";
    if !has_global(module, hp_name) {
        module.globals.push(LlvmGlobal {
            linkage: Linkage::External,
            name: GlobalId(hp_name.into()),
            ty: LlvmType::Ptr,
            is_constant: false,
            value: None,
            attrs: vec![],
        });
    }
    let lim_name = "flux_arena_limit";
    if !has_global(module, lim_name) {
        module.globals.push(LlvmGlobal {
            linkage: Linkage::External,
            name: GlobalId(lim_name.into()),
            ty: LlvmType::Ptr,
            is_constant: false,
            value: None,
            attrs: vec![],
        });
    }

    // ── Declare @flux_bump_alloc_slow (C runtime fallback) ───────
    let slow_name = "flux_bump_alloc_slow";
    if !module.declarations.iter().any(|d| d.name.0 == slow_name) {
        module.declarations.push(crate::core_to_llvm::LlvmDecl {
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

    // ── Declare @llvm.memset.p0.i64 intrinsic ────────────────────
    let memset_name = "llvm.memset.p0.i64";
    if !module.declarations.iter().any(|d| d.name.0 == memset_name) {
        module.declarations.push(crate::core_to_llvm::LlvmDecl {
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

    // ── Define flux_bump_alloc_inline ─────────────────────────────
    let fn_name = "flux_bump_alloc_inline";
    if has_function(module, fn_name) {
        return;
    }

    // Signature: fastcc ptr @flux_bump_alloc_inline(i32 %size, i32 %scan, i32 %tag)
    //
    // Fast path (~4 instructions):
    //   %aligned = add i32 %size, 7
    //   %masked  = and i32 %aligned, -8
    //   %total_i32 = add i32 %masked, 8   ; + FLUX_HEADER_SIZE
    //   %total   = zext i32 %total_i32 to i64
    //   %hp      = load ptr, ptr @flux_arena_hp
    //   %new_hp  = getelementptr i8, ptr %hp, i64 %total
    //   %lim     = load ptr, ptr @flux_arena_limit
    //   %ok      = icmp ule ptr %new_hp, %lim
    //   br i1 %ok, label %fast, label %slow
    //
    // fast:
    //   store ptr %new_hp, ptr @flux_arena_hp
    //   ; init header at %hp: refcount=1, scan_fsize, obj_tag, reserved=0
    //   store i32 1, ptr %hp              ; refcount
    //   %scan8 = trunc i32 %scan to i8
    //   %sf_ptr = getelementptr i8, ptr %hp, i64 4
    //   store i8 %scan8, ptr %sf_ptr      ; scan_fsize
    //   %tag8 = trunc i32 %tag to i8
    //   %ot_ptr = getelementptr i8, ptr %hp, i64 5
    //   store i8 %tag8, ptr %ot_ptr       ; obj_tag
    //   %res_ptr = getelementptr i8, ptr %hp, i64 6
    //   store i16 0, ptr %res_ptr         ; reserved
    //   %payload = getelementptr i8, ptr %hp, i64 8
    //   call void @llvm.memset.p0.i64(ptr %payload, i8 0, i64 %aligned64, i1 false)
    //   ret ptr %payload
    //
    // slow:
    //   %p = call ccc ptr @flux_bump_alloc_slow(i32 %size, i32 %scan, i32 %tag)
    //   ret ptr %p

    let i32_ty = LlvmType::i32();
    let i64_ty = LlvmType::i64();
    let ptr_ty = LlvmType::Ptr;
    let i8_ty = LlvmType::Integer(8);
    let i16_ty = LlvmType::Integer(16);
    let i1_ty = LlvmType::i1();

    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: GlobalId(fn_name.into()),
        sig: LlvmFunctionSig {
            ret: ptr_ty.clone(),
            params: vec![i32_ty.clone(), i32_ty.clone(), i32_ty.clone()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("size".into()),
            LlvmLocal("scan".into()),
            LlvmLocal("tag".into()),
        ],
        attrs: helper_attrs(),
        blocks: vec![
            // ── entry: compute aligned total, load hp/limit, branch ──
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![
                    // %aligned = add i32 %size, 7
                    LlvmInstr::Binary {
                        dst: LlvmLocal("aligned".into()),
                        op: LlvmValueKind::Add,
                        ty: i32_ty.clone(),
                        lhs: local("size"),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 7 }),
                    },
                    // %masked = and i32 %aligned, -8
                    LlvmInstr::Binary {
                        dst: LlvmLocal("masked".into()),
                        op: LlvmValueKind::And,
                        ty: i32_ty.clone(),
                        lhs: local("aligned"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: -8i32 as i128,
                        }),
                    },
                    // %total_i32 = add i32 %masked, 8  (FLUX_HEADER_SIZE)
                    LlvmInstr::Binary {
                        dst: LlvmLocal("total_i32".into()),
                        op: LlvmValueKind::Add,
                        ty: i32_ty.clone(),
                        lhs: local("masked"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 32,
                            value: FLUX_HEADER_SIZE as i128,
                        }),
                    },
                    // %total = zext i32 %total_i32 to i64
                    LlvmInstr::Cast {
                        dst: LlvmLocal("total".into()),
                        op: LlvmValueKind::ZExt,
                        from_ty: i32_ty.clone(),
                        operand: local("total_i32"),
                        to_ty: i64_ty.clone(),
                    },
                    // %hp = load ptr, ptr @flux_arena_hp
                    LlvmInstr::Load {
                        dst: LlvmLocal("hp".into()),
                        ty: ptr_ty.clone(),
                        ptr: LlvmOperand::Global(GlobalId(hp_name.into())),
                        align: Some(8),
                    },
                    // %new_hp = getelementptr i8, ptr %hp, i64 %total
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("new_hp".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), local("total"))],
                    },
                    // %lim = load ptr, ptr @flux_arena_limit
                    LlvmInstr::Load {
                        dst: LlvmLocal("lim".into()),
                        ty: ptr_ty.clone(),
                        ptr: LlvmOperand::Global(GlobalId(lim_name.into())),
                        align: Some(8),
                    },
                    // %ok = icmp ule ptr %new_hp, %lim
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("ok".into()),
                        op: LlvmCmpOp::Ule,
                        ty: ptr_ty.clone(),
                        lhs: local("new_hp"),
                        rhs: local("lim"),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: i1_ty.clone(),
                    cond: local("ok"),
                    then_label: LabelId("fast".into()),
                    else_label: LabelId("slow".into()),
                },
            },
            // ── fast: bump hp, init header, zero payload, return ──
            LlvmBlock {
                label: LabelId("fast".into()),
                instrs: vec![
                    // store ptr %new_hp, ptr @flux_arena_hp
                    LlvmInstr::Store {
                        ty: ptr_ty.clone(),
                        value: local("new_hp"),
                        ptr: LlvmOperand::Global(GlobalId(hp_name.into())),
                        align: Some(8),
                    },
                    // store i32 1, ptr %hp  (refcount = 1)
                    LlvmInstr::Store {
                        ty: i32_ty.clone(),
                        value: LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }),
                        ptr: local("hp"),
                        align: Some(4),
                    },
                    // %scan8 = trunc i32 %scan to i8
                    LlvmInstr::Cast {
                        dst: LlvmLocal("scan8".into()),
                        op: LlvmValueKind::Trunc,
                        from_ty: i32_ty.clone(),
                        operand: local("scan"),
                        to_ty: i8_ty.clone(),
                    },
                    // %sf_ptr = getelementptr i8, ptr %hp, i64 4
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("sf_ptr".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), const_i64_operand(4))],
                    },
                    // store i8 %scan8, ptr %sf_ptr
                    LlvmInstr::Store {
                        ty: i8_ty.clone(),
                        value: local("scan8"),
                        ptr: local("sf_ptr"),
                        align: Some(1),
                    },
                    // %tag8 = trunc i32 %tag to i8
                    LlvmInstr::Cast {
                        dst: LlvmLocal("tag8".into()),
                        op: LlvmValueKind::Trunc,
                        from_ty: i32_ty.clone(),
                        operand: local("tag"),
                        to_ty: i8_ty.clone(),
                    },
                    // %ot_ptr = getelementptr i8, ptr %hp, i64 5
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("ot_ptr".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), const_i64_operand(5))],
                    },
                    // store i8 %tag8, ptr %ot_ptr
                    LlvmInstr::Store {
                        ty: i8_ty.clone(),
                        value: local("tag8"),
                        ptr: local("ot_ptr"),
                        align: Some(1),
                    },
                    // %res_ptr = getelementptr i8, ptr %hp, i64 6
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("res_ptr".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), const_i64_operand(6))],
                    },
                    // store i16 0, ptr %res_ptr  (reserved = 0)
                    LlvmInstr::Store {
                        ty: i16_ty.clone(),
                        value: LlvmOperand::Const(LlvmConst::Int { bits: 16, value: 0 }),
                        ptr: local("res_ptr"),
                        align: Some(2),
                    },
                    // %payload = getelementptr i8, ptr %hp, i64 8
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("payload".into()),
                        inbounds: false,
                        element_ty: i8_ty.clone(),
                        base: local("hp"),
                        indices: vec![(i64_ty.clone(), const_i64_operand(FLUX_HEADER_SIZE))],
                    },
                    // %aligned64 = zext i32 %masked to i64
                    LlvmInstr::Cast {
                        dst: LlvmLocal("aligned64".into()),
                        op: LlvmValueKind::ZExt,
                        from_ty: i32_ty.clone(),
                        operand: local("masked"),
                        to_ty: i64_ty.clone(),
                    },
                    // call void @llvm.memset.p0.i64(ptr %payload, i8 0, i64 %aligned64, i1 false)
                    LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(GlobalId(memset_name.into())),
                        args: vec![
                            (ptr_ty.clone(), local("payload")),
                            (
                                i8_ty.clone(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 8, value: 0 }),
                            ),
                            (i64_ty.clone(), local("aligned64")),
                            (
                                i1_ty.clone(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 1, value: 0 }),
                            ),
                        ],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::Ret {
                    ty: ptr_ty.clone(),
                    value: local("payload"),
                },
            },
            // ── slow: fall back to C runtime malloc path ──
            LlvmBlock {
                label: LabelId("slow".into()),
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
                term: LlvmTerminator::Ret {
                    ty: ptr_ty.clone(),
                    value: local("slow_ptr"),
                },
            },
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
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i1(),
                    value: local("is_thunk_tag"),
                },
            },
            LlvmBlock {
                label: LabelId("not_thunk".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i1(),
                    value: const_i1_operand(false),
                },
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("tagged"),
            },
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::ptr(),
                value: local("ptr"),
            },
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
    LlvmConst::Int {
        bits: 64,
        value: value.into(),
    }
}

fn const_i64_operand(value: i64) -> LlvmOperand {
    LlvmOperand::Const(const_i64(value))
}

fn const_i1_operand(value: bool) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 1,
        value: i128::from(value as i8),
    })
}

#[cfg(test)]
mod tests {
    use crate::core_to_llvm::{LlvmModule, render_module};

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
}
