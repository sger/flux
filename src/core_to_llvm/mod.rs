pub mod ir;

pub use ir::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCallingConv, LlvmCmpOp, LlvmConst,
    LlvmDecl, LlvmFunction, LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule,
    LlvmOperand, LlvmTerminator, LlvmType, LlvmTypeDef, LlvmValueKind, render_module,
};
