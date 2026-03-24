#![cfg(feature = "native")]

use flux::core_to_llvm::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmDecl, LlvmFunction,
    LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator,
    LlvmType, LlvmTypeDef, LlvmValueKind, render_module,
};

#[test]
fn hand_written_module_snapshot() {
    let module = LlvmModule {
        source_filename: Some("snapshot.flx".into()),
        target_triple: Some("x86_64-unknown-linux-gnu".into()),
        data_layout: Some("e-m:e-p:64:64-i64:64-n8:16:32:64-S128".into()),
        type_defs: vec![LlvmTypeDef {
            name: "Closure".into(),
            ty: LlvmType::Struct {
                packed: false,
                fields: vec![
                    LlvmType::ptr(),
                    LlvmType::i32(),
                    LlvmType::i32(),
                    LlvmType::Array {
                        len: 0,
                        element: Box::new(LlvmType::i64()),
                    },
                ],
            },
        }],
        globals: vec![LlvmGlobal {
            linkage: Linkage::Private,
            name: GlobalId("flux.tag.true".into()),
            ty: LlvmType::i64(),
            is_constant: true,
            value: LlvmConst::Int {
                bits: 64,
                value: 9221120237041090562,
            },
            attrs: vec![],
        }],
        declarations: vec![LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId("flux_print".into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::Void,
                params: vec![LlvmType::i64()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec![],
        }],
        functions: vec![LlvmFunction {
            linkage: Linkage::Internal,
            name: GlobalId("flux.main".into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: vec![LlvmType::i64()],
                varargs: false,
                call_conv: CallConv::Fastcc,
            },
            params: vec![LlvmLocal("arg".into())],
            attrs: vec!["alwaysinline".into()],
            blocks: vec![
                LlvmBlock {
                    label: LabelId("entry".into()),
                    instrs: vec![
                        LlvmInstr::Alloca {
                            dst: LlvmLocal("slot".into()),
                            ty: LlvmType::i64(),
                            count: None,
                            align: Some(8),
                        },
                        LlvmInstr::Store {
                            ty: LlvmType::i64(),
                            value: LlvmOperand::Local(LlvmLocal("arg".into())),
                            ptr: LlvmOperand::Local(LlvmLocal("slot".into())),
                            align: Some(8),
                        },
                        LlvmInstr::Load {
                            dst: LlvmLocal("loaded".into()),
                            ty: LlvmType::i64(),
                            ptr: LlvmOperand::Local(LlvmLocal("slot".into())),
                            align: Some(8),
                        },
                        LlvmInstr::Icmp {
                            dst: LlvmLocal("is_zero".into()),
                            op: LlvmCmpOp::Eq,
                            ty: LlvmType::i64(),
                            lhs: LlvmOperand::Local(LlvmLocal("loaded".into())),
                            rhs: LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 0 }),
                        },
                    ],
                    term: LlvmTerminator::CondBr {
                        cond_ty: LlvmType::i1(),
                        cond: LlvmOperand::Local(LlvmLocal("is_zero".into())),
                        then_label: LabelId("zero".into()),
                        else_label: LabelId("nonzero".into()),
                    },
                },
                LlvmBlock {
                    label: LabelId("zero".into()),
                    instrs: vec![LlvmInstr::Call {
                        dst: None,
                        tail: false,
                        call_conv: Some(CallConv::Ccc),
                        ret_ty: LlvmType::Void,
                        callee: LlvmOperand::Global(GlobalId("flux_print".into())),
                        args: vec![(
                            LlvmType::i64(),
                            LlvmOperand::Const(LlvmConst::Int {
                                bits: 64,
                                value: 9221120237041090562,
                            }),
                        )],
                        attrs: vec![],
                    }],
                    term: LlvmTerminator::Br {
                        target: LabelId("join".into()),
                    },
                },
                LlvmBlock {
                    label: LabelId("nonzero".into()),
                    instrs: vec![LlvmInstr::Binary {
                        dst: LlvmLocal("dec".into()),
                        op: LlvmValueKind::Sub,
                        ty: LlvmType::i64(),
                        lhs: LlvmOperand::Local(LlvmLocal("loaded".into())),
                        rhs: LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 1 }),
                    }],
                    term: LlvmTerminator::Br {
                        target: LabelId("join".into()),
                    },
                },
                LlvmBlock {
                    label: LabelId("join".into()),
                    instrs: vec![LlvmInstr::Phi {
                        dst: LlvmLocal("result".into()),
                        ty: LlvmType::i64(),
                        incoming: vec![
                            (
                                LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 0 }),
                                LabelId("zero".into()),
                            ),
                            (
                                LlvmOperand::Local(LlvmLocal("dec".into())),
                                LabelId("nonzero".into()),
                            ),
                        ],
                    }],
                    term: LlvmTerminator::Ret {
                        ty: LlvmType::i64(),
                        value: LlvmOperand::Local(LlvmLocal("result".into())),
                    },
                },
            ],
        }],
    };

    insta::with_settings!({
        snapshot_path => "snapshots/core_to_llvm",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("hand_written_module", render_module(&module));
    });
}
