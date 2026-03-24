use crate::core_to_llvm::{
    CallConv, GlobalId, LabelId, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmFunction, LlvmFunctionSig,
    LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmValueKind,
};

use super::prelude::{
    FluxNanboxLayout, emit_prelude, flux_prelude_symbol, has_function, helper_attrs,
    linkage_internal, unary_i64_sig,
};

pub fn flux_arith_symbol(name: &str) -> GlobalId {
    flux_prelude_symbol(name)
}

pub fn emit_arith(module: &mut LlvmModule) {
    emit_prelude(module);
    emit_iadd(module);
    emit_isub(module);
    emit_imul(module);
    emit_idiv(module);
    emit_imod(module);
    emit_ineg(module);
    emit_fadd(module);
    emit_fsub(module);
    emit_fmul(module);
    emit_fdiv(module);
    emit_fmod(module);
    emit_fneg(module);
    emit_not(module);
    emit_and(module);
    emit_or(module);
    // Runtime-dispatching wrappers (check int vs float at runtime)
    emit_dispatch_binary(module, "flux_add", "flux_iadd", "flux_fadd");
    emit_dispatch_binary(module, "flux_sub", "flux_isub", "flux_fsub");
    emit_dispatch_binary(module, "flux_mul", "flux_imul", "flux_fmul");
    emit_dispatch_binary(module, "flux_div", "flux_idiv", "flux_fdiv");
    emit_dispatch_binary(module, "flux_mod", "flux_imod", "flux_fmod");
    emit_dispatch_unary(module, "flux_neg", "flux_ineg", "flux_fneg");
}

fn emit_iadd(module: &mut LlvmModule) {
    emit_tagged_binary_helper(module, "flux_iadd", LlvmValueKind::Add);
}

fn emit_isub(module: &mut LlvmModule) {
    emit_tagged_binary_helper(module, "flux_isub", LlvmValueKind::Sub);
}

fn emit_imul(module: &mut LlvmModule) {
    emit_tagged_binary_helper(module, "flux_imul", LlvmValueKind::Mul);
}

fn emit_idiv(module: &mut LlvmModule) {
    emit_tagged_binary_helper(module, "flux_idiv", LlvmValueKind::SDiv);
}

fn emit_imod(module: &mut LlvmModule) {
    emit_tagged_binary_helper(module, "flux_imod", LlvmValueKind::SRem);
}

fn emit_tagged_binary_helper(module: &mut LlvmModule, name: &str, op: LlvmValueKind) {
    if has_function(module, name) {
        return;
    }

    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: binary_i64_sig(),
        params: vec![LlvmLocal("a".into()), LlvmLocal("b".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                call_i64("a_raw", "flux_untag_int", vec![local("a")]),
                call_i64("b_raw", "flux_untag_int", vec![local("b")]),
                LlvmInstr::Binary {
                    dst: LlvmLocal("result_raw".into()),
                    op,
                    ty: LlvmType::i64(),
                    lhs: local("a_raw"),
                    rhs: local("b_raw"),
                },
                call_i64("result", "flux_tag_int", vec![local("result_raw")]),
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

fn emit_ineg(module: &mut LlvmModule) {
    let name = "flux_ineg";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("a".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                call_i64("a_raw", "flux_untag_int", vec![local("a")]),
                LlvmInstr::Binary {
                    dst: LlvmLocal("neg".into()),
                    op: LlvmValueKind::Sub,
                    ty: LlvmType::i64(),
                    lhs: LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 0 }),
                    rhs: local("a_raw"),
                },
                call_i64("result", "flux_tag_int", vec![local("neg")]),
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

fn emit_fadd(module: &mut LlvmModule) {
    emit_float_binary_helper(module, "flux_fadd", LlvmValueKind::FAdd);
}

fn emit_fsub(module: &mut LlvmModule) {
    emit_float_binary_helper(module, "flux_fsub", LlvmValueKind::FSub);
}

fn emit_fmul(module: &mut LlvmModule) {
    emit_float_binary_helper(module, "flux_fmul", LlvmValueKind::FMul);
}

fn emit_fdiv(module: &mut LlvmModule) {
    emit_float_binary_helper(module, "flux_fdiv", LlvmValueKind::FDiv);
}

fn emit_fneg(module: &mut LlvmModule) {
    let name = "flux_fneg";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("a".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Cast {
                    dst: LlvmLocal("a_bits".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::i64(),
                    operand: local("a"),
                    to_ty: LlvmType::Double,
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("neg".into()),
                    op: LlvmValueKind::FSub,
                    ty: LlvmType::Double,
                    lhs: LlvmOperand::Const(LlvmConst::Float(0.0)),
                    rhs: local("a_bits"),
                },
                LlvmInstr::Cast {
                    dst: LlvmLocal("result".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::Double,
                    operand: local("neg"),
                    to_ty: LlvmType::i64(),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

fn emit_fmod(module: &mut LlvmModule) {
    emit_float_binary_helper(module, "flux_fmod", LlvmValueKind::FRem);
}

/// Emit a binary dispatch wrapper: check if the first arg is a NaN-boxed int
/// (sentinel bits set) and call the int helper, otherwise call the float helper.
fn emit_dispatch_binary(module: &mut LlvmModule, name: &str, int_helper: &str, float_helper: &str) {
    if has_function(module, name) {
        return;
    }
    let sentinel_mask = FluxNanboxLayout::SENTINEL_MASK_U64 as i128;
    let sentinel = FluxNanboxLayout::NANBOX_SENTINEL_U64 as i128;

    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: binary_i64_sig(),
        params: vec![LlvmLocal("a".into()), LlvmLocal("b".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![
                    LlvmInstr::Binary {
                        dst: LlvmLocal("masked".into()),
                        op: LlvmValueKind::And,
                        ty: LlvmType::i64(),
                        lhs: local("a"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 64,
                            value: sentinel_mask,
                        }),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("is_nanbox".into()),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::i64(),
                        lhs: local("masked"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 64,
                            value: sentinel,
                        }),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_nanbox"),
                    then_label: LabelId("do_int".into()),
                    else_label: LabelId("do_float".into()),
                },
            },
            LlvmBlock {
                label: LabelId("do_int".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("int_result".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_prelude_symbol(int_helper)),
                    args: vec![(LlvmType::i64(), local("a")), (LlvmType::i64(), local("b"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("int_result"),
                },
            },
            LlvmBlock {
                label: LabelId("do_float".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("float_result".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_prelude_symbol(float_helper)),
                    args: vec![(LlvmType::i64(), local("a")), (LlvmType::i64(), local("b"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("float_result"),
                },
            },
        ],
    });
}

/// Emit a unary dispatch wrapper: check if the arg is NaN-boxed int
/// and call the int helper, otherwise call the float helper.
fn emit_dispatch_unary(module: &mut LlvmModule, name: &str, int_helper: &str, float_helper: &str) {
    if has_function(module, name) {
        return;
    }
    let sentinel_mask = FluxNanboxLayout::SENTINEL_MASK_U64 as i128;
    let sentinel = FluxNanboxLayout::NANBOX_SENTINEL_U64 as i128;

    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("a".into())],
        attrs: helper_attrs(),
        blocks: vec![
            LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![
                    LlvmInstr::Binary {
                        dst: LlvmLocal("masked".into()),
                        op: LlvmValueKind::And,
                        ty: LlvmType::i64(),
                        lhs: local("a"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 64,
                            value: sentinel_mask,
                        }),
                    },
                    LlvmInstr::Icmp {
                        dst: LlvmLocal("is_nanbox".into()),
                        op: LlvmCmpOp::Eq,
                        ty: LlvmType::i64(),
                        lhs: local("masked"),
                        rhs: LlvmOperand::Const(LlvmConst::Int {
                            bits: 64,
                            value: sentinel,
                        }),
                    },
                ],
                term: LlvmTerminator::CondBr {
                    cond_ty: LlvmType::i1(),
                    cond: local("is_nanbox"),
                    then_label: LabelId("do_int".into()),
                    else_label: LabelId("do_float".into()),
                },
            },
            LlvmBlock {
                label: LabelId("do_int".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("int_result".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_prelude_symbol(int_helper)),
                    args: vec![(LlvmType::i64(), local("a"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("int_result"),
                },
            },
            LlvmBlock {
                label: LabelId("do_float".into()),
                instrs: vec![LlvmInstr::Call {
                    dst: Some(LlvmLocal("float_result".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_prelude_symbol(float_helper)),
                    args: vec![(LlvmType::i64(), local("a"))],
                    attrs: vec![],
                }],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: local("float_result"),
                },
            },
        ],
    });
}

fn emit_float_binary_helper(module: &mut LlvmModule, name: &str, op: LlvmValueKind) {
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: binary_i64_sig(),
        params: vec![LlvmLocal("a".into()), LlvmLocal("b".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Cast {
                    dst: LlvmLocal("a_bits".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::i64(),
                    operand: local("a"),
                    to_ty: LlvmType::Double,
                },
                LlvmInstr::Cast {
                    dst: LlvmLocal("b_bits".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::i64(),
                    operand: local("b"),
                    to_ty: LlvmType::Double,
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("result_f".into()),
                    op,
                    ty: LlvmType::Double,
                    lhs: local("a_bits"),
                    rhs: local("b_bits"),
                },
                LlvmInstr::Cast {
                    dst: LlvmLocal("result".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::Double,
                    operand: local("result_f"),
                    to_ty: LlvmType::i64(),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

pub fn emit_not(module: &mut LlvmModule) {
    // Flux truthiness: false, None, EmptyList, and Uninit are falsy.
    // Everything else is truthy. Not(x) returns true if x is falsy.
    let name = "flux_not";
    if has_function(module, name) {
        return;
    }
    let true_bits = tagged_bool_bits(true);
    let false_bits = tagged_bool_bits(false);
    let none_bits = tagged_special_bits(0x2); // TAG_NONE
    let empty_list_bits = tagged_special_bits(0x4); // TAG_EMPTY_LIST
    let uninit_bits = tagged_special_bits(0x3); // TAG_UNINIT

    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: unary_i64_sig(),
        params: vec![LlvmLocal("a".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                // Check if value is one of the falsy values
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_false".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("a"),
                    rhs: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: false_bits.into(),
                    }),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_none".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("a"),
                    rhs: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: none_bits.into(),
                    }),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_empty".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("a"),
                    rhs: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: empty_list_bits.into(),
                    }),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("is_uninit".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("a"),
                    rhs: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: uninit_bits.into(),
                    }),
                },
                // falsy = is_false || is_none || is_empty || is_uninit
                LlvmInstr::Binary {
                    dst: LlvmLocal("f1".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i1(),
                    lhs: local("is_false"),
                    rhs: local("is_none"),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("f2".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i1(),
                    lhs: local("is_empty"),
                    rhs: local("is_uninit"),
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("is_falsy".into()),
                    op: LlvmValueKind::Or,
                    ty: LlvmType::i1(),
                    lhs: local("f1"),
                    rhs: local("f2"),
                },
                // Not(falsy_value) = true, Not(truthy_value) = false
                LlvmInstr::Select {
                    dst: LlvmLocal("result".into()),
                    cond_ty: LlvmType::i1(),
                    cond: local("is_falsy"),
                    value_ty: LlvmType::i64(),
                    then_value: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: true_bits.into(),
                    }),
                    else_value: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: false_bits.into(),
                    }),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

pub fn emit_and(module: &mut LlvmModule) {
    emit_bool_binary_helper(module, "flux_and", true);
}

pub fn emit_or(module: &mut LlvmModule) {
    emit_bool_binary_helper(module, "flux_or", false);
}

fn emit_bool_binary_helper(module: &mut LlvmModule, name: &str, is_and: bool) {
    if has_function(module, name) {
        return;
    }
    let true_bits = tagged_bool_bits(true);
    let false_bits = tagged_bool_bits(false);
    let true_const = LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: true_bits.into(),
    });
    let false_const = LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: false_bits.into(),
    });
    // and: both must be true  => a == true_bits && b == true_bits
    // or:  either can be true => a == true_bits || b == true_bits
    let (combine_op, identity) = if is_and {
        (LlvmValueKind::And, true_const.clone())
    } else {
        (LlvmValueKind::Or, false_const.clone())
    };
    let _ = identity; // suppress unused warning
    module.functions.push(LlvmFunction {
        linkage: linkage_internal(),
        name: flux_arith_symbol(name),
        sig: binary_i64_sig(),
        params: vec![LlvmLocal("a".into()), LlvmLocal("b".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Icmp {
                    dst: LlvmLocal("a_is_true".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("a"),
                    rhs: true_const.clone(),
                },
                LlvmInstr::Icmp {
                    dst: LlvmLocal("b_is_true".into()),
                    op: LlvmCmpOp::Eq,
                    ty: LlvmType::i64(),
                    lhs: local("b"),
                    rhs: true_const,
                },
                LlvmInstr::Binary {
                    dst: LlvmLocal("combined".into()),
                    op: combine_op,
                    ty: LlvmType::i1(),
                    lhs: local("a_is_true"),
                    rhs: local("b_is_true"),
                },
                LlvmInstr::Select {
                    dst: LlvmLocal("result".into()),
                    cond_ty: LlvmType::i1(),
                    cond: local("combined"),
                    value_ty: LlvmType::i64(),
                    then_value: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: true_bits.into(),
                    }),
                    else_value: LlvmOperand::Const(LlvmConst::Int {
                        bits: 64,
                        value: false_bits.into(),
                    }),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local("result"),
            },
        }],
    });
}

fn binary_i64_sig() -> LlvmFunctionSig {
    LlvmFunctionSig {
        ret: LlvmType::i64(),
        params: vec![LlvmType::i64(), LlvmType::i64()],
        varargs: false,
        call_conv: CallConv::Fastcc,
    }
}

fn local(name: &str) -> LlvmOperand {
    LlvmOperand::Local(LlvmLocal(name.into()))
}

fn call_i64(dst: &str, callee: &str, args: Vec<LlvmOperand>) -> LlvmInstr {
    LlvmInstr::Call {
        dst: Some(LlvmLocal(dst.into())),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::i64(),
        callee: LlvmOperand::Global(flux_prelude_symbol(callee)),
        args: args.into_iter().map(|arg| (LlvmType::i64(), arg)).collect(),
        attrs: vec![],
    }
}

fn tagged_special_bits(tag: u64) -> i64 {
    (FluxNanboxLayout::NANBOX_SENTINEL_U64 | (tag << FluxNanboxLayout::TAG_SHIFT)) as i64
}

fn tagged_bool_bits(value: bool) -> i64 {
    use crate::runtime::nanbox::NanTag;
    (FluxNanboxLayout::NANBOX_SENTINEL_U64
        | ((NanTag::Boolean as u64) << FluxNanboxLayout::TAG_SHIFT)
        | u64::from(value)) as i64
}

#[cfg(test)]
mod tests {
    use crate::core_to_llvm::{LlvmModule, render_module};

    use super::*;

    #[test]
    fn emits_integer_arithmetic_helpers() {
        let mut module = LlvmModule::new();
        emit_arith(&mut module);
        let rendered = render_module(&module);
        assert!(
            rendered.contains("define internal fastcc i64 @flux_iadd(i64 %a, i64 %b) alwaysinline")
        );
        assert!(rendered.contains("%a_raw = call fastcc i64 @flux_untag_int(i64 %a)"));
        assert!(rendered.contains("%result_raw = add i64 %a_raw, %b_raw"));
        assert!(
            rendered.contains("define internal fastcc i64 @flux_idiv(i64 %a, i64 %b) alwaysinline")
        );
        assert!(rendered.contains("%result_raw = sdiv i64 %a_raw, %b_raw"));
    }

    #[test]
    fn emits_float_add_via_bitcast_roundtrip() {
        let mut module = LlvmModule::new();
        emit_arith(&mut module);
        let rendered = render_module(&module);
        assert!(
            rendered.contains("define internal fastcc i64 @flux_fadd(i64 %a, i64 %b) alwaysinline")
        );
        assert!(rendered.contains("%a_bits = bitcast i64 %a to double"));
        assert!(rendered.contains("%result_f = fadd double %a_bits, %b_bits"));
        assert!(rendered.contains("%result = bitcast double %result_f to i64"));
    }
}
