pub mod codegen;
pub mod ir;

pub use codegen::{
    CoreToLlvmError, FluxNanboxLayout, compile_program, compile_program_with_interner, emit_arith,
    emit_prelude, emit_prelude_and_arith, flux_arith_symbol, flux_prelude_symbol,
};
pub use ir::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCallingConv, LlvmCmpOp, LlvmConst,
    LlvmDecl, LlvmFunction, LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule,
    LlvmOperand, LlvmTerminator, LlvmType, LlvmTypeDef, LlvmValueKind, render_module,
};
