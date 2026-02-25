use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::{
    bytecode::{
        binding::Binding,
        compiler::{
            Compiler,
            contracts::{FnContract, convert_type_expr},
        },
        debug_info::FunctionDebugInfo,
        op_code::OpCode,
        symbol_scope::SymbolScope,
    },
    diagnostics::{
        ADT_NON_EXHAUSTIVE_MATCH, CONSTRUCTOR_ARITY_MISMATCH, DUPLICATE_PARAMETER, Diagnostic,
        DiagnosticBuilder, ICE_SYMBOL_SCOPE_PATTERN, ICE_TEMP_SYMBOL_LEFT_BINDING,
        ICE_TEMP_SYMBOL_LEFT_PATTERN, ICE_TEMP_SYMBOL_MATCH, ICE_TEMP_SYMBOL_RIGHT_BINDING,
        ICE_TEMP_SYMBOL_RIGHT_PATTERN, ICE_TEMP_SYMBOL_SOME_BINDING, ICE_TEMP_SYMBOL_SOME_PATTERN,
        LEGACY_LIST_TAIL_NONE, MODULE_NOT_IMPORTED, TYPE_MISMATCH, UNKNOWN_BASE_MEMBER,
        UNKNOWN_CONSTRUCTOR, UNKNOWN_INFIX_OPERATOR, UNKNOWN_MODULE_MEMBER,
        UNKNOWN_PREFIX_OPERATOR, diag_enhanced,
        types::ErrorType,
        position::{Position, Span},
    },
    primop::{PrimEffect, resolve_primop_call},
    runtime::{
        base::{BaseModule, is_base_fastcall_allowlisted},
        compiled_function::CompiledFunction,
        handler_descriptor::HandlerDescriptor,
        perform_descriptor::PerformDescriptor,
        runtime_type::RuntimeType,
        value::Value,
    },
    syntax::{
        block::Block,
        expression::{Expression, HandleArm, MatchArm, Pattern, StringPart},
        module_graph::is_valid_module_name,
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
    },
    types::type_env::TypeEnv,
};

type CompileResult<T> = Result<T, Box<Diagnostic>>;

impl Compiler {
    pub(super) fn compile_expression(&mut self, expression: &Expression) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(expression.span());
        match expression {
            Expression::Integer { value, .. } => {
                let idx = self.add_constant(Value::Integer(*value));
                self.emit_constant_index(idx);
            }
            Expression::Float { value, .. } => {
                let idx = self.add_constant(Value::Float(*value));
                self.emit_constant_index(idx);
            }
            Expression::String { value, .. } => {
                let idx = self.add_constant(Value::String(value.clone().into()));
                self.emit_constant_index(idx);
            }
            Expression::InterpolatedString { parts, .. } => {
                self.compile_interpolated_string(parts)?;
            }
            Expression::Boolean { value, .. } => {
                if *value {
                    self.emit(OpCode::OpTrue, &[]);
                } else {
                    self.emit(OpCode::OpFalse, &[]);
                }
            }
            Expression::Identifier { name, span } => {
                let name = *name;
                if let Some(symbol) = self.resolve_visible_symbol(name) {
                    self.load_symbol(&symbol);
                } else if let Some(prefix) = self.current_module_prefix {
                    let qualified = self.interner.intern_join(prefix, name);
                    if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                        self.load_symbol(&symbol);
                    } else if let Some(constant_value) = self.module_constants.get(&qualified) {
                        // Module constant - inline the value
                        self.emit_constant_value(constant_value.clone());
                    } else if let Some(info) = self.adt_registry.lookup_constructor(name) {
                        // Zero-arg ADT constructor used inside a module (e.g. `Dot`, `Leaf`)
                        if info.arity != 0 {
                            let name_str = self.interner.resolve(name).to_string();
                            return Err(Self::boxed(
                                diag_enhanced(&CONSTRUCTOR_ARITY_MISMATCH)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "Constructor `{}` expects {} argument(s) but got 0.",
                                        name_str, info.arity
                                    )),
                            ));
                        }

                        let constructor_name = self.interner.resolve(name).to_string();
                        let const_idx =
                            self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
                        self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                    } else {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_undefined_variable_error(name_str, *span),
                        ));
                    }
                } else if let Some(info) = self.adt_registry.lookup_constructor(name) {
                    // Zero-arg ADT constructor used as a value (e.g. `Point`, `None_`)
                    if info.arity != 0 {
                        let name_str = self.interner.resolve(name).to_string();
                        return Err(Self::boxed(
                            diag_enhanced(&CONSTRUCTOR_ARITY_MISMATCH)
                                .with_span(*span)
                                .with_message(format!(
                                    "Constructor `{}` expects {} argument(s) but got 0.",
                                    name_str, info.arity
                                )),
                        ));
                    }
                    let constructor_name = self.interner.resolve(name).to_string();
                    let const_idx =
                        self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
                    self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                } else {
                    let name_str = self.sym(name);
                    return Err(Self::boxed(
                        self.make_undefined_variable_error(name_str, *span),
                    ));
                }
            }
            Expression::Prefix {
                operator, right, ..
            } => {
                self.compile_non_tail_expression(right)?;
                match operator.as_str() {
                    "!" => self.emit(OpCode::OpBang, &[]),
                    "-" => self.emit(OpCode::OpMinus, &[]),
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &UNKNOWN_PREFIX_OPERATOR,
                            &[operator],
                            self.file_path.clone(),
                            expression.span(),
                        )));
                    }
                };
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => {
                if operator == "<" {
                    self.compile_non_tail_expression(right)?;
                    self.compile_non_tail_expression(left)?;
                    self.emit(OpCode::OpGreaterThan, &[]);
                    return Ok(());
                }

                if operator == "<=" {
                    self.compile_non_tail_expression(left)?;
                    self.compile_non_tail_expression(right)?;
                    self.emit(OpCode::OpLessThanOrEqual, &[]);
                    return Ok(());
                }
                // a && b: if a is falsy, result is a (short-circuit); otherwise result is b
                // OpJumpNotTruthy: peeks value, jumps if falsy (keeps value), pops if truthy
                if operator == "&&" {
                    self.compile_non_tail_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                    self.compile_non_tail_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }
                // a || b: if a is truthy, result is a (short-circuit); otherwise result is b
                // OpJumpTruthy: peeks value, jumps if truthy (keeps value), pops if falsy
                if operator == "||" {
                    self.compile_non_tail_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpTruthy, &[9999]);
                    self.compile_non_tail_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }

                self.compile_non_tail_expression(left)?;
                self.compile_non_tail_expression(right)?;

                match operator.as_str() {
                    "+" => self.emit(OpCode::OpAdd, &[]),
                    "-" => self.emit(OpCode::OpSub, &[]),
                    "*" => self.emit(OpCode::OpMul, &[]),
                    "/" => self.emit(OpCode::OpDiv, &[]),
                    "%" => self.emit(OpCode::OpMod, &[]),
                    "==" => self.emit(OpCode::OpEqual, &[]),
                    "!=" => self.emit(OpCode::OpNotEqual, &[]),
                    ">" => self.emit(OpCode::OpGreaterThan, &[]),
                    ">=" => self.emit(OpCode::OpGreaterThanOrEqual, &[]),
                    _ => {
                        return Err(Self::boxed(
                            Diagnostic::make_error(
                                &UNKNOWN_INFIX_OPERATOR,
                                &[operator],
                                self.file_path.clone(),
                                expression.span(),
                            )
                            .with_secondary_label(left.span(), "left operand")
                            .with_secondary_label(right.span(), "right operand"),
                        ));
                    }
                };
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.compile_if_expression(condition, consequence, alternative)?;
            }
            Expression::DoBlock { block, .. } => {
                self.compile_block_with_tail(block)?;
                if !self.block_has_value_tail(block) {
                    self.emit(OpCode::OpNone, &[]);
                }
            }
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                self.compile_function_literal(
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                )?;
            }
            Expression::ListLiteral { elements, .. } => {
                // Lower list literals through base `list(...)` to avoid deep
                // recursive lowering for large literals.
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("base list must be defined");
                self.load_symbol(&symbol);
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit(OpCode::OpCall, &[elements.len()]);
            }
            Expression::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit_array_count(elements.len());
            }
            Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit_tuple_count(elements.len());
            }
            Expression::EmptyList { .. } => {
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("base list must be defined");
                self.load_symbol(&symbol);
                self.emit(OpCode::OpCall, &[0]);
            }
            Expression::Hash { pairs, .. } => {
                let mut sorted_pairs: Vec<_> = pairs.iter().collect();
                sorted_pairs.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

                for (key, value) in sorted_pairs {
                    self.compile_non_tail_expression(key)?;
                    self.compile_non_tail_expression(value)?;
                }
                self.emit_hash_count(pairs.len() * 2);
            }
            Expression::Index { left, index, .. } => {
                self.compile_non_tail_expression(left)?;
                self.compile_non_tail_expression(index)?;
                self.emit(OpCode::OpIndex, &[]);
            }
            // Note: Pipe operator (|>) is handled at parse time by transforming
            // `a |> f(b, c)` into `f(a, b, c)` - a regular Call expression.
            // No special compilation needed here.
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.check_static_contract_call(function, arguments)?;

                if self.try_emit_adt_constructor_call(function, arguments, expression.span())? {
                    self.current_span = previous_span;
                    return Ok(());
                }

                if self.try_emit_primop_call(function, arguments)? {
                    self.current_span = previous_span;
                    return Ok(());
                }
                if self.try_emit_call_base(function, arguments)? {
                    self.current_span = previous_span;
                    return Ok(());
                }

                // Check if this is a self recursive tail call
                let is_self_tail_call = self.in_tail_position && self.is_self_call(function);

                self.compile_non_tail_expression(function)?;

                let mut consumable_counts: HashMap<Symbol, usize> = HashMap::new();

                if is_self_tail_call {
                    for argument in arguments {
                        self.collect_consumable_param_uses(argument, &mut consumable_counts);
                    }
                }

                for argument in arguments {
                    if is_self_tail_call {
                        self.compile_tail_call_argument(argument, &consumable_counts)?;
                    } else {
                        self.compile_non_tail_expression(argument)?;
                    }
                }

                // Emit OpTailCall for self recursive tail calls otherwise OpCall
                if is_self_tail_call {
                    self.emit(OpCode::OpTailCall, &[arguments.len()]);
                } else {
                    self.emit(OpCode::OpCall, &[arguments.len()]);
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                let expr_span = expression.span();
                let member = *member;

                if let Expression::Identifier { name, .. } = object.as_ref()
                    && self.is_base_module_symbol(*name)
                {
                    let member_name = self.sym(member);
                    if let Some(index) = BaseModule::new().index_of(member_name) {
                        self.emit(OpCode::OpGetBase, &[index]);
                        return Ok(());
                    }
                    return Err(Self::boxed(Diagnostic::make_error(
                        &UNKNOWN_BASE_MEMBER,
                        &[member_name],
                        self.file_path.clone(),
                        expr_span,
                    )));
                }

                let (module_binding_name, module_name) = match object.as_ref() {
                    Expression::Identifier { name, .. } => {
                        let name = *name;
                        if let Some(target) = self.import_aliases.get(&name) {
                            (Some(name), Some(*target))
                        } else if self.imported_modules.contains(&name)
                            || self.current_module_prefix == Some(name)
                        {
                            (Some(name), Some(name))
                        } else {
                            (Some(name), None)
                        }
                    }
                    _ => (None, None),
                };

                if let Some(module_name) = module_name {
                    if let Some(binding_name) = module_binding_name
                        && self
                            .imported_module_exclusions
                            .get(&binding_name)
                            .is_some_and(|excluded| excluded.contains(&member))
                    {
                        let module_name_str = self.sym(module_name);
                        let member_str = self.sym(member);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &UNKNOWN_MODULE_MEMBER,
                            &[module_name_str, member_str],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }

                    let member_str = self.sym(member);
                    self.check_private_member(member_str, expr_span, Some(self.sym(module_name)))?;

                    let qualified = self.interner.intern_join(module_name, member);
                    // Module Constants check if this is a compile-time constant
                    // If so, inline the constant value directly instead of loading from symbol
                    if let Some(constant_value) = self.module_constants.get(&qualified) {
                        self.emit_constant_value(constant_value.clone());
                        return Ok(());
                    }

                    if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }

                    let module_name_str = self.sym(module_name);
                    let member_str = self.sym(member);

                    return Err(Self::boxed(Diagnostic::make_error(
                        &UNKNOWN_MODULE_MEMBER,
                        &[module_name_str, member_str],
                        self.file_path.clone(),
                        expr_span,
                    )));
                }

                if let Some(name) = module_binding_name
                    && module_name.is_none()
                    && is_valid_module_name(self.sym(name))
                {
                    let has_symbol = self.resolve_visible_symbol(name).is_some();
                    if !has_symbol {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NOT_IMPORTED,
                            &[name_str],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }
                }

                let member_str = self.sym(member);
                self.check_private_member(member_str, expr_span, None)?;

                // Compile the object (e.g., the module identifier)
                self.compile_non_tail_expression(object)?;

                // Emit the member name as a string constant (the hash key)
                let member_str = self.sym(member).to_string();
                let member_idx = self.add_constant(Value::String(member_str.into()));
                self.emit_constant_index(member_idx);

                // Use index operation to access the member from the hash
                self.emit(OpCode::OpIndex, &[]);
                // Member access should yield the value, not Option.
                self.emit(OpCode::OpUnwrapSome, &[]);
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                self.compile_non_tail_expression(object)?;
                self.emit(OpCode::OpTupleIndex, &[*index]);
            }
            Expression::None { .. } => {
                self.emit(OpCode::OpNone, &[]);
            }
            Expression::Some { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpSome, &[]);
            }
            // Either type expressions
            Expression::Left { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpLeft, &[]);
            }
            Expression::Right { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpRight, &[]);
            }
            Expression::Match {
                scrutinee,
                arms,
                span,
            } => {
                self.compile_match_expression(scrutinee, arms, *span)?;
            }
            Expression::Cons { head, tail, .. } => {
                if let Expression::None { span } = tail.as_ref() {
                    return Err(Self::boxed(Diagnostic::make_error(
                        &LEGACY_LIST_TAIL_NONE,
                        &[],
                        self.file_path.clone(),
                        *span,
                    )));
                }
                self.compile_non_tail_expression(head)?;
                self.compile_non_tail_expression(tail)?;
                self.emit(OpCode::OpCons, &[]);
            }
            Expression::Perform {
                effect,
                operation,
                args,
                ..
            } => {
                self.compile_perform(*effect, *operation, args)?;
            }
            Expression::Handle {
                expr, effect, arms, ..
            } => {
                self.compile_handle(expr, *effect, arms)?;
            }
        }
        self.current_span = previous_span;
        Ok(())
    }

    fn check_static_contract_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<()> {
        self.check_direct_builtin_effect_call(function)?;

        let Some(contract) = self.resolve_call_contract(function, arguments.len()) else {
            return Ok(());
        };

        if !contract.effects.is_empty() {
            let effect_var_bindings = self.bind_effect_vars_for_call(contract, arguments);
            let resolved_bindings = self.resolve_effect_var_bindings(&effect_var_bindings);
            let mut required_effects: HashSet<Symbol> = HashSet::new();
            for required in &contract.effects {
                let required_name = match required {
                    crate::syntax::effect_expr::EffectExpr::Named { name, .. } => *name,
                };
                if self.is_effect_variable(required_name) {
                    if let Some(bound) = resolved_bindings.get(&required_name) {
                        required_effects.extend(bound.iter().copied());
                    }
                } else {
                    required_effects.insert(required_name);
                }
            }

            for required_name in required_effects {
                if !self.is_effect_available(required_name) {
                    let function_name = match function {
                        Expression::Identifier { name, .. } => self.sym(*name).to_string(),
                        Expression::MemberAccess { member, .. } => self.sym(*member).to_string(),
                        _ => "<call>".to_string(),
                    };
                    let missing = self.sym(required_name).to_string();
                    return Err(Self::boxed(
                        Diagnostic::make_error_dynamic(
                            "E400",
                            "MISSING EFFECT",
                            ErrorType::Compiler,
                            format!(
                                "Call to `{}` requires effect `{}` in this function signature.",
                                function_name, missing
                            ),
                            Some(format!("Add `with {}` to the enclosing function.", missing)),
                            self.file_path.clone(),
                            function.span(),
                        )
                        .with_primary_label(function.span(), "effectful call occurs here"),
                    ));
                }
            }
        }

        for (index, argument) in arguments.iter().enumerate() {
            let Some(expected_ty) = contract.params.get(index).and_then(|p| p.as_ref()) else {
                continue;
            };
            let Some(expected_runtime) = convert_type_expr(expected_ty, &self.interner) else {
                continue;
            };
            let Some(actual_runtime) = self.static_expr_type(argument) else {
                continue;
            };
            if !Self::runtime_types_compatible(&expected_runtime, &actual_runtime) {
                let expected = expected_runtime.type_name();
                let actual = actual_runtime.type_name();

                return Err(Self::boxed(
                    Diagnostic::make_error(
                        &TYPE_MISMATCH,
                        &[&expected, &actual],
                        self.file_path.clone(),
                        argument.span(),
                    )
                    .with_primary_label(argument.span(), "argument type is known at compile time")
                    .with_help(format!(
                        "argument #{} does not match function contract",
                        index + 1
                    )),
                ));
            }
        }

        Ok(())
    }

    fn check_direct_builtin_effect_call(&mut self, function: &Expression) -> CompileResult<()> {
        let required_name = match function {
            Expression::Identifier { name, .. } => {
                let Some(binding) = self.resolve_visible_symbol(*name) else {
                    return Ok(());
                };
                if binding.symbol_scope != SymbolScope::Base {
                    return Ok(());
                }
                self.required_effect_for_base_name(self.sym(*name))
            }
            _ => None,
        };

        let Some(required_name) = required_name else {
            return Ok(());
        };

        if self.is_effect_available_name(required_name) {
            return Ok(());
        }

        let function_name = match function {
            Expression::Identifier { name, .. } => self.sym(*name).to_string(),
            _ => "<call>".to_string(),
        };
        Err(Self::boxed(
            Diagnostic::make_error_dynamic(
                "E400",
                "MISSING EFFECT",
                ErrorType::Compiler,
                format!(
                    "Call to `{}` requires effect `{}` in this function signature.",
                    function_name, required_name
                ),
                Some(format!(
                    "Add `with {}` to the enclosing function.",
                    required_name
                )),
                self.file_path.clone(),
                function.span(),
            )
            .with_primary_label(function.span(), "effectful call occurs here"),
        ))
    }

    fn required_effect_for_base_name(&self, base_name: &str) -> Option<&'static str> {
        match base_name {
            "print" | "read_file" | "read_lines" | "read_stdin" => Some("IO"),
            "now" | "clock_now" => Some("Time"),
            _ => None,
        }
    }

    fn bind_effect_vars_for_call(
        &self,
        contract: &FnContract,
        arguments: &[Expression],
    ) -> HashMap<Symbol, HashSet<Symbol>> {
        let mut bindings: HashMap<Symbol, HashSet<Symbol>> = HashMap::new();

        for (idx, argument) in arguments.iter().enumerate() {
            let Some(Some(TypeExpr::Function {
                params,
                effects: param_effects,
                ..
            })) = contract.params.get(idx)
            else {
                continue;
            };

            let arg_effects = self.infer_argument_function_effects(argument, params.len());
            if arg_effects.is_empty() {
                continue;
            }

            for effect in param_effects {
                let effect_name = match effect {
                    crate::syntax::effect_expr::EffectExpr::Named { name, .. } => *name,
                };
                if self.is_effect_variable(effect_name) {
                    bindings
                        .entry(effect_name)
                        .or_default()
                        .extend(arg_effects.iter().copied());
                }
            }
        }

        bindings
    }

    fn resolve_effect_var_bindings(
        &self,
        bindings: &HashMap<Symbol, HashSet<Symbol>>,
    ) -> HashMap<Symbol, HashSet<Symbol>> {
        let mut resolved: HashMap<Symbol, HashSet<Symbol>> = HashMap::new();
        for var in bindings.keys().copied() {
            let mut out = HashSet::new();
            let mut visiting = HashSet::new();
            self.collect_resolved_effect_atoms(var, bindings, &mut visiting, &mut out);
            resolved.insert(var, out);
        }
        resolved
    }

    fn collect_resolved_effect_atoms(
        &self,
        current: Symbol,
        bindings: &HashMap<Symbol, HashSet<Symbol>>,
        visiting: &mut HashSet<Symbol>,
        out: &mut HashSet<Symbol>,
    ) {
        if !visiting.insert(current) {
            return;
        }
        let Some(bound) = bindings.get(&current) else {
            visiting.remove(&current);
            return;
        };
        for effect in bound {
            if self.is_effect_variable(*effect) {
                self.collect_resolved_effect_atoms(*effect, bindings, visiting, out);
            } else {
                out.insert(*effect);
            }
        }
        visiting.remove(&current);
    }

    fn infer_argument_function_effects(
        &self,
        argument: &Expression,
        expected_arity: usize,
    ) -> HashSet<Symbol> {
        match argument {
            Expression::Function { effects, .. } => effects
                .iter()
                .map(|effect| match effect {
                    crate::syntax::effect_expr::EffectExpr::Named { name, .. } => *name,
                })
                .collect(),
            Expression::Identifier { name, .. } => self
                .lookup_unqualified_contract(*name, expected_arity)
                .map(|contract| {
                    contract
                        .effects
                        .iter()
                        .map(|effect| match effect {
                            crate::syntax::effect_expr::EffectExpr::Named { name, .. } => *name,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Expression::MemberAccess { object, member, .. } => {
                let Expression::Identifier { name, .. } = object.as_ref() else {
                    return HashSet::new();
                };
                let module_name = if let Some(target) = self.import_aliases.get(name) {
                    Some(*target)
                } else if self.imported_modules.contains(name)
                    || self.current_module_prefix == Some(*name)
                {
                    Some(*name)
                } else {
                    None
                };
                module_name
                    .and_then(|module_name| self.lookup_contract(Some(module_name), *member, expected_arity))
                    .map(|contract| {
                        contract
                            .effects
                            .iter()
                            .map(|effect| match effect {
                                crate::syntax::effect_expr::EffectExpr::Named { name, .. } => *name,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            _ => HashSet::new(),
        }
    }

    fn resolve_call_contract<'a>(
        &'a self,
        function: &Expression,
        arity: usize,
    ) -> Option<&'a FnContract> {
        match function {
            Expression::Identifier { name, .. } => self.lookup_unqualified_contract(*name, arity),
            Expression::MemberAccess { object, member, .. } => {
                let Expression::Identifier { name, .. } = object.as_ref() else {
                    return None;
                };
                let module_name = if let Some(target) = self.import_aliases.get(name) {
                    Some(*target)
                } else if self.imported_modules.contains(name)
                    || self.current_module_prefix == Some(*name)
                {
                    Some(*name)
                } else {
                    None
                }?;
                self.lookup_contract(Some(module_name), *member, arity)
            }
            _ => None,
        }
    }

    pub(super) fn static_expr_type(&self, expression: &Expression) -> Option<RuntimeType> {
        match expression {
            Expression::Integer { .. } => Some(RuntimeType::Int),
            Expression::Float { .. } => Some(RuntimeType::Float),
            Expression::Boolean { .. } => Some(RuntimeType::Bool),
            Expression::String { .. } | Expression::InterpolatedString { .. } => {
                Some(RuntimeType::String)
            }
            Expression::Identifier { name, .. } => {
                // 1. Annotated types (from contract collection) have highest priority.
                if let Some(rt) = self.lookup_static_type(*name) {
                    return Some(rt);
                }
                // 2. Fall back to HM-inferred types for unannotated let/fn bindings.
                //    Only use monomorphic (non-generic), non-Any results to avoid
                //    false confidence on unresolved type variables.
                if let Some(scheme) = self.type_env.lookup(*name) {
                    if scheme.forall.is_empty() {
                        let rt = TypeEnv::to_runtime(&scheme.infer_type, &Default::default());
                        if rt != RuntimeType::Any {
                            return Some(rt);
                        }
                    }
                }
                None
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let contract = self.resolve_call_contract(function, arguments.len())?;
                self.infer_call_return_type(contract, arguments)
            }
            Expression::TupleLiteral { elements, .. } => Some(RuntimeType::Tuple(
                elements
                    .iter()
                    .map(|e| self.static_expr_type(e))
                    .collect::<Option<Vec<_>>>()?,
            )),
            Expression::ArrayLiteral { elements, .. } => {
                if elements.is_empty() {
                    return Some(RuntimeType::Array(Box::new(RuntimeType::Any)));
                }
                let first = self.static_expr_type(&elements[0])?;
                if elements[1..]
                    .iter()
                    .all(|e| self.static_expr_type(e).is_some_and(|t| t == first))
                {
                    Some(RuntimeType::Array(Box::new(first)))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn infer_call_return_type(
        &self,
        contract: &FnContract,
        arguments: &[Expression],
    ) -> Option<RuntimeType> {
        let ret = contract.ret.as_ref()?;
        // Fast path for non-generic returns.
        if let Some(rt) = convert_type_expr(ret, &self.interner) {
            return Some(rt);
        }

        let mut substitutions: HashMap<Symbol, RuntimeType> = HashMap::new();
        for (idx, argument) in arguments.iter().enumerate() {
            let Some(param_ty) = contract.params.get(idx).and_then(|p| p.as_ref()) else {
                continue;
            };
            let Some(actual_ty) = self.static_expr_type(argument) else {
                continue;
            };
            self.match_type_expr_to_runtime(param_ty, &actual_ty, &mut substitutions)?;
        }

        self.instantiate_type_expr(ret, &substitutions)
    }

    fn match_type_expr_to_runtime(
        &self,
        expected: &TypeExpr,
        actual: &RuntimeType,
        substitutions: &mut HashMap<Symbol, RuntimeType>,
    ) -> Option<()> {
        match expected {
            TypeExpr::Named { name, args, .. } => {
                let name_str = self.sym(*name);
                if args.is_empty() {
                    if let Some(expected_runtime) = convert_type_expr(expected, &self.interner) {
                        return Self::runtime_types_compatible(&expected_runtime, actual).then_some(());
                    }
                    // Treat unresolved named type in this position as a generic variable.
                    if let Some(bound) = substitutions.get(name) {
                        return Self::runtime_types_compatible(bound, actual).then_some(());
                    }
                    substitutions.insert(*name, actual.clone());
                    return Some(());
                }

                match (name_str, args.len(), actual) {
                    ("Option", 1, RuntimeType::Option(inner)) => {
                        self.match_type_expr_to_runtime(&args[0], inner, substitutions)
                    }
                    ("Array", 1, RuntimeType::Array(inner)) => {
                        self.match_type_expr_to_runtime(&args[0], inner, substitutions)
                    }
                    ("Map", 2, RuntimeType::Map(k, v)) => {
                        self.match_type_expr_to_runtime(&args[0], k, substitutions)?;
                        self.match_type_expr_to_runtime(&args[1], v, substitutions)
                    }
                    _ => None,
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                let RuntimeType::Tuple(actual_elements) = actual else {
                    return None;
                };
                if elements.len() != actual_elements.len() {
                    return None;
                }
                for (expected_elem, actual_elem) in elements.iter().zip(actual_elements.iter()) {
                    self.match_type_expr_to_runtime(expected_elem, actual_elem, substitutions)?;
                }
                Some(())
            }
            TypeExpr::Function { .. } => None,
        }
    }

    fn instantiate_type_expr(
        &self,
        ty: &TypeExpr,
        substitutions: &HashMap<Symbol, RuntimeType>,
    ) -> Option<RuntimeType> {
        if let Some(runtime) = convert_type_expr(ty, &self.interner) {
            return Some(runtime);
        }
        match ty {
            TypeExpr::Named { name, args, .. } => {
                if args.is_empty() {
                    return substitutions.get(name).cloned();
                }
                let name_str = self.sym(*name);
                match (name_str, args.len()) {
                    ("Option", 1) => Some(RuntimeType::Option(Box::new(
                        self.instantiate_type_expr(&args[0], substitutions)?,
                    ))),
                    ("Array", 1) => Some(RuntimeType::Array(Box::new(
                        self.instantiate_type_expr(&args[0], substitutions)?,
                    ))),
                    ("Map", 2) => Some(RuntimeType::Map(
                        Box::new(self.instantiate_type_expr(&args[0], substitutions)?),
                        Box::new(self.instantiate_type_expr(&args[1], substitutions)?),
                    )),
                    _ => None,
                }
            }
            TypeExpr::Tuple { elements, .. } => Some(RuntimeType::Tuple(
                elements
                    .iter()
                    .map(|elem| self.instantiate_type_expr(elem, substitutions))
                    .collect::<Option<Vec<_>>>()?,
            )),
            TypeExpr::Function { .. } => None,
        }
    }

    pub(super) fn runtime_types_compatible(expected: &RuntimeType, actual: &RuntimeType) -> bool {
        match expected {
            RuntimeType::Any => true,
            RuntimeType::Option(inner_expected) => match actual {
                RuntimeType::Option(inner_actual) => {
                    Self::runtime_types_compatible(inner_expected, inner_actual)
                }
                _ => false,
            },
            RuntimeType::Array(inner_expected) => match actual {
                RuntimeType::Array(inner_actual) => {
                    Self::runtime_types_compatible(inner_expected, inner_actual)
                }
                _ => false,
            },
            RuntimeType::Map(_, _) => matches!(actual, RuntimeType::Map(_, _)),
            RuntimeType::Tuple(expected_elems) => match actual {
                RuntimeType::Tuple(actual_elems) if expected_elems.len() == actual_elems.len() => {
                    expected_elems
                        .iter()
                        .zip(actual_elems.iter())
                        .all(|(e, a)| Self::runtime_types_compatible(e, a))
                }
                _ => false,
            },
            _ => expected == actual,
        }
    }

    pub(super) fn compile_function_literal(
        &mut self,
        parameters: &[Symbol],
        parameters_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[crate::syntax::effect_expr::EffectExpr],
        body: &Block,
    ) -> CompileResult<()> {
        if let Some(name) = Self::find_duplicate_name(parameters) {
            let name_str = self.sym(name);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[name_str],
                self.file_path.clone(),
                Span::new(Position::default(), Position::default()),
            )));
        }

        self.enter_scope();

        for (index, param) in parameters.iter().enumerate() {
            self.symbol_table.define(*param, Span::default());
            if let Some(Some(param_ty)) = parameters_types.get(index)
                && let Some(runtime_ty) = convert_type_expr(param_ty, &self.interner)
            {
                self.bind_static_type(*param, runtime_ty);
            }
        }

        self.with_function_context(parameters.len(), effects, |compiler| {
            compiler.compile_block_with_tail(body)
        })?;

        if self.block_has_value_tail(body) {
            if self.is_last_instruction(OpCode::OpPop) {
                self.replace_last_pop_with_return();
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

        let free_symbols = self.symbol_table.free_symbols.clone();

        for free in &free_symbols {
            if free.symbol_scope == SymbolScope::Local {
                self.mark_captured_in_current_function(free.index);
            }
        }

        let num_locals = self.symbol_table.num_definitions;

        let (instructions, locations, files, effect_summary) = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let runtime_contract = {
            let contract = FnContract {
                params: parameters_types.to_vec(),
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
                    FunctionDebugInfo::new(None, files, locations)
                        .with_effect_summary(effect_summary),
                ),
            )
            .with_contract(runtime_contract),
        )));

        self.emit_closure_index(fn_idx, free_symbols.len());

        Ok(())
    }

    pub(super) fn compile_interpolated_string(
        &mut self,
        parts: &[StringPart],
    ) -> CompileResult<()> {
        if parts.is_empty() {
            // Empty interpolated string - just push an empty string
            let idx = self.add_constant(Value::String(String::new().into()));
            self.emit_constant_index(idx);
            return Ok(());
        }

        // Compile the first part
        match &parts[0] {
            StringPart::Literal(s) => {
                let idx = self.add_constant(Value::String(s.clone().into()));
                self.emit_constant_index(idx);
            }
            StringPart::Interpolation(expression) => {
                self.compile_non_tail_expression(expression)?;
                self.emit(OpCode::OpToString, &[]);
            }
        }

        // Compile remaining parts, concatenating each with OpAdd
        for part in &parts[1..] {
            match part {
                StringPart::Literal(s) => {
                    let idx = self.add_constant(Value::String(s.clone().into()));
                    self.emit_constant_index(idx);
                }
                StringPart::Interpolation(expression) => {
                    self.compile_non_tail_expression(expression)?;
                    self.emit(OpCode::OpToString, &[]);
                }
            }
            self.emit(OpCode::OpAdd, &[]);
        }

        Ok(())
    }

    pub(super) fn compile_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
    ) -> CompileResult<()> {
        self.compile_non_tail_expression(condition)?;

        let jump_not_truthy_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);

        let consequence_has_value = self.block_has_value_tail(consequence);
        // Consequence branch inherits tail position
        if self.in_tail_position {
            self.compile_block_with_tail(consequence)?;
        } else {
            self.compile_block(consequence)?;
        }

        if consequence_has_value {
            if self.is_last_instruction(OpCode::OpPop) {
                self.remove_last_pop();
            }
        } else {
            self.emit(OpCode::OpNone, &[]);
        }

        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
        self.change_operand(jump_not_truthy_pos, self.current_instructions().len());

        // Pop the condition value that was left on stack when we jumped here
        // (OpJumpNotTruthy keeps value on stack when jumping for short-circuit support)
        self.emit(OpCode::OpPop, &[]);

        // Alternative branch also inherits tail position
        if let Some(alt) = alternative {
            let alternative_has_value = self.block_has_value_tail(alt);
            if self.in_tail_position {
                self.compile_block_with_tail(alt)?;
            } else {
                self.compile_block(alt)?;
            }

            if alternative_has_value {
                if self.is_last_instruction(OpCode::OpPop) {
                    self.remove_last_pop();
                }
            } else {
                self.emit(OpCode::OpNone, &[]);
            }
        } else {
            self.emit(OpCode::OpNone, &[]);
        }

        self.change_operand(jump_pos, self.current_instructions().len());
        Ok(())
    }

    pub(super) fn compile_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        match_span: Span,
    ) -> CompileResult<()> {
        // Exhaustiveness check for ADT patterns (before compiling arms)
        self.check_match_exhaustiveness(arms, match_span)?;
        // Compile scrutinee once and store it in a temp symbol.
        // Keep it in the current scope so top-level matches use globals, not stack-backed locals.
        self.compile_non_tail_expression(scrutinee)?;
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
                    &ICE_TEMP_SYMBOL_MATCH,
                    &[],
                    self.file_path.clone(),
                    Span::new(Position::default(), Position::default()),
                )));
            }
        };

        let mut end_jumps = Vec::new();
        let mut next_arm_jumps: Vec<usize> = Vec::new();

        // Compile each arm
        for arm in arms {
            if !next_arm_jumps.is_empty() {
                let arm_start = self.current_instructions().len();
                for jump_pos in next_arm_jumps.drain(..) {
                    self.change_operand(jump_pos, arm_start);
                }
                // A failed pattern/guard jump leaves its condition on stack.
                self.emit(OpCode::OpPop, &[]);
            }

            // Check whether pattern matches and collect jumps to the next arm.
            let mut arm_next_jumps = self.compile_pattern_check(&temp_symbol, &arm.pattern)?;

            self.enter_block_scope();
            self.compile_pattern_bind(&temp_symbol, &arm.pattern)?;

            // Guard runs only after a successful pattern match and in the arm binding scope.
            if let Some(guard) = &arm.guard {
                self.compile_non_tail_expression(guard)?;
                arm_next_jumps.push(self.emit(OpCode::OpJumpNotTruthy, &[9999]));
            }

            if self.in_tail_position {
                self.with_tail_position(true, |compiler| compiler.compile_expression(&arm.body))?;
            } else {
                self.compile_expression(&arm.body)?;
            }
            self.leave_block_scope();

            // Jump to end after executing this arm's body.
            end_jumps.push(self.emit(OpCode::OpJump, &[9999]));
            next_arm_jumps = arm_next_jumps;
        }

        // If no arm matched (or all guards failed), leave a sentinel value on stack.
        if !next_arm_jumps.is_empty() {
            let no_match_start = self.current_instructions().len();
            for jump_pos in next_arm_jumps {
                self.change_operand(jump_pos, no_match_start);
            }
            self.emit(OpCode::OpPop, &[]);
        }
        self.emit(OpCode::OpNone, &[]);

        // Patch all end jumps to point here
        for jump_pos in end_jumps {
            self.change_operand(jump_pos, self.current_instructions().len());
        }

        Ok(())
    }

    pub(super) fn compile_pattern_check(
        &mut self,
        scrutinee: &Binding,
        pattern: &Pattern,
    ) -> CompileResult<Vec<usize>> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Wildcard always matches, so we never jump to next arm
                // Emit OpTrue and OpJumpNotTruthy (which will never jump)
                // Actually, for wildcard we should always execute this arm
                // So we return a dummy jump position that will never be used
                // For simplicity, emit a condition that's always true
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::Literal { expression, .. } => {
                // Push pattern value onto stack: [scrutinee, pattern]
                // OpEqual compares and pushes boolean: [result]
                // OpJumpNotTruthy jumps when false (no match), continues when true (match)
                self.load_symbol(scrutinee);
                self.compile_non_tail_expression(expression)?;
                self.emit(OpCode::OpEqual, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::None { .. } => {
                // Check if scrutinee is None
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpNone, &[]);
                self.emit(OpCode::OpEqual, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::Some { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsSome, &[]);
                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapSome, &[]);
                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_SOME_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }
                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Left { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsLeft, &[]);

                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapLeft, &[]);

                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_LEFT_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }

                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Right { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsRight, &[]);

                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapRight, &[]);

                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_RIGHT_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }

                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Identifier { .. } => {
                // Identifier patterns always match; value binding is performed in
                // `compile_pattern_bind` after a successful check.
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::EmptyList { .. } => {
                // Check if scrutinee is an empty list
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsEmptyList, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::Cons { head, tail, .. } => {
                // Check if scrutinee is a non-empty cons cell
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsCons, &[]);
                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                // Check head pattern
                let head_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsHead, &[]);

                match head_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[head_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[head_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                match head.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {}
                    _ => {
                        let head_jumps = self.compile_pattern_check(&head_symbol, head)?;
                        jumps.extend(head_jumps);
                    }
                }

                // Check tail pattern
                let tail_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsTail, &[]);

                match tail_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[tail_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[tail_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                match tail.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {}
                    _ => {
                        let tail_jumps = self.compile_pattern_check(&tail_symbol, tail)?;
                        jumps.extend(tail_jumps);
                    }
                }

                Ok(jumps)
            }
            Pattern::Tuple { elements, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsTuple, &[]);
                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                for (index, element) in elements.iter().enumerate() {
                    match element {
                        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => continue,
                        _ => {}
                    }
                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpTupleIndex, &[index]);
                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    let inner_jumps = self.compile_pattern_check(&inner_symbol, element)?;
                    jumps.extend(inner_jumps);
                }

                Ok(jumps)
            }
            Pattern::Constructor { name, fields, span } => {
                // 1. Check if this is a known constructor
                let Some(constructor_info) = self.adt_registry.lookup_constructor(*name) else {
                    let name_str = self.interner.resolve(*name).to_string();
                    return Err(Self::boxed(
                        diag_enhanced(&UNKNOWN_CONSTRUCTOR)
                            .with_span(*span)
                            .with_message(format!("Unknown constructor `{}`.", name_str)),
                    ));
                };

                // tag_idx not used in check
                let _ = constructor_info.tag_idx;

                // 2. Load scrutinee, emit OpIsAdt with constructor name constant
                self.load_symbol(scrutinee);
                let constructor_name = self.interner.resolve(*name).to_string();
                let const_idx =
                    self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
                self.emit(OpCode::OpIsAdt, &[const_idx]);

                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                // 3. For each non-wildcard/non-identifier field pattern, extract and sub-check
                for (field_idx, field_pat) in fields.iter().enumerate() {
                    if matches!(
                        field_pat,
                        Pattern::Wildcard { .. } | Pattern::Identifier { .. }
                    ) {
                        continue;
                    }

                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpAdtField, &[field_idx]);

                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    let inner_jumps = self.compile_pattern_check(&inner_symbol, field_pat)?;
                    jumps.extend(inner_jumps);
                }

                Ok(jumps)
            }
        }
    }

    pub(super) fn compile_pattern_bind(
        &mut self,
        scrutinee: &Binding,
        pattern: &Pattern,
    ) -> CompileResult<()> {
        match pattern {
            Pattern::Identifier { name, span } => {
                self.load_symbol(scrutinee);
                let symbol = self.symbol_table.define(*name, *span);
                match symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_SYMBOL_SCOPE_PATTERN,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                };
            }
            Pattern::Some { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapSome, &[]);
                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_SOME_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            // Either type pattern bindings
            Pattern::Left { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapLeft, &[]);

                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_LEFT_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            Pattern::Right { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapRight, &[]);
                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_RIGHT_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            Pattern::EmptyList { .. } => {}
            Pattern::Cons { head, tail, .. } => {
                // Bind head
                let head_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsHead, &[]);
                match head_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[head_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[head_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&head_symbol, head)?;

                // Bind tail
                let tail_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsTail, &[]);
                match tail_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[tail_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[tail_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&tail_symbol, tail)?;
            }
            Pattern::Tuple { elements, .. } => {
                for (index, element) in elements.iter().enumerate() {
                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpTupleIndex, &[index]);
                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&inner_symbol, element)?;
                }
            }
            Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
            Pattern::Constructor { fields, .. } => {
                for (field_idx, field_pat) in fields.iter().enumerate() {
                    if matches!(field_pat, Pattern::Wildcard { .. }) {
                        continue;
                    }

                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpAdtField, &[field_idx]);

                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&inner_symbol, field_pat)?;
                }
            }
        }
        Ok(())
    }

    fn try_emit_primop_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<bool> {
        let Expression::Identifier { name, .. } = function else {
            return Ok(false);
        };
        if self.excluded_base_symbols.contains(name) {
            return Ok(false);
        }

        // Shadowed names must resolve through the regular call path.
        if let Some(symbol) = self.resolve_visible_symbol(*name)
            && symbol.symbol_scope != SymbolScope::Base
        {
            return Ok(false);
        }

        let primop = resolve_primop_call(self.sym(*name), arguments.len());

        let Some(primop) = primop else {
            return Ok(false);
        };

        let required_name = match primop.effect_kind() {
            PrimEffect::Io => Some("IO"),
            PrimEffect::Time => Some("Time"),
            PrimEffect::Control | PrimEffect::Pure => None,
        };
        if let Some(required_name) = required_name && !self.is_effect_available_name(required_name) {
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Call to `{}` requires effect `{}` in this function signature.",
                        self.sym(*name),
                        required_name
                    ),
                    Some(format!(
                        "Add `with {}` to the enclosing function.",
                        required_name
                    )),
                    self.file_path.clone(),
                    function.span(),
                )
                .with_primary_label(function.span(), "effectful call occurs here"),
            ));
        }

        for argument in arguments {
            self.compile_non_tail_expression(argument)?;
        }

        self.emit(OpCode::OpPrimOp, &[primop.id() as usize, arguments.len()]);
        Ok(true)
    }

    fn try_emit_call_base(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<bool> {
        let Expression::Identifier { name, .. } = function else {
            return Ok(false);
        };

        let base_name = self.sym(*name).to_string();
        if !is_base_fastcall_allowlisted(base_name.as_str()) {
            return Ok(false);
        }

        let Some(symbol) = self.resolve_visible_symbol(*name) else {
            return Ok(false);
        };
        if symbol.symbol_scope != SymbolScope::Base {
            return Ok(false);
        }

        if let Some(required_name) = self.required_effect_for_base_name(base_name.as_str())
            && !self.is_effect_available_name(required_name)
        {
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Call to `{}` requires effect `{}` in this function signature.",
                        base_name, required_name
                    ),
                    Some(format!(
                        "Add `with {}` to the enclosing function.",
                        required_name
                    )),
                    self.file_path.clone(),
                    function.span(),
                )
                .with_primary_label(function.span(), "effectful call occurs here"),
            ));
        }

        for argument in arguments {
            self.compile_non_tail_expression(argument)?;
        }
        self.emit(OpCode::OpCallBase, &[symbol.index, arguments.len()]);
        Ok(true)
    }

    /// Compile `perform Effect.op(args)` — push args, then `OpPerform`.
    fn compile_perform(
        &mut self,
        effect: Symbol,
        op: Symbol,
        args: &[Expression],
    ) -> CompileResult<()> {
        let span = self
            .current_span
            .unwrap_or_else(|| Span::new(Position::default(), Position::default()));

        let Some(has_operation) = self.effect_declared_ops(effect).map(|ops| ops.contains(&op))
        else {
            let effect_name = self.sym(effect).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E403",
                    "UNKNOWN EFFECT",
                    ErrorType::Compiler,
                    format!("Effect `{}` is not declared.", effect_name),
                    Some("Declare the effect before using `perform`.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "unknown effect in perform"),
            ));
        };
        if !has_operation {
            let effect_name = self.sym(effect).to_string();
            let op_name = self.sym(op).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E404",
                    "UNKNOWN EFFECT OPERATION",
                    ErrorType::Compiler,
                    format!(
                        "Effect `{}` has no declared operation `{}`.",
                        effect_name, op_name
                    ),
                    Some("Add the operation to the effect declaration or rename it.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "unknown operation in perform"),
            ));
        }
        if !self.is_effect_available(effect) {
            let effect_name = self.sym(effect).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Performing `{}` requires effect `{}` in this function signature.",
                        self.sym(op),
                        effect_name
                    ),
                    Some(format!("Add `with {}` to the enclosing function.", effect_name)),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "effectful perform occurs here"),
            ));
        }

        for arg in args {
            self.compile_non_tail_expression(arg)?;
        }

        let effect_name = self.interner.resolve(effect).to_string().into_boxed_str();
        let op_name = self.interner.resolve(op).to_string().into_boxed_str();
        let desc = Value::PerformDescriptor(Rc::new(PerformDescriptor {
            effect,
            op,
            effect_name,
            op_name,
        }));
        let const_idx = self.add_constant(desc);
        self.emit(OpCode::OpPerform, &[const_idx, args.len()]);

        Ok(())
    }

    /// Compile `expr handle Effect { op(resume, args) -> body, ... }`.
    ///
    /// Emits one `OpClosure` per arm (leaving closures on the stack in order),
    /// then `OpHandle desc_idx`, then the handled `expr`, then `OpEndHandle`.
    fn compile_handle(
        &mut self,
        expr: &Expression,
        effect: Symbol,
        arms: &[HandleArm],
    ) -> CompileResult<()> {
        let mut operations = Vec::new();
        let mut arm_ops = HashSet::new();

        if let Some(declared_ops) = self.effect_declared_ops(effect) {
            for arm in arms {
                if !declared_ops.contains(&arm.operation_name) {
                    let effect_name = self.sym(effect).to_string();
                    let op_name = self.sym(arm.operation_name).to_string();
                    return Err(Self::boxed(
                        Diagnostic::make_error_dynamic(
                            "E401",
                            "UNKNOWN HANDLER OPERATION",
                            ErrorType::Compiler,
                            format!(
                                "Handler for `{}` includes unknown operation `{}`.",
                                effect_name, op_name
                            ),
                            Some("Add this operation to the effect declaration or remove the arm.".to_string()),
                            self.file_path.clone(),
                            arm.span,
                        )
                        .with_primary_label(arm.span, "unknown operation arm"),
                    ));
                }
                arm_ops.insert(arm.operation_name);
            }

            let mut missing: Vec<Symbol> = declared_ops.difference(&arm_ops).copied().collect();
            if !missing.is_empty() {
                missing.sort_by_key(|sym| self.sym(*sym).to_string());
                let missing_names = missing
                    .iter()
                    .map(|sym| self.sym(*sym).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let effect_name = self.sym(effect).to_string();
                return Err(Self::boxed(
                    Diagnostic::make_error_dynamic(
                        "E402",
                        "INCOMPLETE EFFECT HANDLER",
                        ErrorType::Compiler,
                        format!(
                            "Handler for `{}` is missing operations: {}.",
                            effect_name, missing_names
                        ),
                        Some("Add handler arms for all declared operations of the effect.".to_string()),
                        self.file_path.clone(),
                        expr.span(),
                    )
                    .with_primary_label(expr.span(), "handled expression"),
                ));
            }
        }

        for arm in arms {
            operations.push(arm.operation_name);

            // Build parameter list: [resume_param, param0, param1, ...]
            let mut params: Vec<Symbol> = Vec::with_capacity(1 + arm.params.len());
            params.push(arm.resume_param);
            params.extend_from_slice(&arm.params);

            // Wrap arm body in a synthetic block for compile_function_literal
            let arm_span = arm.body.span();
            let arm_block = Block {
                statements: vec![Statement::Expression {
                    expression: arm.body.clone(),
                    has_semicolon: false,
                    span: arm_span,
                }],
                span: arm_span,
            };

            // compile_function_literal emits OpClosure, leaving a closure on the stack
            self.compile_function_literal(
                &params,
                &vec![None; params.len()],
                &None,
                &[],
                &arm_block,
            )?;
        }

        // Build HandlerDescriptor and emit OpHandle
        let desc = Value::HandlerDescriptor(Rc::new(HandlerDescriptor {
            effect,
            ops: operations,
        }));

        let desc_idx = self.add_constant(desc);
        self.emit(OpCode::OpHandle, &[desc_idx]);

        // Compile the handled expression with the effect available in scope.
        self.with_handled_effect(effect, |compiler| compiler.compile_non_tail_expression(expr))?;

        // Remove the handler frame
        self.emit(OpCode::OpEndHandle, &[]);
        Ok(())
    }

    fn compile_non_tail_expression(&mut self, expression: &Expression) -> CompileResult<()> {
        self.with_tail_position(false, |compiler| compiler.compile_expression(expression))
    }

    fn compile_tail_call_argument(
        &mut self,
        expression: &Expression,
        consumable_counts: &HashMap<Symbol, usize>,
    ) -> CompileResult<()> {
        match expression {
            Expression::Identifier { name, .. } => {
                if self.try_emit_consumed_param(*name, consumable_counts) {
                    Ok(())
                } else {
                    self.compile_non_tail_expression(expression)
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.compile_non_tail_expression(function)?;

                for arg in arguments {
                    if let Expression::Identifier { name, .. } = arg {
                        if !self.try_emit_consumed_param(*name, consumable_counts) {
                            self.compile_non_tail_expression(arg)?;
                        }
                    } else {
                        self.compile_non_tail_expression(arg)?;
                    }
                }
                self.emit(OpCode::OpCall, &[arguments.len()]);
                Ok(())
            }
            _ => self.compile_non_tail_expression(expression),
        }
    }

    fn try_emit_consumed_param(
        &mut self,
        name: Symbol,
        consumable_counts: &HashMap<Symbol, usize>,
    ) -> bool {
        if consumable_counts.get(&name).copied().unwrap_or(0) != 1 {
            return false;
        }
        if let Some(symbol) = self.resolve_visible_symbol(name)
            && self.is_consumable_tail_param(&symbol)
        {
            self.emit(OpCode::OpConsumeLocal, &[symbol.index]);
            return true;
        }
        false
    }

    fn collect_consumable_param_uses_statement(
        &mut self,
        statement: &Statement,
        counts: &mut HashMap<Symbol, usize>,
    ) {
        match statement {
            Statement::Expression { expression, .. } => {
                self.collect_consumable_param_uses(expression, counts);
            }
            Statement::Let { value, .. } | Statement::Assign { value, .. } => {
                self.collect_consumable_param_uses(value, counts)
            }
            Statement::LetDestructure { value, .. } => {
                self.collect_consumable_param_uses(value, counts)
            }
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.collect_consumable_param_uses(value, counts);
                }
            }
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                for statement in &body.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
            }
            Statement::Import { .. } => {}
            Statement::Data { .. } => {}
            Statement::EffectDecl { .. } => {}
        }
    }

    fn collect_consumable_param_uses(
        &mut self,
        expression: &Expression,
        counts: &mut HashMap<Symbol, usize>,
    ) {
        match expression {
            Expression::Identifier { name, .. } => {
                if let Some(symbol) = self.symbol_table.resolve(*name)
                    && self.is_consumable_tail_param(&symbol)
                {
                    *counts.entry(*name).or_insert(0) += 1;
                }
            }
            Expression::Prefix { right, .. } => self.collect_consumable_param_uses(right, counts),
            Expression::Infix { left, right, .. } => {
                self.collect_consumable_param_uses(left, counts);
                self.collect_consumable_param_uses(right, counts);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_consumable_param_uses(condition, counts);
                for statement in &consequence.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
                if let Some(alt) = alternative {
                    for statement in &alt.statements {
                        self.collect_consumable_param_uses_statement(statement, counts);
                    }
                }
            }
            Expression::DoBlock { block, .. } => {
                for statement in &block.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_consumable_param_uses(function, counts);

                for argument in arguments {
                    self.collect_consumable_param_uses(argument, counts);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.collect_consumable_param_uses(element, counts);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_consumable_param_uses(left, counts);
                self.collect_consumable_param_uses(index, counts);
            }
            Expression::Hash { pairs, .. } => {
                for (key, value) in pairs {
                    self.collect_consumable_param_uses(key, counts);
                    self.collect_consumable_param_uses(value, counts);
                }
            }
            Expression::MemberAccess { object, .. } => {
                self.collect_consumable_param_uses(object, counts);
            }
            Expression::TupleFieldAccess { object, .. } => {
                self.collect_consumable_param_uses(object, counts);
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_consumable_param_uses(scrutinee, counts);

                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.collect_consumable_param_uses(guard, counts);
                    }
                    self.collect_consumable_param_uses(&arm.body, counts);
                }
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(expression) = part {
                        self.collect_consumable_param_uses(expression, counts);
                    }
                }
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.collect_consumable_param_uses(value, counts),
            Expression::Cons { head, tail, .. } => {
                self.collect_consumable_param_uses(head, counts);
                self.collect_consumable_param_uses(tail, counts);
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.collect_consumable_param_uses(arg, counts);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_consumable_param_uses(expr, counts);

                for arm in arms {
                    self.collect_consumable_param_uses(&arm.body, counts);
                }
            }
            Expression::Function { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => {}
        }
    }

    /// Check if an expression is a self recursive call
    fn is_self_call(&mut self, expression: &Expression) -> bool {
        match expression {
            Expression::Identifier { name, .. } => {
                if let Some(symbol) = self.symbol_table.resolve(*name) {
                    symbol.symbol_scope == SymbolScope::Function
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn is_consumable_tail_param(&self, symbol: &Binding) -> bool {
        if symbol.symbol_scope != SymbolScope::Local {
            return false;
        }
        if self
            .current_function_captured_locals()
            .is_some_and(|captured| captured.contains(&symbol.index))
        {
            return false;
        }
        match self.current_function_param_count() {
            Some(num_params) => symbol.index < num_params,
            None => false,
        }
    }

    fn try_emit_adt_constructor_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> CompileResult<bool> {
        let Expression::Identifier { name, .. } = function else {
            return Ok(false);
        };

        // Only intercept if the name is a known ADT constructor
        let Some(info) = self.adt_registry.lookup_constructor(*name) else {
            return Ok(false);
        };

        let expected_arity = info.arity;
        let actual_arity = arguments.len();

        if actual_arity != expected_arity {
            let name_str = self.interner.resolve(*name).to_string();
            return Err(Self::boxed(
                diag_enhanced(&CONSTRUCTOR_ARITY_MISMATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Constructor `{}` expects {} argument(s) but got {}.",
                        name_str, expected_arity, actual_arity
                    )),
            ));
        }

        // Compile each argument
        for arg in arguments {
            self.compile_non_tail_expression(arg)?;
        }

        // Add constructor name as a string constant
        let constructor_name = self.interner.resolve(*name).to_string();
        let const_idx = self.add_constant(Value::String(Rc::from(constructor_name.as_str())));
        self.emit(OpCode::OpMakeAdt, &[const_idx, actual_arity]);

        Ok(true)
    }

    fn check_match_exhaustiveness(&self, arms: &[MatchArm], span: Span) -> CompileResult<()> {
        // Collect all constructor patterns from arms
        let constructor_names: Vec<Symbol> = arms
            .iter()
            .filter_map(|arm| {
                if let Pattern::Constructor { name, .. } = &arm.pattern {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        // If no constructor patterns, nothing to check
        if constructor_names.is_empty() {
            return Ok(());
        }

        // If any arm has a wildcard or identifier (catch-all), it's exhaustive
        let has_catch_all = arms.iter().any(|arm| {
            matches!(
                arm.pattern,
                Pattern::Wildcard { .. } | Pattern::Identifier { .. }
            )
        });
        if has_catch_all {
            return Ok(());
        }

        // Look up the ADT from the first constructor name
        let first_constructor = constructor_names[0];
        let Some(constructor_info) = self.adt_registry.lookup_constructor(first_constructor) else {
            return Ok(()); // Unknown constructor, already reported elsewhere
        };
        let adt_name = constructor_info.adt_name;
        let Some(adt_def) = self.adt_registry.lookup_adt(adt_name) else {
            return Ok(());
        };

        // Check if all constructors are covered
        let covered: HashSet<Symbol> = constructor_names.into_iter().collect();
        let missing: Vec<&str> = adt_def
            .constructors
            .iter()
            .filter(|(name, _)| !covered.contains(name))
            .map(|(name, _)| self.interner.resolve(*name))
            .collect();

        if !missing.is_empty() {
            let adt_name_str = self.interner.resolve(adt_name).to_string();
            let missing_list = missing.join(", ");
            return Err(Self::boxed(
                diag_enhanced(&ADT_NON_EXHAUSTIVE_MATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Match on `{}` is missing constructors: {}.",
                        adt_name_str, missing_list
                    ))
                    .with_hint_text(format!(
                        "Add arms for {} or add a `_ -> ...` catch-all.",
                        missing_list
                    )),
            ));
        }

        Ok(())
    }
}
