use std::collections::HashMap;

use crate::{
    ast::free_vars::collect_free_vars,
    diagnostics::position::Span,
    diagnostics::{Diagnostic, DiagnosticBuilder, DiagnosticPhase, ErrorType},
    syntax::{
        Identifier,
        block::Block,
        expression::{ExprId, Expression, MatchArm, Pattern, StringPart},
        program::Program,
        statement::{FunctionTypeParam, Statement},
    },
    types::{infer_type::InferType, type_constructor::TypeConstructor},
};

use super::{
    BlockId, FunctionId, IrBinaryOp, IrBlock, IrBlockParam, IrCallTarget, IrConst, IrExpr,
    IrFunction, IrFunctionOrigin, IrHandleArm, IrInstr, IrListTest, IrMetadata, IrParam, IrProgram,
    IrStringPart, IrTagTest, IrTerminator, IrTopLevelItem, IrType, IrVar,
};

#[allow(clippy::result_large_err)]
pub fn lower_program_to_ir(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
) -> Result<IrProgram, Diagnostic> {
    let mut lowerer = Lowerer::new(program.clone(), hm_expr_types.clone())?;
    lowerer.lower_program()?;
    Ok(lowerer.finish())
}

struct Lowerer {
    top_level_items: Vec<IrTopLevelItem>,
    top_level_statements: Vec<Statement>,
    hm_expr_types: HashMap<ExprId, InferType>,
    functions: Vec<IrFunction>,
    globals: Vec<Identifier>,
    next_function_id: u32,
    next_block_id: u32,
    next_var_id: u32,
}

#[allow(clippy::result_large_err)]
impl Lowerer {
    fn new(
        source_program: Program,
        hm_expr_types: HashMap<ExprId, InferType>,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            top_level_items: source_program
                .statements
                .iter()
                .map(lower_top_level_item)
                .collect::<Result<Vec<_>, _>>()?,
            top_level_statements: source_program.statements,
            hm_expr_types,
            functions: Vec::new(),
            globals: Vec::new(),
            next_function_id: 0,
            next_block_id: 0,
            next_var_id: 0,
        })
    }

    fn finish(self) -> IrProgram {
        IrProgram {
            top_level_items: self.top_level_items,
            functions: self.functions,
            entry: FunctionId(0),
            globals: self.globals,
            global_bindings: Vec::new(),
            hm_expr_types: self.hm_expr_types,
            core: None, // populated by callers via lower_program_ast + lower_core_to_ir
        }
    }

    fn lower_program(&mut self) -> Result<(), Diagnostic> {
        let entry_id = self.next_function();
        let top_level_statements = self.top_level_statements.clone();
        let mut context =
            FunctionLoweringContext::new(self, entry_id, None, IrFunctionOrigin::ModuleTopLevel);
        for stmt in &top_level_statements {
            if let Statement::Function { .. } = stmt {
                continue;
            }
            context.lower_statement(stmt, false)?;
        }
        let ret = context.ensure_return_var();
        context.finish(IrType::Tagged, ret);

        let top_level_statements = self.top_level_statements.clone();
        self.lower_functions_in_statements(&top_level_statements)?;
        Ok(())
    }

    fn lower_functions_in_statements(
        &mut self,
        statements: &[Statement],
    ) -> Result<(), Diagnostic> {
        for stmt in statements {
            match stmt {
                Statement::Function {
                    name,
                    parameter_types,
                    return_type,
                    effects,
                    parameters,
                    body,
                    span,
                    ..
                } => {
                    let function_id = self.next_function();
                    let mut function_context = FunctionLoweringContext::new(
                        self,
                        function_id,
                        Some(*name),
                        IrFunctionOrigin::NamedFunction,
                    );
                    for param in parameters {
                        let var = function_context.next_var();
                        function_context.env.insert(*param, var);
                        function_context.params.push(IrParam {
                            name: *param,
                            var,
                            ty: IrType::Tagged,
                        });
                    }
                    function_context.lower_block(body)?;
                    let ret = function_context.ensure_return_var();
                    function_context.finish_with_metadata(
                        IrType::Tagged,
                        ret,
                        IrMetadata {
                            span: Some(*span),
                            inferred_type: None,
                            expr_id: None,
                        },
                        parameter_types.clone(),
                        return_type.clone(),
                        effects.clone(),
                        Vec::new(),
                        body.span,
                    );
                    self.bind_function_id(*name, function_id);
                }
                Statement::Module { body, .. } => {
                    // Recurse into module bodies to lower nested functions.
                    self.lower_functions_in_statements(&body.statements)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn bind_function_id(&mut self, name: Identifier, function_id: FunctionId) {
        Self::bind_function_id_in_items(&mut self.top_level_items, name, function_id);
    }

    fn bind_function_id_in_items(
        items: &mut [IrTopLevelItem],
        name: Identifier,
        function_id: FunctionId,
    ) -> bool {
        for item in items {
            match item {
                IrTopLevelItem::Function {
                    name: item_name,
                    function_id: item_function_id,
                    ..
                } if *item_name == name && item_function_id.is_none() => {
                    *item_function_id = Some(function_id);
                    return true;
                }
                IrTopLevelItem::Module { body, .. } => {
                    if Self::bind_function_id_in_items(body, name, function_id) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn next_function(&mut self) -> FunctionId {
        let id = FunctionId(self.next_function_id);
        self.next_function_id += 1;
        id
    }

    fn next_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    fn next_var(&mut self) -> IrVar {
        let id = IrVar(self.next_var_id);
        self.next_var_id += 1;
        id
    }
}

struct FunctionLoweringContext<'a> {
    lowerer: &'a mut Lowerer,
    id: FunctionId,
    name: Option<Identifier>,
    origin: IrFunctionOrigin,
    params: Vec<IrParam>,
    blocks: Vec<IrBlock>,
    current_block: usize,
    env: HashMap<Identifier, IrVar>,
    last_value: Option<IrVar>,
}

#[allow(clippy::result_large_err)]
impl<'a> FunctionLoweringContext<'a> {
    fn new(
        lowerer: &'a mut Lowerer,
        id: FunctionId,
        name: Option<Identifier>,
        origin: IrFunctionOrigin,
    ) -> Self {
        let entry = lowerer.next_block();
        Self {
            lowerer,
            id,
            name,
            origin,
            params: Vec::new(),
            blocks: vec![IrBlock {
                id: entry,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            }],
            current_block: 0,
            env: HashMap::new(),
            last_value: None,
        }
    }

    fn finish(self, ret_type: IrType, ret: IrVar) {
        self.finish_with_metadata(
            ret_type,
            ret,
            IrMetadata::empty(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            Span::default(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_with_metadata(
        mut self,
        ret_type: IrType,
        ret: IrVar,
        metadata: IrMetadata,
        parameter_types: Vec<Option<crate::syntax::type_expr::TypeExpr>>,
        return_type_annotation: Option<crate::syntax::type_expr::TypeExpr>,
        effects: Vec<crate::syntax::effect_expr::EffectExpr>,
        captures: Vec<Identifier>,
        body_span: Span,
    ) {
        if matches!(
            self.blocks[self.current_block].terminator,
            IrTerminator::Unreachable(_)
        ) {
            self.blocks[self.current_block].terminator =
                IrTerminator::Return(ret, metadata.clone());
        }
        let entry = self.blocks[0].id;
        self.lowerer.functions.push(IrFunction {
            id: self.id,
            name: self.name,
            params: self.params,
            parameter_types,
            return_type_annotation,
            effects,
            captures,
            body_span,
            ret_type,
            blocks: self.blocks,
            entry,
            origin: self.origin,
            metadata,
            inferred_param_types: Vec::new(),
            inferred_return_type: None,
        });
    }

    fn current_block_mut(&mut self) -> &mut IrBlock {
        &mut self.blocks[self.current_block]
    }

    fn next_var(&mut self) -> IrVar {
        self.lowerer.next_var()
    }

    fn ensure_return_var(&mut self) -> IrVar {
        match self.last_value {
            Some(var) => var,
            None => {
                let var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: var,
                    expr: IrExpr::Const(IrConst::Unit),
                    metadata: IrMetadata::empty(),
                });
                self.last_value = Some(var);
                var
            }
        }
    }

    fn lower_block(&mut self, block: &Block) -> Result<IrVar, Diagnostic> {
        let mut last = None;
        let len = block.statements.len();
        for (i, stmt) in block.statements.iter().enumerate() {
            let is_last = i == len.saturating_sub(1);
            let tail_eligible = matches!(
                stmt,
                Statement::Expression {
                    has_semicolon: false,
                    ..
                } | Statement::Return { .. }
            );
            last = self.lower_statement(stmt, is_last && tail_eligible)?;
        }
        Ok(last.unwrap_or_else(|| self.ensure_return_var()))
    }

    fn lower_statement(
        &mut self,
        stmt: &Statement,
        in_tail_position: bool,
    ) -> Result<Option<IrVar>, Diagnostic> {
        match stmt {
            Statement::Let { name, value, .. } => {
                let var = self.lower_expression(value)?;
                self.env.insert(*name, var);
                self.lowerer.globals.push(*name);
                self.last_value = None;
                Ok(None)
            }
            Statement::Assign { name, value, .. } => {
                let var = self.lower_expression(value)?;
                self.env.insert(*name, var);
                self.last_value = None;
                Ok(None)
            }
            Statement::Expression { expression, .. } => {
                if in_tail_position && self.try_lower_tail_call(expression)? {
                    self.last_value = None;
                    return Ok(None);
                }
                let var = self.lower_expression(expression)?;
                self.last_value = Some(var);
                Ok(Some(var))
            }
            Statement::Return { value, span } => {
                if in_tail_position
                    && let Some(expr) = value
                    && self.try_lower_tail_call(expr)?
                {
                    self.last_value = None;
                    return Ok(None);
                }
                let ret = match value {
                    Some(expr) => self.lower_expression(expr)?,
                    None => self.ensure_return_var(),
                };
                self.current_block_mut().terminator = IrTerminator::Return(
                    ret,
                    IrMetadata {
                        span: Some(*span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
                Ok(Some(ret))
            }
            Statement::Function { .. }
            | Statement::Import { .. }
            | Statement::Module { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::LetDestructure { .. }
            | Statement::Class { .. }
            | Statement::Instance { .. } => Ok(None),
        }
    }

    fn try_lower_tail_call(&mut self, expr: &Expression) -> Result<bool, Diagnostic> {
        let Expression::Call {
            function,
            arguments,
            ..
        } = expr
        else {
            return Ok(false);
        };

        let args = self.lower_expr_list(arguments)?;
        let target = match function.as_ref() {
            Expression::Identifier { name, .. } => IrCallTarget::Named(*name),
            _ => IrCallTarget::Var(self.lower_expression(function)?),
        };
        self.current_block_mut().terminator = IrTerminator::TailCall {
            callee: target,
            args,
            metadata: metadata_for(expr, &self.lowerer.hm_expr_types),
        };
        Ok(true)
    }

    fn lower_expression(&mut self, expr: &Expression) -> Result<IrVar, Diagnostic> {
        match expr {
            Expression::Integer { value, .. } => self.emit_const(
                IrConst::Int(*value),
                metadata_for(expr, &self.lowerer.hm_expr_types),
            ),
            Expression::Float { value, .. } => self.emit_const(
                IrConst::Float(*value),
                metadata_for(expr, &self.lowerer.hm_expr_types),
            ),
            Expression::Boolean { value, .. } => self.emit_const(
                IrConst::Bool(*value),
                metadata_for(expr, &self.lowerer.hm_expr_types),
            ),
            Expression::String { value, .. } => self.emit_const(
                IrConst::String(value.clone()),
                metadata_for(expr, &self.lowerer.hm_expr_types),
            ),
            Expression::InterpolatedString { parts, .. } => {
                let mut lowered_parts = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        StringPart::Literal(text) => {
                            lowered_parts.push(IrStringPart::Literal(text.clone()));
                        }
                        StringPart::Interpolation(expr) => {
                            lowered_parts
                                .push(IrStringPart::Interpolation(self.lower_expression(expr)?));
                        }
                    }
                }
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::InterpolatedString(lowered_parts),
                    metadata,
                });
                Ok(dest)
            }
            Expression::Identifier { name, .. } => {
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                let value_expr = match self.env.get(name).copied() {
                    Some(var) => IrExpr::Var(var),
                    None => IrExpr::LoadName(*name),
                };
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: value_expr,
                    metadata,
                });
                Ok(dest)
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => {
                let lhs = self.lower_expression(left)?;
                let rhs = self.lower_expression(right)?;
                let op = map_binary_op(operator).ok_or_else(|| {
                    unsupported_lowering(expr.span(), "unsupported infix operator")
                })?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Binary(op, lhs, rhs),
                    metadata,
                });
                Ok(dest)
            }
            Expression::Prefix {
                operator, right, ..
            } => {
                let right = self.lower_expression(right)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: operator.clone(),
                        right,
                    },
                    metadata,
                });
                Ok(dest)
            }
            Expression::TupleLiteral { elements, .. } => {
                let vars = self.lower_expr_list(elements)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeTuple(vars),
                    metadata,
                });
                Ok(dest)
            }
            Expression::ArrayLiteral { elements, .. } => {
                let vars = self.lower_expr_list(elements)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeArray(vars),
                    metadata,
                });
                Ok(dest)
            }
            Expression::ListLiteral { elements, .. } => {
                let vars = self.lower_expr_list(elements)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeList(vars),
                    metadata,
                });
                Ok(dest)
            }
            Expression::EmptyList { .. } => {
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::EmptyList,
                    metadata,
                });
                Ok(dest)
            }
            Expression::Index { left, index, .. } => {
                let left = self.lower_expression(left)?;
                let index = self.lower_expression(index)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Index { left, index },
                    metadata,
                });
                Ok(dest)
            }
            Expression::Hash { pairs, .. } => {
                let mut lowered_pairs = Vec::with_capacity(pairs.len());
                for (key, value) in pairs {
                    lowered_pairs
                        .push((self.lower_expression(key)?, self.lower_expression(value)?));
                }
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeHash(lowered_pairs),
                    metadata,
                });
                Ok(dest)
            }
            Expression::MemberAccess { object, member, .. } => {
                let module_name = match object.as_ref() {
                    Expression::Identifier { name, .. } => Some(*name),
                    _ => None,
                };
                let object = self.lower_expression(object)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MemberAccess {
                        object,
                        member: *member,
                        module_name,
                    },
                    metadata,
                });
                Ok(dest)
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                let object = self.lower_expression(object)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::TupleFieldAccess {
                        object,
                        index: *index,
                    },
                    metadata,
                });
                Ok(dest)
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let args = self.lower_expr_list(arguments)?;
                let target = match function.as_ref() {
                    Expression::Identifier { name, .. } => IrCallTarget::Named(*name),
                    _ => IrCallTarget::Var(self.lower_expression(function)?),
                };
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Call {
                    dest,
                    target,
                    args,
                    metadata,
                });
                Ok(dest)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => self.lower_if_expression(expr, condition, consequence, alternative.as_ref()),
            Expression::DoBlock { block, .. } => self.lower_block(block),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                if let Some(var) = self.try_lower_literal_match_expression(expr, scrutinee, arms)? {
                    return Ok(var);
                }
                if let Some(var) = self.try_lower_tag_match_expression(expr, scrutinee, arms)? {
                    return Ok(var);
                }
                if let Some(var) = self.try_lower_tuple_match_expression(expr, scrutinee, arms)? {
                    return Ok(var);
                }
                if let Some(var) = self.try_lower_list_match_expression(expr, scrutinee, arms)? {
                    return Ok(var);
                }
                if let Some(var) =
                    self.try_lower_constructor_match_expression(expr, scrutinee, arms)?
                {
                    return Ok(var);
                }
                self.lower_general_match(expr, scrutinee, arms)
            }
            Expression::None { .. } => {
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::None,
                    metadata,
                });
                Ok(dest)
            }
            Expression::Some { value, .. } => {
                let value = self.lower_expression(value)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Some(value),
                    metadata,
                });
                Ok(dest)
            }
            Expression::Left { value, .. } => {
                let value = self.lower_expression(value)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Left(value),
                    metadata,
                });
                Ok(dest)
            }
            Expression::Right { value, .. } => {
                let value = self.lower_expression(value)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Right(value),
                    metadata,
                });
                Ok(dest)
            }
            Expression::Cons { head, tail, .. } => {
                let head = self.lower_expression(head)?;
                let tail = self.lower_expression(tail)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Cons { head, tail },
                    metadata,
                });
                Ok(dest)
            }
            Expression::Perform {
                effect,
                operation,
                args,
                ..
            } => {
                let args = self.lower_expr_list(args)?;
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Perform {
                        effect: *effect,
                        operation: *operation,
                        args,
                    },
                    metadata,
                });
                Ok(dest)
            }
            Expression::Handle {
                expr: handled,
                effect,
                arms,
                ..
            } => {
                let handled = self.lower_expression(handled)?;
                let mut lowered_arms = Vec::with_capacity(arms.len());
                for arm in arms {
                    lowered_arms.push(IrHandleArm {
                        operation_name: arm.operation_name,
                        resume_param: arm.resume_param,
                        params: arm.params.clone(),
                        body: Box::new(arm.body.clone()),
                        metadata: IrMetadata {
                            span: Some(arm.span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                }
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Handle {
                        expr: handled,
                        effect: *effect,
                        arms: lowered_arms,
                    },
                    metadata,
                });
                Ok(dest)
            }
            Expression::Function {
                parameters, body, ..
            } => {
                let free_vars = collect_free_vars(expr);
                let mut capture_names: Vec<_> = free_vars
                    .into_iter()
                    .filter(|name| self.env.contains_key(name))
                    .collect();
                capture_names.sort_by_key(|name| name.as_u32());
                let function_id = self.lower_nested_function(parameters, body, &capture_names)?;
                let capture_vars = capture_names
                    .iter()
                    .filter_map(|name| self.env.get(name).copied())
                    .collect();
                let dest = self.next_var();
                let metadata = metadata_for(expr, &self.lowerer.hm_expr_types);
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeClosure(function_id, capture_vars),
                    metadata,
                });
                Ok(dest)
            }
            Expression::NamedConstructor { .. } | Expression::Spread { .. } => {
                unreachable!(
                    "named-field expression must be desugared during type inference \
                     (proposal 0152 Phase 3)"
                );
            }
        }
    }

    fn try_lower_literal_match_expression(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<IrVar>, Diagnostic> {
        let Some((checked_arms, fallback)) = split_match_fallback(arms) else {
            return Ok(None);
        };

        let mut arm_literals = Vec::with_capacity(checked_arms.len());
        for arm in checked_arms {
            let Some(value) = match_literal_pattern(&arm.pattern) else {
                return Ok(None);
            };
            arm_literals.push((value, arm));
        }
        let fallback_binding = fallback.and_then(|(binding, _)| binding);
        let exhaustive_without_fallback =
            fallback.is_none() && literals_are_exhaustive_without_fallback(&arm_literals);
        if fallback.is_none() && !exhaustive_without_fallback {
            return Ok(None);
        }

        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        for (value, arm) in arm_literals {
            let arm_match_block_id = self.lowerer.next_block();
            let next_block_id = self.lowerer.next_block();
            let const_var = self.emit_const(
                value,
                IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            )?;
            let cond_var = self.next_var();
            self.current_block_mut().instrs.push(IrInstr::Assign {
                dest: cond_var,
                expr: IrExpr::Binary(IrBinaryOp::Eq, scrutinee, const_var),
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            self.current_block_mut().terminator = IrTerminator::Branch {
                cond: cond_var,
                then_block: arm_match_block_id,
                else_block: next_block_id,
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            };

            self.blocks.push(IrBlock {
                id: arm_match_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
            self.env = saved_env.clone();

            if let Some(guard) = &arm.guard {
                let body_block_id = self.lowerer.next_block();
                let guard_fail_block_id = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: guard_fail_block_id,
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };

                self.blocks.push(IrBlock {
                    id: body_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
                self.env = saved_env.clone();
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );

                self.blocks.push(IrBlock {
                    id: guard_fail_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
            } else {
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
            }

            self.blocks.push(IrBlock {
                id: next_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
        }

        if let Some((_, fallback_arm)) = fallback {
            self.env = saved_env.clone();
            if let Some(name) = fallback_binding {
                self.env.insert(name, scrutinee);
            }
            let fallback_var = self.lower_expression(&fallback_arm.body)?;
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![fallback_var],
                IrMetadata {
                    span: Some(fallback_arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            );
        }

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(Some(result_var))
    }

    fn try_lower_tag_match_expression(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<IrVar>, Diagnostic> {
        let Some((checked_arms, fallback)) = split_match_fallback(arms) else {
            return Ok(None);
        };

        let mut arm_tags = Vec::with_capacity(checked_arms.len());
        for arm in checked_arms {
            let Some((tag, binding)) = match_tag_pattern(&arm.pattern) else {
                return Ok(None);
            };
            arm_tags.push((tag, binding, arm));
        }
        let fallback_binding = fallback.and_then(|(binding, _)| binding);
        let exhaustive_without_fallback =
            fallback.is_none() && tags_are_exhaustive_without_fallback(&arm_tags);
        if fallback.is_none() && !exhaustive_without_fallback {
            return Ok(None);
        }

        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        for (tag, payload_binding, arm) in arm_tags {
            let arm_match_block_id = self.lowerer.next_block();
            let next_block_id = self.lowerer.next_block();
            let cond_var = self.next_var();
            self.current_block_mut().instrs.push(IrInstr::Assign {
                dest: cond_var,
                expr: IrExpr::TagTest {
                    value: scrutinee,
                    tag,
                },
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            self.current_block_mut().terminator = IrTerminator::Branch {
                cond: cond_var,
                then_block: arm_match_block_id,
                else_block: next_block_id,
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            };

            self.blocks.push(IrBlock {
                id: arm_match_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
            self.env = saved_env.clone();
            if let Some(name) = payload_binding {
                let payload_var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload_var,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag,
                    },
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.env.insert(name, payload_var);
            }

            if let Some(guard) = &arm.guard {
                let body_block_id = self.lowerer.next_block();
                let guard_fail_block_id = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: guard_fail_block_id,
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };

                self.blocks.push(IrBlock {
                    id: body_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
                self.env = saved_env.clone();
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );

                self.blocks.push(IrBlock {
                    id: guard_fail_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
            } else {
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
            }

            self.blocks.push(IrBlock {
                id: next_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
        }

        if let Some((_, fallback_arm)) = fallback {
            self.env = saved_env.clone();
            if let Some(name) = fallback_binding {
                self.env.insert(name, scrutinee);
            }
            let fallback_var = self.lower_expression(&fallback_arm.body)?;
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![fallback_var],
                IrMetadata {
                    span: Some(fallback_arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            );
        }

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(Some(result_var))
    }

    fn try_lower_tuple_match_expression(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<IrVar>, Diagnostic> {
        let Some((fallback_arm, checked_arms)) = arms.split_last() else {
            return Ok(None);
        };
        if fallback_arm.guard.is_some() {
            return Ok(None);
        }
        let fallback_binding = match &fallback_arm.pattern {
            Pattern::Wildcard { .. } => None,
            Pattern::Identifier { name, .. } => Some(*name),
            _ => return Ok(None),
        };

        let mut arm_tuples = Vec::with_capacity(checked_arms.len());
        for arm in checked_arms {
            let Some(bindings) = match_tuple_pattern(&arm.pattern) else {
                return Ok(None);
            };
            arm_tuples.push((bindings, arm));
        }

        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        for (bindings, arm) in arm_tuples {
            let arm_match_block_id = self.lowerer.next_block();
            let next_block_id = self.lowerer.next_block();
            let cond_var = self.next_var();
            self.current_block_mut().instrs.push(IrInstr::Assign {
                dest: cond_var,
                expr: IrExpr::TupleArityTest {
                    value: scrutinee,
                    arity: bindings.len(),
                },
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            self.current_block_mut().terminator = IrTerminator::Branch {
                cond: cond_var,
                then_block: arm_match_block_id,
                else_block: next_block_id,
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            };

            self.blocks.push(IrBlock {
                id: arm_match_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
            self.env = saved_env.clone();
            for (index, binding) in bindings.iter().enumerate() {
                if let Some(name) = binding {
                    let field_var = self.next_var();
                    self.current_block_mut().instrs.push(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::TupleFieldAccess {
                            object: scrutinee,
                            index,
                        },
                        metadata: IrMetadata {
                            span: Some(arm.span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                    self.env.insert(*name, field_var);
                }
            }

            if let Some(guard) = &arm.guard {
                let body_block_id = self.lowerer.next_block();
                let guard_fail_block_id = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: guard_fail_block_id,
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };

                self.blocks.push(IrBlock {
                    id: body_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
                self.env = saved_env.clone();
                for (index, binding) in bindings.iter().enumerate() {
                    if let Some(name) = binding {
                        let field_var = self.next_var();
                        self.current_block_mut().instrs.push(IrInstr::Assign {
                            dest: field_var,
                            expr: IrExpr::TupleFieldAccess {
                                object: scrutinee,
                                index,
                            },
                            metadata: IrMetadata {
                                span: Some(arm.span),
                                inferred_type: None,
                                expr_id: None,
                            },
                        });
                        self.env.insert(*name, field_var);
                    }
                }
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );

                self.blocks.push(IrBlock {
                    id: guard_fail_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
            } else {
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
            }

            self.blocks.push(IrBlock {
                id: next_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
        }

        self.env = saved_env.clone();
        if let Some(name) = fallback_binding {
            self.env.insert(name, scrutinee);
        }
        let fallback_var = self.lower_expression(&fallback_arm.body)?;
        self.current_block_mut().terminator = IrTerminator::Jump(
            merge_block_id,
            vec![fallback_var],
            IrMetadata {
                span: Some(fallback_arm.span),
                inferred_type: None,
                expr_id: None,
            },
        );

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(Some(result_var))
    }

    fn try_lower_list_match_expression(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<IrVar>, Diagnostic> {
        let Some((checked_arms, fallback)) = split_match_fallback(arms) else {
            return Ok(None);
        };

        let mut arm_lists = Vec::with_capacity(checked_arms.len());
        for arm in checked_arms {
            let Some(pattern) = match_list_pattern(&arm.pattern) else {
                return Ok(None);
            };
            arm_lists.push((pattern, arm));
        }
        let fallback_binding = fallback.and_then(|(binding, _)| binding);
        let exhaustive_without_fallback =
            fallback.is_none() && lists_are_exhaustive_without_fallback(&arm_lists);
        if fallback.is_none() && !exhaustive_without_fallback {
            return Ok(None);
        }

        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        for (pattern, arm) in arm_lists {
            let arm_match_block_id = self.lowerer.next_block();
            let next_block_id = self.lowerer.next_block();
            let cond_var = self.next_var();
            self.current_block_mut().instrs.push(IrInstr::Assign {
                dest: cond_var,
                expr: IrExpr::ListTest {
                    value: scrutinee,
                    tag: pattern.tag(),
                },
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            self.current_block_mut().terminator = IrTerminator::Branch {
                cond: cond_var,
                then_block: arm_match_block_id,
                else_block: next_block_id,
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            };

            self.blocks.push(IrBlock {
                id: arm_match_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
            self.env = saved_env.clone();
            pattern.bind(self, scrutinee, arm.span);

            if let Some(guard) = &arm.guard {
                let body_block_id = self.lowerer.next_block();
                let guard_fail_block_id = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: guard_fail_block_id,
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };

                self.blocks.push(IrBlock {
                    id: body_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
                self.env = saved_env.clone();
                pattern.bind(self, scrutinee, arm.span);
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );

                self.blocks.push(IrBlock {
                    id: guard_fail_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
            } else {
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
            }

            self.blocks.push(IrBlock {
                id: next_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
        }

        if let Some((_, fallback_arm)) = fallback {
            self.env = saved_env.clone();
            if let Some(name) = fallback_binding {
                self.env.insert(name, scrutinee);
            }
            let fallback_var = self.lower_expression(&fallback_arm.body)?;
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![fallback_var],
                IrMetadata {
                    span: Some(fallback_arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            );
        }

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(Some(result_var))
    }

    fn try_lower_constructor_match_expression(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<IrVar>, Diagnostic> {
        let Some((fallback_arm, checked_arms)) = arms.split_last() else {
            return Ok(None);
        };
        if fallback_arm.guard.is_some() {
            return Ok(None);
        }
        let fallback_binding = match &fallback_arm.pattern {
            Pattern::Wildcard { .. } => None,
            Pattern::Identifier { name, .. } => Some(*name),
            _ => return Ok(None),
        };

        let mut arm_constructors = Vec::with_capacity(checked_arms.len());
        for arm in checked_arms {
            let Some(pattern) = match_constructor_pattern(&arm.pattern) else {
                return Ok(None);
            };
            arm_constructors.push((pattern, arm));
        }

        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        for (pattern, arm) in arm_constructors {
            let arm_match_block_id = self.lowerer.next_block();
            let next_block_id = self.lowerer.next_block();
            let cond_var = self.next_var();
            self.current_block_mut().instrs.push(IrInstr::Assign {
                dest: cond_var,
                expr: IrExpr::AdtTagTest {
                    value: scrutinee,
                    constructor: pattern.constructor,
                },
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            self.current_block_mut().terminator = IrTerminator::Branch {
                cond: cond_var,
                then_block: arm_match_block_id,
                else_block: next_block_id,
                metadata: IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            };

            self.blocks.push(IrBlock {
                id: arm_match_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
            self.env = saved_env.clone();
            pattern.bind(self, scrutinee, arm.span);

            if let Some(guard) = &arm.guard {
                let body_block_id = self.lowerer.next_block();
                let guard_fail_block_id = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block_id,
                    else_block: guard_fail_block_id,
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };

                self.blocks.push(IrBlock {
                    id: body_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
                self.env = saved_env.clone();
                pattern.bind(self, scrutinee, arm.span);
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );

                self.blocks.push(IrBlock {
                    id: guard_fail_block_id,
                    params: Vec::new(),
                    instrs: Vec::new(),
                    terminator: IrTerminator::Unreachable(IrMetadata::empty()),
                });
                self.current_block = self.blocks.len() - 1;
            } else {
                let body_var = self.lower_expression(&arm.body)?;
                self.current_block_mut().terminator = IrTerminator::Jump(
                    merge_block_id,
                    vec![body_var],
                    IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                );
            }

            self.blocks.push(IrBlock {
                id: next_block_id,
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: IrTerminator::Unreachable(IrMetadata::empty()),
            });
            self.current_block = self.blocks.len() - 1;
        }

        self.env = saved_env.clone();
        if let Some(name) = fallback_binding {
            self.env.insert(name, scrutinee);
        }
        let fallback_var = self.lower_expression(&fallback_arm.body)?;
        self.current_block_mut().terminator = IrTerminator::Jump(
            merge_block_id,
            vec![fallback_var],
            IrMetadata {
                span: Some(fallback_arm.span),
                inferred_type: None,
                expr_id: None,
            },
        );

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(Some(result_var))
    }

    fn push_block(&mut self, id: BlockId) {
        self.blocks.push(IrBlock {
            id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
    }

    /// Lower a match expression using a general recursive pattern compiler that
    /// handles all pattern types including mixed/nested patterns.
    ///
    /// Each arm is compiled to:
    ///   test_blocks → pass_block (body → Jump merge) | fail_block (next arm)
    /// The `fail_blocks` are pre-allocated so branches can reference them before
    /// the blocks are pushed, matching how the merge block is handled elsewhere.
    fn lower_general_match(
        &mut self,
        full_expr: &Expression,
        scrutinee_expr: &Expression,
        arms: &[MatchArm],
    ) -> Result<IrVar, Diagnostic> {
        let scrutinee = self.lower_expression(scrutinee_expr)?;
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );
        let merge_block_id = self.lowerer.next_block();
        let saved_env = self.env.clone();

        // Pre-allocate fail blocks: fail_blocks[i] is entered when arm i's
        // pattern or guard does not match. The last fail block is Unreachable
        // because exhaustive match is guaranteed by the type checker.
        let fail_blocks: Vec<BlockId> =
            (0..arms.len()).map(|_| self.lowerer.next_block()).collect();

        for (i, arm) in arms.iter().enumerate() {
            self.env = saved_env.clone();

            // Emit pattern tests; on failure, branch to fail_blocks[i].
            // On success, fall through with bindings applied to self.env.
            self.emit_pattern_test(scrutinee, &arm.pattern, fail_blocks[i], arm.span)?;

            if let Some(guard) = &arm.guard {
                let body_block = self.lowerer.next_block();
                let guard_var = self.lower_expression(guard)?;
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond: guard_var,
                    then_block: body_block,
                    else_block: fail_blocks[i],
                    metadata: IrMetadata {
                        span: Some(arm.span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(body_block);
                // Restore env and re-emit field-extraction instructions so the
                // body block has fresh IrVar definitions (the test-block defs are
                // still valid via SSA dominance, but CSE will deduplicate them).
                self.env = saved_env.clone();
                self.emit_pattern_bindings(scrutinee, &arm.pattern, arm.span)?;
            }

            let body_var = self.lower_expression(&arm.body)?;
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![body_var],
                IrMetadata {
                    span: Some(arm.span),
                    inferred_type: None,
                    expr_id: None,
                },
            );

            // Push the fail block for this arm; the next iteration begins here.
            self.push_block(fail_blocks[i]);
        }

        // After the last fail block: push merge block with result as block param.
        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.env = saved_env;
        self.last_value = Some(result_var);
        Ok(result_var)
    }

    /// Emit pattern test instructions into the current block.
    ///
    /// On pattern match: falls through with bindings inserted into `self.env`.
    /// On pattern mismatch: branches to `fail_block`.
    ///
    /// For each structural test a new "pass" block is created and pushed so that
    /// `Branch.then_block` is always the immediately-following block — satisfying
    /// the JIT CFG ordering invariant.
    fn emit_pattern_test(
        &mut self,
        scrutinee: IrVar,
        pattern: &Pattern,
        fail_block: BlockId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Always matches; no test, no binding.
            }
            Pattern::Identifier { name, .. } => {
                self.env.insert(*name, scrutinee);
            }
            Pattern::None { .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::TagTest {
                        value: scrutinee,
                        tag: IrTagTest::None,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
            }
            Pattern::Some { pattern: inner, .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::TagTest {
                        value: scrutinee,
                        tag: IrTagTest::Some,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Some,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_test(payload, inner, fail_block, span)?;
            }
            Pattern::Left { pattern: inner, .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::TagTest {
                        value: scrutinee,
                        tag: IrTagTest::Left,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Left,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_test(payload, inner, fail_block, span)?;
            }
            Pattern::Right { pattern: inner, .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::TagTest {
                        value: scrutinee,
                        tag: IrTagTest::Right,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Right,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_test(payload, inner, fail_block, span)?;
            }
            Pattern::EmptyList { .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::ListTest {
                        value: scrutinee,
                        tag: IrListTest::Empty,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
            }
            Pattern::Cons { head, tail, .. } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::ListTest {
                        value: scrutinee,
                        tag: IrListTest::Cons,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                let head_var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: head_var,
                    expr: IrExpr::ListHead { value: scrutinee },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                let tail_var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: tail_var,
                    expr: IrExpr::ListTail { value: scrutinee },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_test(head_var, head, fail_block, span)?;
                self.emit_pattern_test(tail_var, tail, fail_block, span)?;
            }
            Pattern::Tuple {
                elements,
                span: tuple_span,
            } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::TupleArityTest {
                        value: scrutinee,
                        arity: elements.len(),
                    },
                    metadata: IrMetadata {
                        span: Some(*tuple_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(*tuple_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                for (i, element) in elements.iter().enumerate() {
                    let field_var = self.next_var();
                    self.current_block_mut().instrs.push(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::TupleFieldAccess {
                            object: scrutinee,
                            index: i,
                        },
                        metadata: IrMetadata {
                            span: Some(*tuple_span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                    self.emit_pattern_test(field_var, element, fail_block, *tuple_span)?;
                }
            }
            Pattern::Constructor {
                name,
                fields,
                span: ctor_span,
            } => {
                let pass_block = self.lowerer.next_block();
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::AdtTagTest {
                        value: scrutinee,
                        constructor: *name,
                    },
                    metadata: IrMetadata {
                        span: Some(*ctor_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(*ctor_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
                for (i, field) in fields.iter().enumerate() {
                    let field_var = self.next_var();
                    self.current_block_mut().instrs.push(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::AdtField {
                            value: scrutinee,
                            index: i,
                        },
                        metadata: IrMetadata {
                            span: Some(*ctor_span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                    self.emit_pattern_test(field_var, field, fail_block, *ctor_span)?;
                }
            }
            Pattern::NamedConstructor { .. } => {
                unreachable!(
                    "named-field pattern must be desugared during type inference \
                     (proposal 0152 Phase 3)"
                );
            }
            Pattern::Literal {
                expression: _,
                span: lit_span,
            } => {
                let Some(const_val) = match_literal_pattern(pattern) else {
                    return Err(unsupported_lowering(
                        *lit_span,
                        "unsupported literal pattern in general match lowering",
                    ));
                };
                let pass_block = self.lowerer.next_block();
                let const_var = self.emit_const(
                    const_val,
                    IrMetadata {
                        span: Some(*lit_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                )?;
                let cond = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: cond,
                    expr: IrExpr::Binary(IrBinaryOp::Eq, scrutinee, const_var),
                    metadata: IrMetadata {
                        span: Some(*lit_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.current_block_mut().terminator = IrTerminator::Branch {
                    cond,
                    then_block: pass_block,
                    else_block: fail_block,
                    metadata: IrMetadata {
                        span: Some(*lit_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                };
                self.push_block(pass_block);
            }
        }
        Ok(())
    }

    /// Emit only the field-extraction instructions for a pattern without any
    /// test branches. Updates `self.env` with the resulting bindings.
    ///
    /// Called in the body block after a guard check succeeds, to re-establish
    /// valid `IrVar` definitions for all pattern-bound names (the ones emitted
    /// during the test phase are still in scope via SSA dominance, but CSE will
    /// deduplicate the extra instructions).
    fn emit_pattern_bindings(
        &mut self,
        scrutinee: IrVar,
        pattern: &Pattern,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match pattern {
            Pattern::Wildcard { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. }
            | Pattern::Literal { .. } => {}
            Pattern::Identifier { name, .. } => {
                self.env.insert(*name, scrutinee);
            }
            Pattern::Some { pattern: inner, .. } => {
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Some,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_bindings(payload, inner, span)?;
            }
            Pattern::Left { pattern: inner, .. } => {
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Left,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_bindings(payload, inner, span)?;
            }
            Pattern::Right { pattern: inner, .. } => {
                let payload = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: payload,
                    expr: IrExpr::TagPayload {
                        value: scrutinee,
                        tag: IrTagTest::Right,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_bindings(payload, inner, span)?;
            }
            Pattern::Cons {
                head,
                tail,
                span: cons_span,
            } => {
                let head_var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: head_var,
                    expr: IrExpr::ListHead { value: scrutinee },
                    metadata: IrMetadata {
                        span: Some(*cons_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                let tail_var = self.next_var();
                self.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: tail_var,
                    expr: IrExpr::ListTail { value: scrutinee },
                    metadata: IrMetadata {
                        span: Some(*cons_span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                self.emit_pattern_bindings(head_var, head, span)?;
                self.emit_pattern_bindings(tail_var, tail, span)?;
            }
            Pattern::Tuple {
                elements,
                span: tuple_span,
            } => {
                for (i, element) in elements.iter().enumerate() {
                    let field_var = self.next_var();
                    self.current_block_mut().instrs.push(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::TupleFieldAccess {
                            object: scrutinee,
                            index: i,
                        },
                        metadata: IrMetadata {
                            span: Some(*tuple_span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                    self.emit_pattern_bindings(field_var, element, span)?;
                }
            }
            Pattern::Constructor {
                fields,
                span: ctor_span,
                ..
            } => {
                for (i, field) in fields.iter().enumerate() {
                    let field_var = self.next_var();
                    self.current_block_mut().instrs.push(IrInstr::Assign {
                        dest: field_var,
                        expr: IrExpr::AdtField {
                            value: scrutinee,
                            index: i,
                        },
                        metadata: IrMetadata {
                            span: Some(*ctor_span),
                            inferred_type: None,
                            expr_id: None,
                        },
                    });
                    self.emit_pattern_bindings(field_var, field, span)?;
                }
            }
            Pattern::NamedConstructor { .. } => {
                unreachable!(
                    "named-field pattern must be desugared during type inference \
                     (proposal 0152 Phase 3)"
                );
            }
        }
        Ok(())
    }

    fn lower_if_expression(
        &mut self,
        full_expr: &Expression,
        condition: &Expression,
        consequence: &Block,
        alternative: Option<&Block>,
    ) -> Result<IrVar, Diagnostic> {
        let cond = self.lower_expression(condition)?;
        let then_block_id = self.lowerer.next_block();
        let else_block_id = self.lowerer.next_block();
        let merge_block_id = self.lowerer.next_block();
        let result_var = self.next_var();
        let result_ty = infer_type_to_ir(
            metadata_for(full_expr, &self.lowerer.hm_expr_types)
                .inferred_type
                .as_ref(),
        );

        self.current_block_mut().terminator = IrTerminator::Branch {
            cond,
            then_block: then_block_id,
            else_block: else_block_id,
            metadata: metadata_for(full_expr, &self.lowerer.hm_expr_types),
        };

        self.blocks.push(IrBlock {
            id: then_block_id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        let then_value = self.lower_block(consequence)?;
        if matches!(
            self.current_block_mut().terminator,
            IrTerminator::Unreachable(_)
        ) {
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![then_value],
                metadata_for(full_expr, &self.lowerer.hm_expr_types),
            );
        }

        self.blocks.push(IrBlock {
            id: else_block_id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        let else_value = if let Some(alternative) = alternative {
            self.lower_block(alternative)?
        } else {
            self.emit_const(IrConst::Unit, IrMetadata::empty())?
        };
        if matches!(
            self.current_block_mut().terminator,
            IrTerminator::Unreachable(_)
        ) {
            self.current_block_mut().terminator = IrTerminator::Jump(
                merge_block_id,
                vec![else_value],
                metadata_for(full_expr, &self.lowerer.hm_expr_types),
            );
        }

        self.blocks.push(IrBlock {
            id: merge_block_id,
            params: vec![IrBlockParam {
                var: result_var,
                ty: result_ty,
                inferred_ty: None,
            }],
            instrs: Vec::new(),
            terminator: IrTerminator::Unreachable(IrMetadata::empty()),
        });
        self.current_block = self.blocks.len() - 1;
        self.last_value = Some(result_var);
        Ok(result_var)
    }

    fn lower_nested_function(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        captures: &[Identifier],
    ) -> Result<FunctionId, Diagnostic> {
        let function_id = self.lowerer.next_function();
        let mut nested = FunctionLoweringContext::new(
            self.lowerer,
            function_id,
            None,
            IrFunctionOrigin::FunctionLiteral,
        );
        for capture in captures {
            let var = nested.next_var();
            nested.env.insert(*capture, var);
            nested.params.push(IrParam {
                name: *capture,
                var,
                ty: IrType::Tagged,
            });
        }
        for param in parameters {
            let var = nested.next_var();
            nested.env.insert(*param, var);
            nested.params.push(IrParam {
                name: *param,
                var,
                ty: IrType::Tagged,
            });
        }
        nested.lower_block(body)?;
        let ret = nested.ensure_return_var();
        nested.finish_with_metadata(
            IrType::Tagged,
            ret,
            IrMetadata::empty(),
            vec![None; parameters.len()],
            None,
            Vec::new(),
            captures.to_vec(),
            body.span,
        );
        Ok(function_id)
    }

    fn lower_expr_list(&mut self, expressions: &[Expression]) -> Result<Vec<IrVar>, Diagnostic> {
        expressions
            .iter()
            .map(|expr| self.lower_expression(expr))
            .collect()
    }

    fn emit_const(&mut self, value: IrConst, metadata: IrMetadata) -> Result<IrVar, Diagnostic> {
        let dest = self.next_var();
        self.current_block_mut().instrs.push(IrInstr::Assign {
            dest,
            expr: IrExpr::Const(value),
            metadata,
        });
        Ok(dest)
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn lower_top_level_item(statement: &Statement) -> Result<IrTopLevelItem, Diagnostic> {
    match statement {
        Statement::Let {
            is_public,
            name,
            type_annotation,
            value,
            span,
        } => Ok(IrTopLevelItem::Let {
            is_public: *is_public,
            name: *name,
            type_annotation: type_annotation.clone(),
            value: value.clone(),
            span: *span,
        }),
        Statement::LetDestructure {
            is_public,
            pattern,
            value,
            span,
        } => Ok(IrTopLevelItem::LetDestructure {
            is_public: *is_public,
            pattern: pattern.clone(),
            value: value.clone(),
            span: *span,
        }),
        Statement::Return { value, span } => Ok(IrTopLevelItem::Return {
            value: value.clone(),
            span: *span,
        }),
        Statement::Expression {
            expression,
            has_semicolon,
            span,
        } => Ok(IrTopLevelItem::Expression {
            expression: expression.clone(),
            has_semicolon: *has_semicolon,
            span: *span,
        }),
        Statement::Function {
            is_public,
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => Ok(IrTopLevelItem::Function {
            is_public: *is_public,
            name: *name,
            type_params: Statement::function_type_param_names(type_params),
            function_id: None,
            parameters: parameters.clone(),
            parameter_types: parameter_types.clone(),
            return_type: return_type.clone(),
            effects: effects.clone(),
            body: body.clone(),
            span: *span,
        }),
        Statement::Assign { name, value, span } => Ok(IrTopLevelItem::Assign {
            name: *name,
            value: value.clone(),
            span: *span,
        }),
        Statement::Module { name, body, span } => Ok(IrTopLevelItem::Module {
            name: *name,
            body: body
                .statements
                .iter()
                .map(lower_top_level_item)
                .collect::<Result<Vec<_>, _>>()?,
            span: *span,
        }),
        Statement::Import {
            name,
            alias,
            except,
            exposing,
            span,
        } => Ok(IrTopLevelItem::Import {
            name: *name,
            alias: *alias,
            except: except.clone(),
            exposing: exposing.clone(),
            span: *span,
        }),
        Statement::Data {
            // Proposal 0151: ADT visibility is enforced at the class
            // visibility walker; the IR layer is visibility-blind.
            is_public: _,
            name,
            type_params,
            variants,
            span,
            deriving: _,
        } => Ok(IrTopLevelItem::Data {
            name: *name,
            type_params: type_params.clone(),
            variants: variants.clone(),
            span: *span,
        }),
        Statement::EffectDecl { name, ops, span } => Ok(IrTopLevelItem::EffectDecl {
            name: *name,
            ops: ops.clone(),
            span: *span,
        }),
        Statement::Class {
            // Proposal 0151: visibility is enforced in higher-level passes
            // (class collection, name resolution). The cfg/IR layer is
            // visibility-blind, so we drop the field here.
            is_public: _,
            name,
            type_params,
            superclasses,
            methods,
            span,
        } => Ok(IrTopLevelItem::Class {
            name: *name,
            type_params: type_params.clone(),
            superclasses: superclasses.clone(),
            methods: methods.clone(),
            span: *span,
        }),
        Statement::Instance {
            is_public: _,
            class_name,
            type_args,
            context,
            methods,
            span,
        } => Ok(IrTopLevelItem::Instance {
            class_name: *class_name,
            type_args: type_args.clone(),
            context: context.clone(),
            methods: methods.clone(),
            span: *span,
        }),
    }
}

#[allow(dead_code, clippy::only_used_in_recursion)]
pub(crate) fn ir_top_level_item_to_statement(
    item: &IrTopLevelItem,
    functions: &[super::IrFunction],
) -> Statement {
    match item {
        IrTopLevelItem::Let {
            is_public,
            name,
            type_annotation,
            value,
            span,
        } => Statement::Let {
            is_public: *is_public,
            name: *name,
            type_annotation: type_annotation.clone(),
            value: value.clone(),
            span: *span,
        },
        IrTopLevelItem::LetDestructure {
            is_public,
            pattern,
            value,
            span,
        } => Statement::LetDestructure {
            is_public: *is_public,
            pattern: pattern.clone(),
            value: value.clone(),
            span: *span,
        },
        IrTopLevelItem::Return { value, span } => Statement::Return {
            value: value.clone(),
            span: *span,
        },
        IrTopLevelItem::Expression {
            expression,
            has_semicolon,
            span,
        } => Statement::Expression {
            expression: expression.clone(),
            has_semicolon: *has_semicolon,
            span: *span,
        },
        IrTopLevelItem::Function {
            is_public,
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => Statement::Function {
            is_public: *is_public,
            name: *name,
            type_params: type_params
                .iter()
                .map(|name| FunctionTypeParam {
                    name: *name,
                    constraints: vec![],
                })
                .collect(),
            parameters: parameters.clone(),
            parameter_types: parameter_types.clone(),
            return_type: return_type.clone(),
            effects: effects.clone(),
            body: body.clone(),
            span: *span,
            fip: None,
        },
        IrTopLevelItem::Assign { name, value, span } => Statement::Assign {
            name: *name,
            value: value.clone(),
            span: *span,
        },
        IrTopLevelItem::Module { name, body, span } => Statement::Module {
            name: *name,
            body: Block {
                statements: body
                    .iter()
                    .map(|item| ir_top_level_item_to_statement(item, functions))
                    .collect(),
                span: *span,
            },
            span: *span,
        },
        IrTopLevelItem::Import {
            name,
            alias,
            except,
            exposing,
            span,
        } => Statement::Import {
            name: *name,
            alias: *alias,
            except: except.clone(),
            exposing: exposing.clone(),
            span: *span,
        },
        IrTopLevelItem::Data {
            name,
            type_params,
            variants,
            span,
        } => Statement::Data {
            // IR layer doesn't carry ADT visibility — defaults to private
            // when reconstructing the AST. Visibility checks happen before
            // IR lowering, so this is safe.
            is_public: false,
            name: *name,
            type_params: type_params.clone(),
            variants: variants.clone(),
            span: *span,
            deriving: vec![],
        },
        IrTopLevelItem::EffectDecl { name, ops, span } => Statement::EffectDecl {
            name: *name,
            ops: ops.clone(),
            span: *span,
        },
        IrTopLevelItem::Class {
            name,
            type_params,
            superclasses,
            methods,
            span,
        } => Statement::Class {
            // Proposal 0151: cfg/IR is visibility-blind, so the round-trip
            // through IR loses the original `is_public` value. This function
            // is `#[allow(dead_code)]` and reserved for future use; if it
            // becomes load-bearing for visibility-sensitive paths, the IR
            // type must grow an `is_public` field too.
            is_public: false,
            name: *name,
            type_params: type_params.clone(),
            superclasses: superclasses.clone(),
            methods: methods.clone(),
            span: *span,
        },
        IrTopLevelItem::Instance {
            class_name,
            type_args,
            context,
            methods,
            span,
        } => Statement::Instance {
            is_public: false,
            class_name: *class_name,
            type_args: type_args.clone(),
            context: context.clone(),
            methods: methods.clone(),
            span: *span,
        },
    }
}

fn metadata_for(expr: &Expression, hm_expr_types: &HashMap<ExprId, InferType>) -> IrMetadata {
    let expr_id = expr.expr_id();
    IrMetadata {
        span: Some(expr.span()),
        inferred_type: hm_expr_types.get(&expr_id).cloned(),
        expr_id: Some(expr_id),
    }
}

fn map_binary_op(op: &str) -> Option<IrBinaryOp> {
    match op {
        "+" => Some(IrBinaryOp::Add),
        "-" => Some(IrBinaryOp::Sub),
        "*" => Some(IrBinaryOp::Mul),
        "/" => Some(IrBinaryOp::Div),
        "%" => Some(IrBinaryOp::Mod),
        "==" => Some(IrBinaryOp::Eq),
        "!=" => Some(IrBinaryOp::NotEq),
        "<" => Some(IrBinaryOp::Lt),
        "<=" => Some(IrBinaryOp::Le),
        ">" => Some(IrBinaryOp::Gt),
        ">=" => Some(IrBinaryOp::Ge),
        "&&" => Some(IrBinaryOp::And),
        "||" => Some(IrBinaryOp::Or),
        _ => None,
    }
}

fn infer_type_to_ir(infer_type: Option<&InferType>) -> IrType {
    match infer_type {
        Some(InferType::Con(TypeConstructor::Int)) => IrType::Int,
        Some(InferType::Con(TypeConstructor::Float)) => IrType::Float,
        Some(InferType::Con(TypeConstructor::Bool)) => IrType::Bool,
        Some(InferType::Con(TypeConstructor::String)) => IrType::String,
        Some(InferType::Con(TypeConstructor::Unit)) => IrType::Unit,
        _ => IrType::Tagged,
    }
}

fn match_literal_pattern(pattern: &Pattern) -> Option<IrConst> {
    let Pattern::Literal { expression, .. } = pattern else {
        return None;
    };
    match expression {
        Expression::Integer { value, .. } => Some(IrConst::Int(*value)),
        Expression::Float { value, .. } => Some(IrConst::Float(*value)),
        Expression::Boolean { value, .. } => Some(IrConst::Bool(*value)),
        Expression::String { value, .. } => Some(IrConst::String(value.clone())),
        _ => None,
    }
}

type MatchFallback<'a> = Option<(Option<Identifier>, &'a MatchArm)>;

#[allow(clippy::type_complexity)]
fn split_match_fallback(arms: &[MatchArm]) -> Option<(&[MatchArm], MatchFallback<'_>)> {
    let (last_arm, checked_arms) = arms.split_last()?;
    if last_arm.guard.is_none() {
        match &last_arm.pattern {
            Pattern::Wildcard { .. } => return Some((checked_arms, Some((None, last_arm)))),
            Pattern::Identifier { name, .. } => {
                return Some((checked_arms, Some((Some(*name), last_arm))));
            }
            _ => {}
        }
    }

    Some((arms, None))
}

fn literals_are_exhaustive_without_fallback(arms: &[(IrConst, &MatchArm)]) -> bool {
    if arms.iter().any(|(_, arm)| arm.guard.is_some()) {
        return false;
    }
    let mut seen_true = false;
    let mut seen_false = false;
    for (value, _) in arms {
        match value {
            IrConst::Bool(true) => seen_true = true,
            IrConst::Bool(false) => seen_false = true,
            _ => return false,
        }
    }
    seen_true && seen_false
}

fn match_tag_pattern(pattern: &Pattern) -> Option<(IrTagTest, Option<Identifier>)> {
    match pattern {
        Pattern::None { .. } => Some((IrTagTest::None, None)),
        Pattern::Some { pattern, .. } => match pattern.as_ref() {
            Pattern::Wildcard { .. } => Some((IrTagTest::Some, None)),
            Pattern::Identifier { name, .. } => Some((IrTagTest::Some, Some(*name))),
            _ => None,
        },
        Pattern::Left { pattern, .. } => match pattern.as_ref() {
            Pattern::Wildcard { .. } => Some((IrTagTest::Left, None)),
            Pattern::Identifier { name, .. } => Some((IrTagTest::Left, Some(*name))),
            _ => None,
        },
        Pattern::Right { pattern, .. } => match pattern.as_ref() {
            Pattern::Wildcard { .. } => Some((IrTagTest::Right, None)),
            Pattern::Identifier { name, .. } => Some((IrTagTest::Right, Some(*name))),
            _ => None,
        },
        _ => None,
    }
}

fn tags_are_exhaustive_without_fallback(
    arms: &[(IrTagTest, Option<Identifier>, &MatchArm)],
) -> bool {
    if arms.iter().any(|(_, _, arm)| arm.guard.is_some()) {
        return false;
    }
    let mut seen_none = false;
    let mut seen_some = false;
    let mut seen_left = false;
    let mut seen_right = false;
    for (tag, _, _) in arms {
        match tag {
            IrTagTest::None => seen_none = true,
            IrTagTest::Some => seen_some = true,
            IrTagTest::Left => seen_left = true,
            IrTagTest::Right => seen_right = true,
        }
    }
    (seen_none && seen_some && !seen_left && !seen_right)
        || (!seen_none && !seen_some && seen_left && seen_right)
}

fn match_tuple_pattern(pattern: &Pattern) -> Option<Vec<Option<Identifier>>> {
    let Pattern::Tuple { elements, .. } = pattern else {
        return None;
    };
    let mut bindings = Vec::with_capacity(elements.len());
    for element in elements {
        match element {
            Pattern::Wildcard { .. } => bindings.push(None),
            Pattern::Identifier { name, .. } => bindings.push(Some(*name)),
            _ => return None,
        }
    }
    Some(bindings)
}

#[derive(Clone, Copy)]
enum SimpleListMatchPattern {
    Empty,
    Cons {
        head_binding: Option<Identifier>,
        tail_binding: Option<Identifier>,
    },
}

impl SimpleListMatchPattern {
    fn tag(self) -> IrListTest {
        match self {
            Self::Empty => IrListTest::Empty,
            Self::Cons { .. } => IrListTest::Cons,
        }
    }

    fn bind(self, lowering: &mut FunctionLoweringContext<'_>, scrutinee: IrVar, span: Span) {
        let Self::Cons {
            head_binding,
            tail_binding,
        } = self
        else {
            return;
        };

        if let Some(name) = head_binding {
            let head_var = lowering.next_var();
            lowering.current_block_mut().instrs.push(IrInstr::Assign {
                dest: head_var,
                expr: IrExpr::ListHead { value: scrutinee },
                metadata: IrMetadata {
                    span: Some(span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            lowering.env.insert(name, head_var);
        }

        if let Some(name) = tail_binding {
            let tail_var = lowering.next_var();
            lowering.current_block_mut().instrs.push(IrInstr::Assign {
                dest: tail_var,
                expr: IrExpr::ListTail { value: scrutinee },
                metadata: IrMetadata {
                    span: Some(span),
                    inferred_type: None,
                    expr_id: None,
                },
            });
            lowering.env.insert(name, tail_var);
        }
    }
}

fn match_list_pattern(pattern: &Pattern) -> Option<SimpleListMatchPattern> {
    match pattern {
        Pattern::EmptyList { .. } => Some(SimpleListMatchPattern::Empty),
        Pattern::Cons { head, tail, .. } => {
            let head_binding = match head.as_ref() {
                Pattern::Wildcard { .. } => None,
                Pattern::Identifier { name, .. } => Some(*name),
                _ => return None,
            };
            let tail_binding = match tail.as_ref() {
                Pattern::Wildcard { .. } => None,
                Pattern::Identifier { name, .. } => Some(*name),
                _ => return None,
            };
            Some(SimpleListMatchPattern::Cons {
                head_binding,
                tail_binding,
            })
        }
        _ => None,
    }
}

fn lists_are_exhaustive_without_fallback(arms: &[(SimpleListMatchPattern, &MatchArm)]) -> bool {
    if arms.iter().any(|(_, arm)| arm.guard.is_some()) {
        return false;
    }
    let mut seen_empty = false;
    let mut seen_cons = false;
    for (pattern, _) in arms {
        match pattern {
            SimpleListMatchPattern::Empty => seen_empty = true,
            SimpleListMatchPattern::Cons { .. } => seen_cons = true,
        }
    }
    seen_empty && seen_cons
}

#[derive(Clone)]
struct SimpleConstructorMatchPattern {
    constructor: Identifier,
    field_bindings: Vec<Option<Identifier>>,
}

impl SimpleConstructorMatchPattern {
    fn bind(&self, lowering: &mut FunctionLoweringContext<'_>, scrutinee: IrVar, span: Span) {
        for (index, binding) in self.field_bindings.iter().enumerate() {
            if let Some(name) = binding {
                let field_var = lowering.next_var();
                lowering.current_block_mut().instrs.push(IrInstr::Assign {
                    dest: field_var,
                    expr: IrExpr::AdtField {
                        value: scrutinee,
                        index,
                    },
                    metadata: IrMetadata {
                        span: Some(span),
                        inferred_type: None,
                        expr_id: None,
                    },
                });
                lowering.env.insert(*name, field_var);
            }
        }
    }
}

fn match_constructor_pattern(pattern: &Pattern) -> Option<SimpleConstructorMatchPattern> {
    let Pattern::Constructor { name, fields, .. } = pattern else {
        return None;
    };
    let mut field_bindings = Vec::with_capacity(fields.len());
    for field in fields {
        match field {
            Pattern::Wildcard { .. } => field_bindings.push(None),
            Pattern::Identifier { name, .. } => field_bindings.push(Some(*name)),
            _ => return None,
        }
    }
    Some(SimpleConstructorMatchPattern {
        constructor: *name,
        field_bindings,
    })
}

fn unsupported_lowering(span: crate::diagnostics::position::Span, message: &str) -> Diagnostic {
    Diagnostic::warning("Flux IR lowering fallback")
        .with_error_type(ErrorType::Compiler)
        .with_phase(DiagnosticPhase::Validation)
        .with_span(span)
        .with_message(message)
}

#[cfg(test)]
mod tests {
    use crate::{
        cfg::validate_ir,
        compiler::Compiler,
        syntax::{lexer::Lexer, parser::Parser},
    };

    use super::lower_program_to_ir;

    fn lower(source: &str) -> String {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();
        let mut compiler = Compiler::new_with_interner("<test>", interner);
        let hm = compiler.infer_expr_types_for_program(&program);
        let ir = lower_program_to_ir(&program, &hm).expect("lowering should succeed");
        ir.dump_text()
    }

    /// Like `lower`, but also asserts the IR passes `validate_ir`.
    fn lower_and_validate(source: &str) -> String {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();
        let mut compiler = Compiler::new_with_interner("<test>", interner);
        let hm = compiler.infer_expr_types_for_program(&program);
        let ir = lower_program_to_ir(&program, &hm).expect("lowering should succeed");
        validate_ir(&ir).expect("lowered IR should be structurally valid");
        ir.dump_text()
    }

    #[test]
    fn lowers_if_expression_into_cfg_blocks() {
        let ir = lower("let x = if true { 1 } else { 2 };");
        assert!(ir.contains("Branch"));
        assert!(ir.contains("b1("), "then-block b1 should exist");
        assert!(ir.contains("b3("), "merge-block b3 should exist");
    }

    #[test]
    fn lowers_function_literals_into_separate_functions() {
        let ir = lower("let f = fn(x) { x + 1 };");
        assert!(ir.contains("FunctionLiteral"));
        assert!(ir.contains("MakeClosure"));
    }

    #[test]
    fn lowers_tail_position_calls_into_tailcall_terminators() {
        let ir = lower("fn f(n) { f(n - 1) }");
        assert!(ir.contains("TailCall"));
    }

    #[test]
    fn debug_factorial_ir_structure() {
        let ir =
            lower("fn factorial(n, acc) { if n == 0 { acc } else { factorial(n - 1, n * acc) } }");
        println!("{}", ir);
        // This test is purely for inspection — always passes
    }

    #[test]
    fn lowers_boolean_match_with_fallback_into_cfg_blocks() {
        let ir = lower("let x = match true { true -> 1, _ -> 0 };");
        assert!(ir.contains("Branch"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_integer_match_with_fallback_into_cfg_blocks() {
        let ir = lower("let x = match 2 { 1 -> 10, 2 -> 20, n -> n };");
        assert!(ir.contains("Branch"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_guarded_literal_match_with_fallback_into_cfg_blocks() {
        let ir = lower("let x = match 2 { 1 if false -> 10, 2 if true -> 20, _ -> 0 };");
        assert!(ir.contains("Branch"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_option_tag_match_with_fallback_into_cfg_blocks() {
        let ir = lower("let x = match Some(1) { Some(_) -> 1, None -> 0, _ -> 2 };");
        assert!(ir.contains("TagTest"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_option_tag_match_with_payload_binding_into_cfg_blocks() {
        let ir = lower("let x = match Some(1) { Some(n) -> n, None -> 0, _ -> 2 };");
        assert!(ir.contains("TagTest"));
        assert!(ir.contains("TagPayload"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_either_tag_match_with_guard_into_cfg_blocks() {
        let ir = lower("let x = match Left(1) { Left(_) if true -> 1, Right(_) -> 2, _ -> 3 };");
        assert!(ir.contains("TagTest"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_either_tag_match_with_payload_binding_into_cfg_blocks() {
        let ir = lower("let x = match Left(1) { Left(n) -> n, Right(m) -> m, _ -> 3 };");
        assert!(ir.contains("TagTest"));
        assert!(ir.contains("TagPayload"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_tuple_match_with_binding_into_cfg_blocks() {
        let ir = lower("let x = match (1, 2) { (a, _) -> a, _ -> 0 };");
        assert!(ir.contains("TupleArityTest"));
        assert!(ir.contains("TupleFieldAccess"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_guarded_tuple_match_with_binding_into_cfg_blocks() {
        let ir = lower("let x = match (1, 2) { (a, b) if true -> a, _ -> 0 };");
        assert!(ir.contains("TupleArityTest"));
        assert!(ir.contains("TupleFieldAccess"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_list_match_with_binding_into_cfg_blocks() {
        let ir = lower("let x = match [1, 2] { [head | tail] -> head, [] -> 0, _ -> 2 };");
        assert!(ir.contains("ListTest"));
        assert!(ir.contains("ListHead"));
        assert!(ir.contains("ListTail"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_guarded_list_match_with_binding_into_cfg_blocks() {
        let ir = lower("let x = match [1, 2] { [head | tail] if true -> head, _ -> 0 };");
        assert!(ir.contains("ListTest"));
        assert!(ir.contains("ListHead"));
        assert!(ir.contains("ListTail"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_constructor_match_with_binding_into_cfg_blocks() {
        let ir = lower(
            "data MaybeInt { SomeInt(Int), NoneInt }\nlet x = match SomeInt(1) { SomeInt(n) -> n, NoneInt -> 0, _ -> 2 };",
        );
        assert!(ir.contains("AdtTagTest"));
        assert!(ir.contains("AdtField"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_guarded_constructor_match_with_binding_into_cfg_blocks() {
        let ir = lower(
            "data MaybeInt { SomeInt(Int), NoneInt }\nlet x = match SomeInt(1) { SomeInt(n) if true -> n, _ -> 0 };",
        );
        assert!(ir.contains("AdtTagTest"));
        assert!(ir.contains("AdtField"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_exhaustive_bool_match_without_fallback_into_cfg_blocks() {
        let ir = lower("let x = match true { true -> 1, false -> 0 };");
        assert!(ir.contains("Branch"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_exhaustive_option_match_without_fallback_into_cfg_blocks() {
        let ir = lower("let x = match Some(1) { Some(n) -> n, None -> 0 };");
        assert!(ir.contains("TagTest"));
        assert!(ir.contains("TagPayload"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_exhaustive_list_match_without_fallback_into_cfg_blocks() {
        let ir = lower("let x = match [1, 2] { [] -> 0, [head | tail] -> head };");
        assert!(ir.contains("ListTest"));
        assert!(ir.contains("ListHead"));
        assert!(ir.contains("ListTail"));
        assert!(!ir.contains("Match {"));
    }

    #[test]
    fn lowers_high_level_expression_forms_without_ast_fallback() {
        let ir = lower(
            r#"
let idx = [1, 2][0];
let field = (1, 2).0;
let opt = Some(1);
let either = Left(2);
let str = "x #{1}";
let matched = match opt { Some(x) -> x, _ -> 0 };
"#,
        );
        assert!(!ir.contains("Ast("));
        assert!(ir.contains("Index"));
        assert!(ir.contains("TupleFieldAccess"));
        assert!(ir.contains("Some"));
        assert!(ir.contains("Left"));
        assert!(ir.contains("InterpolatedString"));
        assert!(ir.contains("Branch"));
    }

    #[test]
    fn lowers_mixed_pattern_match_to_cfg_blocks() {
        // Mixed literal + wildcard: none of the specialized helpers handle this
        let ir = lower("let x = match 1 { 0 -> \"zero\", 1 -> \"one\", _ -> \"other\" }");
        assert!(
            !ir.contains("Match {"),
            "fallback IrExpr::Match must not appear"
        );
        assert!(ir.contains("Branch"));
    }

    #[test]
    fn lowers_nested_tag_pattern_to_cfg_blocks() {
        let ir = lower("let x = match Some([1, 2]) { Some([h | _]) -> h, _ -> 0 }");
        assert!(
            !ir.contains("Match {"),
            "fallback IrExpr::Match must not appear"
        );
        assert!(ir.contains("Branch"));
    }

    #[test]
    fn lowers_nested_constructor_pattern_to_cfg_blocks() {
        // Nested constructor match: general matcher handles fields with non-trivial sub-patterns
        let ir = lower(
            r#"
data Tree { Leaf, Node(Tree, Int, Tree) }
fn depth(t) {
    match t {
        Leaf -> 0,
        Node(l, _, r) -> 1,
    }
}
"#,
        );
        assert!(
            !ir.contains("Match {"),
            "fallback IrExpr::Match must not appear"
        );
        assert!(ir.contains("Branch"));
    }

    #[test]
    fn lowers_guarded_general_match_to_cfg_blocks() {
        let ir = lower("let x = match (1, 2) { (a, b) if a > 0 -> a + b, _ -> 0 }");
        assert!(
            !ir.contains("Match {"),
            "fallback IrExpr::Match must not appear"
        );
        assert!(ir.contains("Branch"));
    }

    // ── New broad-coverage tests ────────────────────────────────────────────

    #[test]
    fn lowers_simple_named_function_produces_valid_ir() {
        let ir = lower_and_validate("fn double(x) { x + x }");
        // A named fn must appear with the NamedFunction origin tag.
        assert!(
            ir.contains("NamedFunction"),
            "fn should have NamedFunction origin"
        );
        // The function body must terminate with a Return, not fall off the end.
        assert!(
            ir.contains("Return"),
            "fn should end with Return terminator"
        );
        // The addition must be present as a binary IR node.
        assert!(
            ir.contains("Binary(Add"),
            "fn body should contain binary Add"
        );
    }

    #[test]
    fn lowers_multi_arm_match_produces_valid_ir() {
        // validate_ir checks every variable is defined before use and that all
        // block-param arities match their jump sites — a thorough structural check.
        let ir = lower_and_validate(
            r#"
fn classify(n) {
    match n {
        0 -> "zero",
        1 -> "one",
        _ -> "other",
    }
}
"#,
        );
        assert!(
            !ir.contains("Match {"),
            "no fallback Match node should remain"
        );
        assert!(
            ir.contains("Branch"),
            "arms must be encoded as Branch terminators"
        );
        assert!(ir.contains("Return"), "all arms must eventually return");
    }

    #[test]
    fn lowers_closure_capturing_outer_variable_produces_valid_ir() {
        let ir = lower_and_validate("let base = 10; let add_base = fn(x) { x + base };");
        // The closure body is compiled as a separate FunctionLiteral function.
        assert!(
            ir.contains("FunctionLiteral"),
            "closure body should be a FunctionLiteral"
        );
        // The outer function must emit a MakeClosure that closes over `base`.
        assert!(
            ir.contains("MakeClosure"),
            "closure creation should use MakeClosure"
        );
    }

    #[test]
    fn lowers_perform_and_handle_to_ir_nodes() {
        let ir = lower_and_validate(
            r#"
effect Log {
    write: String -> ()
}

fn greet(name) {
    perform Log.write(name)
}

greet("world") handle Log {
    write(resume, _msg) -> resume(())
}
"#,
        );
        // perform lowers to IrExpr::Perform in the flat IR.
        assert!(
            ir.contains("Perform"),
            "perform should lower to a Perform IR node"
        );
        // handle lowers to IrExpr::Handle wrapping the scrutinee var.
        assert!(
            ir.contains("Handle"),
            "handle should lower to a Handle IR node"
        );
    }
}
