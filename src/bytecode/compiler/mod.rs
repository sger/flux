use std::collections::{HashMap, HashSet};

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
        diagnostics::{
            CIRCULAR_DEPENDENCY, DUPLICATE_NAME, Diagnostic, ErrorType, IMPORT_NAME_COLLISION,
            PRIVATE_MEMBER, lookup_error_code,
        },
        position::{Position, Span},
        program::Program,
        statement::Statement,
    },
    runtime::object::Object,
};

mod expression;
mod statement;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

pub struct Compiler {
    constants: Vec<Object>,
    pub symbol_table: SymbolTable,
    pub(super) scopes: Vec<CompilationScope>,
    pub(super) scope_index: usize,
    pub errors: Vec<Diagnostic>,
    pub(super) file_path: String,
    imported_files: HashSet<String>,
    pub(super) file_scope_symbols: HashSet<String>,
    pub(super) imported_modules: HashSet<String>,
    pub(super) import_aliases: HashMap<String, String>,
    pub(super) current_module_prefix: Option<String>,
    pub(super) current_span: Option<Span>,
    // Module Constants - stores compile-time evaluated module constants
    pub(super) module_constants: HashMap<String, Object>,
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
        symbol_table.define_builtin(7, "concat");
        symbol_table.define_builtin(8, "reverse");
        symbol_table.define_builtin(9, "contains");
        symbol_table.define_builtin(10, "slice");
        symbol_table.define_builtin(11, "sort");
        symbol_table.define_builtin(12, "split");
        symbol_table.define_builtin(13, "join");
        symbol_table.define_builtin(14, "trim");
        symbol_table.define_builtin(15, "upper");
        symbol_table.define_builtin(16, "lower");
        symbol_table.define_builtin(17, "chars");
        symbol_table.define_builtin(18, "substring");
        symbol_table.define_builtin(19, "keys");
        symbol_table.define_builtin(20, "values");
        symbol_table.define_builtin(21, "has_key");
        symbol_table.define_builtin(22, "merge");
        symbol_table.define_builtin(23, "abs");
        symbol_table.define_builtin(24, "min");
        symbol_table.define_builtin(25, "max");
        // Type Checking Builtins (5.5)
        symbol_table.define_builtin(26, "type_of");
        symbol_table.define_builtin(27, "is_int");
        symbol_table.define_builtin(28, "is_float");
        symbol_table.define_builtin(29, "is_string");
        symbol_table.define_builtin(30, "is_bool");
        symbol_table.define_builtin(31, "is_array");
        symbol_table.define_builtin(32, "is_hash");
        symbol_table.define_builtin(33, "is_none");
        symbol_table.define_builtin(34, "is_some");

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
            // Module Constants
            module_constants: HashMap::new(),
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

    pub(super) fn boxed(diag: Diagnostic) -> Box<Diagnostic> {
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
                // Check for duplicate declaration first (takes precedence)
                if let Some(existing) = self.symbol_table.resolve(name)
                    && self.symbol_table.exists_in_current_scope(name)
                {
                    self.errors.push(
                        Diagnostic::make_error(
                            &DUPLICATE_NAME,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )
                        .with_hint_labeled(
                            "",
                            existing.span,
                            "first defined here",
                        ),
                    );
                    continue;
                }
                // Check for import collision
                if self.scope_index == 0 && self.file_scope_symbols.contains(name) {
                    self.errors.push(Diagnostic::make_error(
                        &IMPORT_NAME_COLLISION,
                        &[name],
                        self.file_path.clone(),
                        *span,
                    ));
                    continue;
                }
                // Predeclare the function name
                self.symbol_table.define(name, *span);
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

    pub(super) fn emit(&mut self, op_code: OpCode, operands: &[usize]) -> usize {
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

    // Module Constants helper to emit any Object as a constant
    pub(super) fn emit_constant_object(&mut self, obj: Object) {
        match obj {
            Object::Boolean(true) => self.emit(OpCode::OpTrue, &[]),
            Object::Boolean(false) => self.emit(OpCode::OpFalse, &[]),
            Object::None => self.emit(OpCode::OpNone, &[]),
            _ => {
                let idx = self.add_constant(obj);
                self.emit(OpCode::OpConstant, &[idx])
            }
        };
    }

    pub(super) fn add_constant(&mut self, obj: Object) -> usize {
        self.constants.push(obj);
        self.constants.len() - 1
    }

    pub(super) fn load_symbol(&mut self, symbol: &Symbol) {
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

    pub(super) fn check_private_member(
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

        Err(Self::boxed(Diagnostic::make_error(
            &PRIVATE_MEMBER,
            &[member],
            self.file_path.clone(),
            expr_span,
        )))
    }
    pub(super) fn enter_scope(&mut self) {
        self.scopes.push(CompilationScope::new());
        self.scope_index += 1;
        self.symbol_table = SymbolTable::new_enclosed(self.symbol_table.clone());
    }

    pub(super) fn leave_scope(&mut self) -> (Instructions, Vec<InstructionLocation>, Vec<String>) {
        let scope = self.scopes.pop().unwrap();
        self.scope_index -= 1;
        if let Some(outer) = self.symbol_table.outer.take() {
            self.symbol_table = *outer;
        }

        (scope.instructions, scope.locations, scope.files)
    }

    pub(super) fn enter_block_scope(&mut self) {
        let mut block_table = SymbolTable::new_block(self.symbol_table.clone());
        block_table.num_definitions = self.symbol_table.num_definitions;
        self.symbol_table = block_table;
    }

    pub(super) fn leave_block_scope(&mut self) {
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

    pub(super) fn is_last_instruction(&self, opcode: OpCode) -> bool {
        self.scopes[self.scope_index].last_instruction.opcode == Some(opcode)
    }

    pub(super) fn remove_last_pop(&mut self) {
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

    pub(super) fn change_operand(&mut self, op_pos: usize, operand: usize) {
        let op_code = OpCode::from(self.current_instructions()[op_pos]);
        self.replace_instruction(op_pos, make(op_code, &[operand]));
    }

    pub(super) fn current_instructions(&self) -> &Instructions {
        &self.scopes[self.scope_index].instructions
    }

    fn replace_instruction(&mut self, pos: usize, new_instruction: Instructions) {
        for (i, byte) in new_instruction.iter().enumerate() {
            self.scopes[self.scope_index].instructions[pos + i] = *byte;
        }
    }

    pub(super) fn replace_last_pop_with_return(&mut self) {
        let pos = self.scopes[self.scope_index].last_instruction.position;
        self.replace_instruction(pos, make(OpCode::OpReturnValue, &[]));
        self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnValue);
    }

    pub(super) fn find_duplicate_name(names: &[String]) -> Option<&str> {
        let mut seen = HashSet::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Converts a `ConstCompileError` to a `Diagnostic`.
    pub(super) fn convert_const_compile_error(
        &self,
        err: super::module_constants::ConstCompileError,
        position: Position,
    ) -> Diagnostic {
        match err {
            super::module_constants::ConstCompileError::CircularDependency(cycle) => {
                let cycle_str = cycle.join(" -> ");
                Diagnostic::make_error(
                    &CIRCULAR_DEPENDENCY,
                    &[&cycle_str],
                    self.file_path.clone(),
                    Span::new(position, position),
                )
            }
            super::module_constants::ConstCompileError::EvalError {
                position: pos,
                error,
                ..
            } => {
                // Try to look up the error code in the registry to get proper title and type
                let (title, error_type) = lookup_error_code(error.code)
                    .map(|ec| (ec.title, ec.error_type))
                    .unwrap_or(("CONSTANT EVALUATION ERROR", ErrorType::Compiler));

                Diagnostic::make_error_dynamic(
                    error.code,
                    title,
                    error_type,
                    error.message,
                    error.hint,
                    self.file_path.clone(),
                    Span::new(pos, pos),
                )
            }
        }
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
