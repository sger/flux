use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::ir::{
    BlockId, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr, IrProgram,
    IrStructuredBlock, IrStructuredExpr, IrStructuredHandleArm, IrStructuredMatchArm, IrTerminator,
    IrTopLevelItem, IrVar, ir_structured_block_to_block, ir_structured_expr_to_expression,
    ir_structured_pattern_to_pattern,
};
use crate::{
    ast::type_infer::display_infer_type,
    bytecode::{
        compiler::{Compiler, contracts::convert_type_expr, suggestions::suggest_effect_name},
        debug_info::FunctionDebugInfo,
        module_constants::compile_module_constants,
        op_code::{OpCode, make},
        symbol_scope::SymbolScope,
    },
    diagnostics::{
        DUPLICATE_PARAMETER, Diagnostic, DiagnosticBuilder, DiagnosticPhase, ICE_SYMBOL_SCOPE_LET,
        IMPORT_SCOPE, INVALID_MODULE_CONTENT, INVALID_MODULE_NAME, MODULE_NAME_CLASH, MODULE_SCOPE,
        compiler_errors::{fun_return_annotation_mismatch, let_annotation_type_mismatch},
        position::{Position, Span},
        types::ErrorType,
    },
    runtime::{compiled_function::CompiledFunction, value::Value},
    syntax::{
        block::Block,
        data_variant::DataVariant,
        effect_expr::EffectExpr,
        expression::Pattern,
        module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
    },
    types::{infer_type::InferType, type_env::TypeEnv, unify::unify},
};

use super::hm_expr_typer::HmExprTypeResult;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

impl Compiler {
    #[allow(dead_code)]
    fn emit_store_binding(&mut self, binding: &crate::bytecode::binding::Binding) {
        match binding.symbol_scope {
            SymbolScope::Local => {
                self.emit(OpCode::OpSetLocal, &[binding.index]);
            }
            SymbolScope::Global => {
                self.emit(OpCode::OpSetGlobal, &[binding.index]);
            }
            _ => {}
        }
    }

    fn empty_ir_program() -> IrProgram {
        IrProgram {
            top_level_items: Vec::new(),
            functions: Vec::new(),
            entry: crate::ir::FunctionId(0),
            globals: Vec::new(),
            hm_expr_types: HashMap::new(),
        }
    }

    fn ir_top_level_item_span(item: &IrTopLevelItem) -> Span {
        match item {
            IrTopLevelItem::Let { span, .. }
            | IrTopLevelItem::LetDestructure { span, .. }
            | IrTopLevelItem::Return { span, .. }
            | IrTopLevelItem::Expression { span, .. }
            | IrTopLevelItem::Function { span, .. }
            | IrTopLevelItem::Assign { span, .. }
            | IrTopLevelItem::Module { span, .. }
            | IrTopLevelItem::Import { span, .. }
            | IrTopLevelItem::Data { span, .. }
            | IrTopLevelItem::EffectDecl { span, .. } => *span,
        }
    }

    fn ir_expr_contains_tail_call(
        &self,
        expression: &IrStructuredExpr,
        tail_call_spans: &[Span],
    ) -> bool {
        match expression {
            IrStructuredExpr::Call {
                function,
                arguments,
                span,
                ..
            } => {
                tail_call_spans.contains(span)
                    || self.ir_expr_contains_tail_call(function, tail_call_spans)
                    || arguments
                        .iter()
                        .any(|arg| self.ir_expr_contains_tail_call(arg, tail_call_spans))
            }
            IrStructuredExpr::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.ir_expr_contains_tail_call(condition, tail_call_spans)
                    || self.ir_block_contains_tail_call(consequence, tail_call_spans)
                    || alternative.as_ref().is_some_and(|block| {
                        self.ir_block_contains_tail_call(block, tail_call_spans)
                    })
            }
            IrStructuredExpr::DoBlock { block, .. } => {
                self.ir_block_contains_tail_call(block, tail_call_spans)
            }
            IrStructuredExpr::Match {
                scrutinee, arms, ..
            } => {
                self.ir_expr_contains_tail_call(scrutinee, tail_call_spans)
                    || arms
                        .iter()
                        .any(|arm| self.ir_match_arm_contains_tail_call(arm, tail_call_spans))
            }
            IrStructuredExpr::Handle { expr, arms, .. } => {
                self.ir_expr_contains_tail_call(expr, tail_call_spans)
                    || arms
                        .iter()
                        .any(|arm| self.ir_handle_arm_contains_tail_call(arm, tail_call_spans))
            }
            IrStructuredExpr::Perform { args, .. }
            | IrStructuredExpr::ListLiteral { elements: args, .. }
            | IrStructuredExpr::ArrayLiteral { elements: args, .. }
            | IrStructuredExpr::TupleLiteral { elements: args, .. } => args
                .iter()
                .any(|arg| self.ir_expr_contains_tail_call(arg, tail_call_spans)),
            IrStructuredExpr::Hash { pairs, .. } => pairs.iter().any(|(key, value)| {
                self.ir_expr_contains_tail_call(key, tail_call_spans)
                    || self.ir_expr_contains_tail_call(value, tail_call_spans)
            }),
            IrStructuredExpr::Index { left, index, .. } => {
                self.ir_expr_contains_tail_call(left, tail_call_spans)
                    || self.ir_expr_contains_tail_call(index, tail_call_spans)
            }
            IrStructuredExpr::MemberAccess { object, .. }
            | IrStructuredExpr::TupleFieldAccess { object, .. } => {
                self.ir_expr_contains_tail_call(object, tail_call_spans)
            }
            IrStructuredExpr::Some { value, .. }
            | IrStructuredExpr::Left { value, .. }
            | IrStructuredExpr::Right { value, .. }
            | IrStructuredExpr::Prefix { right: value, .. } => {
                self.ir_expr_contains_tail_call(value, tail_call_spans)
            }
            IrStructuredExpr::Cons { head, tail, .. }
            | IrStructuredExpr::Infix {
                left: head,
                right: tail,
                ..
            } => {
                self.ir_expr_contains_tail_call(head, tail_call_spans)
                    || self.ir_expr_contains_tail_call(tail, tail_call_spans)
            }
            IrStructuredExpr::Function { body, .. } => {
                self.ir_block_contains_tail_call(body, tail_call_spans)
            }
            IrStructuredExpr::Identifier { .. }
            | IrStructuredExpr::Integer { .. }
            | IrStructuredExpr::Float { .. }
            | IrStructuredExpr::String { .. }
            | IrStructuredExpr::Boolean { .. }
            | IrStructuredExpr::InterpolatedString { .. }
            | IrStructuredExpr::EmptyList { .. }
            | IrStructuredExpr::None { .. } => false,
        }
    }

    fn ir_match_arm_contains_tail_call(
        &self,
        arm: &IrStructuredMatchArm,
        tail_call_spans: &[Span],
    ) -> bool {
        arm.guard
            .as_ref()
            .is_some_and(|guard| self.ir_expr_contains_tail_call(guard, tail_call_spans))
            || self.ir_expr_contains_tail_call(&arm.body, tail_call_spans)
    }

    fn ir_handle_arm_contains_tail_call(
        &self,
        arm: &IrStructuredHandleArm,
        tail_call_spans: &[Span],
    ) -> bool {
        self.ir_expr_contains_tail_call(&arm.body, tail_call_spans)
    }

    fn ir_top_level_item_contains_tail_call(
        &self,
        item: &IrTopLevelItem,
        tail_call_spans: &[Span],
    ) -> bool {
        match item {
            IrTopLevelItem::Expression { expression, .. } => {
                self.ir_expr_contains_tail_call(expression, tail_call_spans)
            }
            IrTopLevelItem::Return { value, .. } => value
                .as_ref()
                .is_some_and(|expr| self.ir_expr_contains_tail_call(expr, tail_call_spans)),
            _ => false,
        }
    }

    fn ir_block_contains_tail_call(
        &self,
        block: &IrStructuredBlock,
        tail_call_spans: &[Span],
    ) -> bool {
        block
            .statements
            .iter()
            .any(|item| self.ir_top_level_item_contains_tail_call(item, tail_call_spans))
    }

    fn collect_tail_call_spans_for_ir_function(function: &IrFunction) -> Vec<Span> {
        function
            .blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                crate::ir::IrTerminator::TailCall { metadata, .. } => metadata.span,
                _ => None,
            })
            .collect()
    }

    #[allow(dead_code)]
    fn can_compile_ir_cfg_function(function: &IrFunction) -> bool {
        fn supported_expr(expr: &IrExpr) -> bool {
            match expr {
                IrExpr::Const(IrConst::Int(_))
                | IrExpr::Const(IrConst::Float(_))
                | IrExpr::Const(IrConst::Bool(_))
                | IrExpr::Const(IrConst::String(_))
                | IrExpr::Const(IrConst::Unit)
                | IrExpr::Var(_)
                | IrExpr::None
                | IrExpr::TupleFieldAccess { .. }
                | IrExpr::TupleArityTest { .. }
                | IrExpr::TagTest { .. }
                | IrExpr::TagPayload { .. }
                | IrExpr::ListTest { .. }
                | IrExpr::ListHead { .. }
                | IrExpr::ListTail { .. }
                | IrExpr::AdtTagTest { .. }
                | IrExpr::AdtField { .. } => true,
                IrExpr::Binary(op, _, _) => matches!(
                    op,
                    crate::ir::IrBinaryOp::Add
                        | crate::ir::IrBinaryOp::Sub
                        | crate::ir::IrBinaryOp::Mul
                        | crate::ir::IrBinaryOp::Div
                        | crate::ir::IrBinaryOp::Eq
                        | crate::ir::IrBinaryOp::NotEq
                        | crate::ir::IrBinaryOp::Lt
                        | crate::ir::IrBinaryOp::Gt
                        | crate::ir::IrBinaryOp::Ge
                        | crate::ir::IrBinaryOp::Le
                ),
                _ => false,
            }
        }

        let Some(entry_index) = function
            .blocks
            .iter()
            .position(|block| block.id == function.entry)
        else {
            return false;
        };
        if entry_index != 0 {
            return false;
        }
        let block_indices: HashMap<_, _> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.id, index))
            .collect();

        for (index, block) in function.blocks.iter().enumerate() {
            if !block.instrs.iter().all(|instr| match instr {
                IrInstr::Assign { expr, .. } => supported_expr(expr),
                IrInstr::Call { target, .. } => {
                    matches!(
                        target,
                        IrCallTarget::Named(_) | IrCallTarget::Direct(_) | IrCallTarget::Var(_)
                    )
                }
            }) {
                return false;
            }

            match &block.terminator {
                IrTerminator::Return(..) | IrTerminator::TailCall { .. } => {}
                IrTerminator::Jump(target, args, _) => {
                    let Some(target_index) = block_indices.get(target).copied() else {
                        return false;
                    };
                    if target_index <= index {
                        return false;
                    }
                    if function.blocks[target_index].params.len() != args.len() {
                        return false;
                    }
                }
                IrTerminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    let Some(then_index) = block_indices.get(then_block).copied() else {
                        return false;
                    };
                    let Some(else_index) = block_indices.get(else_block).copied() else {
                        return false;
                    };
                    if then_index != index + 1 || else_index <= index {
                        return false;
                    }
                    if !function.blocks[then_index].params.is_empty()
                        || !function.blocks[else_index].params.is_empty()
                    {
                        return false;
                    }
                }
                IrTerminator::Unreachable(_) => return false,
            }
        }

        matches!(
            function.blocks.last().map(|block| &block.terminator),
            Some(IrTerminator::Return(..) | IrTerminator::TailCall { .. })
        )
    }

    #[allow(dead_code)]
    pub(super) fn compile_ir_block(&mut self, block: &IrStructuredBlock) -> CompileResult<()> {
        let block_for_counts = ir_structured_block_to_block(block, &[]);
        let mut consumable_counts = HashMap::new();
        for statement in &block_for_counts.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            let empty_program = Self::empty_ir_program();
            for item in &block.statements {
                compiler.compile_ir_top_level_item(item, &empty_program)?;
            }
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub(super) fn compile_ir_block_with_tail(
        &mut self,
        block: &IrStructuredBlock,
    ) -> CompileResult<()> {
        let mut errors = self
            .compile_ir_block_with_tail_collect_errors(block)
            .into_iter();
        if let Some(first) = errors.next() {
            for err in errors {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
            return Err(first);
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub(super) fn compile_ir_block_has_value_tail(&self, block: &IrStructuredBlock) -> bool {
        matches!(
            block.statements.last(),
            Some(IrTopLevelItem::Expression {
                has_semicolon: false,
                ..
            })
        )
    }

    #[allow(clippy::vec_box)]
    fn compile_ir_block_with_tail_collect_errors(
        &mut self,
        block: &IrStructuredBlock,
    ) -> Vec<Box<Diagnostic>> {
        self.compile_ir_block_with_tail_collect_errors_for_spans(block, None)
    }

    #[allow(clippy::vec_box)]
    fn compile_ir_block_with_tail_collect_errors_for_spans(
        &mut self,
        block: &IrStructuredBlock,
        tail_call_spans_override: Option<&[Span]>,
    ) -> Vec<Box<Diagnostic>> {
        let len = block.statements.len();
        let block_for_counts = ir_structured_block_to_block(block, &[]);
        let mut errors = Vec::new();
        let mut consumable_counts = HashMap::new();
        for statement in &block_for_counts.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        let owned_tail_call_spans;
        let tail_call_spans: &[Span] = if let Some(spans) = tail_call_spans_override {
            spans
        } else {
            owned_tail_call_spans = self
                .tail_calls
                .iter()
                .map(|tail_call| tail_call.span)
                .collect::<Vec<_>>();
            &owned_tail_call_spans
        };

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            let empty_program = Self::empty_ir_program();
            for (i, item) in block.statements.iter().enumerate() {
                let is_last = i == len.saturating_sub(1);
                let structural_tail_eligible = matches!(
                    item,
                    IrTopLevelItem::Expression {
                        has_semicolon: false,
                        ..
                    } | IrTopLevelItem::Return { .. }
                );
                let ir_tail_eligible = compiler.analyze_enabled
                    && compiler.ir_top_level_item_contains_tail_call(item, tail_call_spans);
                let tail_eligible =
                    structural_tail_eligible && (!compiler.analyze_enabled || ir_tail_eligible);

                let result = if is_last && tail_eligible {
                    compiler.with_tail_position(true, |compiler| {
                        compiler.compile_ir_top_level_item(item, &empty_program)
                    })
                } else {
                    compiler.with_tail_position(false, |compiler| {
                        compiler.compile_ir_top_level_item(item, &empty_program)
                    })
                };

                if let Err(err) = result {
                    errors.push(err);
                }
            }
        });

        errors
    }

    fn compile_ir_let_item(
        &mut self,
        name: Symbol,
        type_annotation: &Option<TypeExpr>,
        value: &IrStructuredExpr,
        span: Span,
    ) -> CompileResult<()> {
        // Check for duplicate in current scope FIRST (takes precedence)
        if let Some(existing) = self.symbol_table.resolve(name)
            && self.symbol_table.exists_in_current_scope(name)
            && existing.symbol_scope != SymbolScope::Base
        {
            let name_str = self.sym(name);
            return Err(Self::boxed(self.make_redeclaration_error(
                name_str,
                span,
                Some(existing.span),
                None,
            )));
        }
        // Then check for import collision (only if not a duplicate in same scope)
        if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
            let name_str = self.sym(name);
            return Err(Self::boxed(
                self.make_import_collision_error(name_str, span),
            ));
        }

        let symbol = self.symbol_table.define(name, span);

        if let Some(annotation) = type_annotation {
            if let Some(expected_infer) =
                TypeEnv::infer_type_from_type_expr(annotation, &Default::default(), &self.interner)
            {
                if let Some((expected_str, actual_str)) =
                    self.known_concrete_ir_expr_type_mismatch(&expected_infer, value)
                {
                    let name_str = self.sym(name).to_string();
                    return Err(Self::boxed(let_annotation_type_mismatch(
                        self.file_path.clone(),
                        annotation.span(),
                        value.span(),
                        &name_str,
                        &expected_str,
                        &actual_str,
                    )));
                }
                self.validate_ir_expr_expected_type_with_policy(
                    &expected_infer,
                    value,
                    "initializer type is known at compile time",
                    "binding initializer does not match type annotation".to_string(),
                    "typed let initializer",
                    true,
                )?;
            } else if self.strict_mode {
                return Err(Self::boxed(self.unresolved_boundary_error_for_span(
                    value.span(),
                    "typed let initializer",
                )));
            }
        }

        self.compile_ir_expr(value)?;

        if let Some(annotation) = type_annotation
            && let Some(ty) = convert_type_expr(annotation, &self.interner)
        {
            self.bind_static_type(name, ty);
        } else if let HmExprTypeResult::Known(inferred) = self.hm_ir_expr_type_strict_path(value) {
            let runtime = TypeEnv::to_runtime(&inferred, &Default::default());
            if runtime != crate::runtime::runtime_type::RuntimeType::Any {
                self.bind_static_type(name, runtime);
            }
        }

        // Track aliases of effectful base functions so indirect calls
        // like `let p = print; p(...)` keep static effect checking.
        self.track_effect_alias_for_ir_binding(name, value);

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => {
                return Err(Self::boxed(Diagnostic::make_error(
                    &ICE_SYMBOL_SCOPE_LET,
                    &[],
                    self.file_path.clone(),
                    span,
                )));
            }
        };

        self.symbol_table.mark_assigned(name).ok();
        if self.scope_index == 0 {
            self.file_scope_symbols.insert(name);
        }
        Ok(())
    }

    fn compile_ir_let_destructure_item(
        &mut self,
        pattern: &crate::ir::IrStructuredPattern,
        value: &IrStructuredExpr,
        _span: Span,
    ) -> CompileResult<()> {
        let ast_pattern = ir_structured_pattern_to_pattern(pattern);
        if let Some(expected_infer) = self.destructure_pattern_expected_type(&ast_pattern) {
            self.validate_ir_expr_expected_type_with_policy(
                &expected_infer,
                value,
                "destructure source type is known at compile time",
                "destructure source does not match pattern shape".to_string(),
                "tuple destructure source",
                true,
            )?;
        }
        self.compile_ir_expr(value)?;
        let temp_symbol = self.symbol_table.define_temp();
        match temp_symbol.symbol_scope {
            SymbolScope::Global => {
                self.emit(OpCode::OpSetGlobal, &[temp_symbol.index]);
            }
            SymbolScope::Local => {
                self.emit(OpCode::OpSetLocal, &[temp_symbol.index]);
            }
            _ => {
                return Err(Self::boxed(Diagnostic::make_error(
                    &ICE_SYMBOL_SCOPE_LET,
                    &[],
                    self.file_path.clone(),
                    Span::new(Position::default(), Position::default()),
                )));
            }
        }
        self.compile_pattern_bind(&temp_symbol, &ast_pattern)?;
        Ok(())
    }

    fn compile_ir_return_item(
        &mut self,
        value: &Option<IrStructuredExpr>,
        _span: Span,
    ) -> CompileResult<()> {
        match value {
            Some(expr) => {
                self.compile_ir_expr(expr)?;
                if !self.is_last_instruction(OpCode::OpTailCall) {
                    self.emit(OpCode::OpReturnValue, &[]);
                }
            }
            None => {
                self.emit(OpCode::OpReturn, &[]);
            }
        }
        Ok(())
    }

    fn compile_ir_assign_item(
        &mut self,
        name: Symbol,
        value: &crate::ir::IrStructuredExpr,
        span: Span,
    ) -> CompileResult<()> {
        self.compile_statement(&Statement::Assign {
            name,
            value: ir_structured_expr_to_expression(value),
            span,
        })
    }

    fn compile_ir_import_item(
        &mut self,
        name: Symbol,
        alias: Option<Symbol>,
        except: &[Symbol],
        span: Span,
    ) -> CompileResult<()> {
        if self.scope_index > 0 {
            let name_str = self.sym(name);
            return Err(Self::boxed(Diagnostic::make_error(
                &IMPORT_SCOPE,
                &[name_str],
                self.file_path.clone(),
                span,
            )));
        }
        if self.is_base_module_symbol(name) {
            self.compile_import_statement(name, alias, except)?;
            return Ok(());
        }
        let name_str = self.sym(name).to_string();
        let alias_str = alias.map(|a| self.sym(a).to_string());
        let binding_name_str = import_binding_name(&name_str, alias_str.as_deref()).to_string();
        let binding_name = self.interner.intern(&binding_name_str);

        if self.file_scope_symbols.contains(&binding_name) {
            let binding_name_str = self.sym(binding_name);
            return Err(Self::boxed(
                self.make_import_collision_error(binding_name_str, span),
            ));
        }
        // Reserve the name for this file so later declarations can't collide.
        self.file_scope_symbols.insert(binding_name);
        self.compile_import_statement(name, alias, except)?;
        Ok(())
    }

    #[allow(dead_code)]
    fn try_compile_ir_cfg_function_body(
        &mut self,
        function: &IrFunction,
        current_name: Symbol,
    ) -> Option<CompileResult<()>> {
        if !Self::can_compile_ir_cfg_function(function) {
            return None;
        }

        let mut bindings: HashMap<IrVar, crate::bytecode::binding::Binding> = HashMap::new();
        for param in &function.params {
            let binding = self.symbol_table.resolve(param.name)?;
            bindings.insert(param.var, binding);
        }
        for block in &function.blocks {
            for param in &block.params {
                bindings
                    .entry(param.var)
                    .or_insert_with(|| self.symbol_table.define_temp());
            }
            for instr in &block.instrs {
                if let IrInstr::Assign { dest, .. } = instr {
                    bindings
                        .entry(*dest)
                        .or_insert_with(|| self.symbol_table.define_temp());
                }
            }
        }

        let block_map: HashMap<_, _> = function
            .blocks
            .iter()
            .map(|block| (block.id, block))
            .collect();

        Some((|| {
            let mut block_offsets = HashMap::<BlockId, usize>::new();
            let mut pending_jumps = Vec::<(usize, BlockId)>::new();
            let mut false_target_blocks = HashSet::<BlockId>::new();

            for block in &function.blocks {
                block_offsets.insert(block.id, self.current_instructions().len());
                if false_target_blocks.remove(&block.id) {
                    self.emit(OpCode::OpPop, &[]);
                }

                for instr in &block.instrs {
                    self.compile_ir_cfg_instr(instr, &bindings, current_name)?;
                }

                match &block.terminator {
                    IrTerminator::Return(..) | IrTerminator::TailCall { .. } => {
                        self.compile_ir_cfg_terminator(&block.terminator, &bindings, current_name)?;
                    }
                    IrTerminator::Jump(target, args, _) => {
                        let target_block = block_map.get(target).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode jump target block",
                            ))
                        })?;
                        self.compile_ir_cfg_jump_args(target_block, args, &bindings)?;
                        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
                        pending_jumps.push((jump_pos, *target));
                    }
                    IrTerminator::Branch {
                        cond, else_block, ..
                    } => {
                        let cond_binding = bindings.get(cond).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode branch condition binding",
                            ))
                        })?;
                        self.load_symbol(cond_binding);
                        let false_jump = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                        pending_jumps.push((false_jump, *else_block));
                        false_target_blocks.insert(*else_block);
                    }
                    IrTerminator::Unreachable(_) => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "unsupported unreachable CFG bytecode block",
                        )));
                    }
                }
            }

            for (jump_pos, target) in pending_jumps {
                let target_pos = block_offsets.get(&target).copied().ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode block offset"))
                })?;
                self.change_operand(jump_pos, target_pos);
            }

            Ok(())
        })())
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_instr(
        &mut self,
        instr: &IrInstr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        match instr {
            IrInstr::Assign { dest, expr, .. } => {
                self.compile_ir_cfg_expr(expr, bindings)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR var",
                    ))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
            IrInstr::Call {
                dest, target, args, ..
            } => {
                self.compile_ir_cfg_call(target, args, bindings, current_name)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR call dest",
                    ))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_expr(
        &mut self,
        expr: &IrExpr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
    ) -> CompileResult<()> {
        match expr {
            IrExpr::Const(IrConst::Int(value)) => {
                self.emit_constant_value(Value::Integer(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::Float(value)) => {
                self.emit_constant_value(Value::Float(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::Bool(value)) => {
                self.emit_constant_value(Value::Boolean(*value));
                Ok(())
            }
            IrExpr::Const(IrConst::String(value)) => {
                self.emit_constant_value(Value::String(value.clone().into()));
                Ok(())
            }
            IrExpr::Const(IrConst::Unit) | IrExpr::None => {
                self.emit(OpCode::OpNone, &[]);
                Ok(())
            }
            IrExpr::Var(var) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode binding for IR var",
                    ))
                })?);
                Ok(())
            }
            IrExpr::TagTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tag-test binding"))
                })?);
                match tag {
                    crate::ir::IrTagTest::None => {
                        self.emit(OpCode::OpNone, &[]);
                        self.emit(OpCode::OpEqual, &[]);
                    }
                    crate::ir::IrTagTest::Some => {
                        self.emit(OpCode::OpIsSome, &[]);
                    }
                    crate::ir::IrTagTest::Left => {
                        self.emit(OpCode::OpIsLeft, &[]);
                    }
                    crate::ir::IrTagTest::Right => {
                        self.emit(OpCode::OpIsRight, &[]);
                    }
                }
                Ok(())
            }
            IrExpr::TupleArityTest { value, .. } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tuple-test binding",
                    ))
                })?);
                self.emit(OpCode::OpIsTuple, &[]);
                Ok(())
            }
            IrExpr::TupleFieldAccess { object, index } => {
                self.load_symbol(bindings.get(object).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tuple-field binding",
                    ))
                })?);
                self.emit(OpCode::OpTupleIndex, &[*index]);
                Ok(())
            }
            IrExpr::TagPayload { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode tag-payload binding",
                    ))
                })?);
                match tag {
                    crate::ir::IrTagTest::Some => self.emit(OpCode::OpUnwrapSome, &[]),
                    crate::ir::IrTagTest::Left => self.emit(OpCode::OpUnwrapLeft, &[]),
                    crate::ir::IrTagTest::Right => self.emit(OpCode::OpUnwrapRight, &[]),
                    crate::ir::IrTagTest::None => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "invalid CFG bytecode None payload lowering",
                        )));
                    }
                };
                Ok(())
            }
            IrExpr::ListTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-test binding",
                    ))
                })?);
                match tag {
                    crate::ir::IrListTest::Empty => self.emit(OpCode::OpIsEmptyList, &[]),
                    crate::ir::IrListTest::Cons => self.emit(OpCode::OpIsCons, &[]),
                };
                Ok(())
            }
            IrExpr::ListHead { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-head binding",
                    ))
                })?);
                self.emit(OpCode::OpConsHead, &[]);
                Ok(())
            }
            IrExpr::ListTail { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode list-tail binding",
                    ))
                })?);
                self.emit(OpCode::OpConsTail, &[]);
                Ok(())
            }
            IrExpr::AdtTagTest { value, constructor } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode adt-tag binding"))
                })?);
                let const_idx =
                    self.add_constant(Value::String(self.sym(*constructor).to_string().into()));
                let jump_to_false = self.emit(OpCode::OpIsAdtJump, &[const_idx, 9999]);
                self.emit(OpCode::OpPop, &[]);
                self.emit(OpCode::OpTrue, &[]);
                let jump_to_end = self.emit(OpCode::OpJump, &[9999]);
                let false_pos = self.current_instructions().len();
                self.replace_instruction(
                    jump_to_false,
                    make(OpCode::OpIsAdtJump, &[const_idx, false_pos]),
                );
                self.emit(OpCode::OpPop, &[]);
                self.emit(OpCode::OpFalse, &[]);
                let end_pos = self.current_instructions().len();
                self.change_operand(jump_to_end, end_pos);
                Ok(())
            }
            IrExpr::AdtField { value, index } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode adt-field binding",
                    ))
                })?);
                self.emit(OpCode::OpAdtField, &[*index]);
                Ok(())
            }
            IrExpr::Binary(op, lhs, rhs) => {
                let lhs_binding = bindings.get(lhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode lhs binding"))
                })?;
                let rhs_binding = bindings.get(rhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode rhs binding"))
                })?;
                if matches!(op, crate::ir::IrBinaryOp::Lt) {
                    self.load_symbol(rhs_binding);
                    self.load_symbol(lhs_binding);
                } else {
                    self.load_symbol(lhs_binding);
                    self.load_symbol(rhs_binding);
                }
                match op {
                    crate::ir::IrBinaryOp::Add => self.emit(OpCode::OpAdd, &[]),
                    crate::ir::IrBinaryOp::Sub => self.emit(OpCode::OpSub, &[]),
                    crate::ir::IrBinaryOp::Mul => self.emit(OpCode::OpMul, &[]),
                    crate::ir::IrBinaryOp::Div => self.emit(OpCode::OpDiv, &[]),
                    crate::ir::IrBinaryOp::Eq => self.emit(OpCode::OpEqual, &[]),
                    crate::ir::IrBinaryOp::NotEq => self.emit(OpCode::OpNotEqual, &[]),
                    crate::ir::IrBinaryOp::Lt => self.emit(OpCode::OpGreaterThan, &[]),
                    crate::ir::IrBinaryOp::Gt => self.emit(OpCode::OpGreaterThan, &[]),
                    crate::ir::IrBinaryOp::Ge => self.emit(OpCode::OpGreaterThanOrEqual, &[]),
                    crate::ir::IrBinaryOp::Le => self.emit(OpCode::OpLessThanOrEqual, &[]),
                    _ => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "unsupported CFG bytecode binary op",
                        )));
                    }
                };
                Ok(())
            }
            _ => Err(Self::boxed(Diagnostic::warning(
                "unsupported CFG bytecode expression lowering",
            ))),
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_jump_args(
        &mut self,
        target: &IrBlock,
        args: &[IrVar],
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
    ) -> CompileResult<()> {
        for (param, arg) in target.params.iter().zip(args) {
            self.load_symbol(bindings.get(arg).ok_or_else(|| {
                Self::boxed(Diagnostic::warning("missing CFG bytecode jump arg binding"))
            })?);
            self.emit_store_binding(bindings.get(&param.var).ok_or_else(|| {
                Self::boxed(Diagnostic::warning(
                    "missing CFG bytecode block param binding",
                ))
            })?);
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_terminator(
        &mut self,
        terminator: &IrTerminator,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        match terminator {
            IrTerminator::Return(var, _) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode return binding"))
                })?);
                self.emit(OpCode::OpReturnValue, &[]);
                Ok(())
            }
            IrTerminator::TailCall { callee, args, .. } => {
                let is_self = matches!(callee, IrCallTarget::Named(name) if *name == current_name);
                if !is_self {
                    match callee {
                        IrCallTarget::Named(name) => {
                            let symbol = self.symbol_table.resolve(*name).ok_or_else(|| {
                                Self::boxed(Diagnostic::warning(
                                    "missing CFG bytecode tail-call target binding",
                                ))
                            })?;
                            self.load_symbol(&symbol);
                        }
                        IrCallTarget::Var(var) => {
                            self.load_symbol(bindings.get(var).ok_or_else(|| {
                                Self::boxed(Diagnostic::warning(
                                    "missing CFG bytecode tail-call callee binding",
                                ))
                            })?);
                        }
                        IrCallTarget::Direct(_) => {
                            let function_id = match callee {
                                IrCallTarget::Direct(id) => id,
                                _ => unreachable!(),
                            };
                            let symbol = self
                                .ir_function_symbols
                                .get(function_id)
                                .and_then(|symbol| self.symbol_table.resolve(*symbol))
                                .ok_or_else(|| {
                                    Self::boxed(Diagnostic::warning(
                                        "missing direct CFG bytecode tail-call target binding",
                                    ))
                                })?;
                            self.load_symbol(&symbol);
                        }
                    }
                }
                for arg in args {
                    self.load_symbol(bindings.get(arg).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode tail-call arg binding",
                        ))
                    })?);
                }
                if is_self {
                    self.emit(OpCode::OpTailCall, &[args.len()]);
                } else {
                    self.emit(OpCode::OpCall, &[args.len()]);
                    self.emit(OpCode::OpReturnValue, &[]);
                }
                Ok(())
            }
            _ => Err(Self::boxed(Diagnostic::warning(
                "unsupported CFG bytecode terminator lowering",
            ))),
        }
    }

    #[allow(dead_code)]
    fn compile_ir_cfg_call(
        &mut self,
        target: &IrCallTarget,
        args: &[IrVar],
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        let is_self = matches!(target, IrCallTarget::Named(name) if *name == current_name);
        if !is_self {
            match target {
                IrCallTarget::Named(name) => {
                    let symbol = self.symbol_table.resolve(*name).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode call target binding",
                        ))
                    })?;
                    self.load_symbol(&symbol);
                }
                IrCallTarget::Var(var) => {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode call callee binding",
                        ))
                    })?);
                }
                IrCallTarget::Direct(_) => {
                    let function_id = match target {
                        IrCallTarget::Direct(id) => id,
                        _ => unreachable!(),
                    };
                    let symbol = self
                        .ir_function_symbols
                        .get(function_id)
                        .and_then(|symbol| self.symbol_table.resolve(*symbol))
                        .ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing direct CFG bytecode call target binding",
                            ))
                        })?;
                    self.load_symbol(&symbol);
                }
            }
        }

        for arg in args {
            self.load_symbol(bindings.get(arg).ok_or_else(|| {
                Self::boxed(Diagnostic::warning("missing CFG bytecode call arg binding"))
            })?);
        }

        if is_self {
            self.emit(OpCode::OpCallSelf, &[args.len()]);
        } else {
            self.emit(OpCode::OpCall, &[args.len()]);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_ir_function_statement(
        &mut self,
        name: Symbol,
        parameters: &[Symbol],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &IrStructuredBlock,
        ir_function: Option<&IrFunction>,
        position: Position,
    ) -> CompileResult<()> {
        let function_span = Span::new(position, position);
        let function_tail_call_spans =
            ir_function.map(Self::collect_tail_call_spans_for_ir_function);

        if let Some(param) = Self::find_duplicate_name(parameters) {
            let param_str = self.sym(param);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param_str],
                self.file_path.clone(),
                function_span,
            )));
        }

        for effect_expr in effects {
            for effect_name in effect_expr.normalized_names() {
                if !self.is_known_function_effect_annotation(effect_name) {
                    let span =
                        Self::effect_named_span(effect_expr, effect_name).unwrap_or(function_span);
                    return Err(Self::boxed(
                        self.unknown_function_effect_diagnostic(effect_name, span),
                    ));
                }
            }
        }

        let symbol = if self.symbol_table.exists_in_current_scope(name) {
            self.symbol_table
                .resolve(name)
                .expect("current-scope function binding must resolve")
        } else {
            self.symbol_table.define(name, function_span)
        };

        self.enter_scope();
        self.symbol_table.define_function_name(name, function_span);

        for (index, param) in parameters.iter().enumerate() {
            self.symbol_table.define(*param, Span::default());
            if let Some(Some(param_ty)) = parameter_types.get(index)
                && let Some(runtime_ty) = convert_type_expr(param_ty, &self.interner)
            {
                self.bind_static_type(*param, runtime_ty);
            }
        }

        let compile_result: CompileResult<()> = (|| {
            if let Some(ret_annotation) = return_type
                && let Some(expected_ret) = TypeEnv::infer_type_from_type_expr(
                    ret_annotation,
                    &Default::default(),
                    &self.interner,
                )
                && self.compile_ir_block_has_value_tail(body)
                && let Some(IrTopLevelItem::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                }) = body.statements.last()
            {
                let expression = ir_structured_expr_to_expression(expression);
                if let Some((expected_str, actual_str)) =
                    self.known_concrete_expr_type_mismatch(&expected_ret, &expression)
                {
                    let fn_name = self.sym(name).to_string();
                    return Err(Self::boxed(fun_return_annotation_mismatch(
                        self.file_path.clone(),
                        ret_annotation.span(),
                        expression.span(),
                        &fn_name,
                        &expected_str,
                        &actual_str,
                    )));
                }
                self.validate_expr_expected_type_with_policy(
                    &expected_ret,
                    &expression,
                    "return expression type is known at compile time",
                    "return expression type does not match the declared return type".to_string(),
                    "function return expression",
                    true,
                )?;
            }

            let param_effect_rows = self.build_param_effect_rows(parameters, parameter_types);
            // The structured path handles all validation (immutability, scope checks, type
            // annotations, effects) and correctly emits OpTailCall when in_tail_position is
            // propagated. The CFG path is kept for future use but not activated here.
            let _ = ir_function;
            {
                let body_errors = self.with_function_context_with_param_effect_rows(
                    parameters.len(),
                    effects,
                    param_effect_rows,
                    |compiler| {
                        compiler.compile_ir_block_with_tail_collect_errors_for_spans(
                            body,
                            function_tail_call_spans.as_deref(),
                        )
                    },
                );
                if body_errors.is_empty() {
                    if self.compile_ir_block_has_value_tail(body) {
                        if self.is_last_instruction(OpCode::OpPop) {
                            self.replace_last_pop_with_return();
                        } else if self.replace_last_local_read_with_return() {
                        } else if !self.is_last_instruction(OpCode::OpReturnValue)
                            && !self.is_last_instruction(OpCode::OpReturnLocal)
                        {
                            self.emit(OpCode::OpReturnValue, &[]);
                        }
                    } else if !self.is_last_instruction(OpCode::OpReturnValue)
                        && !self.is_last_instruction(OpCode::OpReturnLocal)
                    {
                        self.emit(OpCode::OpReturn, &[]);
                    }
                }
                for err in body_errors {
                    let mut diag = *err;
                    if diag.phase().is_none() {
                        diag.phase = Some(DiagnosticPhase::TypeCheck);
                    }
                    self.errors.push(diag);
                }
            }
            Ok(())
        })();

        let free_symbols = self.symbol_table.free_symbols.clone();
        for free in &free_symbols {
            if free.symbol_scope == SymbolScope::Local {
                self.mark_captured_in_current_function(free.index);
            }
        }

        let num_locals = self.symbol_table.num_definitions;

        let (instructions, locations, files, effect_summary) = self.leave_scope();

        compile_result?;

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let runtime_contract = {
            let contract = crate::bytecode::compiler::contracts::FnContract {
                type_params: vec![],
                params: parameter_types.to_vec(),
                ret: return_type.clone(),
                effects: effects.to_vec(),
            };
            self.to_runtime_contract(&contract)
        };

        let fn_idx = self.add_constant(Value::Function(Rc::new(
            CompiledFunction::new(
                instructions,
                num_locals,
                parameters.len(),
                Some(
                    FunctionDebugInfo::new(Some(self.sym(name).to_string()), files, locations)
                        .with_effect_summary(effect_summary),
                ),
            )
            .with_contract(runtime_contract),
        )));
        self.emit_closure_index(fn_idx, free_symbols.len());

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => 0,
        };
        Ok(())
    }

    pub(super) fn compile_ir_module_statement(
        &mut self,
        name: Symbol,
        body: &IrStructuredBlock,
        ir_program: &IrProgram,
        position: Position,
    ) -> CompileResult<()> {
        let name_str = self.sym(name);
        if !is_valid_module_name(name_str) {
            let name_str = self.sym(name);
            return Err(Self::boxed(Diagnostic::make_error(
                &INVALID_MODULE_NAME,
                &[name_str],
                self.file_path.clone(),
                Span::new(position, position),
            )));
        }
        let binding_name_str = module_binding_name(name_str).to_string();
        let binding_name = self.interner.intern(&binding_name_str);
        if self.symbol_table.exists_in_current_scope(binding_name) {
            let binding_name_str = self.sym(binding_name);
            return Err(Self::boxed(self.make_redeclaration_error(
                binding_name_str,
                Span::new(position, position),
                None,
                None,
            )));
        }

        for item in &body.statements {
            match item {
                IrTopLevelItem::Function { name: fn_name, .. } => {
                    if *fn_name == binding_name {
                        let pos = Self::ir_top_level_item_span(item).start;
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NAME_CLASH,
                            &[binding_name_str],
                            self.file_path.clone(),
                            Span::new(pos, pos),
                        )));
                    }
                }
                IrTopLevelItem::Let { .. } | IrTopLevelItem::Data { .. } => {}
                _ => {
                    let pos = Self::ir_top_level_item_span(item).start;
                    return Err(Self::boxed(Diagnostic::make_error(
                        &INVALID_MODULE_CONTENT,
                        &[],
                        self.file_path.clone(),
                        Span::new(pos, pos),
                    )));
                }
            }
        }

        self.imported_modules.insert(binding_name);
        let previous_module = self.current_module_prefix;
        self.current_module_prefix = Some(binding_name);

        let module_body = ir_structured_block_to_block(body, &[]);
        let constants =
            match compile_module_constants(&module_body, binding_name, &mut self.interner) {
                Ok(result) => result,
                Err(err) => {
                    self.current_module_prefix = previous_module;
                    return Err(Self::boxed(self.convert_const_compile_error(err, position)));
                }
            };
        self.module_constants.extend(constants);

        for item in &body.statements {
            if let IrTopLevelItem::Function {
                name: fn_name,
                span,
                ..
            } = item
            {
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                if let Some(existing) = self.symbol_table.resolve(qualified_name)
                    && self.symbol_table.exists_in_current_scope(qualified_name)
                {
                    self.current_module_prefix = previous_module;
                    let qualified_name_str = self.sym(qualified_name);
                    return Err(Self::boxed(self.make_redeclaration_error(
                        qualified_name_str,
                        *span,
                        Some(existing.span),
                        Some("Use a different name or remove the previous definition"),
                    )));
                }
                self.symbol_table.define(qualified_name, *span);
            }
        }

        for item in &body.statements {
            if let IrTopLevelItem::Function {
                name: fn_name,
                function_id,
                parameters,
                parameter_types,
                return_type,
                effects,
                body: fn_body,
                span,
                ..
            } = item
            {
                let position = span.start;
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                    if effects.is_empty() {
                        self.lookup_contract(Some(binding_name), *fn_name, parameters.len())
                            .map(|contract| contract.effects.clone())
                            .unwrap_or_default()
                    } else {
                        effects.clone()
                    };
                if let Err(err) = self.compile_ir_function_statement(
                    qualified_name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    fn_body,
                    function_id.and_then(|id| ir_program.function(id)),
                    position,
                ) {
                    let mut diag = *err;
                    if diag.phase().is_none() {
                        diag.phase = Some(DiagnosticPhase::TypeCheck);
                    }
                    self.errors.push(diag);
                }
            }
        }

        self.current_module_prefix = previous_module;

        Ok(())
    }

    pub(super) fn compile_ir_top_level_item(
        &mut self,
        item: &IrTopLevelItem,
        ir_program: &IrProgram,
    ) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(Self::ir_top_level_item_span(item));
        let result = (|| match item {
            IrTopLevelItem::Expression {
                expression,
                has_semicolon,
                ..
            } => {
                self.compile_ir_expr(expression)?;
                if !self.in_tail_position || *has_semicolon {
                    self.emit(OpCode::OpPop, &[]);
                }
                Ok(())
            }
            IrTopLevelItem::Let {
                name,
                type_annotation,
                value,
                span,
            } => self.compile_ir_let_item(*name, type_annotation, value, *span),
            IrTopLevelItem::LetDestructure {
                pattern,
                value,
                span,
            } => self.compile_ir_let_destructure_item(pattern, value, *span),
            IrTopLevelItem::Assign { name, value, span } => {
                self.compile_ir_assign_item(*name, value, *span)
            }
            IrTopLevelItem::Return { value, span } => self.compile_ir_return_item(value, *span),
            IrTopLevelItem::Function {
                name,
                function_id,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } => {
                let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                    if effects.is_empty() {
                        self.lookup_unqualified_contract(*name, parameters.len())
                            .map(|contract| contract.effects.clone())
                            .unwrap_or_default()
                    } else {
                        effects.clone()
                    };
                self.compile_ir_function_statement(
                    *name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    body,
                    function_id.and_then(|id| ir_program.function(id)),
                    span.start,
                )?;
                if self.scope_index == 0 {
                    self.file_scope_symbols.insert(*name);
                }
                Ok(())
            }
            IrTopLevelItem::Module { name, body, span } => {
                self.compile_ir_module_statement(*name, body, ir_program, span.start)
            }
            IrTopLevelItem::Import {
                name,
                alias,
                except,
                span,
            } => self.compile_ir_import_item(*name, *alias, except, *span),
            IrTopLevelItem::Data { name, variants, .. } => {
                self.compile_data_statement(*name, variants)
            }
            IrTopLevelItem::EffectDecl { .. } => Ok(()),
        })();
        self.current_span = previous_span;
        result
    }

    fn destructure_pattern_expected_type(&mut self, pattern: &Pattern) -> Option<InferType> {
        match pattern {
            Pattern::Tuple { elements, .. } => Some(InferType::Tuple(
                elements
                    .iter()
                    .map(|elem| {
                        self.destructure_pattern_expected_type(elem)
                            .unwrap_or_else(|| self.type_env.alloc_infer_type_var())
                    })
                    .collect(),
            )),
            Pattern::Some { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Option,
                vec![
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                ],
            )),
            Pattern::None { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Option,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Left { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Either,
                vec![
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                    self.type_env.alloc_infer_type_var(),
                ],
            )),
            Pattern::Right { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Either,
                vec![
                    self.type_env.alloc_infer_type_var(),
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                ],
            )),
            Pattern::EmptyList { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::List,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Cons { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::List,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Identifier { .. } | Pattern::Wildcard { .. } => {
                Some(self.type_env.alloc_infer_type_var())
            }
            Pattern::Literal { .. } | Pattern::Constructor { .. } => None,
        }
    }

    fn known_concrete_ir_expr_type_mismatch(
        &self,
        expected: &InferType,
        expr: &IrStructuredExpr,
    ) -> Option<(String, String)> {
        let HmExprTypeResult::Known(actual) = self.hm_ir_expr_type_strict_path(expr) else {
            return None;
        };
        if !expected.is_concrete()
            || !actual.is_concrete()
            || expected.contains_any()
            || actual.contains_any()
        {
            return None;
        }

        let compatible = if let Ok(subst) = unify(expected, &actual) {
            let resolved_expected = expected.apply_type_subst(&subst);
            let resolved_actual = actual.apply_type_subst(&subst);
            resolved_expected == resolved_actual
        } else {
            false
        };
        if compatible {
            return None;
        }

        Some((
            display_infer_type(expected, &self.interner),
            display_infer_type(&actual, &self.interner),
        ))
    }

    fn known_concrete_expr_type_mismatch(
        &self,
        expected: &InferType,
        expression: &crate::syntax::expression::Expression,
    ) -> Option<(String, String)> {
        let HmExprTypeResult::Known(actual) = self.hm_expr_type_strict_path(expression) else {
            return None;
        };
        if !expected.is_concrete()
            || !actual.is_concrete()
            || expected.contains_any()
            || actual.contains_any()
        {
            return None;
        }

        let compatible = if let Ok(subst) = unify(expected, &actual) {
            let resolved_expected = expected.apply_type_subst(&subst);
            let resolved_actual = actual.apply_type_subst(&subst);
            resolved_expected == resolved_actual
        } else {
            false
        };
        if compatible {
            return None;
        }

        Some((
            display_infer_type(expected, &self.interner),
            display_infer_type(&actual, &self.interner),
        ))
    }

    fn is_known_function_effect_annotation(&self, effect: Symbol) -> bool {
        if self.effect_declared_ops(effect).is_some() {
            return true;
        }
        matches!(self.sym(effect), "IO" | "Time" | "State")
    }

    fn effect_named_span(effect: &EffectExpr, target: Symbol) -> Option<Span> {
        match effect {
            EffectExpr::Named { name, span } => (*name == target).then_some(*span),
            EffectExpr::RowVar { name, span } => (*name == target).then_some(*span),
            EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
                Self::effect_named_span(left, target)
                    .or_else(|| Self::effect_named_span(right, target))
            }
        }
    }

    fn unknown_function_effect_diagnostic(&self, effect: Symbol, span: Span) -> Diagnostic {
        let effect_name = self.sym(effect).to_string();
        let hint = suggest_effect_name(&effect_name).unwrap_or_else(|| {
            "Use a declared effect name in `with ...` or declare the effect first.".to_string()
        });
        Diagnostic::make_error_dynamic(
            "E407",
            "UNKNOWN FUNCTION EFFECT",
            ErrorType::Compiler,
            format!(
                "Function effect annotation references unknown effect `{}`.",
                effect_name
            ),
            Some(hint),
            self.file_path.clone(),
            span,
        )
        .with_display_title("Unknown Effect")
        .with_category(crate::diagnostics::DiagnosticCategory::Effects)
        .with_primary_label(span, "unknown effect in function annotation")
        .with_phase(DiagnosticPhase::Effect)
    }

    fn compile_statement_collect_error(
        &mut self,
        statement: &Statement,
    ) -> Option<Box<Diagnostic>> {
        self.compile_statement(statement).err()
    }

    pub(super) fn compile_statement(&mut self, statement: &Statement) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(statement.span());
        let result = (|| {
            match statement {
                Statement::Expression {
                    expression,
                    has_semicolon,
                    ..
                } => {
                    self.compile_expression(expression)?;
                    if !self.in_tail_position || *has_semicolon {
                        self.emit(OpCode::OpPop, &[]);
                    }
                }
                Statement::Let {
                    name,
                    type_annotation,
                    value,
                    span,
                    ..
                } => {
                    let name = *name;
                    // Check for duplicate in current scope FIRST (takes precedence)
                    if let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                        && existing.symbol_scope != SymbolScope::Base
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            None,
                        )));
                    }
                    // Then check for import collision (only if not a duplicate in same scope)
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(name_str, *span),
                        ));
                    }

                    let symbol = self.symbol_table.define(name, *span);
                    if let Some(annotation) = type_annotation {
                        if let Some(expected_infer) = TypeEnv::infer_type_from_type_expr(
                            annotation,
                            &Default::default(),
                            &self.interner,
                        ) {
                            if let Some((expected_str, actual_str)) =
                                self.known_concrete_expr_type_mismatch(&expected_infer, value)
                            {
                                let name_str = self.sym(name).to_string();
                                return Err(Self::boxed(let_annotation_type_mismatch(
                                    self.file_path.clone(),
                                    annotation.span(),
                                    value.span(),
                                    &name_str,
                                    &expected_str,
                                    &actual_str,
                                )));
                            }
                            self.validate_expr_expected_type_with_policy(
                                &expected_infer,
                                value,
                                "initializer type is known at compile time",
                                "binding initializer does not match type annotation".to_string(),
                                "typed let initializer",
                                true,
                            )?;
                        } else if self.strict_mode {
                            return Err(Self::boxed(
                                self.unresolved_boundary_error(value, "typed let initializer"),
                            ));
                        }
                    }
                    self.compile_expression(value)?;
                    if let Some(annotation) = type_annotation
                        && let Some(ty) = convert_type_expr(annotation, &self.interner)
                    {
                        self.bind_static_type(name, ty);
                    } else if let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(inferred) =
                        self.hm_expr_type_strict_path(value)
                    {
                        let runtime = TypeEnv::to_runtime(&inferred, &Default::default());
                        if runtime != crate::runtime::runtime_type::RuntimeType::Any {
                            self.bind_static_type(name, runtime);
                        }
                    }

                    // Track aliases of effectful base functions so indirect calls
                    // like `let p = print; p(...)` keep static effect checking.
                    self.track_effect_alias_for_binding(name, value);

                    match symbol.symbol_scope {
                        SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                        SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_SYMBOL_SCOPE_LET,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    };

                    self.symbol_table.mark_assigned(name).ok();
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::LetDestructure { pattern, value, .. } => {
                    if let Some(expected_infer) = self.destructure_pattern_expected_type(pattern) {
                        self.validate_expr_expected_type_with_policy(
                            &expected_infer,
                            value,
                            "destructure source type is known at compile time",
                            "destructure source does not match pattern shape".to_string(),
                            "tuple destructure source",
                            true,
                        )?;
                    }
                    self.compile_expression(value)?;
                    let temp_symbol = self.symbol_table.define_temp();
                    match temp_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[temp_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[temp_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_SYMBOL_SCOPE_LET,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&temp_symbol, pattern)?;
                }
                Statement::Assign { name, span, .. } => {
                    let name = *name;
                    // Check if variable exists
                    let name_str = self.sym(name).to_string();
                    let symbol = self.resolve_visible_symbol(name).ok_or_else(|| {
                        Self::boxed(self.make_undefined_variable_error(&name_str, *span))
                    })?;

                    if symbol.symbol_scope == SymbolScope::Free {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_outer_assignment_error(name_str, *span),
                        ));
                    }

                    // Flux bindings are immutable today: assignment syntax is parsed so we can
                    // emit a targeted diagnostic, but reassignment is not allowed.
                    let name_str = self.sym(name);
                    return Err(Self::boxed(self.make_immutability_error(name_str, *span)));
                }
                Statement::Return { value, .. } => match value {
                    Some(expr) => {
                        self.compile_expression(expr)?;
                        // Don't emit OpReturnValue if we just emitted OpTailCall
                        // because the tail call will loop back instead of returning
                        if !self.is_last_instruction(OpCode::OpTailCall) {
                            self.emit(OpCode::OpReturnValue, &[]);
                        }
                    }
                    None => {
                        self.emit(OpCode::OpReturn, &[]);
                    }
                },
                Statement::Function {
                    name,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                    span,
                    ..
                } => {
                    let name = *name;
                    // For top-level functions, checks were already done in pass 1
                    // Only check for nested functions (scope_index > 0)
                    if self.scope_index > 0
                        && let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                        && existing.symbol_scope != SymbolScope::Base
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            Some("Use a different name or remove the previous definition"),
                        )));
                    }
                    let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                        if effects.is_empty() {
                            self.lookup_unqualified_contract(name, parameters.len())
                                .map(|contract| contract.effects.clone())
                                .unwrap_or_default()
                        } else {
                            effects.clone()
                        };
                    self.compile_function_statement(
                        name,
                        parameters,
                        parameter_types,
                        return_type,
                        &effective_effects,
                        body,
                        span.start,
                    )?;
                    // For nested functions, add to file_scope_symbols
                    if self.scope_index == 0 {
                        // Already added in pass 1 for top-level functions
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::Module { name, body, span } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_SCOPE,
                            &[],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let name_str = self.sym(name).to_string();
                    let binding_name_str = module_binding_name(&name_str).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    let name_str = self.sym(name);
                    if !is_valid_module_name(name_str) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &INVALID_MODULE_NAME,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    self.compile_module_statement(name, body, span.start)?;
                    if self.scope_index == 0 {
                        let name_str = self.sym(name).to_string();
                        let binding_name_str = module_binding_name(&name_str).to_string();
                        let binding_name = self.interner.intern(&binding_name_str);
                        self.file_scope_symbols.insert(binding_name);
                    }
                }
                Statement::Import {
                    name,
                    alias,
                    except,
                    span,
                } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_SCOPE,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    if self.is_base_module_symbol(name) {
                        self.compile_import_statement(name, *alias, except)?;
                        return Ok(());
                    }
                    let name_str = self.sym(name).to_string();
                    let alias_str = alias.map(|a| self.sym(a).to_string());
                    let binding_name_str =
                        import_binding_name(&name_str, alias_str.as_deref()).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);

                    if self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    // Reserve the name for this file so later declarations can't collide.
                    self.file_scope_symbols.insert(binding_name);
                    self.compile_import_statement(name, *alias, except)?;
                }
                Statement::Data { name, variants, .. } => {
                    self.compile_data_statement(*name, variants)?;
                }
                // Effect declarations are syntax only for now no bytecode emitted.
                Statement::EffectDecl { .. } => {}
            }
            Ok(())
        })();
        self.current_span = previous_span;
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_function_statement(
        &mut self,
        name: Symbol,
        parameters: &[Symbol],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        let function_span = Span::new(position, position);

        if let Some(param) = Self::find_duplicate_name(parameters) {
            let param_str = self.sym(param);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param_str],
                self.file_path.clone(),
                function_span,
            )));
        }

        for effect_expr in effects {
            for effect_name in effect_expr.normalized_names() {
                if !self.is_known_function_effect_annotation(effect_name) {
                    let span =
                        Self::effect_named_span(effect_expr, effect_name).unwrap_or(function_span);
                    return Err(Self::boxed(
                        self.unknown_function_effect_diagnostic(effect_name, span),
                    ));
                }
            }
        }

        // Only reuse a current-scope predeclaration (for top-level/module pass 1).
        // If an outer or Base binding already exists, this nested function must
        // create a new local binding so it correctly shadows that name.
        let symbol = if self.symbol_table.exists_in_current_scope(name) {
            self.symbol_table
                .resolve(name)
                .expect("current-scope function binding must resolve")
        } else {
            self.symbol_table.define(name, function_span)
        };

        self.enter_scope();
        self.symbol_table.define_function_name(name, function_span);

        for (index, param) in parameters.iter().enumerate() {
            self.symbol_table.define(*param, Span::default());
            if let Some(Some(param_ty)) = parameter_types.get(index)
                && let Some(runtime_ty) = convert_type_expr(param_ty, &self.interner)
            {
                self.bind_static_type(*param, runtime_ty);
            }
        }

        let compile_result: CompileResult<()> = (|| {
            // Compile-time return type check: if the declared return type and
            // the static type of the tail expression are both known, verify
            // they're compatible.
            if let Some(ret_annotation) = return_type
                && let Some(expected_ret) = TypeEnv::infer_type_from_type_expr(
                    ret_annotation,
                    &Default::default(),
                    &self.interner,
                )
                && self.block_has_value_tail(body)
                && let Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                }) = body.statements.last()
            {
                if let Some((expected_str, actual_str)) =
                    self.known_concrete_expr_type_mismatch(&expected_ret, expression)
                {
                    let fn_name = self.sym(name).to_string();
                    return Err(Self::boxed(fun_return_annotation_mismatch(
                        self.file_path.clone(),
                        ret_annotation.span(),
                        expression.span(),
                        &fn_name,
                        &expected_str,
                        &actual_str,
                    )));
                }
                self.validate_expr_expected_type_with_policy(
                    &expected_ret,
                    expression,
                    "return expression type is known at compile time",
                    "return expression type does not match the declared return type".to_string(),
                    "function return expression",
                    true,
                )?;
            }

            let param_effect_rows = self.build_param_effect_rows(parameters, parameter_types);
            let body_errors = self.with_function_context_with_param_effect_rows(
                parameters.len(),
                effects,
                param_effect_rows,
                |compiler| compiler.compile_block_with_tail_collect_errors(body),
            );
            if body_errors.is_empty() {
                if self.block_has_value_tail(body) {
                    if self.is_last_instruction(OpCode::OpPop) {
                        self.replace_last_pop_with_return();
                    } else if self.replace_last_local_read_with_return() {
                    } else if !self.is_last_instruction(OpCode::OpReturnValue)
                        && !self.is_last_instruction(OpCode::OpReturnLocal)
                    {
                        self.emit(OpCode::OpReturnValue, &[]);
                    }
                } else if !self.is_last_instruction(OpCode::OpReturnValue)
                    && !self.is_last_instruction(OpCode::OpReturnLocal)
                {
                    self.emit(OpCode::OpReturn, &[]);
                }
            }
            for err in body_errors {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
            Ok(())
        })();

        let free_symbols = self.symbol_table.free_symbols.clone();
        for free in &free_symbols {
            if free.symbol_scope == SymbolScope::Local {
                self.mark_captured_in_current_function(free.index);
            }
        }

        let num_locals = self.symbol_table.num_definitions;

        let (instructions, locations, files, effect_summary) = self.leave_scope();

        compile_result?;

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let runtime_contract = {
            let contract = crate::bytecode::compiler::contracts::FnContract {
                type_params: vec![],
                params: parameter_types.to_vec(),
                ret: return_type.clone(),
                effects: effects.to_vec(),
            };
            self.to_runtime_contract(&contract)
        };

        let fn_idx = self.add_constant(Value::Function(Rc::new(
            CompiledFunction::new(
                instructions,
                num_locals,
                parameters.len(),
                Some(
                    FunctionDebugInfo::new(Some(self.sym(name).to_string()), files, locations)
                        .with_effect_summary(effect_summary),
                ),
            )
            .with_contract(runtime_contract),
        )));
        self.emit_closure_index(fn_idx, free_symbols.len());

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => 0,
        };
        Ok(())
    }

    pub(super) fn compile_module_statement(
        &mut self,
        name: Symbol,
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        // Check if module is already defined
        let name_str = self.sym(name);
        let binding_name_str = module_binding_name(name_str).to_string();
        let binding_name = self.interner.intern(&binding_name_str);
        if self.symbol_table.exists_in_current_scope(binding_name) {
            let binding_name_str = self.sym(binding_name);
            return Err(Self::boxed(self.make_redeclaration_error(
                binding_name_str,
                Span::new(position, position),
                None,
                None,
            )));
        }

        // Collect all functions from the module body and validate contents
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if *fn_name == binding_name {
                        let pos = statement.position();
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NAME_CLASH,
                            &[binding_name_str],
                            self.file_path.clone(),
                            Span::new(pos, pos),
                        )));
                    }
                }
                // Module Constants Allow let statements in modules
                Statement::Let { .. } => {
                    // Let statements are allowed for module constants
                }
                // ADT type declarations are allowed inside modules
                Statement::Data { .. } => {}
                _ => {
                    let pos = statement.position();
                    return Err(Self::boxed(Diagnostic::make_error(
                        &INVALID_MODULE_CONTENT,
                        &[],
                        self.file_path.clone(),
                        Span::new(pos, pos),
                    )));
                }
            }
        }

        self.imported_modules.insert(binding_name);
        let previous_module = self.current_module_prefix;
        self.current_module_prefix = Some(binding_name);

        // ====================================================================
        // START: MODULE CONSTANTS (bytecode/module_constants/)
        // ====================================================================
        // PASS 0: MODULE CONSTANTS
        // Compile-time constant evaluation with automatic dependency resolution.
        // Implementation uses utilities from bytecode/module_constants/:
        // - find_constant_refs: Find dependencies in expressions
        // - topological_sort_constants: Order constants (dependencies first)
        // - eval_const_expr: Evaluate constant expressions at compile time
        // ====================================================================

        // Compile module constants (analysis + evaluation)
        let constants = match compile_module_constants(body, binding_name, &mut self.interner) {
            Ok(result) => result,
            Err(err) => {
                self.current_module_prefix = previous_module;
                return Err(Self::boxed(self.convert_const_compile_error(err, position)));
            }
        };

        // Store evaluated constants in compiler's module_constants map
        self.module_constants.extend(constants);

        // ====================================================================
        // END: MODULE CONSTANTS
        // ====================================================================

        // PASS 1: Predeclare all module function names with qualified names
        // This enables forward references within the module
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                span,
                ..
            } = statement
            {
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                // Check for duplicate declaration
                if let Some(existing) = self.symbol_table.resolve(qualified_name)
                    && self.symbol_table.exists_in_current_scope(qualified_name)
                {
                    self.current_module_prefix = previous_module;
                    let qualified_name_str = self.sym(qualified_name);
                    return Err(Self::boxed(self.make_redeclaration_error(
                        qualified_name_str,
                        *span,
                        Some(existing.span),
                        Some("Use a different name or remove the previous definition"),
                    )));
                }
                // Predeclare the function
                self.symbol_table.define(qualified_name, *span);
            }
        }

        // PASS 2: Compile each function body
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                parameters,
                parameter_types,
                return_type,
                effects,
                body: fn_body,
                span,
                ..
            } = statement
            {
                let position = span.start;
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                    if effects.is_empty() {
                        self.lookup_contract(Some(binding_name), *fn_name, parameters.len())
                            .map(|contract| contract.effects.clone())
                            .unwrap_or_default()
                    } else {
                        effects.clone()
                    };
                if let Err(err) = self.compile_function_statement(
                    qualified_name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    fn_body,
                    position,
                ) {
                    let mut diag = *err;
                    if diag.phase().is_none() {
                        diag.phase = Some(DiagnosticPhase::TypeCheck);
                    }
                    self.errors.push(diag);
                }
            }
        }

        self.current_module_prefix = previous_module;

        Ok(())
    }

    pub(super) fn compile_data_statement(
        &mut self,
        name: Symbol,
        variants: &[DataVariant],
    ) -> CompileResult<()> {
        // Data declarations are handled at the registry level (collect_adt_definitions).
        // No bytecode is emitted here — constructors are compiled on-demand when called.
        let _ = (name, variants);
        Ok(())
    }

    pub(super) fn compile_import_statement(
        &mut self,
        name: Symbol,
        alias: Option<Symbol>,
        except: &[Symbol],
    ) -> CompileResult<()> {
        if self.is_base_module_symbol(name) {
            return Ok(());
        }

        if !except.is_empty() {
            let binding_name = alias.unwrap_or(name);
            let excluded: std::collections::HashSet<Symbol> = except.iter().copied().collect();
            self.imported_module_exclusions
                .insert(binding_name, excluded);
        }

        if let Some(alias) = alias {
            self.import_aliases.insert(alias, name);
        } else {
            self.imported_modules.insert(name);
        }
        Ok(())
    }

    pub(super) fn compile_block(&mut self, block: &Block) -> CompileResult<()> {
        let mut consumable_counts = HashMap::new();
        for statement in &block.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            for statement in &block.statements {
                if let Some(err) = compiler.compile_statement_collect_error(statement) {
                    return Err(err);
                }
            }
            Ok(())
        })
    }

    #[allow(clippy::vec_box)]
    fn compile_block_with_tail_collect_errors(&mut self, block: &Block) -> Vec<Box<Diagnostic>> {
        let len = block.statements.len();
        let mut errors = Vec::new();
        let mut consumable_counts = HashMap::new();
        for statement in &block.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            for (i, statement) in block.statements.iter().enumerate() {
                let is_last = i == len - 1;
                let tail_eligible = matches!(
                    statement,
                    Statement::Expression {
                        has_semicolon: false,
                        ..
                    } | Statement::Return { .. }
                );

                let result = if is_last && tail_eligible {
                    compiler
                        .with_tail_position(true, |compiler| compiler.compile_statement(statement))
                } else {
                    compiler
                        .with_tail_position(false, |compiler| compiler.compile_statement(statement))
                };

                if let Err(err) = result {
                    errors.push(err);
                }
            }
        });

        errors
    }

    /// Compile a block with tail position awareness for the last statement
    pub(super) fn compile_block_with_tail(&mut self, block: &Block) -> CompileResult<()> {
        let mut errors = self
            .compile_block_with_tail_collect_errors(block)
            .into_iter();
        if let Some(first) = errors.next() {
            for err in errors {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
            return Err(first);
        }

        Ok(())
    }

    pub(super) fn block_has_value_tail(&self, block: &Block) -> bool {
        matches!(
            block.statements.last(),
            Some(Statement::Expression {
                has_semicolon: false,
                ..
            })
        )
    }
}
