use std::{collections::HashMap, rc::Rc};

use crate::backend_ir::{IrFunction, IrProgram};
use crate::{
    ast::type_infer::display_infer_type,
    bytecode::{
        compiler::{Compiler, contracts::convert_type_expr, suggestions::suggest_effect_name},
        debug_info::FunctionDebugInfo,
        module_constants::compile_module_constants,
        op_code::OpCode,
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
    pub(super) fn find_ir_function_by_symbol<'a>(
        &self,
        ir_program: &'a IrProgram,
        symbol: Symbol,
    ) -> Option<&'a IrFunction> {
        ir_program.functions().iter().find(|function| {
            self.ir_function_symbols
                .get(&function.id)
                .is_some_and(|mapped| *mapped == symbol)
        })
    }

    fn block_contains_cfg_incompatible_statements_ast(body: &Block) -> bool {
        body.statements
            .iter()
            .any(|statement| matches!(statement, Statement::Module { .. }))
    }

    #[allow(dead_code)]
    pub(super) fn emit_store_binding(&mut self, binding: &crate::bytecode::binding::Binding) {
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
                        && existing.symbol_scope != SymbolScope::Function
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
                        None,
                        *span,
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
                    self.compile_module_statement(name, body, span.start, None)?;
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
        ir_function: Option<&IrFunction>,
        function_span: Span,
    ) -> CompileResult<()> {
        let definition_span = Span::new(function_span.start, function_span.start);

        if let Some(param) = Self::find_duplicate_name(parameters) {
            let param_str = self.sym(param);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param_str],
                self.file_path.clone(),
                definition_span,
            )));
        }

        for effect_expr in effects {
            for effect_name in effect_expr.normalized_names() {
                if !self.is_known_function_effect_annotation(effect_name) {
                    let span = Self::effect_named_span(effect_expr, effect_name)
                        .unwrap_or(definition_span);
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
            self.symbol_table.define(name, definition_span)
        };

        self.enter_scope();
        self.symbol_table
            .define_function_name(name, definition_span);

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
            if let Some(ir_function) = ir_function
                && !Self::block_contains_cfg_incompatible_statements_ast(body)
                && let Some(cfg_result) =
                    self.try_compile_ir_cfg_function_body(ir_function, name)
            {
                return cfg_result;
            }
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

        let boundary_location = {
            let mut files = files;
            let file_id = files
                .iter()
                .position(|file| file == &self.file_path)
                .map(|index| index as u32)
                .unwrap_or_else(|| {
                    files.push(self.file_path.clone());
                    (files.len() - 1) as u32
                });
            (
                files,
                crate::bytecode::debug_info::Location {
                    file_id,
                    span: function_span,
                },
            )
        };
        let (files, boundary_location) = boundary_location;

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
                        .with_boundary_location(Some(boundary_location))
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
        ir_program: Option<&IrProgram>,
    ) -> CompileResult<()> {
        let name_str = self.sym(name);
        if !is_valid_module_name(name_str) {
            return Err(Self::boxed(Diagnostic::make_error(
                &INVALID_MODULE_NAME,
                &[name_str],
                self.file_path.clone(),
                Span::new(position, position),
            )));
        }

        // Check if module is already defined
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
                    ir_program.and_then(|program| {
                        self.find_ir_function_by_symbol(program, qualified_name)
                    }),
                    *span,
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
