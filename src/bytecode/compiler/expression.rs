use std::{collections::HashMap, rc::Rc};

use crate::{
    bytecode::{
        binding::Binding, compiler::Compiler, debug_info::FunctionDebugInfo, op_code::OpCode,
        symbol_scope::SymbolScope,
    },
    diagnostics::{
        DUPLICATE_PARAMETER, Diagnostic, DiagnosticBuilder, ICE_SYMBOL_SCOPE_PATTERN,
        ICE_TEMP_SYMBOL_LEFT_BINDING, ICE_TEMP_SYMBOL_LEFT_PATTERN, ICE_TEMP_SYMBOL_MATCH,
        ICE_TEMP_SYMBOL_RIGHT_BINDING, ICE_TEMP_SYMBOL_RIGHT_PATTERN, ICE_TEMP_SYMBOL_SOME_BINDING,
        ICE_TEMP_SYMBOL_SOME_PATTERN, LEGACY_LIST_TAIL_NONE, MODULE_NOT_IMPORTED, UNKNOWN_INFIX_OPERATOR,
        UNKNOWN_MODULE_MEMBER, UNKNOWN_PREFIX_OPERATOR,
        position::{Position, Span},
    },
    runtime::{compiled_function::CompiledFunction, value::Value},
    syntax::{
        block::Block,
        expression::{Expression, MatchArm, Pattern, StringPart},
        module_graph::is_valid_module_name,
        statement::Statement,
        symbol::Symbol,
    },
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
                if let Some(symbol) = self.symbol_table.resolve(name) {
                    self.load_symbol(&symbol);
                } else if let Some(prefix) = self.current_module_prefix {
                    let qualified = self.interner.intern_join(prefix, name);
                    if let Some(symbol) = self.symbol_table.resolve(qualified) {
                        self.load_symbol(&symbol);
                    } else if let Some(constant_value) = self.module_constants.get(&qualified) {
                        // Module constant - inline the value
                        self.emit_constant_value(constant_value.clone());
                    } else {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_undefined_variable_error(name_str, *span),
                        ));
                    }
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
            Expression::Function {
                parameters, body, ..
            } => {
                self.compile_function_literal(parameters, body)?;
            }
            Expression::ListLiteral { elements, .. } => {
                // Lower list literals through builtin `list(...)` to avoid deep
                // recursive lowering for large literals.
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("builtin list must be defined");
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
            Expression::EmptyList { .. } => {
                let list_sym = self.interner.intern("list");
                let symbol = self
                    .symbol_table
                    .resolve(list_sym)
                    .expect("builtin list must be defined");
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
                let module_name = match object.as_ref() {
                    Expression::Identifier { name, .. } => {
                        let name = *name;
                        if let Some(target) = self.import_aliases.get(&name) {
                            Some(*target)
                        } else if self.imported_modules.contains(&name)
                            || self.current_module_prefix == Some(name)
                        {
                            Some(name)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(module_name) = module_name {
                    let member_str = self.sym(member);
                    self.check_private_member(member_str, expr_span, Some(self.sym(module_name)))?;

                    let qualified = self.interner.intern_join(module_name, member);
                    // Module Constants check if this is a compile-time constant
                    // If so, inline the constant value directly instead of loading from symbol
                    if let Some(constant_value) = self.module_constants.get(&qualified) {
                        self.emit_constant_value(constant_value.clone());
                        return Ok(());
                    }

                    if let Some(symbol) = self.symbol_table.resolve(qualified) {
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

                if let Expression::Identifier { name, .. } = object.as_ref()
                    && module_name.is_none()
                {
                    let name = *name;
                    if is_valid_module_name(self.sym(name)) {
                        let has_symbol = self.symbol_table.resolve(name).is_some();
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
        }
        self.current_span = previous_span;
        Ok(())
    }

    pub(super) fn compile_function_literal(
        &mut self,
        parameters: &[Symbol],
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

        for param in parameters {
            self.symbol_table.define(*param, Span::default());
        }

        self.with_function_context(parameters.len(), |compiler| {
            compiler.compile_block_with_tail(body)
        })?;

        if self.is_last_instruction(OpCode::OpPop) {
            self.replace_last_pop_with_return();
        }

        if !self.is_last_instruction(OpCode::OpReturnValue)
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
        let (instructions, locations, files) = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let fn_idx = self.add_constant(Value::Function(Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            parameters.len(),
            Some(FunctionDebugInfo::new(None, files, locations)),
        ))));

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

        // Consequence branch inherits tail position
        if self.in_tail_position {
            self.compile_block_with_tail(consequence)?;
        } else {
            self.compile_block(consequence)?;
        }

        if self.is_last_instruction(OpCode::OpPop) {
            self.remove_last_pop();
        }

        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
        self.change_operand(jump_not_truthy_pos, self.current_instructions().len());

        // Pop the condition value that was left on stack when we jumped here
        // (OpJumpNotTruthy keeps value on stack when jumping for short-circuit support)
        self.emit(OpCode::OpPop, &[]);

        // Alternative branch also inherits tail position
        if let Some(alt) = alternative {
            if self.in_tail_position {
                self.compile_block_with_tail(alt)?;
            } else {
                self.compile_block(alt)?;
            }

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
                // Identifier always matches and binds the value
                // For now, we'll treat it like wildcard
                // TODO: Implement proper binding
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
            Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
        }
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
        if let Some(symbol) = self.symbol_table.resolve(name)
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
            | Expression::ArrayLiteral { elements, .. } => {
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
}
