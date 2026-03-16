//! Canonical backend IR facade.
//!
//! `crate::backend_ir` is the stable architectural name for Flux's backend IR
//! layer. Today it re-exports the CFG-based implementation, while `crate::cfg`
//! remains the private backend engine under this boundary.

use std::collections::HashMap;

use crate::{
    core::{lower_ast::lower_program_ast, passes::run_core_passes, to_ir::lower_core_to_ir},
    diagnostics::Diagnostic,
    syntax::{expression::ExprId, program::Program},
    types::infer_type::InferType,
};

pub mod primitives;
pub mod lower {
    pub use crate::cfg::lower::*;
}

pub mod passes {
    pub use crate::cfg::passes::*;
}

pub mod validate {
    pub use crate::cfg::validate::*;
}

#[allow(clippy::result_large_err)]
pub fn lower_program_to_ir(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
) -> Result<IrProgram, Diagnostic> {
    let mut core = lower_program_ast(program, hm_expr_types);
    run_core_passes(&mut core);
    let mut ir = lower_core_to_ir(&core);
    ir.hm_expr_types = hm_expr_types.clone();
    ir.core = Some(core);
    Ok(ir)
}

pub use crate::cfg::{
    AdtId, BlockId, EffectId, FunctionId, GlobalId, HandleScopeArm, IrBinaryOp, IrBlock,
    IrBlockParam, IrCallTarget, IrConst, IrExpr, IrFunction, IrFunctionOrigin, IrGlobalBinding,
    IrHandleArm, IrInstr, IrListTest, IrMetadata, IrParam, IrPassContext, IrProgram, IrStringPart,
    IrTagTest, IrTerminator, IrTopLevelItem, IrType, IrVar, LiteralId,
    patch_function_ids_from_core, run_ir_pass_pipeline, validate_ir,
};
