use crate::{
    core_to_llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmDecl,
        LlvmFunction, LlvmFunctionSig, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand,
        LlvmTerminator, LlvmType, LlvmTypeDef, LlvmValueKind,
    },
    runtime::nanbox::NanTag,
};

use super::prelude::{FluxNanboxLayout, has_function, helper_attrs};

pub const FLUX_CLOSURE_TYPE_NAME: &str = "FluxClosure";
pub const FLUX_CLOSURE_HEADER_SIZE: i64 = 24;

pub const FLUX_CLOSURE_FN_FIELD: u32 = 0;
pub const FLUX_CLOSURE_REMAINING_ARITY_FIELD: u32 = 1;
pub const FLUX_CLOSURE_CAPTURE_COUNT_FIELD: u32 = 2;
pub const FLUX_CLOSURE_APPLIED_COUNT_FIELD: u32 = 3;
pub const FLUX_CLOSURE_PAYLOAD_FIELD: u32 = 5;

pub fn flux_closure_symbol(name: &str) -> GlobalId {
    GlobalId(name.to_string())
}

pub fn emit_closure_support(module: &mut LlvmModule) {
    emit_closure_type(module);
    emit_gc_alloc_decl(module);
    emit_copy_helper(module);
    emit_tag_boxed_ptr(module);
    emit_untag_boxed_ptr(module);
    emit_make_closure(module);
    emit_call_closure(module);
    emit_call_closure_c(module);
}

pub fn closure_type() -> LlvmType {
    LlvmType::Named(FLUX_CLOSURE_TYPE_NAME.into())
}

#[allow(dead_code)]
pub fn closure_entry_sig() -> LlvmFunctionSig {
    LlvmFunctionSig {
        ret: LlvmType::i64(),
        params: vec![LlvmType::i64(), LlvmType::ptr(), LlvmType::i32()],
        varargs: false,
        call_conv: CallConv::Fastcc,
    }
}

pub fn boxed_nanbox_tag_bits() -> i64 {
    (FluxNanboxLayout::NANBOX_SENTINEL_U64
        | ((NanTag::BoxedValue as u64) << FluxNanboxLayout::TAG_SHIFT)) as i64
}

fn emit_closure_type(module: &mut LlvmModule) {
    if module
        .type_defs
        .iter()
        .any(|def| def.name == FLUX_CLOSURE_TYPE_NAME)
    {
        return;
    }
    module.type_defs.push(LlvmTypeDef {
        name: FLUX_CLOSURE_TYPE_NAME.into(),
        ty: LlvmType::Struct {
            packed: false,
            fields: vec![
                LlvmType::ptr(),  // fn_ptr (offset 0, 8 bytes)
                LlvmType::i32(),  // remaining_arity (offset 8)
                LlvmType::i32(),  // capture_count (offset 12)
                LlvmType::i32(),  // applied_count (offset 16)
                LlvmType::i32(),  // padding to align payload to 8 bytes (offset 20)
                LlvmType::Array { // payload[] (offset 24)
                    len: 0,
                    element: Box::new(LlvmType::i64()),
                },
            ],
        },
    });
}

fn emit_gc_alloc_decl(module: &mut LlvmModule) {
    let name = "flux_gc_alloc";
    if module.declarations.iter().any(|decl| decl.name.0 == name) {
        return;
    }
    module.declarations.push(LlvmDecl {
        linkage: Linkage::External,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec![],
    });
}

fn emit_copy_helper(module: &mut LlvmModule) {
    let name = "flux_copy_i64s";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![LlvmType::ptr(), LlvmType::ptr(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("dst".into()),
            LlvmLocal("src".into()),
            LlvmLocal("count".into()),
        ],
        attrs: vec![],
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![LlvmInstr::Icmp {
                    dst: LlvmLocal("empty".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i32(),
                    lhs: local("count"),
                    rhs: const_i32_operand(0),
                }],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("empty"),
                    then_label: LabelId("done".into()),
                    else_label: LabelId("loop".into()),
                },
            },
            LlvmBlock {
                label: LabelId("loop".into()),
                instrs: vec![
                    LlvmInstr::Phi {
                        dst: LlvmLocal("idx".into()),
                        ty: LlvmType::i32(),
                        incoming: vec![
                            (const_i32_operand(0), LabelId("entry".into())),
                            (
                                LlvmOperand::Local(LlvmLocal("idx.next".into())),
                                LabelId("loop".into()),
                            ),
                        ],
                    },
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("src.slot".into()),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: local("src"),
                        indices: vec![(LlvmType::i32(), local("idx"))],
                    },
                    LlvmInstr::Load {
                        dst: LlvmLocal("value".into()),
                        ty: LlvmType::i64(),
                        ptr: local("src.slot"),
                        align: Some(8),
                    },
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("dst.slot".into()),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: local("dst"),
                        indices: vec![(LlvmType::i32(), local("idx"))],
                    },
                    LlvmInstr::Store {
                        ty: LlvmType::i64(),
                        value: local("value"),
                        ptr: local("dst.slot"),
                        align: Some(8),
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("idx.next".into()),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i32(),
                        lhs: local("idx"),
                        rhs: const_i32_operand(1),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("keep".into()),
                        op: LlvmCmpOp::Slt,
                        ty: LlvmType::i32(),
                        lhs: local("idx.next"),
                        rhs: local("count"),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("keep"),
                    then_label: LabelId("loop".into()),
                    else_label: LabelId("done".into()),
                },
            },
            LlvmBlock {
                label: LabelId("done".into()),
                instrs: vec![],
                term: LlvmTerminator::RetVoid,
            },
        ],
    });
}

fn emit_tag_boxed_ptr(module: &mut LlvmModule) {
    let name = "flux_tag_boxed_ptr";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_closure_symbol(name),
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
                    rhs: const_i64_operand(boxed_nanbox_tag_bits()),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("tagged"),
            },
        }],
    });
}

fn emit_untag_boxed_ptr(module: &mut LlvmModule) {
    let name = "flux_untag_boxed_ptr";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("value".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload".into()),
                    op: LlvmValueKind::And,
                    ty: LlvmType::i64(),
                    lhs: local("value"),
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

fn emit_make_closure(module: &mut LlvmModule) {
    let name = "flux_make_closure";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![
                LlvmType::ptr(),
                LlvmType::i32(),
                LlvmType::ptr(),
                LlvmType::i32(),
                LlvmType::ptr(),
                LlvmType::i32(),
            ],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("fn_ptr".into()),
            LlvmLocal("remaining_arity".into()),
            LlvmLocal("capture_values".into()),
            LlvmLocal("capture_count".into()),
            LlvmLocal("applied_values".into()),
            LlvmLocal("applied_count".into()),
        ],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload_count".into()),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i32(),
                    lhs: local("capture_count"),
                    rhs: local("applied_count"),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("payload_bytes".into()),
                    op: LlvmValueKind::Mul,
                    ty: LlvmType::i32(),
                    lhs: local("payload_count"),
                    rhs: const_i32_operand(8),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("size".into()),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i32(),
                    lhs: local("payload_bytes"),
                    rhs: const_i32_operand(FLUX_CLOSURE_HEADER_SIZE as i32),
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("mem".into())),
                    tail: false,
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::ptr(),
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_gc_alloc")),
                    args: vec![(LlvmType::i32(), local("size"))],
                    attrs: vec![],
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("fn_ptr.slot".into()),
                    inbounds: true,
                    element_ty: closure_type(),
                    base: local("mem"),
                    indices: vec![
                        (LlvmType::i32(), const_i32_operand(0)),
                        (
                            LlvmType::i32(),
                            const_i32_operand(FLUX_CLOSURE_FN_FIELD as i32),
                        ),
                    ],
                },
                LlvmInstr::Store {
                    ty: LlvmType::ptr(),
                    value: local("fn_ptr"),
                    ptr: local("fn_ptr.slot"),
                    align: Some(8),
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("remaining.slot".into()),
                    inbounds: true,
                    element_ty: closure_type(),
                    base: local("mem"),
                    indices: vec![
                        (LlvmType::i32(), const_i32_operand(0)),
                        (
                            LlvmType::i32(),
                            const_i32_operand(FLUX_CLOSURE_REMAINING_ARITY_FIELD as i32),
                        ),
                    ],
                },
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local("remaining_arity"),
                    ptr: local("remaining.slot"),
                    align: Some(4),
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("capture.slot".into()),
                    inbounds: true,
                    element_ty: closure_type(),
                    base: local("mem"),
                    indices: vec![
                        (LlvmType::i32(), const_i32_operand(0)),
                        (
                            LlvmType::i32(),
                            const_i32_operand(FLUX_CLOSURE_CAPTURE_COUNT_FIELD as i32),
                        ),
                    ],
                },
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local("capture_count"),
                    ptr: local("capture.slot"),
                    align: Some(4),
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("applied.slot".into()),
                    inbounds: true,
                    element_ty: closure_type(),
                    base: local("mem"),
                    indices: vec![
                        (LlvmType::i32(), const_i32_operand(0)),
                        (
                            LlvmType::i32(),
                            const_i32_operand(FLUX_CLOSURE_APPLIED_COUNT_FIELD as i32),
                        ),
                    ],
                },
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local("applied_count"),
                    ptr: local("applied.slot"),
                    align: Some(4),
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("payload".into()),
                    inbounds: true,
                    element_ty: closure_type(),
                    base: local("mem"),
                    indices: vec![
                        (LlvmType::i32(), const_i32_operand(0)),
                        (
                            LlvmType::i32(),
                            const_i32_operand(FLUX_CLOSURE_PAYLOAD_FIELD as i32),
                        ),
                        (LlvmType::i32(), const_i32_operand(0)),
                    ],
                },
                LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                    args: vec![
                        (LlvmType::ptr(), local("payload")),
                        (LlvmType::ptr(), local("capture_values")),
                        (LlvmType::i32(), local("capture_count")),
                    ],
                    attrs: vec![],
                },
                LlvmInstr::GetElementPtr {
                    dst: LlvmLocal("applied.dst".into()),
                    inbounds: true,
                    element_ty: LlvmType::i64(),
                    base: local("payload"),
                    indices: vec![(LlvmType::i32(), local("capture_count"))],
                },
                LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                    args: vec![
                        (LlvmType::ptr(), local("applied.dst")),
                        (LlvmType::ptr(), local("applied_values")),
                        (LlvmType::i32(), local("applied_count")),
                    ],
                    attrs: vec![],
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("tagged".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_tag_boxed_ptr")),
                    args: vec![(LlvmType::ptr(), local("mem"))],
                    attrs: vec![],
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("tagged"),
            },
        }],
    });
}

fn emit_call_closure(module: &mut LlvmModule) {
    let name = "flux_call_closure";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::i64(), LlvmType::ptr(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("closure_value".into()),
            LlvmLocal("args".into()),
            LlvmLocal("nargs".into()),
        ],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: common_closure_load_instrs(local("closure_value")),
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_exact"),
                    then_label: LabelId("exact".into()),
                    else_label: LabelId("non_exact".into()),
                },
            },
            LlvmBlock {
                label: LabelId("non_exact".into()),
                instrs: vec![],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_under"),
                    then_label: LabelId("under".into()),
                    else_label: LabelId("over".into()),
                },
            },
            LlvmBlock {
                label: LabelId("exact".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("exact.result".into())),
                    tail: false,
                    // Use ccc for indirect call — target functions are ccc
                    // in the LIR→LLVM path.  Must match the function
                    // definition's calling convention.
                    call_conv: Some(CallConv::Ccc),
                    ret_ty: LlvmType::i64(),
                    callee: local("fn_ptr"),
                    args: vec![
                        (LlvmType::i64(), local("closure_value")),
                        (LlvmType::ptr(), local("args")),
                        (LlvmType::i32(), local("nargs")),
                    ],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("exact.result"),
                },
            },
            LlvmBlock {
                label: LabelId("under".into()),
                instrs: vec![
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("existing.applied".into()),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: local("payload"),
                        indices: vec![(LlvmType::i32(), local("capture_count"))],
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("new.applied.count".into()),
                        op: LlvmValueKind::Add,
                        ty: LlvmType::i32(),
                        lhs: local("applied_count"),
                        rhs: local("nargs"),
                    },
                    LlvmInstr::Alloca {
                        dst: LlvmLocal("new.applied".into()),
                        ty: LlvmType::i64(),
                        count: Some((LlvmType::i32(), local("new.applied.count"))),
                        align: Some(8),
                    },
                    LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                        args: vec![
                            (LlvmType::ptr(), local("new.applied")),
                            (LlvmType::ptr(), local("existing.applied")),
                            (LlvmType::i32(), local("applied_count")),
                        ],
                        attrs: vec![],
                    },
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("new.applied.tail".into()),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: local("new.applied"),
                        indices: vec![(LlvmType::i32(), local("applied_count"))],
                    },
                    LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                        args: vec![
                            (LlvmType::ptr(), local("new.applied.tail")),
                            (LlvmType::ptr(), local("args")),
                            (LlvmType::i32(), local("nargs")),
                        ],
                        attrs: vec![],
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("new.remaining".into()),
                        op: LlvmValueKind::Sub,
                        ty: LlvmType::i32(),
                        lhs: local("remaining_arity"),
                        rhs: local("nargs"),
                    },
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("under.result".into())),
                        tail: false,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::i64(),
                        callee: LlvmOperand::Global(flux_closure_symbol("flux_make_closure")),
                        args: vec![
                            (LlvmType::ptr(), local("fn_ptr")),
                            (LlvmType::i32(), local("new.remaining")),
                            (LlvmType::ptr(), local("payload")),
                            (LlvmType::i32(), local("capture_count")),
                            (LlvmType::ptr(), local("new.applied")),
                            (LlvmType::i32(), local("new.applied.count")),
                        ],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("under.result"),
                },
            },
            LlvmBlock {
                label: LabelId("over".into()),
                instrs: vec![
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("over.first".into())),
                        tail: false,
                        // Use ccc for indirect call — matches ccc function defs.
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::i64(),
                        callee: local("fn_ptr"),
                        args: vec![
                            (LlvmType::i64(), local("closure_value")),
                            (LlvmType::ptr(), local("args")),
                            (LlvmType::i32(), local("remaining_arity")),
                        ],
                        attrs: vec![],
                    },
                    LlvmInstr::Binary {
                        dst: LlvmLocal("leftover.count".into()),
                        op: LlvmValueKind::Sub,
                        ty: LlvmType::i32(),
                        lhs: local("nargs"),
                        rhs: local("remaining_arity"),
                    },
                    LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("leftover.args".into()),
                        inbounds: true,
                        element_ty: LlvmType::i64(),
                        base: local("args"),
                        indices: vec![(LlvmType::i32(), local("remaining_arity"))],
                    },
                    LlvmInstr::Call {
                        dst: Some(LlvmLocal("over.result".into())),
                        tail: true,
                        call_conv: Some(CallConv::Fastcc),
                        ret_ty: LlvmType::i64(),
                        callee: LlvmOperand::Global(flux_closure_symbol("flux_call_closure")),
                        args: vec![
                            (LlvmType::i64(), local("over.first")),
                            (LlvmType::ptr(), local("leftover.args")),
                            (LlvmType::i32(), local("leftover.count")),
                        ],
                        attrs: vec![],
                    },
                ],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("over.result"),
                },
            },
        ],
    });
}

/// C-callable trampoline for `flux_call_closure`.
/// Uses Ccc so C runtime functions (map, filter, sort, any) can call closures.
fn emit_call_closure_c(module: &mut LlvmModule) {
    let name = "flux_call_closure_c";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::External,
        name: flux_closure_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::i64(), LlvmType::ptr(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        params: vec![
            LlvmLocal("closure_value".into()),
            LlvmLocal("args".into()),
            LlvmLocal("nargs".into()),
        ],
        attrs: vec![],
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![LlvmInstr::Call {
                dst: Some(LlvmLocal("result".into())),
                tail: false,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::i64(),
                callee: LlvmOperand::Global(flux_closure_symbol("flux_call_closure")),
                args: vec![
                    (LlvmType::i64(), local("closure_value")),
                    (LlvmType::ptr(), local("args")),
                    (LlvmType::i32(), local("nargs")),
                ],
                attrs: vec![],
            }],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

pub(super) fn common_closure_load_instrs(closure_value: LlvmOperand) -> Vec<LlvmInstr> {
    vec![
        LlvmInstr::Call {
            dst: Some(LlvmLocal("closure".into())),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::ptr(),
            callee: LlvmOperand::Global(flux_closure_symbol("flux_untag_boxed_ptr")),
            args: vec![(LlvmType::i64(), closure_value)],
            attrs: vec![],
        },
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("fn_ptr.slot".into()),
            inbounds: true,
            element_ty: closure_type(),
            base: local("closure"),
            indices: vec![
                (LlvmType::i32(), const_i32_operand(0)),
                (
                    LlvmType::i32(),
                    const_i32_operand(FLUX_CLOSURE_FN_FIELD as i32),
                ),
            ],
        },
        LlvmInstr::Load {
            dst: LlvmLocal("fn_ptr".into()),
            ty: LlvmType::ptr(),
            ptr: local("fn_ptr.slot"),
            align: Some(8),
        },
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("remaining.slot".into()),
            inbounds: true,
            element_ty: closure_type(),
            base: local("closure"),
            indices: vec![
                (LlvmType::i32(), const_i32_operand(0)),
                (
                    LlvmType::i32(),
                    const_i32_operand(FLUX_CLOSURE_REMAINING_ARITY_FIELD as i32),
                ),
            ],
        },
        LlvmInstr::Load {
            dst: LlvmLocal("remaining_arity".into()),
            ty: LlvmType::i32(),
            ptr: local("remaining.slot"),
            align: Some(4),
        },
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("capture.slot".into()),
            inbounds: true,
            element_ty: closure_type(),
            base: local("closure"),
            indices: vec![
                (LlvmType::i32(), const_i32_operand(0)),
                (
                    LlvmType::i32(),
                    const_i32_operand(FLUX_CLOSURE_CAPTURE_COUNT_FIELD as i32),
                ),
            ],
        },
        LlvmInstr::Load {
            dst: LlvmLocal("capture_count".into()),
            ty: LlvmType::i32(),
            ptr: local("capture.slot"),
            align: Some(4),
        },
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("applied.slot".into()),
            inbounds: true,
            element_ty: closure_type(),
            base: local("closure"),
            indices: vec![
                (LlvmType::i32(), const_i32_operand(0)),
                (
                    LlvmType::i32(),
                    const_i32_operand(FLUX_CLOSURE_APPLIED_COUNT_FIELD as i32),
                ),
            ],
        },
        LlvmInstr::Load {
            dst: LlvmLocal("applied_count".into()),
            ty: LlvmType::i32(),
            ptr: local("applied.slot"),
            align: Some(4),
        },
        LlvmInstr::GetElementPtr {
            dst: LlvmLocal("payload".into()),
            inbounds: true,
            element_ty: closure_type(),
            base: local("closure"),
            indices: vec![
                (LlvmType::i32(), const_i32_operand(0)),
                (
                    LlvmType::i32(),
                    const_i32_operand(FLUX_CLOSURE_PAYLOAD_FIELD as i32),
                ),
                (LlvmType::i32(), const_i32_operand(0)),
            ],
        },
        LlvmInstr::Icmp {
            dst: LlvmLocal("is_exact".into()),
            op: LlvmCmpOp::Eq,
            ty: LlvmType::i32(),
            lhs: local("nargs"),
            rhs: local("remaining_arity"),
        },
        LlvmInstr::Icmp {
            dst: LlvmLocal("is_under".into()),
            op: LlvmCmpOp::Slt,
            ty: LlvmType::i32(),
            lhs: local("nargs"),
            rhs: local("remaining_arity"),
        },
    ]
}

pub(super) fn local(name: &str) -> LlvmOperand {
    LlvmOperand::Local(LlvmLocal(name.into()))
}

pub(super) fn const_i32_operand(value: i32) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 32,
        value: value.into(),
    })
}

pub(super) fn const_i64_operand(value: i64) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: value.into(),
    })
}

#[cfg(test)]
mod tests {
    use crate::core_to_llvm::{LlvmModule, render_module};

    use super::*;

    #[test]
    fn emits_closure_type_and_boxed_ptr_helpers() {
        let mut module = LlvmModule::new();
        emit_closure_support(&mut module);
        let rendered = render_module(&module);

        assert!(rendered.contains("%FluxClosure = type {ptr, i32, i32, i32, i32, [0 x i64]}"));
        assert!(rendered.contains("declare ccc ptr @flux_gc_alloc(i32)"));
        assert!(rendered.contains("define internal fastcc i64 @flux_tag_boxed_ptr(ptr %ptr)"));
        assert!(rendered.contains("lshr i64 %addr, 3"));
        assert!(rendered.contains("define internal fastcc ptr @flux_untag_boxed_ptr(i64 %value)"));
        assert!(rendered.contains("shl i64 %payload, 3"));
    }

    #[test]
    fn emits_make_and_call_closure_helpers() {
        let mut module = LlvmModule::new();
        emit_closure_support(&mut module);
        let rendered = render_module(&module);

        assert!(rendered.contains("define internal fastcc i64 @flux_make_closure("));
        assert!(rendered.contains("call ccc ptr @flux_gc_alloc(i32 %size)"));
        assert!(rendered.contains("call fastcc void @flux_copy_i64s("));
        assert!(rendered.contains("define internal fastcc i64 @flux_call_closure(i64 %closure_value, ptr %args, i32 %nargs)"));
        assert!(
            rendered.contains("call ccc i64 %fn_ptr(i64 %closure_value, ptr %args, i32 %nargs)")
        );
        assert!(rendered.contains("call fastcc i64 @flux_call_closure(i64 %over.first, ptr %leftover.args, i32 %leftover.count)"));
    }
}
