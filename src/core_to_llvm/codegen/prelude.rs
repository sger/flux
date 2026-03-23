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
    emit_dup_stub(module);
    emit_drop_stub(module);
    emit_drop_reuse_stub(module);
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

fn emit_dup_stub(module: &mut LlvmModule) {
    emit_void_stub(
        module,
        "flux_dup",
        vec![LlvmType::i64()],
        vec![LlvmLocal("val".into())],
    );
}

fn emit_drop_stub(module: &mut LlvmModule) {
    emit_void_stub(
        module,
        "flux_drop",
        vec![LlvmType::i64()],
        vec![LlvmLocal("val".into())],
    );
}

fn emit_drop_reuse_stub(module: &mut LlvmModule) {
    let name = "flux_drop_reuse";
    if has_function(module, name) {
        return;
    }
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
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![],
            term: LlvmTerminator::Ret {
                ty: LlvmType::ptr(),
                value: LlvmOperand::Const(LlvmConst::Null),
            },
        }],
    });
}

fn emit_void_stub(
    module: &mut LlvmModule,
    name: &str,
    params: Vec<LlvmType>,
    locals: Vec<LlvmLocal>,
) {
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_prelude_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::Void,
            params,
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: locals,
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![],
            term: LlvmTerminator::RetVoid,
        }],
    });
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
    fn emits_is_ptr_and_stubbed_reference_helpers() {
        let mut module = LlvmModule::new();
        emit_prelude(&mut module);
        let rendered = render_module(&module);
        assert!(rendered.contains("define internal fastcc i1 @flux_is_ptr(i64 %val) alwaysinline"));
        assert!(rendered.contains("%tag = and i64 %tag_bits, 15"));
        assert!(rendered.contains("%result = select i1 %is_boxed, i1 %is_boxed_value, i1 0"));
        assert!(rendered.contains("define internal fastcc void @flux_dup(i64 %val) alwaysinline"));
        assert!(rendered.contains(
            "define internal fastcc ptr @flux_drop_reuse(i64 %val, i32 %size) alwaysinline"
        ));
        assert!(rendered.contains("ret ptr null"));
    }
}
