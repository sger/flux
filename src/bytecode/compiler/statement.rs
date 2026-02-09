use std::rc::Rc;

use crate::{
    bytecode::{
        compiler::Compiler, debug_info::FunctionDebugInfo,
        module_constants::compile_module_constants, op_code::OpCode, symbol_scope::SymbolScope,
    },
    syntax::{
        block::Block,
        diagnostics::{
            DUPLICATE_PARAMETER, Diagnostic, ICE_SYMBOL_SCOPE_LET, IMPORT_SCOPE,
            INVALID_MODULE_CONTENT, INVALID_MODULE_NAME, MODULE_NAME_CLASH, MODULE_SCOPE,
        },
        module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
        position::{Position, Span},
        statement::Statement,
        symbol::Symbol,
    },
    runtime::{compiled_function::CompiledFunction, object::Object},
};

type CompileResult<T> = Result<T, Box<Diagnostic>>;

impl Compiler {
    pub(super) fn compile_statement(&mut self, statement: &Statement) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(statement.span());
        let result = (|| {
            match statement {
                Statement::Expression { expression, .. } => {
                    self.compile_expression(expression)?;
                    self.emit(OpCode::OpPop, &[]);
                }
                Statement::Let { name, value, span } => {
                    let name = *name;
                    // Check for duplicate in current scope FIRST (takes precedence)
                    if let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            None,
                        )));
                    }
                    // Then check for import collision (only if not a duplicate in same scope)
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(name_str, *span),
                        ));
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
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::Assign { name, span, .. } => {
                    let name = *name;
                    // Check if variable exists
                    let name_str = self.sym(name).to_string();
                    let symbol = self.symbol_table.resolve(name).ok_or_else(|| {
                        Self::boxed(self.make_undefined_variable_error(&name_str, *span))
                    })?;

                    if symbol.symbol_scope == SymbolScope::Free {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_outer_assignment_error(name_str, *span),
                        ));
                    }

                    // Flux bindings are immutable today: assignment syntax is parsed so we can
                    // emit a targeted diagnostic, but reassignment is not allowed.
                    let name_str = self.sym(name);
                    return Err(Self::boxed(self.make_immutability_error(name_str, *span)));
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
                    let name = *name;
                    // For top-level functions, checks were already done in pass 1
                    // Only check for nested functions (scope_index > 0)
                    if self.scope_index > 0
                        && let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            Some("Use a different name or remove the previous definition"),
                        )));
                    }
                    self.compile_function_statement(name, parameters, body, span.start)?;
                    // For nested functions, add to file_scope_symbols
                    if self.scope_index == 0 {
                        // Already added in pass 1 for top-level functions
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::Module { name, body, span } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_SCOPE,
                            &[],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let name_str = self.sym(name).to_string();
                    let binding_name_str = module_binding_name(&name_str).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    let name_str = self.sym(name);
                    if !is_valid_module_name(name_str) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &INVALID_MODULE_NAME,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    self.compile_module_statement(name, body, span.start)?;
                    if self.scope_index == 0 {
                        let name_str = self.sym(name).to_string();
                        let binding_name_str = module_binding_name(&name_str).to_string();
                        let binding_name = self.interner.intern(&binding_name_str);
                        self.file_scope_symbols.insert(binding_name);
                    }
                }
                Statement::Import { name, alias, span } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_SCOPE,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let name_str = self.sym(name).to_string();
                    let alias_str = alias.map(|a| self.sym(a).to_string());
                    let binding_name_str =
                        import_binding_name(&name_str, alias_str.as_deref()).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);

                    if self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    // Reserve the name for this file so later declarations can't collide.
                    self.file_scope_symbols.insert(binding_name);
                    self.compile_import_statement(name, *alias)?;
                }
            }
            Ok(())
        })();
        self.current_span = previous_span;
        result
    }

    pub(super) fn compile_function_statement(
        &mut self,
        name: Symbol,
        parameters: &[Symbol],
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        if let Some(param) = Self::find_duplicate_name(parameters) {
            let param_str = self.sym(param);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param_str],
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
            self.symbol_table.define(*param, Span::default());
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

    pub(super) fn compile_module_statement(
        &mut self,
        name: Symbol,
        body: &Block,
        position: Position,
    ) -> CompileResult<()> {
        // Check if module is already defined
        let name_str = self.sym(name);
        let binding_name_str = module_binding_name(name_str).to_string();
        let binding_name = self.interner.intern(&binding_name_str);
        if self.symbol_table.exists_in_current_scope(binding_name) {
            let binding_name_str = self.sym(binding_name);
            return Err(Self::boxed(self.make_redeclaration_error(
                binding_name_str,
                Span::new(position, position),
                None,
                None,
            )));
        }

        // Collect all functions from the module body and validate contents
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if *fn_name == binding_name {
                        let pos = statement.position();
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NAME_CLASH,
                            &[binding_name_str],
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

        self.imported_modules.insert(binding_name);
        let previous_module = self.current_module_prefix;
        self.current_module_prefix = Some(binding_name);

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
        let constants = match compile_module_constants(body, binding_name, &mut self.interner) {
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
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                // Check for duplicate declaration
                if let Some(existing) = self.symbol_table.resolve(qualified_name)
                    && self.symbol_table.exists_in_current_scope(qualified_name)
                {
                    self.current_module_prefix = previous_module;
                    let qualified_name_str = self.sym(qualified_name);
                    return Err(Self::boxed(self.make_redeclaration_error(
                        qualified_name_str,
                        *span,
                        Some(existing.span),
                        Some("Use a different name or remove the previous definition"),
                    )));
                }
                // Predeclare the function
                self.symbol_table.define(qualified_name, *span);
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
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                if let Err(err) =
                    self.compile_function_statement(qualified_name, parameters, fn_body, position)
                {
                    self.current_module_prefix = previous_module;
                    return Err(err);
                }
            }
        }

        self.current_module_prefix = previous_module;

        Ok(())
    }

    pub(super) fn compile_import_statement(
        &mut self,
        name: Symbol,
        alias: Option<Symbol>,
    ) -> CompileResult<()> {
        if let Some(alias) = alias {
            self.import_aliases.insert(alias, name);
        } else {
            self.imported_modules.insert(name);
        }
        Ok(())
    }

    pub(super) fn compile_block(&mut self, block: &Block) -> CompileResult<()> {
        for statement in &block.statements {
            self.compile_statement(statement)?;
        }

        Ok(())
    }
}
