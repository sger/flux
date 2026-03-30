use crate::core_to_llvm::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmFunction,
    LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator,
    LlvmType, LlvmValueKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluxNanboxLayout;

impl FluxNanboxLayout {
    pub const NANBOX_SENTINEL_U64: u64 = 0x7FFC_0000_0000_0000;
    pub const SENTINEL_MASK_U64: u64 = 0xFFFC_0000_0000_0000;
    pub const TAG_SHIFT: u32 = 46;
    pub const TAG_MASK_U64: u64 = 0xF;
    pub const PAYLOAD_MASK_U64: u64 = (1u64 << 46) - 1;
    pub const THUNK_TAG_U8: u8 = 0x6;
    pub const BOXED_VALUE_TAG_U8: u8 = 0x8;
    pub const MIN_INLINE_INT: i64 = crate::runtime::nanbox::MIN_INLINE_INT;
    pub const MAX_INLINE_INT: i64 = crate::runtime::nanbox::MAX_INLINE_INT;

    pub const fn nanbox_sentinel_i64() -> i64 {
        Self::NANBOX_SENTINEL_U64 as i64
    }

    pub const fn sentinel_mask_i64() -> i64 {
        Self::SENTINEL_MASK_U64 as i64
    }

    pub const fn tag_mask_i64() -> i64 {
        Self::TAG_MASK_U64 as i64
    }

    pub const fn payload_mask_i64() -> i64 {
        Self::PAYLOAD_MASK_U64 as i64
    }

    pub const fn boxed_value_tag_i64() -> i64 {
        Self::BOXED_VALUE_TAG_U8 as i64
    }
}

const NANBOX_CONSTANTS: [(&str, i64); 8] = [
    (
        "flux.nanbox.sentinel",
        FluxNanboxLayout::nanbox_sentinel_i64(),
    ),
    (
        "flux.nanbox.sentinel_mask",
        FluxNanboxLayout::sentinel_mask_i64(),
    ),
    ("flux.nanbox.tag_shift", FluxNanboxLayout::TAG_SHIFT as i64),
    ("flux.nanbox.tag_mask", FluxNanboxLayout::tag_mask_i64()),
    (
        "flux.nanbox.payload_mask",
        FluxNanboxLayout::payload_mask_i64(),
    ),
    (
        "flux.nanbox.boxed_value_tag",
        FluxNanboxLayout::boxed_value_tag_i64(),
    ),
    (
        "flux.nanbox.min_inline_int",
        FluxNanboxLayout::MIN_INLINE_INT,
    ),
    (
        "flux.nanbox.max_inline_int",
        FluxNanboxLayout::MAX_INLINE_INT,
    ),
];

pub fn flux_prelude_symbol(name: &str) -> GlobalId {
    GlobalId(name.to_string())
}

pub fn emit_prelude(module: &mut LlvmModule) {
    emit_nanbox_constants(module);
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
}

pub fn emit_prelude_and_arith(module: &mut LlvmModule) {
    emit_prelude(module);
    super::emit_arith(module);
}

fn emit_nanbox_constants(module: &mut LlvmModule) {
    for (name, value) in NANBOX_CONSTANTS {
        if has_global(module, name) {
            continue;
        }
        module.globals.push(LlvmGlobal {
            linkage: Linkage::Private,
            name: flux_prelude_symbol(name),
            ty: LlvmType::i64(),
            is_constant: true,
            value: const_i64(value),
            attrs: vec![],
        });
    }
}

fn emit_tag_int(module: &mut LlvmModule) {
    let name = "flux_tag_int";
    if has_function(module, name) {
        return;
    }
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
                    dst: LlvmLocal("masked".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("raw"),
                    rhs: const_i64_operand(FluxNanboxLayout::payload_mask_i64()),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tagged".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i64(),
                    lhs: local("masked"),
                    rhs: const_i64_operand(FluxNanboxLayout::nanbox_sentinel_i64()),
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
    let shift = 64 - 46;
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("val".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::payload_mask_i64()),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("shifted".into()),
                    op: LlvmValueKind::Shl,
                    ty: LlvmType::i64(),
                    lhs: local("payload"),
                    rhs: const_i64_operand(shift as i64),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("raw".into()),
                    op: LlvmValueKind::AShr,
                    ty: LlvmType::i64(),
                    lhs: local("shifted"),
                    rhs: const_i64_operand(shift as i64),
                },
            ],
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
                LlvmInstr::Binary {
                    dst: LlvmLocal("masked".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::sentinel_mask_i64()),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_boxed".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("masked"),
                    rhs: const_i64_operand(FluxNanboxLayout::nanbox_sentinel_i64()),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tag_bits".into()),
                    op: LlvmValueKind::LShr,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::TAG_SHIFT as i64),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tag".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("tag_bits"),
                    rhs: const_i64_operand(FluxNanboxLayout::tag_mask_i64()),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_boxed_value".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("tag"),
                    rhs: const_i64_operand(FluxNanboxLayout::boxed_value_tag_i64()),
                },
                LlvmInstr::Select {
                    dst: LlvmLocal("result".into()),
                    cond_ty: LlvmType::i1(),
                    cond: local("is_boxed"),
                    value_ty: LlvmType::i1(),
                    then_value: local("is_boxed_value"),
                    else_value: const_i1_operand(false),
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
/// The implementation lives in `runtime/c/gc.c` (Aether RC).
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
/// The implementation lives in `runtime/c/gc.c` (Aether RC).
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
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::ptr(),
                        callee: LlvmOperand::Global(GlobalId("flux_gc_alloc".into())),
                        args: vec![(LlvmType::i32(), local("size"))],
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

/// Shared instruction sequence: extract the raw pointer and RC word
/// address from a NaN-boxed heap value in `%val`.
///
/// Produces locals: `%payload`, `%shifted`, `%ptr`, `%rc_ptr`.
fn rc_extract_ptr_instrs() -> Vec<LlvmInstr> {
    vec![
        LlvmInstr::Binary {
            dst: LlvmLocal("payload".into()),
            op: LlvmValueKind::And,
            ty: LlvmType::i64(),
            lhs: local("val"),
            rhs: const_i64_operand(FluxNanboxLayout::payload_mask_i64()),
        },
        LlvmInstr::Binary {
            dst: LlvmLocal("shifted".into()),
            op: LlvmValueKind::Shl,
            ty: LlvmType::i64(),
            lhs: local("payload"),
            rhs: const_i64_operand(3), // PTR_SHIFT
        },
        LlvmInstr::Cast {
            dst: LlvmLocal("ptr".into()),
            op: LlvmValueKind::IntToPtr,
            from_ty: LlvmType::i64(),
            operand: local("shifted"),
            to_ty: LlvmType::ptr(),
        },
        // FluxHeader is at ptr - 8.  Its first field is i32 refcount.
        // GEP by -8 bytes (not -1 i64) to get the header start.
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

/// NaN-box tag bits for Thunk (SENTINEL | (0x6 << 46)).
pub fn thunk_nanbox_tag_bits() -> i64 {
    (FluxNanboxLayout::NANBOX_SENTINEL_U64
        | ((FluxNanboxLayout::THUNK_TAG_U8 as u64) << FluxNanboxLayout::TAG_SHIFT)) as i64
}

/// `flux_is_thunk(i64 %val) -> i1`: check if NaN-box tag == 0x6 (Thunk).
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
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                // Extract tag: (val >> 46) & 0xF
                LlvmInstr::Binary {
                    dst: LlvmLocal("tag_bits".into()),
                    op: LlvmValueKind::LShr,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::TAG_SHIFT as i64),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tag".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("tag_bits"),
                    rhs: const_i64_operand(FluxNanboxLayout::tag_mask_i64()),
                },
                // Check sentinel
                LlvmInstr::Binary {
                    dst: LlvmLocal("masked".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::sentinel_mask_i64()),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_nanbox".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("masked"),
                    rhs: const_i64_operand(FluxNanboxLayout::nanbox_sentinel_i64()),
                },
                // Check tag == 0x6
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_thunk_tag".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("tag"),
                    rhs: const_i64_operand(FluxNanboxLayout::THUNK_TAG_U8 as i64),
                },
                LlvmInstr::Select {
                    dst: LlvmLocal("result".into()),
                    cond_ty: LlvmType::i1(),
                    cond: local("is_nanbox"),
                    value_ty: LlvmType::i1(),
                    then_value: local("is_thunk_tag"),
                    else_value: const_i1_operand(false),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i1(),
                value: local("result"),
            },
        }],
    });
}

/// `flux_tag_thunk(ptr %ptr) -> i64`: NaN-box a heap pointer with Thunk tag.
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
            instrs: vec![
                LlvmInstr::Cast {
                    dst: LlvmLocal("addr".into()),
                    op: LlvmValueKind::PtrToInt,
                    from_ty: LlvmType::ptr(),
                    operand: local("ptr"),
                    to_ty: LlvmType::i64(),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("shifted".into()),
                    op: LlvmValueKind::LShr,
                    ty: LlvmType::i64(),
                    lhs: local("addr"),
                    rhs: const_i64_operand(3),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("shifted"),
                    rhs: const_i64_operand(FluxNanboxLayout::payload_mask_i64()),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("tagged".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i64(),
                    lhs: local("payload"),
                    rhs: const_i64_operand(thunk_nanbox_tag_bits()),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("tagged"),
            },
        }],
    });
}

/// `flux_untag_thunk_ptr(i64 %val) -> ptr`: extract raw pointer from a thunk-tagged NaN-box.
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
            instrs: vec![
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("val"),
                    rhs: const_i64_operand(FluxNanboxLayout::payload_mask_i64()),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("addr".into()),
                    op: LlvmValueKind::Shl,
                    ty: LlvmType::i64(),
                    lhs: local("payload"),
                    rhs: const_i64_operand(3),
                },
                LlvmInstr::Cast {
                    dst: LlvmLocal("ptr".into()),
                    op: LlvmValueKind::IntToPtr,
                    from_ty: LlvmType::i64(),
                    operand: local("addr"),
                    to_ty: LlvmType::ptr(),
                },
            ],
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
    use std::{mem::transmute, rc::Rc};

    use crate::{
        core_to_llvm::{LlvmModule, render_module},
        runtime::{
            nanbox::{NanBox, NanTag},
            value::Value,
        },
    };

    use super::*;

    #[test]
    fn nanbox_layout_matches_runtime_behavior() {
        assert_eq!(
            FluxNanboxLayout::MIN_INLINE_INT,
            crate::runtime::nanbox::MIN_INLINE_INT
        );
        assert_eq!(
            FluxNanboxLayout::MAX_INLINE_INT,
            crate::runtime::nanbox::MAX_INLINE_INT
        );

        let zero_bits: u64 = unsafe { transmute(NanBox::from_int(0)) };
        let one_bits: u64 = unsafe { transmute(NanBox::from_int(1)) };
        assert_eq!(zero_bits, FluxNanboxLayout::NANBOX_SENTINEL_U64);
        assert_eq!(one_bits, FluxNanboxLayout::NANBOX_SENTINEL_U64 | 1);

        let neg_one = NanBox::from_int(-1);
        assert_eq!(neg_one.as_int(), -1);

        let boxed = NanBox::from_value(Value::String(Rc::new("boxed".to_string())));
        assert_eq!(boxed.tag(), NanTag::BoxedValue);
    }

    #[test]
    fn emits_tag_int_and_untag_int_helpers() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(
            rendered.contains("@flux.nanbox.sentinel = private constant i64 9222246136947933184")
        );
        assert!(
            rendered.contains("define internal fastcc i64 @flux_tag_int(i64 %raw) alwaysinline")
        );
        assert!(rendered.contains("%masked = and i64 %raw, 70368744177663"));
        assert!(rendered.contains("%tagged = or i64 %masked, 9222246136947933184"));
        assert!(rendered.contains("%shifted = shl i64 %payload, 18"));
        assert!(rendered.contains("%raw = ashr i64 %shifted, 18"));
    }

    #[test]
    fn emits_is_ptr_and_aether_rc_helpers() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(rendered.contains("define internal fastcc i1 @flux_is_ptr(i64 %val) alwaysinline"));
        assert!(rendered.contains("%tag = and i64 %tag_bits, 15"));
        assert!(rendered.contains("%result = select i1 %is_boxed, i1 %is_boxed_value, i1 0"));
        // flux_dup: external C declaration (Aether RC in gc.c)
        assert!(rendered.contains("declare ccc void @flux_dup(i64) nounwind"));
        // flux_drop: external C declaration (Aether RC in gc.c)
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
