use std::collections::{HashMap, HashSet};

use crate::{
    ast::{TailCall, collect_free_vars_in_program, find_tail_calls, type_infer::infer_program},
    bytecode::{
        binding::Binding,
        bytecode::Bytecode,
        compilation_scope::CompilationScope,
        compiler::{
            adt_registry::AdtRegistry,
            contracts::{ContractKey, FnContract, ModuleContractTable, to_runtime_contract},
        },
        debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation},
        op_code::{Instructions, OpCode, make},
        symbol_table::SymbolTable,
    },
    diagnostics::{
        CIRCULAR_DEPENDENCY, Diagnostic, ErrorType, UNKNOWN_BASE_MEMBER, lookup_error_code,
        position::{Position, Span},
    },
    runtime::{
        base::BaseModule, function_contract::FunctionContract, runtime_type::RuntimeType,
        value::Value,
    },
    syntax::{
        interner::Interner, pattern_validate::validate_program_patterns, program::Program,
        statement::Statement, symbol::Symbol,
    },
    types::type_env::TypeEnv,
};

mod adt_definition;
mod adt_registry;
mod builder;
mod constructor_info;
mod contracts;
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
    pub(super) imported_module_exclusions: HashMap<Symbol, HashSet<Symbol>>,
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
    pub(super) excluded_base_symbols: HashSet<Symbol>,
    pub module_contracts: ModuleContractTable,
    pub(super) static_type_scopes: Vec<HashMap<Symbol, RuntimeType>>,
    pub(super) adt_registry: AdtRegistry,
    /// HM-inferred type environment, populated before PASS 2 by `infer_program`.
    pub(super) type_env: TypeEnv,
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
        for (index, name) in BaseModule::new().names().enumerate() {
            symbol_table.define_base_function(index, interner.intern(name));
        }

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
            imported_module_exclusions: HashMap::new(),
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
            excluded_base_symbols: HashSet::new(),
            module_contracts: HashMap::new(),
            static_type_scopes: vec![HashMap::new()],
            adt_registry: AdtRegistry::new(),
            type_env: TypeEnv::new(),
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
        self.imported_module_exclusions.clear();
        self.current_module_prefix = None;
        self.current_span = None;
        self.excluded_base_symbols.clear();
        self.module_contracts.clear();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.type_env = TypeEnv::new();
    }

    pub(super) fn boxed(diag: Diagnostic) -> Box<Diagnostic> {
        Box::new(diag)
    }

    fn collect_adt_definitions(&mut self, program: &Program) {
        self.adt_registry = AdtRegistry::new();
        for statement in &program.statements {
            self.collect_adt_definitions_from_stmt(statement);
        }
    }

    fn collect_adt_definitions_from_stmt(&mut self, statement: &Statement) {
        match statement {
            Statement::Data { name, variants, .. } => {
                self.adt_registry.register_adt(*name, variants);
            }
            Statement::Module { body, .. } => {
                for statement in &body.statements {
                    self.collect_adt_definitions_from_stmt(statement);
                }
            }
            _ => {}
        }
    }

    fn collect_module_contracts(&mut self, program: &Program) {
        self.module_contracts.clear();

        for statement in &program.statements {
            self.collect_contracts_from_statement(statement, None);
        }
    }

    fn collect_contracts_from_statement(
        &mut self,
        statement: &Statement,
        module_name: Option<Symbol>,
    ) {
        match statement {
            Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                effects,
                ..
            } => {
                let has_annotations = parameter_types.iter().any(Option::is_some)
                    || return_type.is_some()
                    || !effects.is_empty();

                if has_annotations {
                    self.module_contracts.insert(
                        ContractKey {
                            module_name,
                            function_name: *name,
                            arity: parameters.len(),
                        },
                        FnContract {
                            params: parameter_types.clone(),
                            ret: return_type.clone(),
                            effects: effects.clone(),
                        },
                    );
                }
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.collect_contracts_from_statement(nested, Some(*name));
                }
            }
            _ => {}
        }
    }

    #[inline]
    pub(super) fn sym(&self, s: Symbol) -> &str {
        self.interner.resolve(s)
    }

    pub(super) fn lookup_contract(
        &self,
        module_name: Option<Symbol>,
        function_name: Symbol,
        arity: usize,
    ) -> Option<&FnContract> {
        self.module_contracts.get(&ContractKey {
            module_name,
            function_name,
            arity,
        })
    }

    pub(super) fn lookup_unqualified_contract(
        &self,
        function_name: Symbol,
        arity: usize,
    ) -> Option<&FnContract> {
        if let Some(module_name) = self.current_module_prefix
            && let Some(contract) = self.lookup_contract(Some(module_name), function_name, arity)
        {
            return Some(contract);
        }

        self.lookup_contract(None, function_name, arity)
    }

    pub(super) fn to_runtime_contract(&self, contract: &FnContract) -> Option<FunctionContract> {
        to_runtime_contract(contract, &self.interner)
    }

    #[inline]
    pub(super) fn bind_static_type(&mut self, name: Symbol, ty: RuntimeType) {
        if let Some(scope) = self.static_type_scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    #[inline]
    pub(super) fn lookup_static_type(&self, name: Symbol) -> Option<RuntimeType> {
        self.static_type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name))
            .cloned()
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
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
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
        self.imported_module_exclusions.clear();
        self.current_module_prefix = None;
        self.excluded_base_symbols.clear();
        self.module_contracts.clear();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.process_base_directives(program);
        self.collect_module_contracts(program);
        self.collect_adt_definitions(program);

        // PASS 1: Predeclare all module-level function names
        // This enables forward references and mutual recursion
        for statement in &program.statements {
            if let Statement::Function { name, span, .. } = statement {
                let name = *name;
                // Check for duplicate declaration first (takes precedence)
                if let Some(existing) = self.symbol_table.resolve(name)
                    && self.symbol_table.exists_in_current_scope(name)
                    && existing.symbol_scope != crate::bytecode::symbol_scope::SymbolScope::Base
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

        // HM type inference pass — runs after predeclaration, before code generation.
        // The resulting TypeEnv is used by `static_expr_type` to enrich identifier
        // type lookup for unannotated bindings.
        //
        // Diagnostics from this pass are guarded by a concrete-types-only filter
        // inside `unify_reporting`: only errors where *both* conflicting types are
        // fully resolved (no free type variables) are emitted. This prevents
        // spurious failures in partially-typed programs where base-function return
        // types are not yet registered in the inference environment.
        let hm_diagnostics = {
            let (type_env, diags) =
                infer_program(program, &self.interner, Some(self.file_path.clone()));
            self.type_env = type_env;
            diags
        };

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

        // HM diagnostics appended after bytecode errors so that specific,
        // actionable errors (e.g. E077 legacy list tail, E055 contract mismatch)
        // surface first in the error list.
        self.errors.extend(hm_diagnostics);

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
        self.static_type_scopes.push(HashMap::new());
    }

    pub(super) fn leave_scope(
        &mut self,
    ) -> (
        Instructions,
        Vec<InstructionLocation>,
        Vec<String>,
        EffectSummary,
    ) {
        let scope = self.scopes.pop().unwrap();
        self.scope_index -= 1;
        if let Some(outer) = self.symbol_table.outer.take() {
            self.symbol_table = *outer;
        }
        let _ = self.static_type_scopes.pop();

        (
            scope.instructions,
            scope.locations,
            scope.files,
            scope.effect_summary,
        )
    }

    pub(super) fn enter_block_scope(&mut self) {
        let mut block_table = SymbolTable::new_block(self.symbol_table.clone());
        block_table.num_definitions = self.symbol_table.num_definitions;
        self.symbol_table = block_table;
        self.static_type_scopes.push(HashMap::new());
    }

    pub(super) fn leave_block_scope(&mut self) {
        let num_definitions = self.symbol_table.num_definitions;
        if let Some(outer) = self.symbol_table.outer.take() {
            let mut outer = *outer;
            outer.num_definitions = num_definitions;
            self.symbol_table = outer;
        }
        let _ = self.static_type_scopes.pop();
    }

    pub fn bytecode(&self) -> Bytecode {
        Bytecode {
            instructions: self.scopes[self.scope_index].instructions.clone(),
            constants: self.constants.clone(),
            debug_info: Some(
                FunctionDebugInfo::new(
                    Some("<main>".to_string()),
                    self.scopes[self.scope_index].files.clone(),
                    self.scopes[self.scope_index].locations.clone(),
                )
                .with_effect_summary(self.scopes[self.scope_index].effect_summary),
            ),
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

    pub(super) fn is_base_module_symbol(&self, name: Symbol) -> bool {
        self.sym(name) == "Base"
    }

    pub(super) fn resolve_visible_symbol(&mut self, name: Symbol) -> Option<Binding> {
        let binding = self.symbol_table.resolve(name)?;
        if binding.symbol_scope == crate::bytecode::symbol_scope::SymbolScope::Base
            && self.excluded_base_symbols.contains(&name)
        {
            return None;
        }
        Some(binding)
    }

    fn process_base_directives(&mut self, program: &Program) {
        let mut seen = HashSet::new();

        for statement in &program.statements {
            let Statement::Import {
                name,
                alias,
                except,
                span,
            } = statement
            else {
                continue;
            };

            if !self.is_base_module_symbol(*name) {
                continue;
            }

            if let Some(alias) = alias {
                let alias_name = self.sym(*alias);
                self.errors
                    .push(self.make_base_alias_error(alias_name, *span));
            }

            for excluded in except {
                let excluded_name = self.sym(*excluded);
                if !seen.insert(*excluded) {
                    self.errors
                        .push(self.make_duplicate_base_exclusion_error(excluded_name, *span));
                    continue;
                }

                if BaseModule::new().index_of(excluded_name).is_none() {
                    self.errors.push(Diagnostic::make_error(
                        &UNKNOWN_BASE_MEMBER,
                        &[excluded_name],
                        self.file_path.clone(),
                        *span,
                    ));
                    continue;
                }

                self.excluded_base_symbols.insert(*excluded);
            }
        }
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
