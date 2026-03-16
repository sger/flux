use std::collections::{HashMap, HashSet};

use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

use super::{
    BlockId, IrBinaryOp, IrConst, IrExpr, IrFunction, IrInstr, IrMetadata, IrProgram, IrTerminator,
    IrVar, validate::validate_ir,
};

#[derive(Debug, Clone, Default)]
pub struct IrPassContext;

#[allow(clippy::result_large_err)]
pub fn run_ir_pass_pipeline(
    program: &mut IrProgram,
    _context: &IrPassContext,
) -> Result<(), crate::diagnostics::Diagnostic> {
    // Remove unreachable blocks first so the validator does not reject dead
    // code that references variables defined in predecessor blocks that are
    // no longer reachable (e.g. the fail-block of a wildcard match arm).
    dead_block_elimination(program);
    validate_ir(program)?;
    canonicalize_cfg(program);
    validate_ir(program)?;
    constant_fold(program);
    // constant_fold can fold conditional branches into unconditional jumps,
    // leaving the dead branch target unreachable.  Eliminate those blocks
    // before the validator runs so it does not reject dead code that
    // references variables defined only in the (now-removed) predecessor.
    dead_block_elimination(program);
    validate_ir(program)?;
    local_cse(program);
    validate_ir(program)?;
    tail_call_introduction(program);
    validate_ir(program)?;
    intern_unit_adts(program);
    validate_ir(program)?;
    type_directed_unboxing(program);
    validate_ir(program)?;
    Ok(())
}

fn canonicalize_cfg(program: &mut IrProgram) {
    for function in &mut program.functions {
        for block in &mut function.blocks {
            if matches!(block.terminator, IrTerminator::Unreachable(_))
                && let Some(last) = block.instrs.last()
                && let IrInstr::Assign { dest, metadata, .. } = last
            {
                block.terminator = IrTerminator::Return(*dest, metadata.clone());
            }
        }
    }
}

/// Convert tail-position calls into `TailCall` terminators.
///
/// Recognises two patterns:
///   1. `Call { dest: v }; Return(v)` — direct tail call.
///   2. `Call { dest: v }; Jump(target, [v])` where the target block is a
///      single-param return block (`params: [p], instrs: [], Return(p)`).
///      This pattern arises from `if`/`match` merge blocks.
///
/// This pass enables the bytecode compiler to emit `OpTailCall` for
/// self-recursive tail calls, avoiding stack overflow.
fn tail_call_introduction(program: &mut IrProgram) {
    for function in &mut program.functions {
        let block_map: HashMap<BlockId, TailReturnSummary> = function
            .blocks
            .iter()
            .map(|block| {
                (
                    block.id,
                    TailReturnSummary {
                        first_param: block.params.first().map(|param| param.var),
                        params_len: block.params.len(),
                        is_empty: block.instrs.is_empty(),
                        terminator: block.terminator.clone(),
                    },
                )
            })
            .collect();

        for block in &mut function.blocks {
            // Pattern 1: Call; Return(v)
            if let IrTerminator::Return(return_var, _) = &block.terminator
                && let return_var = *return_var
                && let Some(IrInstr::Call {
                    dest,
                    target,
                    args,
                    metadata,
                }) = block.instrs.last()
                && *dest == return_var
            {
                let tc = IrTerminator::TailCall {
                    callee: target.clone(),
                    args: args.clone(),
                    metadata: metadata.clone(),
                };
                block.instrs.pop();
                block.terminator = tc;
                continue;
            }
            // Pattern 2: Call; Jump(return_block, [v])
            if let IrTerminator::Jump(target, jump_args, _) = &block.terminator
                && jump_args.len() == 1
                && resolves_to_single_param_return_block(&block_map, *target)
                && let Some(IrInstr::Call {
                    dest,
                    target,
                    args,
                    metadata,
                }) = block.instrs.last()
                && *dest == jump_args[0]
            {
                let tc = IrTerminator::TailCall {
                    callee: target.clone(),
                    args: args.clone(),
                    metadata: metadata.clone(),
                };
                block.instrs.pop();
                block.terminator = tc;
            }
        }
    }
}

fn resolves_to_single_param_return_block(
    block_map: &HashMap<BlockId, TailReturnSummary>,
    mut block_id: BlockId,
) -> bool {
    let mut seen = HashSet::new();
    while seen.insert(block_id) {
        let Some(block) = block_map.get(&block_id) else {
            return false;
        };
        if block.params_len == 1
            && block.is_empty
            && matches!((&block.terminator, block.first_param), (IrTerminator::Return(v, _), Some(param)) if *v == param)
        {
            return true;
        }
        if block.params_len == 1
            && block.is_empty
            && let IrTerminator::Jump(next, jump_args, _) = &block.terminator
            && jump_args.len() == 1
            && Some(jump_args[0]) == block.first_param
        {
            block_id = *next;
            continue;
        }
        return false;
    }
    false
}

#[derive(Clone)]
struct TailReturnSummary {
    first_param: Option<IrVar>,
    params_len: usize,
    is_empty: bool,
    terminator: IrTerminator,
}

fn constant_fold(program: &mut IrProgram) {
    for function in &mut program.functions {
        let mut consts: HashMap<IrVar, IrConst> = HashMap::new();
        for block in &mut function.blocks {
            for instr in &mut block.instrs {
                if let IrInstr::Assign { dest, expr, .. } = instr {
                    if let Some(value) = fold_expr(expr, &consts) {
                        *expr = IrExpr::Const(value.clone());
                        consts.insert(*dest, value);
                    } else if let IrExpr::Const(value) = expr {
                        consts.insert(*dest, value.clone());
                    } else {
                        consts.remove(dest);
                    }
                }
            }
            if let IrTerminator::Branch {
                cond,
                then_block,
                else_block,
                metadata,
            } = &block.terminator
                && let Some(IrConst::Bool(value)) = consts.get(cond)
            {
                block.terminator = IrTerminator::Jump(
                    if *value { *then_block } else { *else_block },
                    Vec::new(),
                    metadata.clone(),
                );
            }
        }
    }
}

fn fold_expr(expr: &IrExpr, consts: &HashMap<IrVar, IrConst>) -> Option<IrConst> {
    match expr {
        IrExpr::Binary(op, lhs, rhs) => match (consts.get(lhs), consts.get(rhs)) {
            (Some(IrConst::Int(lhs)), Some(IrConst::Int(rhs))) => match op {
                IrBinaryOp::Add | IrBinaryOp::IAdd => Some(IrConst::Int(lhs.wrapping_add(*rhs))),
                IrBinaryOp::Sub | IrBinaryOp::ISub => Some(IrConst::Int(lhs.wrapping_sub(*rhs))),
                IrBinaryOp::Mul | IrBinaryOp::IMul => Some(IrConst::Int(lhs.wrapping_mul(*rhs))),
                IrBinaryOp::Div | IrBinaryOp::IDiv if *rhs != 0 => Some(IrConst::Int(lhs / rhs)),
                IrBinaryOp::Mod | IrBinaryOp::IMod if *rhs != 0 => Some(IrConst::Int(lhs % rhs)),
                IrBinaryOp::Eq => Some(IrConst::Bool(lhs == rhs)),
                IrBinaryOp::NotEq => Some(IrConst::Bool(lhs != rhs)),
                IrBinaryOp::Lt => Some(IrConst::Bool(lhs < rhs)),
                IrBinaryOp::Le => Some(IrConst::Bool(lhs <= rhs)),
                IrBinaryOp::Gt => Some(IrConst::Bool(lhs > rhs)),
                IrBinaryOp::Ge => Some(IrConst::Bool(lhs >= rhs)),
                _ => None,
            },
            (Some(IrConst::Float(lhs)), Some(IrConst::Float(rhs))) => match op {
                IrBinaryOp::Add | IrBinaryOp::FAdd => Some(IrConst::Float(lhs + rhs)),
                IrBinaryOp::Sub | IrBinaryOp::FSub => Some(IrConst::Float(lhs - rhs)),
                IrBinaryOp::Mul | IrBinaryOp::FMul => Some(IrConst::Float(lhs * rhs)),
                IrBinaryOp::Div | IrBinaryOp::FDiv => Some(IrConst::Float(lhs / rhs)),
                _ => None,
            },
            (Some(IrConst::Bool(lhs)), Some(IrConst::Bool(rhs))) => match op {
                IrBinaryOp::And => Some(IrConst::Bool(*lhs && *rhs)),
                IrBinaryOp::Or => Some(IrConst::Bool(*lhs || *rhs)),
                IrBinaryOp::Eq => Some(IrConst::Bool(lhs == rhs)),
                IrBinaryOp::NotEq => Some(IrConst::Bool(lhs != rhs)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn dead_block_elimination(program: &mut IrProgram) {
    for function in &mut program.functions {
        let mut reachable = HashSet::new();
        mark_reachable(function.entry, function, &mut reachable);
        function
            .blocks
            .retain(|block| reachable.contains(&block.id));
    }
}

fn mark_reachable(block_id: BlockId, function: &IrFunction, reachable: &mut HashSet<BlockId>) {
    if !reachable.insert(block_id) {
        return;
    }
    let Some(block) = function.blocks.iter().find(|block| block.id == block_id) else {
        return;
    };
    match &block.terminator {
        IrTerminator::Jump(target, _, _) => mark_reachable(*target, function, reachable),
        IrTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            mark_reachable(*then_block, function, reachable);
            mark_reachable(*else_block, function, reachable);
        }
        IrTerminator::Return(_, _)
        | IrTerminator::TailCall { .. }
        | IrTerminator::Unreachable(_) => {}
    }
}

fn local_cse(program: &mut IrProgram) {
    for function in &mut program.functions {
        for block in &mut function.blocks {
            let mut seen: HashMap<String, IrVar> = HashMap::new();
            for instr in &mut block.instrs {
                if let IrInstr::Assign {
                    dest,
                    expr,
                    metadata,
                } = instr
                {
                    let key = cse_key(expr);
                    if let Some(existing) = key.as_ref().and_then(|key| seen.get(key)).copied() {
                        *expr = IrExpr::Var(existing);
                    } else if let Some(key) = key {
                        seen.insert(key, *dest);
                    }
                    if matches!(expr, IrExpr::Var(_)) {
                        *metadata = IrMetadata::empty();
                    }
                }
            }
        }
    }
}

fn cse_key(expr: &IrExpr) -> Option<String> {
    match expr {
        IrExpr::Const(value) => Some(format!("const:{:?}", value)),
        IrExpr::Binary(op, lhs, rhs) => Some(format!("bin:{:?}:{:?}:{:?}", op, lhs, rhs)),
        IrExpr::TupleArityTest { value, arity } => {
            Some(format!("tuple_arity:{:?}:{}", value, arity))
        }
        IrExpr::TagTest { value, tag } => Some(format!("tag:{:?}:{:?}", value, tag)),
        IrExpr::TagPayload { value, tag } => Some(format!("payload:{:?}:{:?}", value, tag)),
        IrExpr::ListTest { value, tag } => Some(format!("list:{:?}:{:?}", value, tag)),
        IrExpr::ListHead { value } => Some(format!("list_head:{:?}", value)),
        IrExpr::ListTail { value } => Some(format!("list_tail:{:?}", value)),
        IrExpr::AdtTagTest { value, constructor } => {
            Some(format!("adt_tag:{:?}:{:?}", value, constructor))
        }
        IrExpr::AdtField { value, index } => Some(format!("adt_field:{:?}:{}", value, index)),
        _ => None,
    }
}

fn intern_unit_adts(program: &mut IrProgram) {
    for function in &mut program.functions {
        for block in &mut function.blocks {
            for instr in &mut block.instrs {
                if let IrInstr::Assign { expr, .. } = instr
                    && let IrExpr::MakeAdt(name, fields) = expr
                    && fields.is_empty()
                {
                    *expr = IrExpr::LoadName(*name);
                }
            }
        }
    }
}

fn type_directed_unboxing(program: &mut IrProgram) {
    for function in &mut program.functions {
        for block in &mut function.blocks {
            for instr in &mut block.instrs {
                if let IrInstr::Assign { expr, metadata, .. } = instr
                    && let IrExpr::Binary(op, ..) = expr
                    && let Some(inferred) = metadata.inferred_type.as_ref()
                {
                    *op = rewrite_binary_op(*op, inferred);
                }
            }
        }
    }
}

fn rewrite_binary_op(op: IrBinaryOp, inferred: &InferType) -> IrBinaryOp {
    match inferred {
        InferType::Con(TypeConstructor::Int) => match op {
            IrBinaryOp::Add => IrBinaryOp::IAdd,
            IrBinaryOp::Sub => IrBinaryOp::ISub,
            IrBinaryOp::Mul => IrBinaryOp::IMul,
            IrBinaryOp::Div => IrBinaryOp::IDiv,
            IrBinaryOp::Mod => IrBinaryOp::IMod,
            _ => op,
        },
        InferType::Con(TypeConstructor::Float) => match op {
            IrBinaryOp::Add => IrBinaryOp::FAdd,
            IrBinaryOp::Sub => IrBinaryOp::FSub,
            IrBinaryOp::Mul => IrBinaryOp::FMul,
            IrBinaryOp::Div => IrBinaryOp::FDiv,
            _ => op,
        },
        _ => op,
    }
}

#[cfg(test)]
mod tests {
    use crate::cfg::{
        FunctionId, IrBlock, IrBlockParam, IrCallTarget, IrFunction, IrFunctionOrigin, IrMetadata,
        IrParam, IrProgram, IrTerminator, IrType,
    };
    use crate::syntax::interner::Interner;

    use super::*;

    fn single_block_program(instrs: Vec<IrInstr>) -> IrProgram {
        let mut interner = Interner::new();
        IrProgram {
            top_level_items: Vec::new(),
            functions: vec![IrFunction {
                id: FunctionId(0),
                name: None,
                params: vec![IrParam {
                    name: interner.intern("param"),
                    var: IrVar(99),
                    ty: IrType::Any,
                }],
                parameter_types: vec![None],
                return_type_annotation: None,
                effects: Vec::new(),
                captures: Vec::new(),
                body_span: crate::diagnostics::position::Span::default(),
                ret_type: IrType::Any,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![IrBlockParam {
                        var: IrVar(2),
                        ty: IrType::Int,
                    }],
                    instrs,
                    terminator: IrTerminator::Return(IrVar(2), IrMetadata::empty()),
                }],
                entry: BlockId(0),
                origin: IrFunctionOrigin::ModuleTopLevel,
                metadata: IrMetadata::empty(),
            }],
            entry: FunctionId(0),
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        }
    }

    #[test]
    fn folds_integer_addition_constants() {
        let mut program = single_block_program(vec![
            IrInstr::Assign {
                dest: IrVar(0),
                expr: IrExpr::Const(IrConst::Int(2)),
                metadata: IrMetadata::empty(),
            },
            IrInstr::Assign {
                dest: IrVar(1),
                expr: IrExpr::Const(IrConst::Int(3)),
                metadata: IrMetadata::empty(),
            },
            IrInstr::Assign {
                dest: IrVar(2),
                expr: IrExpr::Binary(IrBinaryOp::Add, IrVar(0), IrVar(1)),
                metadata: IrMetadata::empty(),
            },
        ]);

        run_ir_pass_pipeline(&mut program, &IrPassContext).unwrap();
        let instrs = &program.functions[0].blocks[0].instrs;
        assert!(matches!(
            instrs[2],
            IrInstr::Assign {
                expr: IrExpr::Const(IrConst::Int(5)),
                ..
            }
        ));
    }

    #[test]
    fn interns_nullary_adt_constructors() {
        let mut interner = Interner::new();
        let mut program = single_block_program(vec![IrInstr::Assign {
            dest: IrVar(2),
            expr: IrExpr::MakeAdt(interner.intern("None_"), Vec::new()),
            metadata: IrMetadata::empty(),
        }]);

        run_ir_pass_pipeline(&mut program, &IrPassContext).unwrap();
        assert!(matches!(
            program.functions[0].blocks[0].instrs[0],
            IrInstr::Assign {
                expr: IrExpr::LoadName(_),
                ..
            }
        ));
    }

    #[test]
    fn introduces_tailcall_through_trivial_return_forwarder_chain() {
        let mut interner = Interner::new();
        let call_metadata = IrMetadata::empty();
        let function = IrFunction {
            id: FunctionId(0),
            name: Some(interner.intern("loop_")),
            params: vec![IrParam {
                name: interner.intern("n"),
                var: IrVar(0),
                ty: IrType::Int,
            }],
            parameter_types: vec![None],
            return_type_annotation: None,
            effects: Vec::new(),
            captures: Vec::new(),
            body_span: crate::diagnostics::position::Span::default(),
            ret_type: IrType::Any,
            blocks: vec![
                IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    instrs: vec![IrInstr::Call {
                        dest: IrVar(1),
                        target: IrCallTarget::Direct(FunctionId(0)),
                        args: vec![IrVar(0)],
                        metadata: call_metadata.clone(),
                    }],
                    terminator: IrTerminator::Jump(BlockId(1), vec![IrVar(1)], IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(1),
                    params: vec![IrBlockParam {
                        var: IrVar(2),
                        ty: IrType::Any,
                    }],
                    instrs: vec![],
                    terminator: IrTerminator::Jump(BlockId(2), vec![IrVar(2)], IrMetadata::empty()),
                },
                IrBlock {
                    id: BlockId(2),
                    params: vec![IrBlockParam {
                        var: IrVar(3),
                        ty: IrType::Any,
                    }],
                    instrs: vec![],
                    terminator: IrTerminator::Return(IrVar(3), IrMetadata::empty()),
                },
            ],
            entry: BlockId(0),
            origin: IrFunctionOrigin::ModuleTopLevel,
            metadata: IrMetadata::empty(),
        };
        let mut program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![function],
            entry: FunctionId(0),
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        run_ir_pass_pipeline(&mut program, &IrPassContext).unwrap();

        assert!(matches!(
            program.functions[0].blocks[0].terminator,
            IrTerminator::TailCall { .. }
        ));
        assert!(program.functions[0].blocks[0].instrs.is_empty());
    }
}
