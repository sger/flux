use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::position::Span,
    diagnostics::{Diagnostic, DiagnosticBuilder, ErrorType},
};

use super::{
    BlockId, FunctionId, IrCallTarget, IrExpr, IrHandleArm, IrInstr, IrProgram, IrStringPart,
    IrTerminator, IrVar,
};

#[allow(clippy::result_large_err)]
pub fn validate_ir(program: &IrProgram) -> Result<(), Diagnostic> {
    let function_ids: HashSet<FunctionId> = program
        .functions
        .iter()
        .map(|function| function.id)
        .collect();
    for function in &program.functions {
        let block_ids: HashSet<_> = function.blocks.iter().map(|block| block.id).collect();
        if !block_ids.contains(&function.entry) {
            return Err(invalid_ir(None, "function entry block does not exist"));
        }
        let reachable_defs = compute_reachable_defs(function, &block_ids)?;
        for block in &function.blocks {
            let mut defined = reachable_defs.get(&block.id).cloned().unwrap_or_default();
            defined.extend(block.params.iter().map(|param| param.var));
            for param in &function.params {
                defined.insert(param.var);
            }
            for instr in &block.instrs {
                match instr {
                    IrInstr::Assign { dest, expr, .. } => {
                        ensure_expr_vars_defined(expr, &defined)?;
                        defined.insert(*dest);
                    }
                    IrInstr::Call { dest, args, .. } => {
                        for arg in args {
                            ensure_defined(*arg, &defined)?;
                        }
                        defined.insert(*dest);
                    }
                    IrInstr::HandleScope {
                        dest,
                        body_entry,
                        arms,
                        ..
                    } => {
                        ensure_block_exists(*body_entry, &block_ids)?;
                        for arm in arms {
                            ensure_function_exists(arm.function_id, &function_ids)?;
                            for capture in &arm.capture_vars {
                                ensure_defined(*capture, &defined)?;
                            }
                        }
                        defined.insert(*dest);
                    }
                    IrInstr::AetherDrop { var, .. } => {
                        ensure_defined(*var, &defined)?;
                    }
                }
            }
            validate_terminator(&block.terminator, &defined, &block_ids, &function_ids)?;
        }

        let mut incoming: HashMap<BlockId, Option<usize>> = HashMap::new();
        for block in &function.blocks {
            match &block.terminator {
                IrTerminator::Jump(target, args, _) => {
                    check_block_params(function, *target, args.len())?;
                    incoming.insert(*target, Some(args.len()));
                }
                IrTerminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    incoming.entry(*then_block).or_insert(Some(0));
                    incoming.entry(*else_block).or_insert(Some(0));
                }
                IrTerminator::Return(_, _)
                | IrTerminator::TailCall { .. }
                | IrTerminator::Unreachable(_) => {}
            }
        }
        for block in &function.blocks {
            if let Some(Some(arg_count)) = incoming.get(&block.id)
                && *arg_count != block.params.len()
            {
                return Err(invalid_ir(
                    None,
                    "block parameter arity does not match predecessor jump arguments",
                ));
            }
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn compute_reachable_defs(
    function: &super::IrFunction,
    block_ids: &HashSet<BlockId>,
) -> Result<HashMap<BlockId, HashSet<IrVar>>, Diagnostic> {
    let mut incoming: HashMap<BlockId, HashSet<IrVar>> = HashMap::new();
    for block in &function.blocks {
        incoming.entry(block.id).or_default();
    }
    if let Some(entry) = incoming.get_mut(&function.entry) {
        for param in &function.params {
            entry.insert(param.var);
        }
        if let Some(block) = function
            .blocks
            .iter()
            .find(|block| block.id == function.entry)
        {
            for param in &block.params {
                entry.insert(param.var);
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            let mut defined = incoming.get(&block.id).cloned().unwrap_or_default();
            defined.extend(block.params.iter().map(|param| param.var));
            for param in &function.params {
                defined.insert(param.var);
            }
            for instr in &block.instrs {
                match instr {
                    IrInstr::Assign { dest, .. }
                    | IrInstr::Call { dest, .. }
                    | IrInstr::HandleScope { dest, .. } => {
                        defined.insert(*dest);
                    }
                    IrInstr::AetherDrop { .. } => {
                        // Drop does not define a new variable.
                    }
                }
            }

            match &block.terminator {
                IrTerminator::Jump(target, args, _) => {
                    ensure_block_exists(*target, block_ids)?;
                    let target_defs = incoming.entry(*target).or_default();
                    let before = target_defs.len();
                    target_defs.extend(defined.iter().copied());
                    target_defs.extend(args.iter().copied());
                    if let Some(target_block) = function.blocks.iter().find(|b| b.id == *target) {
                        target_defs.extend(target_block.params.iter().map(|param| param.var));
                    }
                    if target_defs.len() != before {
                        changed = true;
                    }
                }
                IrTerminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    ensure_block_exists(*then_block, block_ids)?;
                    ensure_block_exists(*else_block, block_ids)?;
                    for target in [then_block, else_block] {
                        let target_defs = incoming.entry(*target).or_default();
                        let before = target_defs.len();
                        target_defs.extend(defined.iter().copied());
                        if target_defs.len() != before {
                            changed = true;
                        }
                    }
                }
                IrTerminator::Return(_, _)
                | IrTerminator::TailCall { .. }
                | IrTerminator::Unreachable(_) => {}
            }
        }
    }

    Ok(incoming)
}

#[allow(clippy::result_large_err)]
fn ensure_expr_vars_defined(expr: &IrExpr, defined: &HashSet<IrVar>) -> Result<(), Diagnostic> {
    match expr {
        IrExpr::Var(var) => ensure_defined(*var, defined),
        IrExpr::InterpolatedString(parts) => {
            for part in parts {
                if let IrStringPart::Interpolation(var) = part {
                    ensure_defined(*var, defined)?;
                }
            }
            Ok(())
        }
        IrExpr::Prefix { right, .. } => ensure_defined(*right, defined),
        IrExpr::Binary(_, lhs, rhs) => {
            ensure_defined(*lhs, defined)?;
            ensure_defined(*rhs, defined)
        }
        IrExpr::MakeTuple(vars) | IrExpr::MakeArray(vars) | IrExpr::MakeList(vars) => {
            for var in vars {
                ensure_defined(*var, defined)?;
            }
            Ok(())
        }
        IrExpr::MakeHash(pairs) => {
            for (key, value) in pairs {
                ensure_defined(*key, defined)?;
                ensure_defined(*value, defined)?;
            }
            Ok(())
        }
        IrExpr::MakeAdt(_, fields) | IrExpr::MakeClosure(_, fields) => {
            for field in fields {
                ensure_defined(*field, defined)?;
            }
            Ok(())
        }
        IrExpr::Index { left, index } => {
            ensure_defined(*left, defined)?;
            ensure_defined(*index, defined)
        }
        IrExpr::TupleArityTest { value, .. }
        | IrExpr::TagTest { value, .. }
        | IrExpr::TagPayload { value, .. }
        | IrExpr::ListTest { value, .. }
        | IrExpr::ListHead { value }
        | IrExpr::ListTail { value }
        | IrExpr::AdtTagTest { value, .. }
        | IrExpr::AdtField { value, .. } => ensure_defined(*value, defined),
        IrExpr::MemberAccess { object, .. } | IrExpr::TupleFieldAccess { object, .. } => {
            ensure_defined(*object, defined)
        }
        IrExpr::Some(var) | IrExpr::Left(var) | IrExpr::Right(var) => ensure_defined(*var, defined),
        IrExpr::Cons { head, tail } => {
            ensure_defined(*head, defined)?;
            ensure_defined(*tail, defined)
        }
        IrExpr::DropReuse(var) => ensure_defined(*var, defined),
        IrExpr::ReuseCons { token, head, tail } => {
            ensure_defined(*token, defined)?;
            ensure_defined(*head, defined)?;
            ensure_defined(*tail, defined)
        }
        IrExpr::ReuseSome { token, inner }
        | IrExpr::ReuseLeft { token, inner }
        | IrExpr::ReuseRight { token, inner } => {
            ensure_defined(*token, defined)?;
            ensure_defined(*inner, defined)
        }
        IrExpr::ReuseAdt { token, fields, .. } => {
            ensure_defined(*token, defined)?;
            for field in fields {
                ensure_defined(*field, defined)?;
            }
            Ok(())
        }
        IrExpr::Perform { args, .. } => {
            for arg in args {
                ensure_defined(*arg, defined)?;
            }
            Ok(())
        }
        IrExpr::Handle { expr, arms, .. } => {
            ensure_defined(*expr, defined)?;
            for arm in arms {
                ensure_handle_arm_defined(arm, defined)?;
            }
            Ok(())
        }
        IrExpr::Const(_) | IrExpr::LoadName(_) | IrExpr::EmptyList | IrExpr::None => Ok(()),
    }
}

#[allow(clippy::result_large_err)]
fn ensure_handle_arm_defined(
    _arm: &IrHandleArm,
    _defined: &HashSet<IrVar>,
) -> Result<(), Diagnostic> {
    // IrHandleArm bodies are AST `Expression` nodes and reference Identifiers,
    // not IrVars, so there is nothing to validate against the flat-IR `defined`
    // set.  When Handle is lowered to flat CFG (requires evidence-passing
    // infrastructure), explicit capture IrVars will be added to IrHandleArm
    // and checked here.
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_terminator(
    terminator: &IrTerminator,
    defined: &HashSet<IrVar>,
    block_ids: &HashSet<BlockId>,
    function_ids: &HashSet<FunctionId>,
) -> Result<(), Diagnostic> {
    match terminator {
        IrTerminator::Jump(target, args, _) => {
            ensure_block_exists(*target, block_ids)?;
            for arg in args {
                ensure_defined(*arg, defined)?;
            }
        }
        IrTerminator::Branch {
            cond,
            then_block,
            else_block,
            ..
        } => {
            ensure_defined(*cond, defined)?;
            ensure_block_exists(*then_block, block_ids)?;
            ensure_block_exists(*else_block, block_ids)?;
        }
        IrTerminator::Return(var, _) => ensure_defined(*var, defined)?,
        IrTerminator::TailCall { callee, args, .. } => {
            validate_call_target(callee, function_ids)?;
            for arg in args {
                ensure_defined(*arg, defined)?;
            }
        }
        IrTerminator::Unreachable(_) => {}
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_call_target(
    target: &IrCallTarget,
    function_ids: &HashSet<FunctionId>,
) -> Result<(), Diagnostic> {
    if let IrCallTarget::Direct(function_id) = target
        && !function_ids.contains(function_id)
    {
        return Err(invalid_ir(None, "tail call targets unknown function id"));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn check_block_params(
    function: &super::IrFunction,
    block_id: BlockId,
    arg_count: usize,
) -> Result<(), Diagnostic> {
    let Some(block) = function.blocks.iter().find(|block| block.id == block_id) else {
        return Err(invalid_ir(None, "jump target block does not exist"));
    };
    if block.params.len() != arg_count {
        return Err(invalid_ir(
            None,
            "jump argument count does not match target block params",
        ));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn ensure_defined(var: IrVar, defined: &HashSet<IrVar>) -> Result<(), Diagnostic> {
    if defined.contains(&var) {
        Ok(())
    } else {
        Err(invalid_ir(None, "IR uses a variable before it is defined"))
    }
}

#[allow(clippy::result_large_err)]
fn ensure_block_exists(block_id: BlockId, block_ids: &HashSet<BlockId>) -> Result<(), Diagnostic> {
    if block_ids.contains(&block_id) {
        Ok(())
    } else {
        Err(invalid_ir(
            None,
            "IR references a block that does not exist",
        ))
    }
}

#[allow(clippy::result_large_err)]
fn ensure_function_exists(
    function_id: FunctionId,
    function_ids: &HashSet<FunctionId>,
) -> Result<(), Diagnostic> {
    if function_ids.contains(&function_id) {
        Ok(())
    } else {
        Err(invalid_ir(
            None,
            "IR references a function that does not exist",
        ))
    }
}

fn invalid_ir(span: Option<Span>, message: &str) -> Diagnostic {
    let diagnostic = Diagnostic::warning("Invalid Flux IR")
        .with_error_type(ErrorType::Compiler)
        .with_message(message);
    if let Some(span) = span {
        diagnostic.with_span(span)
    } else {
        diagnostic
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::cfg::{
        BlockId, FunctionId, IrBlock, IrFunction, IrFunctionOrigin, IrMetadata, IrProgram,
        IrTerminator, IrType, IrVar,
    };

    use super::validate_ir;

    #[test]
    fn rejects_undefined_value_use() {
        let program = IrProgram {
            top_level_items: Vec::new(),
            functions: vec![IrFunction {
                id: FunctionId(0),
                name: None,
                params: Vec::new(),
                parameter_types: Vec::new(),
                return_type_annotation: None,
                effects: Vec::new(),
                captures: Vec::new(),
                body_span: crate::diagnostics::position::Span::default(),
                ret_type: IrType::Any,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Return(IrVar(42), IrMetadata::empty()),
                }],
                entry: BlockId(0),
                origin: IrFunctionOrigin::ModuleTopLevel,
                metadata: IrMetadata::empty(),
                inferred_param_types: Vec::new(),
                inferred_return_type: None,
            }],
            entry: FunctionId(0),
            globals: Vec::new(),
            global_bindings: Vec::new(),
            hm_expr_types: HashMap::new(),
            core: None,
        };

        assert!(validate_ir(&program).is_err());
    }
}
