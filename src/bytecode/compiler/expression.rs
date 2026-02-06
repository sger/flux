use std::rc::Rc;

use crate::{
    bytecode::{
        compiler::Compiler, debug_info::FunctionDebugInfo, op_code::OpCode, symbol::Symbol,
        symbol_scope::SymbolScope,
    },
    frontend::{
        block::Block,
        diagnostics::{
            Diagnostic, DiagnosticBuilder, ICE_SYMBOL_SCOPE_PATTERN, ICE_TEMP_SYMBOL_LEFT_BINDING,
            ICE_TEMP_SYMBOL_LEFT_PATTERN, ICE_TEMP_SYMBOL_MATCH, ICE_TEMP_SYMBOL_RIGHT_BINDING,
            ICE_TEMP_SYMBOL_RIGHT_PATTERN, ICE_TEMP_SYMBOL_SOME_BINDING,
            ICE_TEMP_SYMBOL_SOME_PATTERN, MODULE_NOT_IMPORTED, UNKNOWN_INFIX_OPERATOR,
            UNKNOWN_MODULE_MEMBER, UNKNOWN_PREFIX_OPERATOR,
        },
        expression::{Expression, MatchArm, Pattern, StringPart},
        module_graph::is_valid_module_name,
        position::{Position, Span},
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

type CompileResult<T> = Result<T, Box<Diagnostic>>;

impl Compiler {
    pub(super) fn compile_expression(&mut self, expression: &Expression) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(expression.span());
        match expression {
            Expression::Integer { value, .. } => {
                let idx = self.add_constant(Object::Integer(*value));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            Expression::Float { value, .. } => {
                let idx = self.add_constant(Object::Float(*value));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            Expression::String { value, .. } => {
                let idx = self.add_constant(Object::String(value.clone()));
                self.emit(OpCode::OpConstant, &[idx]);
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
                if let Some(symbol) = self.symbol_table.resolve(name) {
                    self.load_symbol(&symbol);
                } else if let Some(prefix) = &self.current_module_prefix {
                    let qualified = format!("{}.{}", prefix, name);
                    if let Some(symbol) = self.symbol_table.resolve(&qualified) {
                        self.load_symbol(&symbol);
                    } else if let Some(constant_value) = self.module_constants.get(&qualified) {
                        // Module constant - inline the value
                        self.emit_constant_object(constant_value.clone());
                    } else {
                        return Err(Self::boxed(self.make_undefined_variable_error(name, *span)));
                    }
                } else {
                    return Err(Self::boxed(self.make_undefined_variable_error(name, *span)));
                }
            }
            Expression::Prefix {
                operator, right, ..
            } => {
                self.compile_expression(right)?;
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
                    self.compile_expression(right)?;
                    self.compile_expression(left)?;
                    self.emit(OpCode::OpGreaterThan, &[]);
                    return Ok(());
                }

                if operator == "<=" {
                    self.compile_expression(left)?;
                    self.compile_expression(right)?;
                    self.emit(OpCode::OpLessThanOrEqual, &[]);
                    return Ok(());
                }
                // a && b: if a is falsy, result is a (short-circuit); otherwise result is b
                // OpJumpNotTruthy: peeks value, jumps if falsy (keeps value), pops if truthy
                if operator == "&&" {
                    self.compile_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                    self.compile_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }
                // a || b: if a is truthy, result is a (short-circuit); otherwise result is b
                // OpJumpTruthy: peeks value, jumps if truthy (keeps value), pops if falsy
                if operator == "||" {
                    self.compile_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpTruthy, &[9999]);
                    self.compile_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }

                self.compile_expression(left)?;
                self.compile_expression(right)?;

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
            Expression::Function {
                parameters, body, ..
            } => {
                self.compile_function_literal(parameters, body)?;
            }
            Expression::Array { elements, .. } => {
                for el in elements {
                    self.compile_expression(el)?;
                }
                self.emit(OpCode::OpArray, &[elements.len()]);
            }
            Expression::Hash { pairs, .. } => {
                let mut sorted_pairs: Vec<_> = pairs.iter().collect();
                sorted_pairs.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

                for (key, value) in sorted_pairs {
                    self.compile_expression(key)?;
                    self.compile_expression(value)?;
                }
                self.emit(OpCode::OpHash, &[pairs.len() * 2]);
            }
            Expression::Index { left, index, .. } => {
                self.compile_expression(left)?;
                self.compile_expression(index)?;
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
                self.compile_expression(function)?;

                for arg in arguments {
                    self.compile_expression(arg)?;
                }

                self.emit(OpCode::OpCall, &[arguments.len()]);
            }
            Expression::MemberAccess { object, member, .. } => {
                let expr_span = expression.span();
                let module_name = match object.as_ref() {
                    Expression::Identifier { name, .. } => {
                        if let Some(target) = self.import_aliases.get(name) {
                            Some(target.clone())
                        } else if self.imported_modules.contains(name)
                            || self.current_module_prefix.as_deref() == Some(name.as_str())
                        {
                            Some(name.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(module_name) = module_name {
                    self.check_private_member(member, expr_span, Some(module_name.as_str()))?;

                    let qualified = format!("{}.{}", module_name, member);

                    // Module Constants check if this is a compile-time constant
                    // If so, inline the constant value directly instead of loading from symbol
                    if let Some(constant_value) = self.module_constants.get(&qualified) {
                        self.emit_constant_object(constant_value.clone());
                        return Ok(());
                    }

                    if let Some(symbol) = self.symbol_table.resolve(&qualified) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }

                    return Err(Self::boxed(Diagnostic::make_error(
                        &UNKNOWN_MODULE_MEMBER,
                        &[&module_name, member],
                        self.file_path.clone(),
                        expr_span,
                    )));
                }

                if let Expression::Identifier { name, .. } = object.as_ref()
                    && module_name.is_none()
                    && is_valid_module_name(name)
                {
                    let has_symbol = self.symbol_table.resolve(name).is_some();
                    if !has_symbol {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NOT_IMPORTED,
                            &[name],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }
                }

                self.check_private_member(member, expr_span, None)?;

                // Compile the object (e.g., the module identifier)
                self.compile_expression(object)?;

                // Emit the member name as a string constant (the hash key)
                let member_idx = self.add_constant(Object::String(member.clone()));
                self.emit(OpCode::OpConstant, &[member_idx]);

                // Use index operation to access the member from the hash
                self.emit(OpCode::OpIndex, &[]);
                // Member access should yield the value, not Option.
                self.emit(OpCode::OpUnwrapSome, &[]);
            }
            Expression::None { .. } => {
                self.emit(OpCode::OpNone, &[]);
            }
            Expression::Some { value, .. } => {
                self.compile_expression(value)?;
                self.emit(OpCode::OpSome, &[]);
            }
            // Either type expressions
            Expression::Left { value, .. } => {
                self.compile_expression(value)?;
                self.emit(OpCode::OpLeft, &[]);
            }
            Expression::Right { value, .. } => {
                self.compile_expression(value)?;
                self.emit(OpCode::OpRight, &[]);
            }
            Expression::Match {
                scrutinee,
                arms,
                span,
            } => {
                self.compile_match_expression(scrutinee, arms, *span)?;
            }
        }
        self.current_span = previous_span;
        Ok(())
    }

    pub(super) fn compile_function_literal(
        &mut self,
        parameters: &[String],
        body: &Block,
    ) -> CompileResult<()> {
        use crate::frontend::diagnostics::DUPLICATE_PARAMETER;

        if let Some(name) = Self::find_duplicate_name(parameters) {
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[name],
                self.file_path.clone(),
                Span::new(Position::default(), Position::default()),
            )));
        }

        self.enter_scope();

        for param in parameters {
            self.symbol_table.define(param, Span::default());
        }

        self.compile_block(body)?;

        if self.is_last_instruction(OpCode::OpPop) {
            self.replace_last_pop_with_return();
        }

        if !self.is_last_instruction(OpCode::OpReturnValue) {
            self.emit(OpCode::OpReturn, &[]);
        }

        let free_symbols = self.symbol_table.free_symbols.clone();
        let num_locals = self.symbol_table.num_definitions;
        let (instructions, locations, files) = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let fn_idx = self.add_constant(Object::Function(Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            parameters.len(),
            Some(FunctionDebugInfo::new(None, files, locations)),
        ))));

        self.emit(OpCode::OpClosure, &[fn_idx, free_symbols.len()]);

        Ok(())
    }

    pub(super) fn compile_interpolated_string(
        &mut self,
        parts: &[StringPart],
    ) -> CompileResult<()> {
        if parts.is_empty() {
            // Empty interpolated string - just push an empty string
            let idx = self.add_constant(Object::String(String::new()));
            self.emit(OpCode::OpConstant, &[idx]);
            return Ok(());
        }

        // Compile the first part
        match &parts[0] {
            StringPart::Literal(s) => {
                let idx = self.add_constant(Object::String(s.clone()));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            StringPart::Interpolation(expr) => {
                self.compile_expression(expr)?;
                self.emit(OpCode::OpToString, &[]);
            }
        }

        // Compile remaining parts, concatenating each with OpAdd
        for part in &parts[1..] {
            match part {
                StringPart::Literal(s) => {
                    let idx = self.add_constant(Object::String(s.clone()));
                    self.emit(OpCode::OpConstant, &[idx]);
                }
                StringPart::Interpolation(expr) => {
                    self.compile_expression(expr)?;
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
        self.compile_expression(condition)?;

        let jump_not_truthy_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);

        self.compile_block(consequence)?;

        if self.is_last_instruction(OpCode::OpPop) {
            self.remove_last_pop();
        }

        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
        self.change_operand(jump_not_truthy_pos, self.current_instructions().len());

        // Pop the condition value that was left on stack when we jumped here
        // (OpJumpNotTruthy keeps value on stack when jumping for short-circuit support)
        self.emit(OpCode::OpPop, &[]);

        if let Some(alt) = alternative {
            self.compile_block(alt)?;

            if self.is_last_instruction(OpCode::OpPop) {
                self.remove_last_pop();
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
        _match_span: Span,
    ) -> CompileResult<()> {
        // Compile scrutinee once and store it in a temp local
        self.enter_block_scope();
        self.compile_expression(scrutinee)?;
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

        // Compile each arm
        for (i, arm) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;

            // For all arms except the last, we need to check if pattern matches
            if !is_last {
                // Duplicate scrutinee for pattern check
                // We'll emit code to check the pattern and jump to next arm if not matched
                let next_arm_jumps = self.compile_pattern_check(&temp_symbol, &arm.pattern)?;

                // Pattern matched, compile the body
                self.enter_block_scope();
                self.compile_pattern_bind(&temp_symbol, &arm.pattern)?;
                self.compile_expression(&arm.body)?;
                self.leave_block_scope();

                // Jump to end after executing this arm's body
                let end_jump = self.emit(OpCode::OpJump, &[9999]);
                end_jumps.push(end_jump);

                // Patch jump to next arm
                for jump_pos in next_arm_jumps {
                    self.change_operand(jump_pos, self.current_instructions().len());
                }
            } else {
                // Last arm: bind identifier (if any) or drop scrutinee, then compile body
                self.enter_block_scope();
                self.compile_pattern_bind(&temp_symbol, &arm.pattern)?;
                self.compile_expression(&arm.body)?;
                self.leave_block_scope();
            }
        }

        // Patch all end jumps to point here
        for jump_pos in end_jumps {
            self.change_operand(jump_pos, self.current_instructions().len());
        }

        self.leave_block_scope();
        Ok(())
    }

    pub(super) fn compile_pattern_check(
        &mut self,
        scrutinee: &Symbol,
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
                self.compile_expression(expression)?;
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
                // Identifier always matches and binds the value
                // For now, we'll treat it like wildcard
                // TODO: Implement proper binding
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
        }
    }

    pub(super) fn compile_pattern_bind(
        &mut self,
        scrutinee: &Symbol,
        pattern: &Pattern,
    ) -> CompileResult<()> {
        match pattern {
            Pattern::Identifier { name, span } => {
                self.load_symbol(scrutinee);
                let symbol = self.symbol_table.define(name.clone(), *span);
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
            Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
        }
        Ok(())
    }
}
