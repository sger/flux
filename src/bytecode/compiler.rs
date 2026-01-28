use std::{collections::HashSet, fs, path::Path, rc::Rc};

use crate::{
    bytecode::{
        bytecode::Bytecode,
        compilation_scope::CompilationScope,
        emitted_instruction::EmittedInstruction,
        op_code::{Instructions, OpCode, make},
        symbol::Symbol,
        symbol_scope::SymbolScope,
        symbol_table::SymbolTable,
    },
    frontend::{
        block::Block,
        diagnostic::Diagnostic,
        expression::{Expression, MatchArm, Pattern},
        lexer::Lexer,
        parser::Parser,
        position::Position,
        program::Program,
        statement::Statement,
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

pub struct Compiler {
    constants: Vec<Object>,
    pub symbol_table: SymbolTable,
    scopes: Vec<CompilationScope>,
    scope_index: usize,
    pub errors: Vec<Diagnostic>,
    file_path: String,
    imported_files: HashSet<String>,
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

        Self {
            constants: Vec::new(),
            symbol_table,
            scopes: vec![CompilationScope::new()],
            scope_index: 0,
            errors: Vec::new(),
            file_path: file_path.into(),
            imported_files: HashSet::new(),
        }
    }

    pub fn new_with_state(symbol_table: SymbolTable, constants: Vec<Object>) -> Self {
        let mut compiler = Self::new();
        compiler.symbol_table = symbol_table;
        compiler.constants = constants;
        compiler
    }

    pub fn compile(&mut self, program: &Program) -> Result<(), Vec<Diagnostic>> {
        for statement in &program.statements {
            // Continue compilation even if there are errors
            if let Err(err) = self.compile_statement(statement) {
                self.errors.push(err);
            }
        }

        // Return all errors at the end
        if !self.errors.is_empty() {
            return Err(std::mem::take(&mut self.errors));
        }

        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn compile_statement(&mut self, statement: &Statement) -> Result<(), Diagnostic> {
        match statement {
            Statement::Expression { expression, .. } => {
                self.compile_expression(expression)?;
                self.emit(OpCode::OpPop, &[]);
            }
            Statement::Let {
                name,
                value,
                position,
            } => {
                if self.symbol_table.exists_in_current_scope(name) {
                    return Err(self.make_redeclaration_error(name, *position));
                }

                let symbol = self.symbol_table.define(name);
                self.compile_expression(value)?;

                match symbol.symbol_scope {
                    SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                    SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                    _ => 0,
                };

                self.symbol_table.mark_assigned(name).ok();
            }
            Statement::Assign {
                name,
                value,
                position,
            } => {
                // Check if variable exists
                let symbol = self
                    .symbol_table
                    .resolve(name)
                    .ok_or_else(|| self.make_undefined_variable_error(name, *position))?;

                if symbol.symbol_scope == SymbolScope::Free {
                    return Err(self.make_outer_assignment_error(name, *position));
                }

                // Check if variable is already assigned (immutability check)
                if symbol.is_assigned {
                    return Err(self.make_immutability_error(name, *position));
                }

                self.compile_expression(value)?;

                match symbol.symbol_scope {
                    SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                    SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                    _ => 0,
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
                position,
                ..
            } => {
                self.compile_function_statement(name, parameters, body, *position)?;
            }
            Statement::Module {
                name,
                body,
                position,
            } => {
                if !Self::is_uppercase_identifier(name) {
                    return Err(self.make_module_name_error(name, *position));
                }
                self.compile_module_statement(name, body, *position)?;
            }
            Statement::Import { name, position } => {
                if self.scope_index > 0 {
                    return Err(self.make_import_scope_error(name, *position));
                }
                self.compile_import_statement(name, *position)?;
            }
        }

        Ok(())
    }

    fn emit(&mut self, op_code: OpCode, operands: &[usize]) -> usize {
        let instruction = make(op_code, operands);
        let pos = self.add_instruction(&instruction);
        self.set_last_instruction(op_code, pos);
        pos
    }

    fn add_instruction(&mut self, instruction: &[u8]) -> usize {
        let pos = self.scopes[self.scope_index].instructions.len();
        self.scopes[self.scope_index]
            .instructions
            .extend_from_slice(instruction);
        pos
    }

    fn set_last_instruction(&mut self, op_code: OpCode, pos: usize) {
        let previous = self.scopes[self.scope_index].last_instruction.clone();
        self.scopes[self.scope_index].previous_instruction = previous;
        self.scopes[self.scope_index].last_instruction = EmittedInstruction {
            opcode: Some(op_code),
            position: pos,
        };
    }

    #[allow(clippy::result_large_err)]
    fn compile_expression(&mut self, expression: &Expression) -> Result<(), Diagnostic> {
        match expression {
            Expression::Integer(value) => {
                let idx = self.add_constant(Object::Integer(*value));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            Expression::Float(value) => {
                let idx = self.add_constant(Object::Float(*value));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            Expression::String(value) => {
                let idx = self.add_constant(Object::String(value.clone()));
                self.emit(OpCode::OpConstant, &[idx]);
            }
            Expression::Boolean(value) => {
                if *value {
                    self.emit(OpCode::OpTrue, &[]);
                } else {
                    self.emit(OpCode::OpFalse, &[]);
                }
            }
            Expression::Null => {
                self.emit(OpCode::OpNull, &[]);
            }
            Expression::Identifier(name) => {
                let symbol = self.symbol_table.resolve(name).ok_or_else(|| {
                    Diagnostic::error(format!("undefined variable `{}`", name))
                        .with_hint(format!("Define it first: let {} = ...;", name))
                })?;
                self.load_symbol(&symbol);
            }
            Expression::Prefix { operator, right } => {
                self.compile_expression(right)?;
                match operator.as_str() {
                    "!" => self.emit(OpCode::OpBang, &[]),
                    "-" => self.emit(OpCode::OpMinus, &[]),
                    _ => {
                        return Err(Diagnostic::error("UNKNOWN PREFIX OPERATOR")
                            .with_code("E010")
                            .with_message(format!("Unknown prefix operator `{}`.", operator)));
                    }
                };
            }
            Expression::Infix {
                left,
                operator,
                right,
            } => {
                if operator == "<" {
                    self.compile_expression(right)?;
                    self.compile_expression(left)?;
                    self.emit(OpCode::OpGreaterThan, &[]);
                    return Ok(());
                }

                self.compile_expression(left)?;
                self.compile_expression(right)?;

                match operator.as_str() {
                    "+" => self.emit(OpCode::OpAdd, &[]),
                    "-" => self.emit(OpCode::OpSub, &[]),
                    "*" => self.emit(OpCode::OpMul, &[]),
                    "/" => self.emit(OpCode::OpDiv, &[]),
                    "==" => self.emit(OpCode::OpEqual, &[]),
                    "!=" => self.emit(OpCode::OpNotEqual, &[]),
                    ">" => self.emit(OpCode::OpGreaterThan, &[]),
                    _ => {
                        return Err(Diagnostic::error("UNKNOWN INFIX OPERATOR")
                            .with_code("E011")
                            .with_message(format!("Unknown infix operator `{}`.", operator))
                            .with_hint("Use a supported operator like +, -, *, /, ==, !=, or >."));
                    }
                };
            }
            Expression::If {
                condition,
                consequence,
                alternative,
            } => {
                self.compile_if_expression(condition, consequence, alternative)?;
            }
            Expression::Function { parameters, body } => {
                self.compile_function_literal(parameters, body)?;
            }
            Expression::Array { elements } => {
                for el in elements {
                    self.compile_expression(el)?;
                }
                self.emit(OpCode::OpArray, &[elements.len()]);
            }
            Expression::Hash { pairs } => {
                let mut sorted_pairs: Vec<_> = pairs.iter().collect();
                sorted_pairs.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

                for (key, value) in sorted_pairs {
                    self.compile_expression(key)?;
                    self.compile_expression(value)?;
                }
                self.emit(OpCode::OpHash, &[pairs.len() * 2]);
            }
            Expression::Index { left, index } => {
                self.compile_expression(left)?;
                self.compile_expression(index)?;
                self.emit(OpCode::OpIndex, &[]);
            }
            Expression::Call {
                function,
                arguments,
            } => {
                self.compile_expression(function)?;

                for arg in arguments {
                    self.compile_expression(arg)?;
                }

                self.emit(OpCode::OpCall, &[arguments.len()]);
            }
            Expression::MemberAccess { object, member } => {
                // Check if accessing a private member (starts with underscore)
                if member.starts_with('_') {
                    return Err(Diagnostic::error("PRIVATE MEMBER")
                        .with_code("E021")
                        .with_file(self.file_path.clone())
                        .with_message(format!("Cannot access private member `{}`.", member))
                        .with_hint(
                            "Private members can only be accessed within the same module.",
                        ));
                }

                // Compile the object (e.g., the module identifier)
                self.compile_expression(object)?;

                // Emit the member name as a string constant (the hash key)
                let member_idx = self.add_constant(Object::String(member.clone()));
                self.emit(OpCode::OpConstant, &[member_idx]);

                // Use index operation to access the member from the hash
                self.emit(OpCode::OpIndex, &[]);
            }
            Expression::None => {
                self.emit(OpCode::OpNone, &[]);
            }
            Expression::Some { value } => {
                self.compile_expression(value)?;
                self.emit(OpCode::OpSome, &[]);
            }
            Expression::Match { scrutinee, arms } => {
                self.compile_match_expression(scrutinee, arms)?;
            }
        }
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

    #[allow(clippy::result_large_err)]
    fn compile_function_literal(
        &mut self,
        parameters: &[String],
        body: &Block,
    ) -> Result<(), Diagnostic> {
        if let Some(name) = Self::find_duplicate_name(parameters) {
            return Err(Diagnostic::error("DUPLICATE PARAMETER")
                .with_code("E012")
                .with_message(format!(
                    "Duplicate parameter `{}` in function literal.",
                    name
                ))
                .with_hint("Parameter names must be unique."));
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
        let instructions = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let fn_idx = self.add_constant(Object::Function(Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            parameters.len(),
        ))));

        self.emit(OpCode::OpClosure, &[fn_idx, free_symbols.len()]);

        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn compile_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
    ) -> Result<(), Diagnostic> {
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
            self.emit(OpCode::OpNull, &[]);
        }

        self.change_operand(jump_pos, self.current_instructions().len());
        Ok(())
    }

    fn compile_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
    ) -> Result<(), Diagnostic> {
        if arms.is_empty() {
            return Err(Diagnostic::error("EMPTY MATCH")
                .with_code("E030")
                .with_message("Match expression must have at least one arm."));
        }

        // Compile scrutinee once and store it in a local variable
        // For simplicity, we'll use a temporary approach: keep it on stack and duplicate

        self.compile_expression(scrutinee)?;

        let mut end_jumps = Vec::new();

        // Compile each arm
        for (i, arm) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;

            // For all arms except the last, we need to check if pattern matches
            if !is_last {
                // Duplicate scrutinee for pattern check
                // We'll emit code to check the pattern and jump to next arm if not matched
                let next_arm_jump = self.compile_pattern_check(scrutinee, &arm.pattern)?;

                // Pattern matched, compile the body
                self.compile_expression(&arm.body)?;

                // Jump to end after executing this arm's body
                let end_jump = self.emit(OpCode::OpJump, &[9999]);
                end_jumps.push(end_jump);

                // Patch jump to next arm
                self.change_operand(next_arm_jump, self.current_instructions().len());

                // When we jump here, stack is empty (OpEqual consumed scrutinee,
                // OpJumpNotTruthy popped the boolean). Push scrutinee for next arm.
                self.compile_expression(scrutinee)?;
            } else {
                // Last arm: pop scrutinee and compile body (assume it matches)
                self.emit(OpCode::OpPop, &[]);
                self.compile_expression(&arm.body)?;
            }
        }

        // Patch all end jumps to point here
        for jump_pos in end_jumps {
            self.change_operand(jump_pos, self.current_instructions().len());
        }

        Ok(())
    }

    fn compile_pattern_check(
        &mut self,
        _scrutinee: &Expression,
        pattern: &Pattern,
    ) -> Result<usize, Diagnostic> {
        match pattern {
            Pattern::Wildcard => {
                // Wildcard always matches, so we never jump to next arm
                // Emit OpTrue and OpJumpNotTruthy (which will never jump)
                // Actually, for wildcard we should always execute this arm
                // So we return a dummy jump position that will never be used
                // For simplicity, emit a condition that's always true
                self.emit(OpCode::OpTrue, &[]);
                Ok(self.emit(OpCode::OpJumpNotTruthy, &[9999]))
            }
            Pattern::Literal(expr) => {
                // Push pattern value onto stack: [scrutinee, pattern]
                // OpEqual compares and pushes boolean: [result]
                // OpJumpNotTruthy jumps when false (no match), continues when true (match)
                self.compile_expression(expr)?;
                self.emit(OpCode::OpEqual, &[]);
                Ok(self.emit(OpCode::OpJumpNotTruthy, &[9999]))
            }
            Pattern::None => {
                // Check if scrutinee is None
                self.emit(OpCode::OpNone, &[]);
                self.emit(OpCode::OpEqual, &[]);
                Ok(self.emit(OpCode::OpJumpNotTruthy, &[9999]))
            }
            Pattern::Some(_inner) => {
                // For now, just check if it's Some type
                // TODO: Implement proper pattern matching for Some(inner)
                // This requires unwrapping the Some and recursively matching
                // For simplicity, we'll leave this as a todo
                return Err(Diagnostic::error("UNSUPPORTED PATTERN")
                    .with_code("E031")
                    .with_message("Pattern matching on Some(inner) is not yet implemented.")
                    .with_hint("Use a simpler pattern or check if value is Some directly."));
            }
            Pattern::Identifier(_name) => {
                // Identifier always matches and binds the value
                // For now, we'll treat it like wildcard
                // TODO: Implement proper binding
                self.emit(OpCode::OpTrue, &[]);
                Ok(self.emit(OpCode::OpJumpNotTruthy, &[9999]))
            }
        }
    }

    #[allow(clippy::result_large_err)]
    fn compile_function_statement(
        &mut self,
        name: &str,
        parameters: &[String],
        body: &Block,
        position: Position,
    ) -> Result<(), Diagnostic> {
        if self.symbol_table.exists_in_current_scope(name) {
            return Err(self.make_redeclaration_error(name, position));
        }

        if let Some(param) = Self::find_duplicate_name(parameters) {
            return Err(Diagnostic::error("DUPLICATE PARAMETER")
                .with_code("E012")
                .with_file(self.file_path.clone())
                .with_position(position)
                .with_message(format!(
                    "Duplicate parameter `{}` in function `{}`.",
                    param, name
                ))
                .with_hint("Use distinct parameter names."));
        }

        let symbol = self.symbol_table.define(name);

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
        let instructions = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let fn_idx = self.add_constant(Object::Function(Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            parameters.len(),
        ))));
        self.emit(OpCode::OpClosure, &[fn_idx, free_symbols.len()]);

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => 0,
        };
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn compile_module_statement(
        &mut self,
        name: &str,
        body: &Block,
        position: Position,
    ) -> Result<(), Diagnostic> {
        // Check if module is already defined
        if self.symbol_table.exists_in_current_scope(name) {
            return Err(self.make_redeclaration_error(name, position));
        }

        // Define the module symbol early so functions can reference it
        let module_symbol = self.symbol_table.define(name);

        // Collect all functions from the module body and validate contents
        let mut function_names = Vec::new();
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if fn_name == name {
                        return Err(Diagnostic::error("MODULE NAME CLASH")
                            .with_code("E018")
                            .with_position(statement.position())
                            .with_message(format!(
                                "Module `{}` cannot define a function with the same name.",
                                name
                            ))
                            .with_hint("Use a different function name."));
                    }
                    function_names.push(fn_name.clone());
                }
                _ => {
                    return Err(Diagnostic::error("INVALID MODULE CONTENT")
                        .with_code("E019")
                        .with_position(statement.position())
                        .with_message("Modules can only contain function declarations."));
                }
            }
        }

        // Compile the module body in its own initializer scope so functions
        // don't leak into the outer scope.
        self.enter_scope();

        // Compile each function in the module initializer scope
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                parameters,
                body: fn_body,
                position,
                ..
            } = statement
                && let Err(err) =
                    self.compile_function_statement(fn_name, parameters, fn_body, *position)
            {
                self.leave_scope();
                return Err(err);
            }
        }

        let public_function_names: Vec<_> = function_names
            .into_iter()
            .filter(|name| !name.starts_with('_'))
            .collect();

        // Now create a hash containing all the public module functions
        // For each function, we need to: emit the key, load the function, emit the hash pair
        let num_functions = public_function_names.len();
        for fn_name in &public_function_names {
            // Emit the key (function name as string)
            let key_idx = self.add_constant(Object::String(fn_name.clone()));
            self.emit(OpCode::OpConstant, &[key_idx]);

            // Load the function value
            if let Some(symbol) = self.symbol_table.resolve(fn_name) {
                self.load_symbol(&symbol);
            }
        }

        // Create the hash with all the function pairs and return it
        self.emit(OpCode::OpHash, &[num_functions * 2]);
        self.emit(OpCode::OpReturnValue, &[]);

        let free_symbols = self.symbol_table.free_symbols.clone();
        let num_locals = self.symbol_table.num_definitions;
        let instructions = self.leave_scope();

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let fn_idx = self.add_constant(Object::Function(Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            0,
        ))));
        self.emit(OpCode::OpClosure, &[fn_idx, free_symbols.len()]);
        self.emit(OpCode::OpCall, &[0]);

        // Store the hash in the module variable
        match module_symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[module_symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[module_symbol.index]),
            _ => 0,
        };

        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn compile_import_statement(
        &mut self,
        name: &str,
        position: Position,
    ) -> Result<(), Diagnostic> {
        if self.symbol_table.exists_in_current_scope(name) {
            return Err(self.make_import_collision_error(name, position));
        }

        let base_dir = Path::new(&self.file_path)
            .parent()
            .unwrap_or(Path::new("."));
        let candidates = [
            base_dir.join(format!("{}.flx", name)),
            base_dir.join(format!("{}.flx", name.to_lowercase())),
        ];

        let import_path = candidates.into_iter().find(|path| path.exists());
        let import_path = match import_path {
            Some(path) => path,
            None => {
                return Err(Diagnostic::error("IMPORT NOT FOUND")
                    .with_code("E032")
                    .with_position(position)
                    .with_message(format!("no module file found for `{}`", name))
                    .with_hint(format!(
                        "Looked for `{}` and `{}` next to this file.",
                        base_dir.join(format!("{}.flx", name)).display(),
                        base_dir
                            .join(format!("{}.flx", name.to_lowercase()))
                            .display()
                    )));
            }
        };

        let canonical_path = fs::canonicalize(&import_path).unwrap_or(import_path);
        let canonical_str = canonical_path.to_string_lossy().to_string();
        if self.imported_files.contains(&canonical_str) {
            return Ok(());
        }
        self.imported_files.insert(canonical_str.clone());

        let source = fs::read_to_string(&canonical_path).map_err(|err| {
            Diagnostic::error("IMPORT READ FAILED")
                .with_code("E033")
                .with_position(position)
                .with_message(format!("{}: {}", canonical_str, err))
        })?;

        let lexer = Lexer::new(&source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        if !parser.errors.is_empty() {
            for diag in parser.errors {
                self.errors.push(diag.with_file(canonical_str.clone()));
            }
            return Ok(());
        }

        let previous_file_path = std::mem::replace(&mut self.file_path, canonical_str);
        for statement in &program.statements {
            if let Err(err) = self.compile_statement(statement) {
                self.errors.push(err);
            }
        }
        self.file_path = previous_file_path;

        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn compile_block(&mut self, block: &Block) -> Result<(), Diagnostic> {
        for statement in &block.statements {
            self.compile_statement(statement)?;
        }

        Ok(())
    }
    fn enter_scope(&mut self) {
        self.scopes.push(CompilationScope::new());
        self.scope_index += 1;
        self.symbol_table = SymbolTable::new_enclosed(self.symbol_table.clone());
    }

    fn leave_scope(&mut self) -> Instructions {
        let scope = self.scopes.pop().unwrap();
        self.scope_index -= 1;
        if let Some(outer) = self.symbol_table.outer.take() {
            self.symbol_table = *outer;
        }

        scope.instructions
    }

    pub fn bytecode(&self) -> Bytecode {
        Bytecode {
            instructions: self.scopes[self.scope_index].instructions.clone(),
            constants: self.constants.clone(),
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
            .with_code("E030")
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

    fn is_uppercase_identifier(name: &str) -> bool {
        name.chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false)
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
