pub mod codegen;
pub mod ir;
pub mod pipeline;
pub mod target;

pub use codegen::{
    CoreToLlvmError, FLUX_ADT_TYPE_NAME, FLUX_CLOSURE_TYPE_NAME, FLUX_TUPLE_TYPE_NAME,
    FluxNanboxLayout, closure_type, emit_adt_support, emit_arith, emit_closure_support,
    emit_prelude, emit_prelude_and_arith, flux_adt_symbol, flux_arith_symbol, flux_closure_symbol,
    flux_prelude_symbol,
};
pub use ir::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCallingConv, LlvmCmpOp, LlvmConst,
    LlvmDecl, LlvmFunction, LlvmFunctionSig, LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule,
    LlvmOperand, LlvmTerminator, LlvmType, LlvmTypeDef, LlvmValueKind, render_module,
};
