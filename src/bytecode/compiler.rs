use std::rc::Rc;

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
        expression::Expression,
        program::Program,
        statement::{self, Statement},
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

pub struct Compiler {
    constants: Vec<Object>,
    pub symbol_table: SymbolTable,
    scopes: Vec<CompilationScope>,
    scope_index: usize,
}

impl Compiler {
    pub fn new() -> Self {
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
        }
    }

    pub fn new_with_state(symbol_table: SymbolTable, constants: Vec<Object>) -> Self {
        let mut compiler = Self::new();
        compiler.symbol_table = symbol_table;
        compiler.constants = constants;
        compiler
    }

    pub fn compile(&mut self, program: &Program) -> Result<(), String> {
        for statement in &program.statements {
            self.compile_statement(statement)?;
        }
        Ok(())
    }

    fn compile_statement(&mut self, statement: &Statement) -> Result<(), String> {
        match statement {
            Statement::Expression { expression } => {
                self.compile_expression(expression)?;
                self.emit(OpCode::OpPop, &[]);
            }
            Statement::Let { name, value } => {
                let symbol = self.symbol_table.define(name);
                self.compile_expression(value)?;

                match symbol.symbol_scope {
                    SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                    SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                    _ => 0,
                };
            }
            Statement::Return { value } => match value {
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
            } => {
                self.compile_function_statement(name, parameters, body)?;
            }
            _ => {
                eprintln!("compile_statement: error {}", statement)
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

    fn compile_expression(&mut self, expression: &Expression) -> Result<(), String> {
        match expression {
            Expression::Integer(value) => {
                let idx = self.add_constant(Object::Integer(*value));
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
                let symbol = self
                    .symbol_table
                    .resolve(name)
                    .ok_or_else(|| format!("undefined variable: {}", name))?;
                self.load_symbol(&symbol);
            }
            Expression::Prefix { operator, right } => {
                self.compile_expression(right)?;
                match operator.as_str() {
                    "!" => self.emit(OpCode::OpBang, &[]),
                    "-" => self.emit(OpCode::OpMinus, &[]),
                    _ => return Err(format!("unknown prefix operator: {}", operator)),
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
                    _ => return Err(format!("unknown infix operator: {}", operator)),
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
            _ => {
                println!(
                    "compile_expression: expression cannot identified: {}",
                    expression
                );
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

    fn compile_function_literal(
        &mut self,
        parameters: &[String],
        body: &Block,
    ) -> Result<(), String> {
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

    fn compile_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
    ) -> Result<(), String> {
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

    fn compile_function_statement(
        &mut self,
        name: &str,
        parameters: &[String],
        body: &Block,
    ) -> Result<(), String> {
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

    fn compile_block(&mut self, block: &Block) -> Result<(), String> {
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
}
