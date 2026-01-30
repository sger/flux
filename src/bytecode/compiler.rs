use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::{
    bytecode::{
        bytecode::Bytecode,
        compilation_scope::CompilationScope,
        debug_info::{FunctionDebugInfo, InstructionLocation, Location},
        emitted_instruction::EmittedInstruction,
        op_code::{Instructions, OpCode, make},
        symbol::Symbol,
        symbol_scope::SymbolScope,
        symbol_table::SymbolTable,
    },
    frontend::{
        block::Block,
        diagnostic::Diagnostic,
        expression::{Expression, MatchArm, Pattern, StringPart},
        module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
        position::{Position, Span},
        program::Program,
        statement::Statement,
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

type CompileResult<T> = Result<T, Box<Diagnostic>>;

pub struct Compiler {
    constants: Vec<Object>,
    pub symbol_table: SymbolTable,
    scopes: Vec<CompilationScope>,
    scope_index: usize,
    pub errors: Vec<Diagnostic>,
    file_path: String,
    imported_files: HashSet<String>,
    file_scope_symbols: HashSet<String>,
    imported_modules: HashSet<String>,
    import_aliases: HashMap<String, String>,
    current_module_prefix: Option<String>,
    current_span: Option<Span>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::new_with_file_path("<unknown>")
    }

    pub fn new_with_file_path(file_path: impl Into<String>) -> Self {
        let mut symbol_table = SymbolTable::new();
        symbol_table.define_builtin(0, "print");
        symbol_table.define_builtin(1, "len");
        symbol_table.define_builtin(2, "first");
        symbol_table.define_builtin(3, "last");
        symbol_table.define_builtin(4, "rest");
        symbol_table.define_builtin(5, "push");
        symbol_table.define_builtin(6, "to_string");

        Self {
            constants: Vec::new(),
            symbol_table,
            scopes: vec![CompilationScope::new()],
            scope_index: 0,
            errors: Vec::new(),
            file_path: file_path.into(),
            imported_files: HashSet::new(),
            file_scope_symbols: HashSet::new(),
            imported_modules: HashSet::new(),
            import_aliases: HashMap::new(),
            current_module_prefix: None,
            current_span: None,
        }
    }

    pub fn new_with_state(symbol_table: SymbolTable, constants: Vec<Object>) -> Self {
        let mut compiler = Self::new();
        compiler.symbol_table = symbol_table;
        compiler.constants = constants;
        compiler
    }

    pub fn set_file_path(&mut self, file_path: impl Into<String>) {
        // Keep diagnostics anchored to the module currently being compiled.
        self.file_path = file_path.into();
        // Reset per-file name tracking for import collision checks.
        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.current_module_prefix = None;
        self.current_span = None;
    }

    fn boxed(diag: Diagnostic) -> Box<Diagnostic> {
        Box::new(diag)
    }

    pub fn compile(&mut self, program: &Program) -> Result<(), Vec<Diagnostic>> {
        // Ensure per-file tracking is clean for each compile pass.
        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.current_module_prefix = None;

        // PASS 1: Predeclare all module-level function names
        // This enables forward references and mutual recursion
        for statement in &program.statements {
            if let Statement::Function { name, span, .. } = statement {
                let position = span.start;
                // Check for duplicate declaration first (takes precedence)
                if self.symbol_table.exists_in_current_scope(name) {
                    self.errors
                        .push(self.make_redeclaration_error(name, position));
                    continue;
                }
                // Check for import collision
                if self.scope_index == 0 && self.file_scope_symbols.contains(name) {
                    self.errors
                        .push(self.make_import_collision_error(name, position));
                    continue;
                }
                // Predeclare the function name
                self.symbol_table.define(name);
                self.file_scope_symbols.insert(name.clone());
            }
        }

        // PASS 2: Compile all statements
        // Function bodies can now reference any function defined at module level
        for statement in &program.statements {
            // Continue compilation even if there are errors
            if let Err(err) = self.compile_statement(statement) {
                self.errors.push(*err);
            }
        }

        // Return all errors at the end
        if !self.errors.is_empty() {
            return Err(std::mem::take(&mut self.errors));
        }

        Ok(())
    }

    fn compile_statement(&mut self, statement: &Statement) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(statement.span());
        let result = (|| {
            match statement {
                Statement::Expression { expression, .. } => {
                    self.compile_expression(expression)?;
                    self.emit(OpCode::OpPop, &[]);
                }
                Statement::Let { name, value, span } => {
                    let position = span.start;
                    if self.scope_index == 0 && self.file_scope_symbols.contains(name) {
                        return Err(Self::boxed(
                            self.make_import_collision_error(name, position),
                        ));
                    }
                    if self.symbol_table.exists_in_current_scope(name) {
                        return Err(Self::boxed(self.make_redeclaration_error(name, position)));
                    }

                    let symbol = self.symbol_table.define(name);
                    self.compile_expression(value)?;

                    match symbol.symbol_scope {
                        SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                        SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                        _ => {
                            return Err(Self::boxed(
                                Diagnostic::error("INTERNAL COMPILER ERROR")
                                    .with_code("ICE001")
                                    .with_message("unexpected symbol scope for let binding")
                                    .with_hint(format!(
                                        "{}:{} ({})",
                                        file!(),
                                        line!(),
                                        module_path!()
                                    )),
                            ));
                        }
                    };

                    self.symbol_table.mark_assigned(name).ok();
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(name.clone());
                    }
                }
                Statement::Assign { name, value, span } => {
                    let position = span.start;
                    // Check if variable exists
                    let symbol = self.symbol_table.resolve(name).ok_or_else(|| {
                        Self::boxed(self.make_undefined_variable_error(name, position))
                    })?;

                    if symbol.symbol_scope == SymbolScope::Free {
                        return Err(Self::boxed(
                            self.make_outer_assignment_error(name, position),
                        ));
                    }

                    // Check if variable is already assigned (immutability check)
                    if symbol.is_assigned {
                        return Err(Self::boxed(self.make_immutability_error(name, position)));
                    }

                    self.compile_expression(value)?;

                    match symbol.symbol_scope {
                        SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                        SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                        _ => {
                            return Err(Self::boxed(
                                Diagnostic::error("INTERNAL COMPILER ERROR")
                                    .with_code("ICE002")
                                    .with_message("unexpected symbol scope for assignment")
                                    .with_hint(format!(
                                        "{}:{} ({})",
                                        file!(),
                                        line!(),
                                        module_path!()
                                    )),
                            ));
                        }
                    };

                    // Mark as assigned
                    self.symbol_table.mark_assigned(name).ok();
                }
                Statement::Return { value, .. } => match value {
                    Some(expr) => {
                        self.compile_expression(expr)?;
                        self.emit(OpCode::OpReturnValue, &[]);
                    }
                    None => {
                        self.emit(OpCode::OpReturn, &[]);
                    }
                },
                Statement::Function {
                    name,
                    parameters,
                    body,
                    span,
                    ..
                } => {
                    let position = span.start;
                    // For top-level functions, checks were already done in pass 1
                    // Only check for nested functions (scope_index > 0)
                    if self.scope_index > 0 && self.symbol_table.exists_in_current_scope(name) {
                        return Err(Self::boxed(self.make_redeclaration_error(name, position)));
                    }
                    self.compile_function_statement(name, parameters, body, position)?;
                    // For nested functions, add to file_scope_symbols
                    if self.scope_index == 0 {
                        // Already added in pass 1 for top-level functions
                        self.file_scope_symbols.insert(name.clone());
                    }
                }
                Statement::Module { name, body, span } => {
                    let position = span.start;
                    if self.scope_index > 0 {
                        return Err(Self::boxed(
                            Diagnostic::error("MODULE SCOPE")
                                .with_code("E039")
                                .with_file(self.file_path.clone())
                                .with_position(position)
                                .with_message("Modules may only be declared at the top level."),
                        ));
                    }
                    let binding_name = module_binding_name(name);
                    if self.scope_index == 0 && self.file_scope_symbols.contains(binding_name) {
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name, position),
                        ));
                    }
                    if !is_valid_module_name(name) {
                        return Err(Self::boxed(self.make_module_name_error(name, position)));
                    }
                    self.compile_module_statement(name, body, position)?;
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(binding_name.to_string());
                    }
                }
                Statement::Import { name, alias, span } => {
                    let position = span.start;
                    if self.scope_index > 0 {
                        return Err(Self::boxed(self.make_import_scope_error(name, position)));
                    }
                    let binding_name = import_binding_name(name, alias.as_deref());
                    if self.file_scope_symbols.contains(binding_name) {
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name, position),
                        ));
                    }
                    // Reserve the name for this file so later declarations can't collide.
                    self.file_scope_symbols.insert(binding_name.to_string());
                    self.compile_import_statement(name, alias.as_deref())?;
                }
            }
            Ok(())
        })();
        self.current_span = previous_span;
        result
    }

    fn emit(&mut self, op_code: OpCode, operands: &[usize]) -> usize {
        let instruction = make(op_code, operands);
        let pos = self.add_instruction(&instruction, self.current_span);
        self.set_last_instruction(op_code, pos);
        pos
    }

    fn add_instruction(&mut self, instruction: &[u8], span: Option<Span>) -> usize {
        let pos = self.scopes[self.scope_index].instructions.len();
        self.scopes[self.scope_index]
            .instructions
            .extend_from_slice(instruction);
        self.add_location(pos, span);
        pos
    }

    fn add_location(&mut self, offset: usize, span: Option<Span>) {
        let file_id = self.file_id_for_current();
        let location = span.map(|span| Location { file_id, span });
        self.scopes[self.scope_index]
            .locations
            .push(InstructionLocation { offset, location });
    }

    fn file_id_for_current(&mut self) -> u32 {
        let files = &mut self.scopes[self.scope_index].files;
        if let Some((index, _)) = files
            .iter()
            .enumerate()
            .find(|(_, file)| file.as_str() == self.file_path)
        {
            return index as u32;
        }
        files.push(self.file_path.clone());
        (files.len() - 1) as u32
    }

    fn set_last_instruction(&mut self, op_code: OpCode, pos: usize) {
        let previous = self.scopes[self.scope_index].last_instruction.clone();
        self.scopes[self.scope_index].previous_instruction = previous;
        self.scopes[self.scope_index].last_instruction = EmittedInstruction {
            opcode: Some(op_code),
            position: pos,
        };
    }

    fn compile_expression(&mut self, expression: &Expression) -> CompileResult<()> {
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
            Expression::Identifier { name, .. } => {
                if let Some(symbol) = self.symbol_table.resolve(name) {
                    self.load_symbol(&symbol);
                } else if let Some(prefix) = &self.current_module_prefix {
                    let qualified = format!("{}.{}", prefix, name);
                    if let Some(symbol) = self.symbol_table.resolve(&qualified) {
                        self.load_symbol(&symbol);
                    } else {
                        return Err(Self::boxed(
                            Diagnostic::error(format!("undefined variable `{}`", name))
                                .with_hint(format!("Define it first: let {} = ...;", name)),
                        ));
                    }
                } else {
                    return Err(Self::boxed(
                        Diagnostic::error(format!("undefined variable `{}`", name))
                            .with_hint(format!("Define it first: let {} = ...;", name)),
                    ));
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
                        return Err(Self::boxed(
                            Diagnostic::error("UNKNOWN PREFIX OPERATOR")
                                .with_code("E010")
                                .with_message(format!("Unknown prefix operator `{}`.", operator)),
                        ));
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
                            Diagnostic::error("UNKNOWN INFIX OPERATOR")
                                .with_code("E011")
                                .with_message(format!("Unknown infix operator `{}`.", operator))
                                .with_hint(
                                    "Use a supported operator like +, -, *, /, ==, !=, or >.",
                                ),
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
                    if let Some(symbol) = self.symbol_table.resolve(&qualified) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }

                    return Err(Self::boxed(
                        Diagnostic::error("UNKNOWN MODULE MEMBER")
                            .with_span(expr_span)
                            .with_message(format!(
                                "Module `{}` has no member `{}`.",
                                module_name, member
                            ))
                            .with_file(self.file_path.clone()),
                    ));
                }

                if let Expression::Identifier { name, .. } = object.as_ref()
                    && module_name.is_none()
                    && is_valid_module_name(name)
                {
                    let has_symbol = self.symbol_table.resolve(name).is_some();
                    if !has_symbol {
                        return Err(Self::boxed(
                            Diagnostic::error("MODULE NOT IMPORTED")
                                .with_message(format!("Module `{}` is not imported.", name))
                                .with_hint(format!("Add `import {}` at the top level.", name))
                                .with_file(self.file_path.clone()),
                        ));
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
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.compile_match_expression(scrutinee, arms)?;
            }
        }
        self.current_span = previous_span;
        Ok(())
    }

    fn add_constant(&mut self, obj: Object) -> usize {
        self.constants.push(obj);
        self.constants.len() - 1
    }

    fn load_symbol(&mut self, symbol: &Symbol) {
        match symbol.symbol_scope {
            SymbolScope::Global => {
                self.emit(OpCode::OpGetGlobal, &[symbol.index]);
            }
            SymbolScope::Local => {
                self.emit(OpCode::OpGetLocal, &[symbol.index]);
            }
            SymbolScope::Builtin => {
                self.emit(OpCode::OpGetBuiltin, &[symbol.index]);
            }
            SymbolScope::Free => {
                self.emit(OpCode::OpGetFree, &[symbol.index]);
            }
            SymbolScope::Function => {
                self.emit(OpCode::OpCurrentClosure, &[]);
            }
        }
    }

    fn compile_function_literal(
        &mut self,
        parameters: &[String],
        body: &Block,
    ) -> CompileResult<()> {
        if let Some(name) = Self::find_duplicate_name(parameters) {
            return Err(Self::boxed(
                Diagnostic::error("DUPLICATE PARAMETER")
                    .with_code("E012")
                    .with_message(format!(
                        "Duplicate parameter `{}` in function literal.",
                        name
                    ))
                    .with_hint("Parameter names must be unique."),
            ));
        }

        self.enter_scope();

        for param in parameters {
            self.symbol_table.define(param);
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

    fn compile_interpolated_string(&mut self, parts: &[StringPart]) -> CompileResult<()> {
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

    fn compile_if_expression(
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

    fn compile_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
    ) -> CompileResult<()> {
        if arms.is_empty() {
            return Err(Self::boxed(
                Diagnostic::error("EMPTY MATCH")
                    .with_code("E030")
                    .with_message("Match expression must have at least one arm."),
            ));
        }
        if arms.len() > 1 {
            for arm in &arms[..arms.len() - 1] {
                if matches!(arm.pattern, Pattern::Identifier(_) | Pattern::Wildcard) {
                    return Err(Self::boxed(
                        Diagnostic::error("INVALID PATTERN")
                            .with_code("E034")
                            .with_message("Catch-all patterns must be the final match arm.")
                            .with_hint("Move `_` or the binding pattern to the last arm."),
                    ));
                }
            }
        }

        if let Some(last) = arms.last()
            && !matches!(last.pattern, Pattern::Wildcard | Pattern::Identifier(_))
        {
            return Err(Self::boxed(
                Diagnostic::error("NON-EXHAUSTIVE MATCH")
                    .with_code("E033")
                    .with_message("Match expressions must end with a `_` or identifier arm.")
                    .with_hint("Add a catch-all arm: `_ -> ...` or `x -> ...`"),
            ));
        }

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
                return Err(Self::boxed(
                    Diagnostic::error("INTERNAL COMPILER ERROR")
                        .with_code("ICE003")
                        .with_message("unexpected temp symbol scope in match scrutinee")
                        .with_hint(format!("{}:{} ({})", file!(), line!(), module_path!())),
                ));
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

    fn compile_pattern_check(
        &mut self,
        scrutinee: &Symbol,
        pattern: &Pattern,
    ) -> CompileResult<Vec<usize>> {
        match pattern {
            Pattern::Wildcard => {
                // Wildcard always matches, so we never jump to next arm
                // Emit OpTrue and OpJumpNotTruthy (which will never jump)
                // Actually, for wildcard we should always execute this arm
                // So we return a dummy jump position that will never be used
                // For simplicity, emit a condition that's always true
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::Literal(expr) => {
                // Push pattern value onto stack: [scrutinee, pattern]
                // OpEqual compares and pushes boolean: [result]
                // OpJumpNotTruthy jumps when false (no match), continues when true (match)
                self.load_symbol(scrutinee);
                self.compile_expression(expr)?;
                self.emit(OpCode::OpEqual, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::None => {
                // Check if scrutinee is None
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpNone, &[]);
                self.emit(OpCode::OpEqual, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
            Pattern::Some(inner) => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsSome, &[]);
                let mut jumps = vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])];

                match inner.as_ref() {
                    Pattern::Wildcard | Pattern::Identifier(_) => Ok(jumps),
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
                                return Err(Self::boxed(
                                    Diagnostic::error("INTERNAL COMPILER ERROR")
                                        .with_code("ICE004")
                                        .with_message(
                                            "unexpected temp symbol scope in Some pattern",
                                        )
                                        .with_hint(format!(
                                            "{}:{} ({})",
                                            file!(),
                                            line!(),
                                            module_path!()
                                        )),
                                ));
                            }
                        }
                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Identifier(_name) => {
                // Identifier always matches and binds the value
                // For now, we'll treat it like wildcard
                // TODO: Implement proper binding
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![self.emit(OpCode::OpJumpNotTruthy, &[9999])])
            }
        }
    }

    fn compile_pattern_bind(&mut self, scrutinee: &Symbol, pattern: &Pattern) -> CompileResult<()> {
        match pattern {
            Pattern::Identifier(name) => {
                self.load_symbol(scrutinee);
                let symbol = self.symbol_table.define(name.clone());
                match symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(
                            Diagnostic::error("INTERNAL COMPILER ERROR")
                                .with_code("ICE005")
                                .with_message("unexpected symbol scope for pattern binding")
                                .with_hint(format!("{}:{} ({})", file!(), line!(), module_path!())),
                        ));
                    }
                };
            }
            Pattern::Some(inner) => {
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
                        return Err(Self::boxed(
                            Diagnostic::error("INTERNAL COMPILER ERROR")
                                .with_code("ICE006")
                                .with_message("unexpected temp symbol scope in Some binding")
                                .with_hint(format!("{}:{} ({})", file!(), line!(), module_path!())),
                        ));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::None => {}
        }
        Ok(())
    }

    fn compile_function_statement(
        &mut self,
        name: &str,
        parameters: &[String],
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        if let Some(param) = Self::find_duplicate_name(parameters) {
            return Err(Self::boxed(
                Diagnostic::error("DUPLICATE PARAMETER")
                    .with_code("E012")
                    .with_file(self.file_path.clone())
                    .with_position(position)
                    .with_message(format!(
                        "Duplicate parameter `{}` in function `{}`.",
                        param, name
                    ))
                    .with_hint("Use distinct parameter names."),
            ));
        }

        // Resolve the symbol - it may have been predeclared in pass 1
        let symbol = if let Some(existing) = self.symbol_table.resolve(name) {
            // Use the existing symbol from pass 1
            existing
        } else {
            // Define new symbol (for nested functions or non-predeclared cases)
            self.symbol_table.define(name)
        };

        self.enter_scope();
        self.symbol_table.define_function_name(name);

        for param in parameters {
            self.symbol_table.define(param);
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
            Some(FunctionDebugInfo::new(
                Some(name.to_string()),
                files,
                locations,
            )),
        ))));
        self.emit(OpCode::OpClosure, &[fn_idx, free_symbols.len()]);

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => 0,
        };
        Ok(())
    }

    fn compile_module_statement(
        &mut self,
        name: &str,
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        // Check if module is already defined
        let binding_name = module_binding_name(name);
        if self.symbol_table.exists_in_current_scope(binding_name) {
            return Err(Self::boxed(
                self.make_redeclaration_error(binding_name, position),
            ));
        }

        // Collect all functions from the module body and validate contents
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if fn_name == binding_name {
                        return Err(Self::boxed(
                            Diagnostic::error("MODULE NAME CLASH")
                                .with_code("E018")
                                .with_position(statement.position())
                                .with_message(format!(
                                    "Module `{}` cannot define a function with the same name.",
                                    binding_name
                                ))
                                .with_hint("Use a different function name."),
                        ));
                    }
                }
                _ => {
                    return Err(Self::boxed(
                        Diagnostic::error("INVALID MODULE CONTENT")
                            .with_code("E019")
                            .with_position(statement.position())
                            .with_message("Modules can only contain function declarations."),
                    ));
                }
            }
        }

        self.imported_modules.insert(binding_name.to_string());
        let previous_module = self.current_module_prefix.clone();
        self.current_module_prefix = Some(binding_name.to_string());

        // PASS 1: Predeclare all module function names with qualified names
        // This enables forward references within the module
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                span,
                ..
            } = statement
            {
                let position = span.start;
                let qualified_name = format!("{}.{}", binding_name, fn_name);
                // Check for duplicate declaration
                if self.symbol_table.exists_in_current_scope(&qualified_name) {
                    self.current_module_prefix = previous_module;
                    return Err(Self::boxed(
                        self.make_redeclaration_error(&qualified_name, position),
                    ));
                }
                // Predeclare the function
                self.symbol_table.define(&qualified_name);
            }
        }

        // PASS 2: Compile each function body
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                parameters,
                body: fn_body,
                span,
                ..
            } = statement
            {
                let position = span.start;
                let qualified_name = format!("{}.{}", binding_name, fn_name);
                if let Err(err) =
                    self.compile_function_statement(&qualified_name, parameters, fn_body, position)
                {
                    self.current_module_prefix = previous_module;
                    return Err(err);
                }
            }
        }

        self.current_module_prefix = previous_module;

        Ok(())
    }

    fn compile_import_statement(&mut self, name: &str, alias: Option<&str>) -> CompileResult<()> {
        if let Some(alias) = alias {
            self.import_aliases
                .insert(alias.to_string(), name.to_string());
        } else {
            self.imported_modules.insert(name.to_string());
        }
        Ok(())
    }

    fn compile_block(&mut self, block: &Block) -> CompileResult<()> {
        for statement in &block.statements {
            self.compile_statement(statement)?;
        }

        Ok(())
    }

    fn check_private_member(
        &self,
        member: &str,
        expr_span: Span,
        module_name: Option<&str>,
    ) -> CompileResult<()> {
        if !member.starts_with('_') {
            return Ok(());
        }

        let same_module =
            module_name.is_some_and(|name| self.current_module_prefix.as_deref() == Some(name));
        if same_module {
            return Ok(());
        }

        Err(Self::boxed(
            Diagnostic::error("PRIVATE MEMBER")
                .with_code("E021")
                .with_file(self.file_path.clone())
                .with_span(expr_span)
                .with_message(format!("Cannot access private member `{}`.", member))
                .with_hint("Private members can only be accessed within the same module."),
        ))
    }
    fn enter_scope(&mut self) {
        self.scopes.push(CompilationScope::new());
        self.scope_index += 1;
        self.symbol_table = SymbolTable::new_enclosed(self.symbol_table.clone());
    }

    fn leave_scope(&mut self) -> (Instructions, Vec<InstructionLocation>, Vec<String>) {
        let scope = self.scopes.pop().unwrap();
        self.scope_index -= 1;
        if let Some(outer) = self.symbol_table.outer.take() {
            self.symbol_table = *outer;
        }

        (scope.instructions, scope.locations, scope.files)
    }

    fn enter_block_scope(&mut self) {
        let mut block_table = SymbolTable::new_block(self.symbol_table.clone());
        block_table.num_definitions = self.symbol_table.num_definitions;
        self.symbol_table = block_table;
    }

    fn leave_block_scope(&mut self) {
        let num_definitions = self.symbol_table.num_definitions;
        if let Some(outer) = self.symbol_table.outer.take() {
            let mut outer = *outer;
            outer.num_definitions = num_definitions;
            self.symbol_table = outer;
        }
    }

    pub fn bytecode(&self) -> Bytecode {
        Bytecode {
            instructions: self.scopes[self.scope_index].instructions.clone(),
            constants: self.constants.clone(),
            debug_info: Some(FunctionDebugInfo::new(
                Some("<main>".to_string()),
                self.scopes[self.scope_index].files.clone(),
                self.scopes[self.scope_index].locations.clone(),
            )),
        }
    }

    pub fn imported_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.imported_files.iter().cloned().collect();
        files.sort();
        files
    }

    fn is_last_instruction(&self, opcode: OpCode) -> bool {
        self.scopes[self.scope_index].last_instruction.opcode == Some(opcode)
    }

    fn remove_last_pop(&mut self) {
        let last_pos = self.scopes[self.scope_index].last_instruction.position;
        let previous = self.scopes[self.scope_index].previous_instruction.clone();

        self.scopes[self.scope_index]
            .instructions
            .truncate(last_pos);
        while let Some(last) = self.scopes[self.scope_index].locations.last() {
            if last.offset >= last_pos {
                self.scopes[self.scope_index].locations.pop();
            } else {
                break;
            }
        }
        self.scopes[self.scope_index].last_instruction = previous;
    }

    fn change_operand(&mut self, op_pos: usize, operand: usize) {
        let op_code = OpCode::from(self.current_instructions()[op_pos]);
        self.replace_instruction(op_pos, make(op_code, &[operand]));
    }

    fn current_instructions(&self) -> &Instructions {
        &self.scopes[self.scope_index].instructions
    }

    fn replace_instruction(&mut self, pos: usize, new_instruction: Instructions) {
        for (i, byte) in new_instruction.iter().enumerate() {
            self.scopes[self.scope_index].instructions[pos + i] = *byte;
        }
    }

    fn replace_last_pop_with_return(&mut self) {
        let pos = self.scopes[self.scope_index].last_instruction.position;
        self.replace_instruction(pos, make(OpCode::OpReturnValue, &[]));
        self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnValue);
    }

    fn make_immutability_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("IMMUTABLE BINDING")
        .with_code("E003")
        .with_file(self.file_path.clone())
        .with_position(position)
        .with_message(format!(
            "Cannot assign twice to immutable variable `{}`.",
            name
        ))
        .with_hint(
            "Variables in Flux are immutable by default; once you bind a value, you cannot change it.",
        )
        .with_hint(format!(
            "Use a different name instead: let {} = ...; let {}2 = ...;",
            name, name
        ))
    }

    fn make_undefined_variable_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("UNDEFINED VARIABLE")
            .with_code("E007")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!("I can't find a value named `{}`.", name))
            .with_hint(format!("Define it first: let {} = ...;", name))
    }

    fn make_redeclaration_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("DUPLICATE NAME")
            .with_code("E001")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!("`{}` was already declared in this scope.", name))
            .with_hint(format!(
                "Use a different name: let {} = ...; let {}2 = ...;",
                name, name
            ))
    }

    fn make_outer_assignment_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("OUTER ASSIGNMENT")
            .with_code("E004")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!(
                "Cannot assign to outer variable `{}` from this scope.",
                name
            ))
            .with_hint(format!(
                "Use a new binding (shadowing) instead: let {} = ...;",
                name
            ))
    }

    fn make_module_name_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("INVALID MODULE NAME")
            .with_code("E016")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!("Invalid module name `{}`.", name))
            .with_hint("Module names must start with an uppercase letter.")
            .with_hint("Use an uppercase identifier, e.g. `module Math { ... }`")
    }

    fn make_import_collision_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("IMPORT NAME COLLISION")
            .with_code("E043")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!(
                "Cannot import `{}`; name already defined in this scope.",
                name
            ))
            .with_hint("Use a different name or remove the existing binding.")
    }

    fn make_import_scope_error(&self, name: &str, position: Position) -> Diagnostic {
        Diagnostic::error("IMPORT SCOPE")
            .with_code("E031")
            .with_file(self.file_path.clone())
            .with_position(position)
            .with_message(format!("Cannot import `{}` inside a function.", name))
            .with_hint("Move the import to the top level.")
    }

    fn find_duplicate_name(names: &[String]) -> Option<&str> {
        let mut seen = HashSet::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                return Some(name.as_str());
            }
        }
        None
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
