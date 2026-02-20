use std::collections::{HashMap, HashSet};

use crate::{
    ast::{TailCall, collect_free_vars_in_program, find_tail_calls},
    bytecode::{
        bytecode::Bytecode,
        compilation_scope::CompilationScope,
        debug_info::{FunctionDebugInfo, InstructionLocation},
        op_code::{Instructions, OpCode, make},
        symbol_table::SymbolTable,
    },
    diagnostics::{
        CIRCULAR_DEPENDENCY, Diagnostic, ErrorType, lookup_error_code,
        position::{Position, Span},
    },
    runtime::value::Value,
    syntax::{
        interner::Interner, pattern_validate::validate_program_patterns, program::Program,
        statement::Statement, symbol::Symbol,
    },
};

mod builder;
mod errors;
mod expression;
mod statement;
mod suggestions;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

pub struct Compiler {
    constants: Vec<Value>,
    pub symbol_table: SymbolTable,
    pub(super) scopes: Vec<CompilationScope>,
    pub(super) scope_index: usize,
    pub errors: Vec<Diagnostic>,
    pub(super) file_path: String,
    imported_files: HashSet<String>,
    pub(super) file_scope_symbols: HashSet<Symbol>,
    pub(super) imported_modules: HashSet<Symbol>,
    pub(super) import_aliases: HashMap<Symbol, Symbol>,
    pub(super) current_module_prefix: Option<Symbol>,
    pub(super) current_span: Option<Span>,
    // Module Constants - stores compile-time evaluated module constants
    pub(super) module_constants: HashMap<Symbol, Value>,
    pub interner: Interner,
    // Tail call optimization - tracks if we are compiling in tail position.
    pub(super) in_tail_position: bool,
    // Function parameter counts for active function scopes innermost last.
    pub(super) function_param_counts: Vec<usize>,
    // For each active function scope track local indexes captured by nested closures.
    pub(super) captured_local_indices: Vec<HashSet<usize>>,
    // Program-level free-variable analysis result for the latest compile pass.
    pub free_vars: HashSet<Symbol>,
    // Program-level tail-position analysis result for the latest optimized compile pass.
    pub tail_calls: Vec<TailCall>,
}

#[cfg(test)]
mod compiler_test;

impl Compiler {
    pub fn new() -> Self {
        Self::new_with_file_path("<unknown>")
    }

    pub fn new_with_file_path(file_path: impl Into<String>) -> Self {
        Self::new_with_interner(file_path, Interner::new())
    }

    pub fn new_with_interner(file_path: impl Into<String>, interner: Interner) -> Self {
        let mut interner = interner;
        let mut symbol_table = SymbolTable::new();
        symbol_table.define_builtin(0, interner.intern("print"));
        symbol_table.define_builtin(1, interner.intern("len"));
        symbol_table.define_builtin(2, interner.intern("first"));
        symbol_table.define_builtin(3, interner.intern("last"));
        symbol_table.define_builtin(4, interner.intern("rest"));
        symbol_table.define_builtin(5, interner.intern("push"));
        symbol_table.define_builtin(6, interner.intern("to_string"));
        symbol_table.define_builtin(7, interner.intern("concat"));
        symbol_table.define_builtin(8, interner.intern("reverse"));
        symbol_table.define_builtin(9, interner.intern("contains"));
        symbol_table.define_builtin(10, interner.intern("slice"));
        symbol_table.define_builtin(11, interner.intern("sort"));
        symbol_table.define_builtin(12, interner.intern("split"));
        symbol_table.define_builtin(13, interner.intern("join"));
        symbol_table.define_builtin(14, interner.intern("trim"));
        symbol_table.define_builtin(15, interner.intern("upper"));
        symbol_table.define_builtin(16, interner.intern("lower"));
        symbol_table.define_builtin(17, interner.intern("starts_with"));
        symbol_table.define_builtin(18, interner.intern("ends_with"));
        symbol_table.define_builtin(19, interner.intern("replace"));
        symbol_table.define_builtin(20, interner.intern("chars"));
        symbol_table.define_builtin(21, interner.intern("substring"));
        symbol_table.define_builtin(22, interner.intern("keys"));
        symbol_table.define_builtin(23, interner.intern("values"));
        symbol_table.define_builtin(24, interner.intern("has_key"));
        symbol_table.define_builtin(25, interner.intern("merge"));
        symbol_table.define_builtin(26, interner.intern("delete"));
        symbol_table.define_builtin(27, interner.intern("abs"));
        symbol_table.define_builtin(28, interner.intern("min"));
        symbol_table.define_builtin(29, interner.intern("max"));
        // Type Checking Builtins
        symbol_table.define_builtin(30, interner.intern("type_of"));
        symbol_table.define_builtin(31, interner.intern("is_int"));
        symbol_table.define_builtin(32, interner.intern("is_float"));
        symbol_table.define_builtin(33, interner.intern("is_string"));
        symbol_table.define_builtin(34, interner.intern("is_bool"));
        symbol_table.define_builtin(35, interner.intern("is_array"));
        symbol_table.define_builtin(36, interner.intern("is_hash"));
        symbol_table.define_builtin(37, interner.intern("is_none"));
        symbol_table.define_builtin(38, interner.intern("is_some"));
        // Higher-order builtins
        symbol_table.define_builtin(39, interner.intern("map"));
        symbol_table.define_builtin(40, interner.intern("filter"));
        symbol_table.define_builtin(41, interner.intern("fold"));
        // List builtins (persistent cons-cell lists)
        symbol_table.define_builtin(42, interner.intern("hd"));
        symbol_table.define_builtin(43, interner.intern("tl"));
        symbol_table.define_builtin(44, interner.intern("is_list"));
        symbol_table.define_builtin(45, interner.intern("to_list"));
        symbol_table.define_builtin(46, interner.intern("to_array"));
        // Map builtins (persistent HAMT maps)
        symbol_table.define_builtin(47, interner.intern("put"));
        symbol_table.define_builtin(48, interner.intern("get"));
        symbol_table.define_builtin(49, interner.intern("is_map"));
        symbol_table.define_builtin(50, interner.intern("list"));
        // I/O and parsing builtins
        symbol_table.define_builtin(51, interner.intern("read_file"));
        symbol_table.define_builtin(52, interner.intern("read_lines"));
        symbol_table.define_builtin(53, interner.intern("read_stdin"));
        symbol_table.define_builtin(54, interner.intern("parse_int"));
        symbol_table.define_builtin(55, interner.intern("now_ms"));
        symbol_table.define_builtin(56, interner.intern("time"));
        symbol_table.define_builtin(57, interner.intern("range"));
        symbol_table.define_builtin(58, interner.intern("sum"));
        symbol_table.define_builtin(59, interner.intern("product"));
        symbol_table.define_builtin(60, interner.intern("parse_ints"));
        symbol_table.define_builtin(61, interner.intern("split_ints"));
        symbol_table.define_builtin(62, interner.intern("flat_map"));
        // Higher-order search and sort builtins
        symbol_table.define_builtin(63, interner.intern("any"));
        symbol_table.define_builtin(64, interner.intern("all"));
        symbol_table.define_builtin(65, interner.intern("find"));
        symbol_table.define_builtin(66, interner.intern("sort_by"));
        symbol_table.define_builtin(67, interner.intern("zip"));
        symbol_table.define_builtin(68, interner.intern("flatten"));
        symbol_table.define_builtin(69, interner.intern("count"));
        // Assert builtins (test framework)
        symbol_table.define_builtin(70, interner.intern("assert_eq"));
        symbol_table.define_builtin(71, interner.intern("assert_neq"));
        symbol_table.define_builtin(72, interner.intern("assert_true"));
        symbol_table.define_builtin(73, interner.intern("assert_false"));
        symbol_table.define_builtin(74, interner.intern("assert_throws"));

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
            interner,
            in_tail_position: false,
            function_param_counts: Vec::new(),
            captured_local_indices: Vec::new(),
            free_vars: HashSet::new(),
            tail_calls: Vec::new(),
        }
    }

    pub fn new_with_state(
        symbol_table: SymbolTable,
        constants: Vec<Value>,
        interner: Interner,
    ) -> Self {
        let mut compiler = Self::new();
        compiler.symbol_table = symbol_table;
        compiler.constants = constants;
        compiler.interner = interner;
        compiler
    }

    /// Consumes the compiler and returns persistent state for REPL reuse.
    /// Pairs with `new_with_state()` to bootstrap the next REPL iteration.
    pub fn take_state(self) -> (SymbolTable, Vec<Value>, Interner) {
        (self.symbol_table, self.constants, self.interner)
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

    #[inline]
    pub(super) fn sym(&self, s: Symbol) -> &str {
        self.interner.resolve(s)
    }

    /// Compile with optional optimization and analysis passes.
    ///
    /// # Parameters
    /// - `optimize`: If true, applies AST transformations (desugar, constant fold, rename)
    /// - `analyze`: If true, collects analysis data (free vars, tail calls)
    ///
    /// # Transformations (when optimize=true)
    /// 1. Desugaring: Eliminates syntactic sugar (!!x → x, !(a==b) → a!=b)
    /// 2. Constant folding: Evaluates compile-time constants (2+3 → 5)
    /// 3. Rename pass: Applies identifier renaming map (currently identity/no-op)
    ///
    /// # Analysis (when analyze=true)
    /// 4. Free-variable analysis: Collects free symbols in the AST
    /// 5. Tail-position analysis: Collects call expressions in tail position
    ///
    /// This requires cloning the program if optimize or analyze is enabled.
    pub fn compile_with_opts(
        &mut self,
        program: &Program,
        optimize: bool,
        analyze: bool,
    ) -> Result<(), Vec<Diagnostic>> {
        // Apply optimizations if requested
        let program_to_compile = if optimize {
            use crate::ast::{constant_fold, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold(desugared);
            // Rename pass (currently no-op, reserved for future alpha-conversion)
            rename(optimized, HashMap::new())
        } else if analyze {
            // Need to clone for analysis even without optimization
            program.clone()
        } else {
            // Borrow directly if no transformations needed
            program.clone()
        };

        // Collect analysis data if requested
        if analyze {
            self.free_vars = collect_free_vars_in_program(&program_to_compile);
            self.tail_calls = find_tail_calls(&program_to_compile);
        } else {
            self.free_vars.clear();
            self.tail_calls.clear();
        }

        self.compile(&program_to_compile)
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
                let name = *name;
                // Check for duplicate declaration first (takes precedence)
                if let Some(existing) = self.symbol_table.resolve(name)
                    && self.symbol_table.exists_in_current_scope(name)
                {
                    let name_str = self.sym(name);
                    self.errors.push(self.make_redeclaration_error(
                        name_str,
                        *span,
                        Some(existing.span),
                        None,
                    ));
                    continue;
                }
                // Check for import collision
                if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
                    let name_str = self.sym(name);
                    self.errors
                        .push(self.make_import_collision_error(name_str, *span));
                    continue;
                }
                // Predeclare the function name
                self.symbol_table.define(name, *span);
                self.file_scope_symbols.insert(name);
            }
        }

        // PASS 2: Compile all statements
        // Function bodies can now reference any function defined at module level
        self.errors.extend(validate_program_patterns(
            program,
            &self.file_path,
            &self.interner,
        ));
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

    // Module Constants helper to emit any Value as a constant
    pub(super) fn emit_constant_value(&mut self, obj: Value) {
        match obj {
            Value::Boolean(true) => {
                self.emit(OpCode::OpTrue, &[]);
            }
            Value::Boolean(false) => {
                self.emit(OpCode::OpFalse, &[]);
            }
            Value::None => {
                self.emit(OpCode::OpNone, &[]);
            }
            _ => {
                let idx = self.add_constant(obj);
                self.emit_constant_index(idx);
            }
        }
    }

    pub(super) fn emit_constant_index(&mut self, idx: usize) {
        if u16::try_from(idx).is_ok() {
            self.emit(OpCode::OpConstant, &[idx]);
        } else {
            self.emit(OpCode::OpConstantLong, &[idx]);
        }
    }

    pub(super) fn emit_closure_index(&mut self, idx: usize, num_free: usize) {
        if u16::try_from(idx).is_ok() {
            self.emit(OpCode::OpClosure, &[idx, num_free]);
        } else {
            self.emit(OpCode::OpClosureLong, &[idx, num_free]);
        }
    }

    pub(super) fn emit_array_count(&mut self, count: usize) {
        if u16::try_from(count).is_ok() {
            self.emit(OpCode::OpArray, &[count]);
        } else {
            self.emit(OpCode::OpArrayLong, &[count]);
        }
    }

    pub(super) fn emit_tuple_count(&mut self, count: usize) {
        if u16::try_from(count).is_ok() {
            self.emit(OpCode::OpTuple, &[count]);
        } else {
            self.emit(OpCode::OpTupleLong, &[count]);
        }
    }

    pub(super) fn emit_hash_count(&mut self, count: usize) {
        if u16::try_from(count).is_ok() {
            self.emit(OpCode::OpHash, &[count]);
        } else {
            self.emit(OpCode::OpHashLong, &[count]);
        }
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

    pub(super) fn current_instructions(&self) -> &Instructions {
        &self.scopes[self.scope_index].instructions
    }

    pub(super) fn replace_last_pop_with_return(&mut self) {
        let scope = &self.scopes[self.scope_index];
        let pop_pos = scope.last_instruction.position;
        let prev_op = scope.previous_instruction.opcode;
        let prev_pos = scope.previous_instruction.position;

        // Superinstruction: GetLocal(n) + Pop → ReturnLocal(n)
        // Only safe when the previous instruction is adjacent AND no jump targets
        // pop_pos (which would land on the operand byte after fusion).
        let adjacent = match prev_op {
            Some(OpCode::OpGetLocal) => prev_pos + 2 == pop_pos,
            Some(OpCode::OpGetLocal0 | OpCode::OpGetLocal1) => prev_pos + 1 == pop_pos,
            _ => false,
        };

        if adjacent && !self.has_jump_target_at(pop_pos) {
            match prev_op {
                Some(OpCode::OpGetLocal) => {
                    let local_idx =
                        self.scopes[self.scope_index].instructions[prev_pos + 1] as usize;
                    self.replace_instruction(prev_pos, make(OpCode::OpReturnLocal, &[local_idx]));
                    self.scopes[self.scope_index].instructions.truncate(pop_pos);
                    while let Some(last) = self.scopes[self.scope_index].locations.last() {
                        if last.offset >= pop_pos {
                            self.scopes[self.scope_index].locations.pop();
                        } else {
                            break;
                        }
                    }
                    self.scopes[self.scope_index].last_instruction.opcode =
                        Some(OpCode::OpReturnLocal);
                    self.scopes[self.scope_index].last_instruction.position = prev_pos;
                    return;
                }
                Some(OpCode::OpGetLocal0) => {
                    self.scopes[self.scope_index].instructions[prev_pos] =
                        OpCode::OpReturnLocal as u8;
                    self.scopes[self.scope_index].instructions[pop_pos] = 0u8;
                    self.scopes[self.scope_index].last_instruction.opcode =
                        Some(OpCode::OpReturnLocal);
                    self.scopes[self.scope_index].last_instruction.position = prev_pos;
                    return;
                }
                Some(OpCode::OpGetLocal1) => {
                    self.scopes[self.scope_index].instructions[prev_pos] =
                        OpCode::OpReturnLocal as u8;
                    self.scopes[self.scope_index].instructions[pop_pos] = 1u8;
                    self.scopes[self.scope_index].last_instruction.opcode =
                        Some(OpCode::OpReturnLocal);
                    self.scopes[self.scope_index].last_instruction.position = prev_pos;
                    return;
                }
                _ => {}
            }
        }

        // Default: just replace Pop with ReturnValue
        self.replace_instruction(pop_pos, make(OpCode::OpReturnValue, &[]));
        self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnValue);
    }

    /// Scans the current scope's instruction stream for jump instructions
    /// targeting `target_pos`. Used by the superinstruction peephole to verify
    /// that fusing instructions at a position won't break jump targets.
    fn has_jump_target_at(&self, target_pos: usize) -> bool {
        use crate::bytecode::op_code::{operand_widths, read_u16};
        let instructions = &self.scopes[self.scope_index].instructions;
        let mut ip = 0;
        while ip < instructions.len() {
            let op = OpCode::from(instructions[ip]);
            match op {
                OpCode::OpJump | OpCode::OpJumpNotTruthy | OpCode::OpJumpTruthy => {
                    let target = read_u16(instructions, ip + 1) as usize;
                    if target == target_pos {
                        return true;
                    }
                    ip += 3;
                }
                _ => {
                    let widths = operand_widths(op);
                    ip += 1 + widths.iter().sum::<usize>();
                }
            }
        }
        false
    }

    pub(super) fn find_duplicate_name(names: &[Symbol]) -> Option<Symbol> {
        let mut seen = HashSet::new();
        for name in names {
            if !seen.insert(*name) {
                return Some(*name);
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

    pub(super) fn with_tail_position<F, R>(&mut self, in_tail: bool, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let saved = self.in_tail_position;
        self.in_tail_position = in_tail;
        let result = f(self);
        self.in_tail_position = saved;
        result
    }

    pub(super) fn with_function_context<F, R>(&mut self, num_params: usize, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.function_param_counts.push(num_params);
        self.captured_local_indices.push(HashSet::new());
        let result = f(self);
        self.captured_local_indices.pop();
        self.function_param_counts.pop();
        result
    }

    pub(super) fn current_function_param_count(&self) -> Option<usize> {
        self.function_param_counts.last().copied()
    }

    pub(super) fn current_function_captured_locals(&self) -> Option<&HashSet<usize>> {
        self.captured_local_indices.last()
    }

    pub(super) fn mark_captured_in_current_function(&mut self, local_index: usize) {
        if self.captured_local_indices.is_empty() {
            return;
        }

        let current_idx = self.captured_local_indices.len() - 1;
        self.captured_local_indices[current_idx].insert(local_index);
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
