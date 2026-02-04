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
        module_constants::compile_module_constants,
        op_code::{Instructions, OpCode, make},
        symbol::Symbol,
        symbol_scope::SymbolScope,
        symbol_table::SymbolTable,
    },
    frontend::{
        block::Block,
        diagnostics::{
            CIRCULAR_DEPENDENCY, DUPLICATE_NAME, DUPLICATE_PARAMETER, Diagnostic, ErrorType,
            ICE_SYMBOL_SCOPE_ASSIGN, ICE_SYMBOL_SCOPE_LET, IMMUTABLE_BINDING,
            IMPORT_NAME_COLLISION, IMPORT_SCOPE, INVALID_MODULE_CONTENT, INVALID_MODULE_NAME,
            MODULE_NAME_CLASH, MODULE_SCOPE, OUTER_ASSIGNMENT, PRIVATE_MEMBER, UNDEFINED_VARIABLE,
            lookup_error_code,
        },
        module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
        position::{Position, Span},
        program::Program,
        statement::Statement,
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

mod expression;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

pub struct Compiler {
    constants: Vec<Object>,
    pub symbol_table: SymbolTable,
    pub(super) scopes: Vec<CompilationScope>,
    pub(super) scope_index: usize,
    pub errors: Vec<Diagnostic>,
    pub(super) file_path: String,
    imported_files: HashSet<String>,
    file_scope_symbols: HashSet<String>,
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
                    // Check for duplicate in current scope FIRST (takes precedence)
                    if let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                    {
                        return Err(Self::boxed(
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
                        ));
                    }
                    // Then check for import collision (only if not a duplicate in same scope)
                    if self.scope_index == 0 && self.file_scope_symbols.contains(name) {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_NAME_COLLISION,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }

                    let symbol = self.symbol_table.define(name, *span);
                    self.compile_expression(value)?;

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
                        self.file_scope_symbols.insert(name.clone());
                    }
                }
                Statement::Assign { name, value, span } => {
                    // Check if variable exists
                    let symbol = self.symbol_table.resolve(name).ok_or_else(|| {
                        Self::boxed(Diagnostic::make_error(
                            &UNDEFINED_VARIABLE,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        ))
                    })?;

                    if symbol.symbol_scope == SymbolScope::Free {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &OUTER_ASSIGNMENT,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }

                    // Check if variable is already assigned (immutability check)
                    if symbol.is_assigned {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMMUTABLE_BINDING,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }

                    self.compile_expression(value)?;

                    match symbol.symbol_scope {
                        SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                        SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_SYMBOL_SCOPE_ASSIGN,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
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
                    // For top-level functions, checks were already done in pass 1
                    // Only check for nested functions (scope_index > 0)
                    if self.scope_index > 0
                        && let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                    {
                        return Err(Self::boxed(
                            Diagnostic::make_error(
                                &DUPLICATE_NAME,
                                &[name],
                                self.file_path.clone(),
                                *span,
                            )
                            .with_hint_text(
                                "Use a different name or remove the previous definition",
                            )
                            .with_hint_labeled(
                                "",
                                existing.span,
                                "first defined here",
                            ),
                        ));
                    }
                    self.compile_function_statement(name, parameters, body, span.start)?;
                    // For nested functions, add to file_scope_symbols
                    if self.scope_index == 0 {
                        // Already added in pass 1 for top-level functions
                        self.file_scope_symbols.insert(name.clone());
                    }
                }
                Statement::Module { name, body, span } => {
                    if self.scope_index > 0 {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_SCOPE,
                            &[],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let binding_name = module_binding_name(name);
                    if self.scope_index == 0 && self.file_scope_symbols.contains(binding_name) {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_NAME_COLLISION,
                            &[binding_name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    if !is_valid_module_name(name) {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &INVALID_MODULE_NAME,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    self.compile_module_statement(name, body, span.start)?;
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(binding_name.to_string());
                    }
                }
                Statement::Import { name, alias, span } => {
                    if self.scope_index > 0 {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_SCOPE,
                            &[name],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let binding_name = import_binding_name(name, alias.as_deref());
                    if self.file_scope_symbols.contains(binding_name) {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_NAME_COLLISION,
                            &[binding_name],
                            self.file_path.clone(),
                            *span,
                        )));
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

    fn compile_function_statement(
        &mut self,
        name: &str,
        parameters: &[String],
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        if let Some(param) = Self::find_duplicate_name(parameters) {
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param],
                self.file_path.clone(),
                Span::new(position, position),
            )));
        }

        // Resolve the symbol - it may have been predeclared in pass 1
        let function_span = Span::new(position, position);
        let symbol = if let Some(existing) = self.symbol_table.resolve(name) {
            // Use the existing symbol from pass 1
            existing
        } else {
            // Define new symbol (for nested functions or non-predeclared cases)
            self.symbol_table.define(name, function_span)
        };

        self.enter_scope();
        self.symbol_table.define_function_name(name, function_span);

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
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_NAME,
                &[binding_name],
                self.file_path.clone(),
                Span::new(position, position),
            )));
        }

        // Collect all functions from the module body and validate contents
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if fn_name == binding_name {
                        let pos = statement.position();
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NAME_CLASH,
                            &[binding_name],
                            self.file_path.clone(),
                            Span::new(pos, pos),
                        )));
                    }
                }
                // Module Constants Allow let statements in modules
                Statement::Let { .. } => {
                    // Let statements are allowed for module constants
                }
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

        self.imported_modules.insert(binding_name.to_string());
        let previous_module = self.current_module_prefix.clone();
        self.current_module_prefix = Some(binding_name.to_string());

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
        let constants = match compile_module_constants(body, binding_name) {
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
                let qualified_name = format!("{}.{}", binding_name, fn_name);
                // Check for duplicate declaration
                if let Some(existing) = self.symbol_table.resolve(&qualified_name)
                    && self.symbol_table.exists_in_current_scope(&qualified_name)
                {
                    self.current_module_prefix = previous_module;
                    return Err(Self::boxed(
                        Diagnostic::make_error(
                            &DUPLICATE_NAME,
                            &[&qualified_name],
                            self.file_path.clone(),
                            *span,
                        )
                        .with_hint_text("Use a different name or remove the previous definition")
                        .with_hint_labeled(
                            "",
                            existing.span,
                            "first defined here",
                        ),
                    ));
                }
                // Predeclare the function
                self.symbol_table.define(&qualified_name, *span);
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

    pub(super) fn compile_block(&mut self, block: &Block) -> CompileResult<()> {
        for statement in &block.statements {
            self.compile_statement(statement)?;
        }

        Ok(())
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
    fn convert_const_compile_error(
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
