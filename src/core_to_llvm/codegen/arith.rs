use crate::core_to_llvm::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmFunction, LlvmFunctionSig, LlvmInstr,
    LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmValueKind,
};

use super::prelude::{emit_prelude, flux_prelude_symbol};

pub fn flux_arith_symbol(name: &str) -> GlobalId {
    GlobalId(name.to_string())
}

pub fn emit_arith(module: &mut LlvmModule) {
    emit_prelude(module);
    emit_iadd(module);
    emit_isub(module);
    emit_imul(module);
    emit_idiv(module);
    emit_fadd(module);
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

fn emit_tagged_binary_helper(module: &mut LlvmModule, name: &str, op: LlvmValueKind) {
    if has_function(module, name) {
        return;
    }

    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
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

fn emit_fadd(module: &mut LlvmModule) {
    let name = "flux_fadd";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
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
                    dst: LlvmLocal("sum".into()),
                    op: LlvmValueKind::FAdd,
                    ty: LlvmType::Double,
                    lhs: local("a_bits"),
                    rhs: local("b_bits"),
                },
                LlvmInstr::Cast {
                    dst: LlvmLocal("result".into()),
                    op: LlvmValueKind::Bitcast,
                    from_ty: LlvmType::Double,
                    operand: local("sum"),
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

fn binary_i64_sig() -> LlvmFunctionSig {
    LlvmFunctionSig {
        ret: LlvmType::i64(),
        params: vec![LlvmType::i64(), LlvmType::i64()],
        varargs: false,
        call_conv: CallConv::Fastcc,
    }
}

fn helper_attrs() -> Vec<String> {
    vec!["alwaysinline".into()]
}

fn has_function(module: &LlvmModule, name: &str) -> bool {
    module.functions.iter().any(|f| f.name.0 == name)
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
        assert!(rendered.contains("%sum = fadd double %a_bits, %b_bits"));
        assert!(rendered.contains("%result = bitcast double %sum to i64"));
    }
}
