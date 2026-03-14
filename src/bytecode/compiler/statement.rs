use std::{collections::{HashMap, HashSet}, rc::Rc};

use crate::cfg::{
    BlockId, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr, IrProgram,
    IrStringPart, IrTerminator, IrTopLevelItem, IrVar,
};
use crate::primop::resolve_primop_call;
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
    runtime::{
        compiled_function::CompiledFunction,
        handler_descriptor::HandlerDescriptor,
        value::Value,
    },
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

enum BodyValidationKind {
    /// `x = expr` — Flux has no mutable bindings.
    Assign,
    /// `let x = expr` where `x` is already a parameter name.
    LetShadowsParam,
}

impl Compiler {
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

    /// Pre-validate the function body for errors that are always fatal
    /// (immutable reassignment, let-shadowing a parameter). Emits diagnostics
    /// directly so the CFG path doesn't need to reproduce these checks.
    /// Returns `true` if any validation errors were found.
    fn validate_body(
        &mut self,
        body: &Block,
        parameters: &[Symbol],
    ) -> bool {
        let mut found = false;
        Self::collect_ast_body_validation_errors(&body.statements, parameters, &mut |name, span, kind| {
            let diag = match kind {
                BodyValidationKind::Assign => {
                    let name_str = self.sym(name);
                    self.make_immutability_error(name_str, span)
                }
                BodyValidationKind::LetShadowsParam => {
                    let name_str = self.sym(name);
                    self.make_redeclaration_error(name_str, span, None, None)
                }
            };
            self.errors.push(diag);
            found = true;
        });
        found
    }

    fn collect_ast_body_validation_errors(
        stmts: &[Statement],
        params: &[Symbol],
        report: &mut dyn FnMut(Symbol, Span, BodyValidationKind),
    ) {
        for stmt in stmts {
            match stmt {
                Statement::Assign { name, span, .. } => {
                    report(*name, *span, BodyValidationKind::Assign);
                }
                Statement::Let { name, span, .. } if params.contains(name) => {
                    report(*name, *span, BodyValidationKind::LetShadowsParam);
                }
                Statement::Function { body, .. } => {
                    Self::collect_ast_body_validation_errors(&body.statements, params, report);
                }
                _ => {}
            }
        }
    }

    /// Check whether an `IrFunction` can be compiled via the CFG bytecode path.
    ///
    /// Entry block must be at index 0.  All other structural constraints
    /// (block ordering, forward jumps) are handled by the linearizer which
    /// emits explicit jumps when blocks are non-adjacent.
    fn can_compile_ir_cfg_function(function: &IrFunction) -> bool {
        let Some(entry_index) = function
            .blocks
            .iter()
            .position(|block| block.id == function.entry)
        else {
            return false;
        };
        // Entry block must be the first block for linear emission.
        entry_index == 0
    }


    fn compile_ir_let_item(
        &mut self,
        name: Symbol,
        type_annotation: &Option<TypeExpr>,
        value: &crate::syntax::expression::Expression,
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
            return Err(Self::boxed(self.make_import_collision_error(name_str, span)));
        }

        let symbol = self.symbol_table.define(name, span);

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
        } else if let HmExprTypeResult::Known(inferred) = self.hm_expr_type_strict_path(value) {
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
        pattern: &Pattern,
        value: &crate::syntax::expression::Expression,
        _span: Span,
    ) -> CompileResult<()> {
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
        Ok(())
    }

    fn compile_ir_return_item(
        &mut self,
        value: &Option<crate::syntax::expression::Expression>,
        _span: Span,
    ) -> CompileResult<()> {
        match value {
            Some(expr) => {
                self.compile_expression(expr)?;
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
        value: &crate::syntax::expression::Expression,
        span: Span,
    ) -> CompileResult<()> {
        self.compile_statement(&Statement::Assign {
            name,
            value: value.clone(),
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
        let binding_name_str =
            import_binding_name(&name_str, alias_str.as_deref()).to_string();
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

    /// Compile an `IrFunction`'s CFG blocks into bytecode.
    ///
    /// This is the core CFG compilation loop: it sets up bindings for all
    /// IR variables, linearises the basic blocks, and patches jump offsets.
    /// Effect validation is **not** performed here — callers are expected to
    /// have validated effects through the AST validation pass.
    fn compile_ir_cfg_body(
        &mut self,
        function: &IrFunction,
        current_name: Symbol,
        ir_program: &IrProgram,
    ) -> CompileResult<()> {
        // Resolve parameter bindings from the symbol table.
        let mut bindings: HashMap<IrVar, crate::bytecode::binding::Binding> = HashMap::new();
        for param in &function.params {
            let binding = self.symbol_table.resolve(param.name).ok_or_else(|| {
                Self::boxed(Diagnostic::warning(format!(
                    "missing CFG bytecode param binding for `{}`",
                    self.sym(param.name),
                )))
            })?;
            bindings.insert(param.var, binding);
        }
        // Allocate temp bindings for block params and instruction destinations.
        for block in &function.blocks {
            for param in &block.params {
                bindings.entry(param.var).or_insert_with(|| self.symbol_table.define_temp());
            }
            for instr in &block.instrs {
                let dest = match instr {
                    IrInstr::Assign { dest, .. }
                    | IrInstr::Call { dest, .. }
                    | IrInstr::HandleScope { dest, .. } => dest,
                };
                bindings.entry(*dest).or_insert_with(|| self.symbol_table.define_temp());
            }
        }

        // Pre-scan: find continuation blocks for HandleScope instructions.
        // The continuation block is the block whose param var matches the
        // HandleScope's `dest` — it receives the handle body's result.
        // We emit OpEndHandle at the start of each continuation block.
        let mut end_handle_blocks = HashSet::<BlockId>::new();
        for block in &function.blocks {
            for instr in &block.instrs {
                if let IrInstr::HandleScope { dest, .. } = instr {
                    for b in &function.blocks {
                        if b.params.iter().any(|p| p.var == *dest) {
                            end_handle_blocks.insert(b.id);
                            break;
                        }
                    }
                }
            }
        }

        let block_map: HashMap<_, _> = function
            .blocks
            .iter()
            .map(|block| (block.id, block))
            .collect();

        let mut block_offsets = HashMap::<BlockId, usize>::new();
        let mut pending_jumps = Vec::<(usize, BlockId)>::new();
        let mut false_target_blocks = HashSet::<BlockId>::new();

        for (index, block) in function.blocks.iter().enumerate() {
            block_offsets.insert(block.id, self.current_instructions().len());
            if false_target_blocks.remove(&block.id) {
                self.emit(OpCode::OpPop, &[]);
            }
            // Emit OpEndHandle at the start of continuation blocks (after
            // the handle body has finished executing).
            if end_handle_blocks.contains(&block.id) {
                self.emit(OpCode::OpEndHandle, &[]);
                self.handled_effects.pop();
            }

            for instr in &block.instrs {
                self.compile_ir_cfg_instr(instr, &bindings, current_name, ir_program)?;
            }

            match &block.terminator {
                IrTerminator::Return(..) | IrTerminator::TailCall { .. } => {
                    self.compile_ir_cfg_terminator(&block.terminator, &bindings, current_name)?;
                }
                IrTerminator::Jump(target, args, _) => {
                    let target_block = block_map.get(target).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode jump target block"))
                    })?;
                    self.compile_ir_cfg_jump_args(target_block, args, &bindings)?;
                    let jump_pos = self.emit(OpCode::OpJump, &[9999]);
                    pending_jumps.push((jump_pos, *target));
                }
                IrTerminator::Branch {
                    cond,
                    then_block,
                    else_block,
                    ..
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

                    // Check if then_block is the immediately next block.
                    // If not, emit an explicit jump to support non-adjacent
                    // then blocks produced by the Core IR lowering.
                    let next_block_id = function
                        .blocks
                        .get(index + 1)
                        .map(|b| b.id);
                    if next_block_id != Some(*then_block) {
                        let then_jump = self.emit(OpCode::OpJump, &[9999]);
                        pending_jumps.push((then_jump, *then_block));
                    }
                }
                IrTerminator::Unreachable(_) => {
                    // Dead block — emit a return to keep bytecode well-formed.
                    // This can occur when the Core IR lowering produces
                    // blocks that are not reachable from any predecessor.
                    self.emit(OpCode::OpReturn, &[]);
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
    }

    /// Try to compile an `IrFunction` via the CFG path.
    ///
    /// Returns `None` when the function cannot be compiled via CFG (e.g. the
    /// entry block is not first). Used for inner functions (closures, handler
    /// arms) where effect validation is done inline.
    fn try_compile_ir_cfg_function_body(
        &mut self,
        function: &IrFunction,
        current_name: Symbol,
        ir_program: &IrProgram,
    ) -> Option<CompileResult<()>> {
        if !Self::can_compile_ir_cfg_function(function) {
            return None;
        }

        // Pre-validate: check effect availability for all calls before emitting
        // any bytecode.  Effect violations are reported as errors directly.
        for block in &function.blocks {
            for instr in &block.instrs {
                match instr {
                    IrInstr::Call { target, args, metadata, .. } => {
                        if let Err(e) = self.check_ir_cfg_call_effects(target, args.len(), metadata.span) {
                            return Some(Err(e));
                        }
                    }
                    IrInstr::Assign { expr: IrExpr::Perform { effect, operation, .. }, .. } => {
                        if !self.is_effect_available(*effect) {
                            let effect_name = self.sym(*effect).to_string();
                            let op_name = self.sym(*operation).to_string();
                            return Some(Err(Self::boxed(
                                Diagnostic::make_error_dynamic(
                                    "E400",
                                    "MISSING EFFECT",
                                    ErrorType::Compiler,
                                    format!(
                                        "Perform `{}.{}` requires effect `{}` in this function signature.",
                                        effect_name, op_name, effect_name
                                    ),
                                    Some(format!("Add `with {}` to the enclosing function.", effect_name)),
                                    self.file_path.clone(),
                                    Span::default(),
                                )
                                .with_display_title("Missing Ambient Effect")
                                .with_category(crate::diagnostics::DiagnosticCategory::Effects)
                                .with_phase(DiagnosticPhase::Effect),
                            )));
                        }
                    }
                    _ => {}
                }
            }
            if let IrTerminator::TailCall { callee, args, metadata, .. } = &block.terminator
                && let Err(e) = self.check_ir_cfg_call_effects(callee, args.len(), metadata.span)
            {
                return Some(Err(e));
            }
        }

        Some(self.compile_ir_cfg_body(function, current_name, ir_program))
    }

    fn compile_ir_cfg_instr(
        &mut self,
        instr: &IrInstr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
        ir_program: &IrProgram,
    ) -> CompileResult<()> {
        match instr {
            IrInstr::Assign { dest, expr, .. } => {
                self.compile_ir_cfg_expr(expr, bindings, ir_program)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode binding for IR var"))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
            IrInstr::Call {
                dest, target, args, ..
            } => {
                self.compile_ir_cfg_call(target, args, bindings, current_name)?;
                let binding = bindings.get(dest).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode binding for IR call dest"))
                })?;
                self.emit_store_binding(binding);
                Ok(())
            }
            IrInstr::HandleScope {
                effect,
                arms,
                ..
            } => {
                self.compile_ir_cfg_handle_scope(
                    *effect,
                    arms,
                    ir_program,
                )
            }
        }
    }

    /// Compile a `HandleScope` instruction: arm closures → OpHandle.
    ///
    /// Body blocks and `OpEndHandle` are compiled by the main `compile_ir_cfg_body`
    /// loop — the body blocks are sandwiched between `OpHandle` (emitted here) and
    /// `OpEndHandle` (emitted at the continuation block).  The handled effect is
    /// pushed onto the effect stack so that `Perform` instructions inside the body
    /// can validate their effect availability.
    #[allow(clippy::too_many_arguments)]
    fn compile_ir_cfg_handle_scope(
        &mut self,
        effect: Symbol,
        arms: &[crate::cfg::HandleScopeArm],
        ir_program: &IrProgram,
    ) -> CompileResult<()> {
        // 1. Compile each arm as a closure function (same pattern as MakeClosure).
        let mut operation_names = Vec::new();
        for arm in arms {
            operation_names.push(arm.operation_name);
            let inner_fn = ir_program.function(arm.function_id).ok_or_else(|| {
                Self::boxed(Diagnostic::warning("missing CFG bytecode handle arm function"))
            })?;

            let num_captures = inner_fn.captures.len();
            let real_param_count = inner_fn.params.len().saturating_sub(num_captures);

            self.enter_scope();
            for param in &inner_fn.params {
                self.symbol_table.define(param.name, Span::default());
            }

            let closure_name = self.interner.intern("<handler>");
            let body_result = self.try_compile_ir_cfg_function_body(
                inner_fn,
                closure_name,
                ir_program,
            );
            match body_result {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    self.leave_scope();
                    return Err(e);
                }
                None => {
                    self.leave_scope();
                    return Err(Self::boxed(Diagnostic::warning(
                        "handle arm body ineligible for CFG bytecode path",
                    )));
                }
            }

            let free_symbols = self.symbol_table.free_symbols.clone();
            for free in &free_symbols {
                if free.symbol_scope == SymbolScope::Local {
                    self.mark_captured_in_current_function(free.index);
                }
            }
            let num_locals = self.symbol_table.num_definitions;
            let (instructions, locations, files, effect_summary) = self.leave_scope();

            // Load captured variables from outer scope.
            for &capture_name in &inner_fn.captures {
                if let Some(sym) = self.symbol_table.resolve(capture_name) {
                    self.load_symbol(&sym);
                }
            }
            for free in &free_symbols {
                self.load_symbol(free);
            }

            let total_free = num_captures + free_symbols.len();
            let fn_idx = self.add_constant(Value::Function(Rc::new(
                CompiledFunction::new(
                    instructions,
                    num_locals,
                    real_param_count,
                    Some(
                        FunctionDebugInfo::new(None, files, locations)
                            .with_effect_summary(effect_summary),
                    ),
                ),
            )));
            self.emit_closure_index(fn_idx, total_free);
        }

        // 2. Build HandlerDescriptor and emit OpHandle.
        let desc = Value::HandlerDescriptor(Rc::new(HandlerDescriptor {
            effect,
            ops: operation_names,
        }));
        let desc_idx = self.add_constant(desc);
        self.emit(OpCode::OpHandle, &[desc_idx]);

        // 3. Push handled effect so Perform instructions inside the body
        //    can validate effect availability.  Popped when the main loop
        //    reaches the continuation block and emits OpEndHandle.
        self.handled_effects.push(effect);

        Ok(())
    }

    fn compile_ir_cfg_expr(
        &mut self,
        expr: &IrExpr,
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        ir_program: &IrProgram,
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
                    Self::boxed(Diagnostic::warning("missing CFG bytecode binding for IR var"))
                })?);
                Ok(())
            }
            IrExpr::TagTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tag-test binding"))
                })?);
                match tag {
                    crate::cfg::IrTagTest::None => {
                        self.emit(OpCode::OpNone, &[]);
                        self.emit(OpCode::OpEqual, &[]);
                    }
                    crate::cfg::IrTagTest::Some => {
                        self.emit(OpCode::OpIsSome, &[]);
                    }
                    crate::cfg::IrTagTest::Left => {
                        self.emit(OpCode::OpIsLeft, &[]);
                    }
                    crate::cfg::IrTagTest::Right => {
                        self.emit(OpCode::OpIsRight, &[]);
                    }
                }
                Ok(())
            }
            IrExpr::TupleArityTest { value, .. } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tuple-test binding"))
                })?);
                self.emit(OpCode::OpIsTuple, &[]);
                Ok(())
            }
            IrExpr::TupleFieldAccess { object, index } => {
                self.load_symbol(bindings.get(object).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tuple-field binding"))
                })?);
                self.emit(OpCode::OpTupleIndex, &[*index]);
                Ok(())
            }
            IrExpr::TagPayload { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode tag-payload binding"))
                })?);
                match tag {
                    crate::cfg::IrTagTest::Some => self.emit(OpCode::OpUnwrapSome, &[]),
                    crate::cfg::IrTagTest::Left => self.emit(OpCode::OpUnwrapLeft, &[]),
                    crate::cfg::IrTagTest::Right => self.emit(OpCode::OpUnwrapRight, &[]),
                    crate::cfg::IrTagTest::None => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "invalid CFG bytecode None payload lowering",
                        )));
                    }
                };
                Ok(())
            }
            IrExpr::ListTest { value, tag } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode list-test binding"))
                })?);
                match tag {
                    crate::cfg::IrListTest::Empty => self.emit(OpCode::OpIsEmptyList, &[]),
                    crate::cfg::IrListTest::Cons => self.emit(OpCode::OpIsCons, &[]),
                };
                Ok(())
            }
            IrExpr::ListHead { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode list-head binding"))
                })?);
                self.emit(OpCode::OpConsHead, &[]);
                Ok(())
            }
            IrExpr::ListTail { value } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode list-tail binding"))
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
                self.replace_instruction(jump_to_false, make(OpCode::OpIsAdtJump, &[const_idx, false_pos]));
                self.emit(OpCode::OpPop, &[]);
                self.emit(OpCode::OpFalse, &[]);
                let end_pos = self.current_instructions().len();
                self.change_operand(jump_to_end, end_pos);
                Ok(())
            }
            IrExpr::AdtField { value, index } => {
                self.load_symbol(bindings.get(value).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode adt-field binding"))
                })?);
                self.emit(OpCode::OpAdtField, &[*index]);
                Ok(())
            }
            IrExpr::LoadName(name) => {
                self.compile_ir_cfg_load_name(*name)
            }
            IrExpr::Prefix { operator, right } => {
                self.load_symbol(bindings.get(right).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode prefix binding"))
                })?);
                match operator.as_str() {
                    "!" => self.emit(OpCode::OpBang, &[]),
                    "-" => self.emit(OpCode::OpMinus, &[]),
                    _ => {
                        return Err(Self::boxed(Diagnostic::warning(
                            "unsupported CFG bytecode prefix operator",
                        )));
                    }
                };
                Ok(())
            }
            IrExpr::InterpolatedString(parts) => {
                if parts.is_empty() {
                    self.emit_constant_value(Value::String(String::new().into()));
                    return Ok(());
                }
                match &parts[0] {
                    IrStringPart::Literal(s) => {
                        self.emit_constant_value(Value::String(s.clone().into()));
                    }
                    IrStringPart::Interpolation(var) => {
                        self.load_symbol(bindings.get(var).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning("missing CFG bytecode interpolation binding"))
                        })?);
                        self.emit(OpCode::OpToString, &[]);
                    }
                }
                for part in &parts[1..] {
                    match part {
                        IrStringPart::Literal(s) => {
                            self.emit_constant_value(Value::String(s.clone().into()));
                        }
                        IrStringPart::Interpolation(var) => {
                            self.load_symbol(bindings.get(var).ok_or_else(|| {
                                Self::boxed(Diagnostic::warning("missing CFG bytecode interpolation binding"))
                            })?);
                            self.emit(OpCode::OpToString, &[]);
                        }
                    }
                    self.emit(OpCode::OpAdd, &[]);
                }
                Ok(())
            }
            IrExpr::MakeArray(elements) => {
                for var in elements {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode array element binding"))
                    })?);
                }
                self.emit_array_count(elements.len());
                Ok(())
            }
            IrExpr::MakeTuple(elements) => {
                for var in elements {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode tuple element binding"))
                    })?);
                }
                self.emit_tuple_count(elements.len());
                Ok(())
            }
            IrExpr::MakeHash(pairs) => {
                for (key, value) in pairs {
                    self.load_symbol(bindings.get(key).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode hash key binding"))
                    })?);
                    self.load_symbol(bindings.get(value).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode hash value binding"))
                    })?);
                }
                self.emit_hash_count(pairs.len() * 2);
                Ok(())
            }
            IrExpr::MakeList(elements) => {
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("base list must be defined");
                self.load_symbol(&symbol);
                for var in elements {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode list element binding"))
                    })?);
                }
                self.emit(OpCode::OpCall, &[elements.len()]);
                Ok(())
            }
            IrExpr::MakeAdt(constructor, fields) => {
                for var in fields {
                    self.load_symbol(bindings.get(var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning("missing CFG bytecode ADT field binding"))
                    })?);
                }
                let const_idx =
                    self.add_constant(Value::String(self.sym(*constructor).to_string().into()));
                self.emit(OpCode::OpMakeAdt, &[const_idx, fields.len()]);
                Ok(())
            }
            IrExpr::Index { left, index } => {
                self.load_symbol(bindings.get(left).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode index left binding"))
                })?);
                self.load_symbol(bindings.get(index).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode index right binding"))
                })?);
                self.emit(OpCode::OpIndex, &[]);
                Ok(())
            }
            IrExpr::Some(var) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode Some binding"))
                })?);
                self.emit(OpCode::OpSome, &[]);
                Ok(())
            }
            IrExpr::Left(var) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode Left binding"))
                })?);
                self.emit(OpCode::OpLeft, &[]);
                Ok(())
            }
            IrExpr::Right(var) => {
                self.load_symbol(bindings.get(var).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode Right binding"))
                })?);
                self.emit(OpCode::OpRight, &[]);
                Ok(())
            }
            IrExpr::Cons { head, tail } => {
                self.load_symbol(bindings.get(head).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode Cons head binding"))
                })?);
                self.load_symbol(bindings.get(tail).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode Cons tail binding"))
                })?);
                self.emit(OpCode::OpCons, &[]);
                Ok(())
            }
            IrExpr::EmptyList => {
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("base list must be defined");
                self.load_symbol(&symbol);
                self.emit(OpCode::OpCall, &[0]);
                Ok(())
            }
            IrExpr::Binary(op, lhs, rhs) => {
                let lhs_binding = bindings.get(lhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode lhs binding"))
                })?;
                let rhs_binding = bindings.get(rhs).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode rhs binding"))
                })?;
                // And/Or: use short-circuit jump pattern
                if matches!(op, crate::cfg::IrBinaryOp::And) {
                    self.load_symbol(lhs_binding);
                    let jump_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                    self.load_symbol(rhs_binding);
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }
                if matches!(op, crate::cfg::IrBinaryOp::Or) {
                    self.load_symbol(lhs_binding);
                    let jump_pos = self.emit(OpCode::OpJumpTruthy, &[9999]);
                    self.load_symbol(rhs_binding);
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }
                if matches!(op, crate::cfg::IrBinaryOp::Lt) {
                    self.load_symbol(rhs_binding);
                    self.load_symbol(lhs_binding);
                } else {
                    self.load_symbol(lhs_binding);
                    self.load_symbol(rhs_binding);
                }
                match op {
                    crate::cfg::IrBinaryOp::Add | crate::cfg::IrBinaryOp::IAdd | crate::cfg::IrBinaryOp::FAdd => {
                        self.emit(OpCode::OpAdd, &[])
                    }
                    crate::cfg::IrBinaryOp::Sub | crate::cfg::IrBinaryOp::ISub | crate::cfg::IrBinaryOp::FSub => {
                        self.emit(OpCode::OpSub, &[])
                    }
                    crate::cfg::IrBinaryOp::Mul | crate::cfg::IrBinaryOp::IMul | crate::cfg::IrBinaryOp::FMul => {
                        self.emit(OpCode::OpMul, &[])
                    }
                    crate::cfg::IrBinaryOp::Div | crate::cfg::IrBinaryOp::IDiv | crate::cfg::IrBinaryOp::FDiv => {
                        self.emit(OpCode::OpDiv, &[])
                    }
                    crate::cfg::IrBinaryOp::Mod | crate::cfg::IrBinaryOp::IMod => {
                        self.emit(OpCode::OpMod, &[])
                    }
                    crate::cfg::IrBinaryOp::Eq => self.emit(OpCode::OpEqual, &[]),
                    crate::cfg::IrBinaryOp::NotEq => self.emit(OpCode::OpNotEqual, &[]),
                    crate::cfg::IrBinaryOp::Lt => self.emit(OpCode::OpGreaterThan, &[]),
                    crate::cfg::IrBinaryOp::Gt => self.emit(OpCode::OpGreaterThan, &[]),
                    crate::cfg::IrBinaryOp::Ge => self.emit(OpCode::OpGreaterThanOrEqual, &[]),
                    crate::cfg::IrBinaryOp::Le => self.emit(OpCode::OpLessThanOrEqual, &[]),
                    // And/Or handled above
                    crate::cfg::IrBinaryOp::And | crate::cfg::IrBinaryOp::Or => unreachable!(),
                };
                Ok(())
            }
            IrExpr::MakeClosure(fn_id, captures) => {
                let inner_fn = ir_program.function(*fn_id).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning("missing CFG bytecode closure function"))
                })?;
                // The inner function's params are: [captures..., real_params...].
                // The VM treats captures as "free variables" attached to the closure,
                // so the real param count is total params minus captures.
                let num_captures = captures.len();
                let real_param_count = inner_fn.params.len().saturating_sub(num_captures);

                self.enter_scope();

                // Captures are accessed via OpGetFree in the VM, so define
                // them as Free bindings.  Only real params become locals.
                for (i, param) in inner_fn.params.iter().enumerate() {
                    if i < num_captures {
                        self.symbol_table.define_free_at(param.name, i);
                    } else {
                        self.symbol_table.define(param.name, Span::default());
                    }
                }

                // Try to compile the inner function's body via CFG path.
                let closure_name = self.interner.intern("<closure>");
                let body_result = self.try_compile_ir_cfg_function_body(
                    inner_fn,
                    closure_name,
                    ir_program,
                );
                match body_result {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        self.leave_scope();
                        return Err(e);
                    }
                    None => {
                        // CFG path ineligible for inner function — bail so the
                        // outer function falls back to the structured path.
                        self.leave_scope();
                        return Err(Self::boxed(Diagnostic::warning(
                            "closure body ineligible for CFG bytecode path",
                        )));
                    }
                }

                let free_symbols = self.symbol_table.free_symbols.clone();
                for free in &free_symbols {
                    if free.symbol_scope == SymbolScope::Local {
                        self.mark_captured_in_current_function(free.index);
                    }
                }
                let num_locals = self.symbol_table.num_definitions;
                let (instructions, locations, files, effect_summary) = self.leave_scope();

                // Load captured variables from the outer scope's bindings.
                for capture_var in captures {
                    self.load_symbol(bindings.get(capture_var).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode closure capture binding",
                        ))
                    })?);
                }
                // Also load any symbol-table-detected free variables from outer scope.
                for free in &free_symbols {
                    self.load_symbol(free);
                }

                let total_free = num_captures + free_symbols.len();
                let fn_idx = self.add_constant(Value::Function(Rc::new(
                    CompiledFunction::new(
                        instructions,
                        num_locals,
                        real_param_count,
                        Some(
                            FunctionDebugInfo::new(None, files, locations)
                                .with_effect_summary(effect_summary),
                        ),
                    ),
                )));
                self.emit_closure_index(fn_idx, total_free);
                Ok(())
            }
            IrExpr::MemberAccess { object, member, module_name } => {
                if let Some(mod_name) = module_name {
                    // Resolve import aliases (e.g., `Test` → `Flow.FTest`).
                    let resolved_mod = self
                        .import_aliases
                        .get(mod_name)
                        .copied()
                        .unwrap_or(*mod_name);
                    if self.is_base_module_symbol(resolved_mod) {
                        let member_name = self.sym(*member);
                        if let Some(index) =
                            crate::runtime::base::BaseModule::new().index_of(member_name)
                        {
                            self.emit(OpCode::OpGetBase, &[index]);
                            return Ok(());
                        }
                        // Unknown base member — validation pass reports this.
                        self.emit(OpCode::OpNone, &[]);
                        return Ok(());
                    }
                    // User module: construct qualified name and resolve.
                    let qualified = self.interner.intern_join(resolved_mod, *member);
                    if let Some(constant_value) = self.module_constants.get(&qualified) {
                        self.emit_constant_value(constant_value.clone());
                        return Ok(());
                    }
                    if let Some(symbol) = self.symbol_table.resolve(qualified) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }
                    // Try the module name directly (might be an imported module
                    // binding that uses a different naming convention).
                    let mod_name_str = self.interner.resolve(*mod_name).to_string();
                    let mod_binding = module_binding_name(&mod_name_str).to_string();
                    let mod_binding_sym = self.interner.intern(&mod_binding);
                    let qualified2 = self.interner.intern_join(mod_binding_sym, *member);
                    if let Some(constant_value) = self.module_constants.get(&qualified2) {
                        self.emit_constant_value(constant_value.clone());
                        return Ok(());
                    }
                    if let Some(symbol) = self.symbol_table.resolve(qualified2) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }
                }
                // Fallback: load the object and emit a runtime member access.
                // This handles cases like `obj.field` where `obj` is a local
                // variable, not a module.  The validation pass reports errors
                // for truly unresolvable member accesses.
                self.load_symbol(bindings.get(object).ok_or_else(|| {
                    Self::boxed(Diagnostic::warning(
                        "missing CFG bytecode MemberAccess object binding",
                    ))
                })?);
                let member_str = self.interner.resolve(*member).to_string();
                let const_idx = self.add_constant(Value::String(Rc::from(member_str.as_str())));
                self.emit_constant_index(const_idx);
                self.emit(OpCode::OpIndex, &[]);
                self.emit(OpCode::OpUnwrapSome, &[]);
                Ok(())
            }
            IrExpr::Perform { effect, operation, args } => {
                // Load arguments from bindings.
                for arg in args {
                    self.load_symbol(bindings.get(arg).ok_or_else(|| {
                        Self::boxed(Diagnostic::warning(
                            "missing CFG bytecode Perform arg binding",
                        ))
                    })?);
                }
                // Build the PerformDescriptor constant.
                let effect_name = self.interner.resolve(*effect).to_string().into_boxed_str();
                let op_name = self.interner.resolve(*operation).to_string().into_boxed_str();
                let desc = Value::PerformDescriptor(Rc::new(
                    crate::runtime::perform_descriptor::PerformDescriptor {
                        effect: *effect,
                        op: *operation,
                        effect_name,
                        op_name,
                    },
                ));
                let const_idx = self.add_constant(desc);
                self.emit(OpCode::OpPerform, &[const_idx, args.len()]);
                Ok(())
            }
            _ => Err(Self::boxed(Diagnostic::warning(
                "unsupported CFG bytecode expression lowering",
            ))),
        }
    }

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
                Self::boxed(Diagnostic::warning("missing CFG bytecode block param binding"))
            })?);
        }
        Ok(())
    }

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
                // Try to fuse GetLocal+Return into ReturnLocal superinstruction
                if !self.replace_last_local_read_with_return() {
                    self.emit(OpCode::OpReturnValue, &[]);
                }
                Ok(())
            }
            IrTerminator::TailCall { callee, args, .. } => {
                let is_self = matches!(callee, IrCallTarget::Named(name) if *name == current_name);

                // Try primop optimization for non-self tail calls to base functions.
                if !is_self {
                    if let IrCallTarget::Named(name) = callee {
                        let is_base = self
                            .resolve_visible_symbol(*name)
                            .is_none_or(|s| s.symbol_scope == SymbolScope::Base);
                        if is_base {
                            if let Some(primop) = resolve_primop_call(self.sym(*name), args.len()) {
                                for arg in args {
                                    self.load_symbol(bindings.get(arg).ok_or_else(|| {
                                        Self::boxed(Diagnostic::warning(
                                            "missing CFG bytecode tail-call arg binding",
                                        ))
                                    })?);
                                }
                                self.emit(OpCode::OpPrimOp, &[primop.id() as usize, args.len()]);
                                self.emit(OpCode::OpReturnValue, &[]);
                                return Ok(());
                            }
                        }
                    }
                }

                // OpTailCall requires [callee, args...] on the stack — always
                // load the callee, including for self-calls.
                match callee {
                    IrCallTarget::Named(name) => {
                        self.compile_ir_cfg_load_name(*name)?;
                    }
                    IrCallTarget::Var(var) => {
                        self.load_symbol(bindings.get(var).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode tail-call callee binding",
                            ))
                        })?);
                    }
                    IrCallTarget::Direct(function_id) => {
                        let symbol =
                            self.ir_function_symbols
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

    /// Check that a CFG call target has the required effects available in the
    /// current function context.  This mirrors the E400 checks from the
    /// structured path (`check_static_contract_call` / `check_direct_builtin_effect_call`).
    fn check_ir_cfg_call_effects(
        &mut self,
        target: &IrCallTarget,
        arg_count: usize,
        span: Option<Span>,
    ) -> CompileResult<()> {
        let IrCallTarget::Named(name) = target else {
            return Ok(());
        };

        let call_span = span.unwrap_or_default();

        // 1. Check base builtins that imply an effect (print→IO, now→Time, etc.).
        if let Some(binding) = self.resolve_visible_symbol(*name)
            && binding.symbol_scope == crate::bytecode::symbol_scope::SymbolScope::Base
        {
            let base_name = self.sym(*name);
            if let Some(required) = self.required_effect_for_base_name(base_name)
                && !self.is_effect_available_name(required)
            {
                let function_name = base_name.to_string();
                return Err(Self::boxed(
                    Diagnostic::make_error_dynamic(
                        "E400",
                        "MISSING EFFECT",
                        crate::diagnostics::types::ErrorType::Compiler,
                        format!(
                            "Call to `{}` requires effect `{}` in this function signature.",
                            function_name, required
                        ),
                        Some(format!(
                            "Add `with {}` to the enclosing function.",
                            required
                        )),
                        self.file_path.clone(),
                        call_span,
                    )
                    .with_display_title("Missing Ambient Effect")
                    .with_category(crate::diagnostics::DiagnosticCategory::Effects)
                    .with_phase(DiagnosticPhase::Effect)
                    .with_primary_label(call_span, "effectful call occurs here"),
                ));
            }
        }

        // 2. Check effect alias mapping (e.g. user-defined effect aliases).
        if let Some(effect) = self.lookup_effect_alias(*name)
            && !self.is_effect_available(effect)
        {
            let function_name = self.sym(*name).to_string();
            let missing = self.sym(effect).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    crate::diagnostics::types::ErrorType::Compiler,
                    format!(
                        "Call to `{}` requires effect `{}` in this function signature.",
                        function_name, missing
                    ),
                    Some(format!("Add `with {}` to the enclosing function.", missing)),
                    self.file_path.clone(),
                    call_span,
                )
                .with_display_title("Missing Ambient Effect")
                .with_category(crate::diagnostics::DiagnosticCategory::Effects)
                .with_phase(DiagnosticPhase::Effect)
                .with_primary_label(call_span, "effectful call occurs here"),
            ));
        }

        // 3. Check function contracts for declared effects.
        if let Some(contract) = self.lookup_unqualified_contract(*name, arg_count).cloned()
            && !contract.effects.is_empty()
        {
            for effect_expr in &contract.effects {
                for effect_name in effect_expr.normalized_names() {
                    if !self.is_effect_available(effect_name) {
                        let function_name = self.sym(*name).to_string();
                        let missing = self.sym(effect_name).to_string();
                        return Err(Self::boxed(
                            Diagnostic::make_error_dynamic(
                                "E400",
                                "MISSING EFFECT",
                                crate::diagnostics::types::ErrorType::Compiler,
                                format!(
                                    "Call to `{}` requires effect `{}` in this function signature.",
                                    function_name, missing
                                ),
                                Some(format!(
                                    "Add `with {}` to the enclosing function.",
                                    missing
                                )),
                                self.file_path.clone(),
                                call_span,
                            )
                            .with_display_title("Missing Ambient Effect")
                            .with_category(crate::diagnostics::DiagnosticCategory::Effects)
                            .with_phase(DiagnosticPhase::Effect)
                            .with_primary_label(call_span, "effectful call occurs here"),
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolve and load a global name in the CFG path.
    ///
    /// Mirrors the identifier resolution logic from `compile_expression`
    /// (Expression::Identifier): symbol table → module-qualified name →
    /// module constants → ADT constructors.
    fn compile_ir_cfg_load_name(&mut self, name: Symbol) -> CompileResult<()> {
        // 1. Direct symbol table lookup (covers predeclared functions, base, imports)
        if let Some(symbol) = self.resolve_visible_symbol(name) {
            self.load_symbol(&symbol);
            return Ok(());
        }
        // 2. Module-qualified lookup (when compiling inside a module)
        if let Some(prefix) = self.current_module_prefix {
            let qualified = self.interner.intern_join(prefix, name);
            if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                self.load_symbol(&symbol);
                return Ok(());
            }
            if let Some(constant_value) = self.module_constants.get(&qualified) {
                self.emit_constant_value(constant_value.clone());
                return Ok(());
            }
            if let Some(info) = self.adt_registry.lookup_constructor(name) {
                if info.arity == 0 {
                    let constructor_name = self.interner.resolve(name).to_string();
                    let const_idx =
                        self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
                    self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                    return Ok(());
                }
            }
        }
        // 3. Zero-arg ADT constructor (outside modules)
        if let Some(info) = self.adt_registry.lookup_constructor(name) {
            if info.arity == 0 {
                let constructor_name = self.interner.resolve(name).to_string();
                let const_idx =
                    self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
                self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                return Ok(());
            }
        }
        // If we get here, the name is truly unresolved.  The validation pass
        // will have already emitted an "undefined variable" diagnostic, so
        // emit a placeholder to keep bytecode well-formed.
        self.emit(OpCode::OpNone, &[]);
        Ok(())
    }

    fn compile_ir_cfg_call(
        &mut self,
        target: &IrCallTarget,
        args: &[IrVar],
        bindings: &HashMap<IrVar, crate::bytecode::binding::Binding>,
        current_name: Symbol,
    ) -> CompileResult<()> {
        // Try primop optimization: base functions with known primop mappings
        // bypass the normal call path and emit OpPrimOp directly.
        if let IrCallTarget::Named(name) = target {
            let is_base = self
                .resolve_visible_symbol(*name)
                .is_none_or(|s| s.symbol_scope == SymbolScope::Base);
            if is_base {
                if let Some(primop) = resolve_primop_call(self.sym(*name), args.len()) {
                    for arg in args {
                        self.load_symbol(bindings.get(arg).ok_or_else(|| {
                            Self::boxed(Diagnostic::warning(
                                "missing CFG bytecode call arg binding",
                            ))
                        })?);
                    }
                    self.emit(OpCode::OpPrimOp, &[primop.id() as usize, args.len()]);
                    return Ok(());
                }
            }
        }

        let is_self = matches!(target, IrCallTarget::Named(name) if *name == current_name);
        if !is_self {
            match target {
                IrCallTarget::Named(name) => {
                    self.compile_ir_cfg_load_name(*name)?;
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
                    let symbol =
                        self.ir_function_symbols
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

    pub(super) fn compile_ir_function_statement(
        &mut self,
        name: Symbol,
        parameters: &[Symbol],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &Block,
        ir_function: Option<&IrFunction>,
        ir_program: &IrProgram,
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

        let symbol = if let Some(existing) = self.symbol_table.resolve(name) {
            existing
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
                && self.block_has_value_tail(body)
                && let Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                }) = body.statements.last()
            {
                let expression = expression.clone();
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

            // Pre-validate the body for always-fatal errors
            // (immutable reassignment, let-shadows-param).
            let _has_body_errors = self.validate_body(body, parameters);

            if let Some(function) = ir_function {
                // ── Validation pass (throwaway scope) ───────────────────
                // Compile the AST body in a nested scope to collect type
                // annotation errors, strict-mode checks, and other
                // diagnostics that are interleaved with compilation.  The
                // bytecode emitted here is discarded.
                {
                    self.enter_scope();
                    self.symbol_table.define_function_name(name, function_span);
                    for (i, param) in parameters.iter().enumerate() {
                        self.symbol_table.define(*param, Span::default());
                        if let Some(Some(param_ty)) = parameter_types.get(i)
                            && let Some(runtime_ty) = convert_type_expr(param_ty, &self.interner)
                        {
                            self.bind_static_type(*param, runtime_ty);
                        }
                    }
                    let validation_errors = self.with_function_context_with_param_effect_rows(
                        parameters.len(),
                        effects,
                        param_effect_rows.clone(),
                        |compiler| {
                            compiler.compile_block_with_tail_collect_errors(body)
                        },
                    );
                    let _ = self.leave_scope(); // discard validation bytecode

                    for err in validation_errors {
                        let mut diag = *err;
                        if diag.phase().is_none() {
                            diag.phase = Some(DiagnosticPhase::TypeCheck);
                        }
                        self.errors.push(diag);
                    }
                }

                // ── Compilation pass (CFG body) ─────────────────────────
                // Bytecode generation goes through the Core IR → CFG path.
                // The CFG representation already contains Return/TailCall
                // terminators, so no post-hoc return-instruction fixups
                // are needed.
                let cfg_result = self.with_function_context_with_param_effect_rows(
                    parameters.len(),
                    effects,
                    param_effect_rows,
                    |compiler| {
                        compiler.compile_ir_cfg_body(function, name, ir_program)
                    },
                );
                if let Err(err) = cfg_result {
                    let mut diag = *err;
                    if diag.phase().is_none() {
                        diag.phase = Some(DiagnosticPhase::TypeCheck);
                    }
                    self.errors.push(diag);
                }
            } else {
                // ── AST compilation (no IR function available) ──────────
                // Module-internal functions and other cases not covered by
                // the Core IR pipeline compile directly from the AST.
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
        body: &[IrTopLevelItem],
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

        for item in body {
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

        let module_body = Block {
            statements: body
                .iter()
                .map(|item| crate::cfg::lower::ir_top_level_item_to_statement(item, &[]))
                .collect(),
            span: Span::new(position, position),
        };
        let constants = match compile_module_constants(&module_body, binding_name, &mut self.interner)
        {
            Ok(result) => result,
            Err(err) => {
                self.current_module_prefix = previous_module;
                return Err(Self::boxed(self.convert_const_compile_error(err, position)));
            }
        };
        self.module_constants.extend(constants);

        for item in body {
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

        for item in body {
            if let IrTopLevelItem::Function {
                name: fn_name,
                function_id: _,
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
                // Module functions use the AST compilation path because the
                // N-ary pipeline does not lower module bodies, so the CFG IR
                // functions for module-nested code lack full context (e.g.
                // match patterns, recursive calls).
                if let Err(err) = self.compile_ir_function_statement(
                    qualified_name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    fn_body,
                    None, // Force AST path for module functions
                    ir_program,
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
                self.compile_expression(expression)?;
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
                let ir_fn = function_id.and_then(|id| ir_program.function(id));
                self.compile_ir_function_statement(
                    *name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    body,
                    ir_fn,
                    ir_program,
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

        // Resolve the symbol - it may have been predeclared in pass 1
        let symbol = if let Some(existing) = self.symbol_table.resolve(name) {
            // Use the existing symbol from pass 1
            existing
        } else {
            // Define new symbol (for nested functions or non-predeclared cases)
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
