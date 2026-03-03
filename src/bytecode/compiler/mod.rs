use std::collections::{HashMap, HashSet};

use crate::syntax::entry;
use crate::types::infer_effect_row::InferEffectRow;
use crate::types::{TypeVarId, infer_type::InferType, scheme::Scheme};
use crate::{
    ast::{
        TailCall, collect_free_vars_in_program, find_tail_calls,
        type_infer::{ExprNodeId, infer_program},
    },
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
        CIRCULAR_DEPENDENCY, Diagnostic, DiagnosticBuilder, DiagnosticPhase, ErrorType,
        UNKNOWN_BASE_MEMBER, lookup_error_code,
        position::{Position, Span},
    },
    runtime::{
        base::BaseModule, function_contract::FunctionContract, runtime_type::RuntimeType,
        value::Value,
    },
    syntax::{
        block::Block,
        effect_expr::EffectExpr,
        expression::{Expression, StringPart},
        interner::Interner,
        pattern_validate::validate_program_patterns,
        program::Program,
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
    },
    types::type_env::TypeEnv,
};

mod adt_definition;
mod adt_registry;
mod builder;
mod constructor_info;
mod contracts;
mod effect_rows;
mod errors;
mod expression;
mod hm_expr_typer;
mod statement;
mod suggestions;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for diag in diags {
        if diag.phase().is_none() {
            diag.phase = Some(phase);
        }
    }
}

#[derive(Debug, Clone)]
struct FunctionEffectSeed {
    key: ContractKey,
    module_name: Option<Symbol>,
    type_params: Vec<Symbol>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type: Option<TypeExpr>,
    declared_effects: HashSet<Symbol>,
    body: Block,
    span: Span,
}

#[derive(Debug, Clone, Copy, Default)]
struct MainValidationState {
    has_main: bool,
    is_unique_main: bool,
    is_valid_signature: bool,
}

pub struct Compiler {
    constants: Vec<Value>,
    pub symbol_table: SymbolTable,
    pub(super) scopes: Vec<CompilationScope>,
    pub(super) scope_index: usize,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
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
    // Declared ambient effects for active function scopes innermost last.
    pub(super) function_effects: Vec<Vec<Symbol>>,
    // Effects currently handled by enclosing `handle ...` scopes.
    pub(super) handled_effects: Vec<Symbol>,
    // For each active function scope track local indexes captured by nested closures.
    pub(super) captured_local_indices: Vec<HashSet<usize>>,
    // Program-level free-variable analysis result for the latest compile pass.
    pub free_vars: HashSet<Symbol>,
    // Program-level tail-position analysis result for the latest optimized compile pass.
    pub tail_calls: Vec<TailCall>,
    pub(super) excluded_base_symbols: HashSet<Symbol>,
    pub module_contracts: ModuleContractTable,
    pub(super) module_function_visibility: HashMap<(Symbol, Symbol), bool>,
    pub(super) module_adt_constructors: HashMap<(Symbol, Symbol), Symbol>,
    pub(super) static_type_scopes: Vec<HashMap<Symbol, RuntimeType>>,
    pub(super) effect_alias_scopes: Vec<HashMap<Symbol, Symbol>>,
    pub(super) adt_registry: AdtRegistry,
    pub(super) effect_ops_registry: HashMap<Symbol, HashSet<Symbol>>,
    pub(super) effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>,
    /// HM-inferred type environment, populated before PASS 2 by `infer_program`.
    pub(super) type_env: TypeEnv,
    pub(super) hm_expr_types: HashMap<ExprNodeId, crate::types::infer_type::InferType>,
    pub(super) expr_ptr_to_id: HashMap<usize, ExprNodeId>,
    strict_mode: bool,
    strict_require_main: bool,
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
            warnings: Vec::new(),
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
            function_effects: Vec::new(),
            handled_effects: Vec::new(),
            captured_local_indices: Vec::new(),
            free_vars: HashSet::new(),
            tail_calls: Vec::new(),
            excluded_base_symbols: HashSet::new(),
            module_contracts: HashMap::new(),
            module_function_visibility: HashMap::new(),
            module_adt_constructors: HashMap::new(),
            static_type_scopes: vec![HashMap::new()],
            effect_alias_scopes: vec![HashMap::new()],
            adt_registry: AdtRegistry::new(),
            effect_ops_registry: HashMap::new(),
            effect_op_signatures: HashMap::new(),
            type_env: TypeEnv::new(),
            hm_expr_types: HashMap::new(),
            expr_ptr_to_id: HashMap::new(),
            strict_mode: false,
            strict_require_main: true,
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
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.type_env = TypeEnv::new();
        self.hm_expr_types.clear();
        self.expr_ptr_to_id.clear();
        self.function_effects.clear();
        self.handled_effects.clear();
        self.effect_ops_registry.clear();
        self.effect_op_signatures.clear();
    }

    pub fn set_strict_mode(&mut self, strict_mode: bool) {
        self.strict_mode = strict_mode;
    }

    pub fn set_strict_require_main(&mut self, strict_require_main: bool) {
        self.strict_require_main = strict_require_main;
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings)
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

    fn collect_effect_declarations(&mut self, program: &Program) {
        self.effect_ops_registry.clear();
        self.effect_op_signatures.clear();
        for statement in &program.statements {
            self.collect_effect_declarations_from_stmt(statement);
        }
    }

    fn collect_effect_declarations_from_stmt(&mut self, statement: &Statement) {
        match statement {
            Statement::EffectDecl { name, ops, .. } => {
                let entry = self.effect_ops_registry.entry(*name).or_default();
                for op in ops {
                    entry.insert(op.name);
                    self.effect_op_signatures
                        .insert((*name, op.name), op.type_expr.clone());
                }
            }
            Statement::Module { body, .. } => {
                for nested in &body.statements {
                    self.collect_effect_declarations_from_stmt(nested);
                }
            }
            _ => {}
        }
    }

    fn collect_module_contracts(&mut self, program: &Program) {
        for statement in &program.statements {
            self.collect_contracts_from_statement(statement, None);
        }
    }

    fn collect_module_function_visibility(&mut self, program: &Program) {
        for statement in &program.statements {
            self.collect_module_function_visibility_from_statement(statement, None);
        }
    }

    fn collect_module_adt_constructors(&mut self, program: &Program) {
        for statement in &program.statements {
            self.collect_module_adt_constructors_from_statement(statement, None);
        }
    }

    fn collect_module_adt_constructors_from_statement(
        &mut self,
        statement: &Statement,
        module_name: Option<Symbol>,
    ) {
        match statement {
            Statement::Data { name, variants, .. } => {
                if let Some(module_name) = module_name {
                    for variant in variants {
                        self.module_adt_constructors
                            .insert((module_name, variant.name), *name);
                        if let Some(short) = self
                            .sym(variant.name)
                            .rsplit('.')
                            .next()
                            .map(ToOwned::to_owned)
                        {
                            let short_sym = self.interner.intern(short.as_str());
                            self.module_adt_constructors
                                .insert((module_name, short_sym), *name);
                        }
                    }
                }
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.collect_module_adt_constructors_from_statement(nested, Some(*name));
                }
            }
            _ => {}
        }
    }

    fn collect_module_function_visibility_from_statement(
        &mut self,
        statement: &Statement,
        module_name: Option<Symbol>,
    ) {
        match statement {
            Statement::Function {
                is_public, name, ..
            } => {
                if let Some(module_name) = module_name {
                    self.module_function_visibility
                        .insert((module_name, *name), *is_public);
                }
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.collect_module_function_visibility_from_statement(nested, Some(*name));
                }
            }
            _ => {}
        }
    }

    fn collect_contracts_from_statement(
        &mut self,
        statement: &Statement,
        module_name: Option<Symbol>,
    ) {
        match statement {
            Statement::Function {
                is_public: _,
                name,
                type_params,
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
                            type_params: type_params.clone(),
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

    fn collect_import_module_bindings(&self, program: &Program) -> HashMap<Symbol, Symbol> {
        let mut bindings = HashMap::new();
        for statement in &program.statements {
            let Statement::Import { name, alias, .. } = statement else {
                continue;
            };
            if self.is_base_module_symbol(*name) {
                continue;
            }
            let binding = alias.unwrap_or(*name);
            bindings.insert(binding, *name);
        }
        bindings
    }

    fn scheme_from_contract(contract: &FnContract, interner: &Interner) -> Option<Scheme> {
        // For HM member lookup we require a complete typed signature.
        if contract.params.iter().any(|p| p.is_none()) || contract.ret.is_none() {
            return None;
        }

        let mut next_var: TypeVarId = 0;
        let mut tp_map = HashMap::new();
        let mut row_var_env: HashMap<Symbol, TypeVarId> = HashMap::new();
        for type_param in &contract.type_params {
            tp_map.insert(*type_param, next_var);
            next_var = next_var.saturating_add(1);
        }

        let mut param_tys = Vec::with_capacity(contract.params.len());
        for param in &contract.params {
            let ty_expr = param.as_ref()?;
            let inferred = TypeEnv::infer_type_from_type_expr_with_row_vars(
                ty_expr,
                &tp_map,
                interner,
                &mut row_var_env,
                &mut next_var,
            )?;
            param_tys.push(inferred);
        }

        let ret_expr = contract.ret.as_ref()?;
        let ret_ty = TypeEnv::infer_type_from_type_expr_with_row_vars(
            ret_expr,
            &tp_map,
            interner,
            &mut row_var_env,
            &mut next_var,
        )?;
        let effects =
            InferEffectRow::from_effect_exprs(&contract.effects, &mut row_var_env, &mut next_var);

        let infer_type = InferType::Fun(param_tys, Box::new(ret_ty), effects);
        let mut forall = infer_type.free_vars().into_iter().collect::<Vec<_>>();
        forall.sort_unstable();
        forall.dedup();
        Some(Scheme { forall, infer_type })
    }

    fn build_preloaded_hm_member_schemes(
        &self,
        program: &Program,
    ) -> HashMap<(Symbol, Symbol), Scheme> {
        let import_bindings = self.collect_import_module_bindings(program);
        if import_bindings.is_empty() {
            return HashMap::new();
        }

        let mut preloaded = HashMap::new();
        for (binding, target_module) in import_bindings {
            for (key, contract) in &self.module_contracts {
                if key.module_name != Some(target_module) {
                    continue;
                }
                if self
                    .module_function_visibility
                    .get(&(target_module, key.function_name))
                    != Some(&true)
                {
                    continue;
                }
                if let Some(scheme) = Self::scheme_from_contract(contract, &self.interner) {
                    preloaded.insert((binding, key.function_name), scheme);
                }
            }
        }

        preloaded
    }

    fn build_preloaded_base_schemes(&mut self) -> HashMap<Symbol, Scheme> {
        let mut preloaded = HashMap::new();
        for base_fn in BaseModule::new().names()  {
            let base_name = self.interner.intern(base_fn);
            let Some(entry) = BaseModule::new().index_of(base_fn).and_then(|i| BaseModule::new().by_index(i))
        else {
            continue;
        };
        

        preloaded
    }

    fn collect_function_effect_seeds(&self, program: &Program) -> Vec<FunctionEffectSeed> {
        let mut out = Vec::new();
        for statement in &program.statements {
            self.collect_function_effect_seeds_from_stmt(statement, None, &mut out);
        }
        out
    }

    fn collect_function_effect_seeds_from_stmt(
        &self,
        statement: &Statement,
        module_name: Option<Symbol>,
        out: &mut Vec<FunctionEffectSeed>,
    ) {
        match statement {
            Statement::Function {
                is_public: _,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } => {
                let declared_effects = effects
                    .iter()
                    .flat_map(EffectExpr::normalized_names)
                    .collect();
                out.push(FunctionEffectSeed {
                    key: ContractKey {
                        module_name,
                        function_name: *name,
                        arity: parameters.len(),
                    },
                    module_name,
                    type_params: type_params.clone(),
                    parameter_types: parameter_types.clone(),
                    return_type: return_type.clone(),
                    declared_effects,
                    body: body.clone(),
                    span: *span,
                });
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.collect_function_effect_seeds_from_stmt(nested, Some(*name), out);
                }
            }
            _ => {}
        }
    }

    fn infer_unannotated_function_effects(&mut self, program: &Program) {
        let io_effect = self.interner.intern("IO");
        let time_effect = self.interner.intern("Time");

        let seeds = self.collect_function_effect_seeds(program);
        if seeds.is_empty() {
            return;
        }

        let mut inferred: HashMap<ContractKey, HashSet<Symbol>> = seeds
            .iter()
            .map(|seed| (seed.key.clone(), seed.declared_effects.clone()))
            .collect();

        let mut changed = true;
        while changed {
            changed = false;
            for seed in &seeds {
                if !seed.declared_effects.is_empty() {
                    continue;
                }
                let is_fully_unannotated =
                    !seed.parameter_types.iter().any(Option::is_some) && seed.return_type.is_none();
                if !is_fully_unannotated {
                    continue;
                }
                let effects = self.infer_effects_from_block(
                    &seed.body,
                    seed.module_name,
                    &inferred,
                    io_effect,
                    time_effect,
                );
                let entry = inferred.entry(seed.key.clone()).or_default();
                if *entry != effects {
                    *entry = effects;
                    changed = true;
                }
            }
        }

        for seed in &seeds {
            let is_fully_unannotated =
                !seed.parameter_types.iter().any(Option::is_some) && seed.return_type.is_none();
            if !is_fully_unannotated {
                continue;
            }
            let Some(effects) = inferred.get(&seed.key) else {
                continue;
            };
            if effects.is_empty() {
                continue;
            }

            let mut sorted_effects: Vec<Symbol> = effects.iter().copied().collect();
            sorted_effects.sort_by_key(|sym| self.sym(*sym).to_string());
            let effect_exprs: Vec<EffectExpr> = sorted_effects
                .into_iter()
                .map(|name| EffectExpr::Named {
                    name,
                    span: seed.span,
                })
                .collect();

            if let Some(contract) = self.module_contracts.get_mut(&seed.key) {
                if contract.effects.is_empty() {
                    contract.effects = effect_exprs;
                }
            } else {
                self.module_contracts.insert(
                    seed.key.clone(),
                    FnContract {
                        type_params: seed.type_params.clone(),
                        params: seed.parameter_types.clone(),
                        ret: seed.return_type.clone(),
                        effects: effect_exprs,
                    },
                );
            }
        }
    }

    fn infer_effects_from_block(
        &self,
        block: &Block,
        current_module: Option<Symbol>,
        inferred: &HashMap<ContractKey, HashSet<Symbol>>,
        io_effect: Symbol,
        time_effect: Symbol,
    ) -> HashSet<Symbol> {
        let mut effects = HashSet::new();
        for statement in &block.statements {
            effects.extend(self.infer_effects_from_statement(
                statement,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ));
        }
        effects
    }

    fn infer_effects_from_statement(
        &self,
        statement: &Statement,
        current_module: Option<Symbol>,
        inferred: &HashMap<ContractKey, HashSet<Symbol>>,
        io_effect: Symbol,
        time_effect: Symbol,
    ) -> HashSet<Symbol> {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => self.infer_effects_from_expr(
                value,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Statement::Return {
                value: Some(value), ..
            } => self.infer_effects_from_expr(
                value,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Statement::Expression { expression, .. } => self.infer_effects_from_expr(
                expression,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            _ => HashSet::new(),
        }
    }

    fn infer_effects_from_expr(
        &self,
        expr: &Expression,
        current_module: Option<Symbol>,
        inferred: &HashMap<ContractKey, HashSet<Symbol>>,
        io_effect: Symbol,
        time_effect: Symbol,
    ) -> HashSet<Symbol> {
        match expr {
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => HashSet::new(),

            Expression::InterpolatedString { parts, .. } => {
                let mut effects = HashSet::new();
                for part in parts {
                    if let StringPart::Interpolation(inner) = part {
                        effects.extend(self.infer_effects_from_expr(
                            inner,
                            current_module,
                            inferred,
                            io_effect,
                            time_effect,
                        ));
                    }
                }
                effects
            }

            Expression::Prefix { right, .. } => self.infer_effects_from_expr(
                right,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Expression::Infix { left, right, .. } => {
                let mut effects = self.infer_effects_from_expr(
                    left,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                effects.extend(self.infer_effects_from_expr(
                    right,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                ));
                effects
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                let mut effects = self.infer_effects_from_expr(
                    condition,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                effects.extend(self.infer_effects_from_block(
                    consequence,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                ));
                if let Some(alt) = alternative {
                    effects.extend(self.infer_effects_from_block(
                        alt,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects
            }
            Expression::DoBlock { block, .. } => self.infer_effects_from_block(
                block,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Expression::Function { .. } => HashSet::new(),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let mut effects = self.infer_effects_from_expr(
                    function,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                for arg in arguments {
                    effects.extend(self.infer_effects_from_expr(
                        arg,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects.extend(self.infer_call_effects(
                    function,
                    arguments,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                ));
                effects
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                let mut effects = HashSet::new();
                for element in elements {
                    effects.extend(self.infer_effects_from_expr(
                        element,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects
            }
            Expression::Index { left, index, .. } => {
                let mut effects = self.infer_effects_from_expr(
                    left,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                effects.extend(self.infer_effects_from_expr(
                    index,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                ));
                effects
            }
            Expression::Hash { pairs, .. } => {
                let mut effects = HashSet::new();
                for (k, v) in pairs {
                    effects.extend(self.infer_effects_from_expr(
                        k,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                    effects.extend(self.infer_effects_from_expr(
                        v,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects
            }
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => self.infer_effects_from_expr(
                object,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                let mut effects = self.infer_effects_from_expr(
                    scrutinee,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        effects.extend(self.infer_effects_from_expr(
                            guard,
                            current_module,
                            inferred,
                            io_effect,
                            time_effect,
                        ));
                    }
                    effects.extend(self.infer_effects_from_expr(
                        &arm.body,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.infer_effects_from_expr(
                value,
                current_module,
                inferred,
                io_effect,
                time_effect,
            ),
            Expression::Cons { head, tail, .. } => {
                let mut effects = self.infer_effects_from_expr(
                    head,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                effects.extend(self.infer_effects_from_expr(
                    tail,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                ));
                effects
            }
            Expression::Perform { effect, args, .. } => {
                let mut effects = HashSet::new();
                for arg in args {
                    effects.extend(self.infer_effects_from_expr(
                        arg,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects.insert(*effect);
                effects
            }
            Expression::Handle {
                expr, effect, arms, ..
            } => {
                let mut effects = self.infer_effects_from_expr(
                    expr,
                    current_module,
                    inferred,
                    io_effect,
                    time_effect,
                );
                effects.remove(effect);
                for arm in arms {
                    effects.extend(self.infer_effects_from_expr(
                        &arm.body,
                        current_module,
                        inferred,
                        io_effect,
                        time_effect,
                    ));
                }
                effects
            }
        }
    }

    fn infer_call_effects(
        &self,
        function: &Expression,
        arguments: &[Expression],
        current_module: Option<Symbol>,
        inferred: &HashMap<ContractKey, HashSet<Symbol>>,
        io_effect: Symbol,
        time_effect: Symbol,
    ) -> HashSet<Symbol> {
        let mut effects = HashSet::new();
        let arity = arguments.len();
        match function {
            Expression::Identifier { name, .. } => {
                let mut resolved = false;
                if let Some(module_name) = current_module {
                    let key = ContractKey {
                        module_name: Some(module_name),
                        function_name: *name,
                        arity,
                    };
                    if let Some(found) = inferred.get(&key) {
                        effects.extend(self.resolve_call_effect_row_with_args(
                            found,
                            self.lookup_contract(Some(module_name), *name, arity),
                            arguments,
                        ));
                        resolved = true;
                    }
                }
                if !resolved {
                    let key = ContractKey {
                        module_name: None,
                        function_name: *name,
                        arity,
                    };
                    if let Some(found) = inferred.get(&key) {
                        effects.extend(self.resolve_call_effect_row_with_args(
                            found,
                            self.lookup_unqualified_contract(*name, arity),
                            arguments,
                        ));
                        resolved = true;
                    }
                }
                if !resolved {
                    if self.excluded_base_symbols.contains(name) {
                        return effects;
                    }
                    let name = self.sym(*name);
                    if matches!(name, "print" | "read_file" | "read_lines" | "read_stdin") {
                        effects.insert(io_effect);
                    } else if matches!(name, "now" | "clock_now" | "now_ms" | "time") {
                        effects.insert(time_effect);
                    }
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                if let Some(module_name) = self.resolve_module_name_from_expr(object) {
                    let key = ContractKey {
                        module_name: Some(module_name),
                        function_name: *member,
                        arity,
                    };
                    if let Some(found) = inferred.get(&key) {
                        effects.extend(self.resolve_call_effect_row_with_args(
                            found,
                            self.lookup_contract(Some(module_name), *member, arity),
                            arguments,
                        ));
                    }
                }
            }
            _ => {}
        }
        effects
    }

    fn resolve_call_effect_row_with_args(
        &self,
        raw_effects: &HashSet<Symbol>,
        contract: Option<&FnContract>,
        arguments: &[Expression],
    ) -> HashSet<Symbol> {
        use crate::bytecode::compiler::effect_rows::{
            EffectRow, RowConstraint, solve_row_constraints,
        };

        let mut effects_as_expr = Vec::new();
        for effect in raw_effects {
            effects_as_expr.push(EffectExpr::Named {
                name: *effect,
                span: Span::default(),
            });
        }
        let required = EffectRow::from_effect_exprs(&effects_as_expr, |effect| {
            self.is_effect_variable(effect)
        });

        let mut constraints = Vec::new();
        if let Some(contract) = contract {
            for (idx, argument) in arguments.iter().enumerate() {
                let Some(Some(TypeExpr::Function {
                    params,
                    effects: param_effects,
                    ..
                })) = contract.params.get(idx)
                else {
                    continue;
                };

                let expected = EffectRow::from_effect_exprs(param_effects, |effect| {
                    self.is_effect_variable(effect)
                });
                let Some(actual) = self.infer_argument_effect_row_for_inference(
                    argument,
                    params.len(),
                    raw_effects,
                    arguments,
                ) else {
                    continue;
                };
                constraints.push(RowConstraint::Eq(expected.clone(), actual.clone()));
                constraints.push(RowConstraint::Subset(expected, actual.clone()));
                for effect in param_effects {
                    self.collect_effect_expr_absence_constraints(effect, &actual, &mut constraints);
                }
            }
        }

        let solved = solve_row_constraints(&constraints);
        required.concrete_effects(&solved)
    }

    fn infer_argument_effect_row_for_inference(
        &self,
        argument: &Expression,
        expected_arity: usize,
        inferred_effects: &HashSet<Symbol>,
        call_arguments: &[Expression],
    ) -> Option<crate::bytecode::compiler::effect_rows::EffectRow> {
        use crate::bytecode::compiler::effect_rows::EffectRow;

        match argument {
            Expression::Function { effects, .. } => {
                Some(EffectRow::from_effect_exprs(effects, |effect| {
                    self.is_effect_variable(effect)
                }))
            }
            Expression::Identifier { name, .. } => self
                .lookup_unqualified_contract(*name, expected_arity)
                .map(|contract| {
                    let mut set: HashSet<Symbol> = contract
                        .effects
                        .iter()
                        .flat_map(EffectExpr::normalized_names)
                        .collect();
                    if set.is_empty() {
                        set.extend(inferred_effects.iter().copied());
                    }
                    let effect_exprs: Vec<EffectExpr> = set
                        .into_iter()
                        .map(|name| EffectExpr::Named {
                            name,
                            span: Span::default(),
                        })
                        .collect();
                    EffectRow::from_effect_exprs(&effect_exprs, |effect| {
                        self.is_effect_variable(effect)
                    })
                }),
            Expression::MemberAccess { object, member, .. } => self
                .resolve_module_name_from_expr(object)
                .and_then(|module| self.lookup_contract(Some(module), *member, expected_arity))
                .map(|contract| {
                    EffectRow::from_effect_exprs(&contract.effects, |effect| {
                        self.is_effect_variable(effect)
                    })
                }),
            _ => {
                let _ = call_arguments;
                None
            }
        }
    }

    fn validate_main_entrypoint(&mut self, program: &Program) -> MainValidationState {
        let main_symbol = self.interner.intern("main");
        let mut mains: Vec<(Span, usize, Option<TypeExpr>)> = Vec::new();

        for statement in &program.statements {
            if let Statement::Function {
                name,
                parameters,
                return_type,
                span,
                ..
            } = statement
                && *name == main_symbol
            {
                mains.push((*span, parameters.len(), return_type.clone()));
            }
        }

        if mains.len() > 1 {
            let (first_span, _, _) = mains[0].clone();
            for (span, _, _) in mains.iter().skip(1) {
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E410",
                        "DUPLICATE MAIN FUNCTION",
                        ErrorType::Compiler,
                        "Program can contain only one top-level `fn main`.",
                        Some("Keep a single `fn main` entry point.".to_string()),
                        self.file_path.clone(),
                        *span,
                    )
                    .with_primary_label(*span, "duplicate `main` declaration")
                    .with_note_label(first_span, "first `main` declared here"),
                );
            }
        }

        let mut is_valid_signature = true;
        if let Some((main_span, param_count, return_type)) = mains.first() {
            if *param_count != 0 {
                is_valid_signature = false;
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E411",
                        "INVALID MAIN SIGNATURE",
                        ErrorType::Compiler,
                        "`fn main` cannot take parameters.",
                        Some("Define `fn main()` with zero parameters.".to_string()),
                        self.file_path.clone(),
                        *main_span,
                    )
                    .with_primary_label(*main_span, "`main` declared with parameters"),
                );
            }

            if let Some(ret) = return_type
                && !Self::is_unit_type_annotation(ret, &self.interner)
            {
                is_valid_signature = false;
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E412",
                        "INVALID MAIN RETURN TYPE",
                        ErrorType::Compiler,
                        "`fn main` must return `Unit` (or omit return type).",
                        Some("Change signature to `fn main() { ... }` or `-> Unit`.".to_string()),
                        self.file_path.clone(),
                        ret.span(),
                    )
                    .with_primary_label(ret.span(), "invalid `main` return type"),
                );
            }
        }

        MainValidationState {
            has_main: !mains.is_empty(),
            is_unique_main: mains.len() <= 1,
            is_valid_signature,
        }
    }

    fn contract_effect_sets(&self) -> HashMap<ContractKey, HashSet<Symbol>> {
        self.module_contracts
            .iter()
            .map(|(key, contract)| {
                let effects = contract
                    .effects
                    .iter()
                    .flat_map(EffectExpr::normalized_names)
                    .collect::<HashSet<_>>();
                (key.clone(), effects)
            })
            .collect()
    }

    fn validate_top_level_effectful_code(&mut self, program: &Program, has_main: bool) {
        let inferred = self.contract_effect_sets();
        let io_effect = self.interner.intern("IO");
        let time_effect = self.interner.intern("Time");
        let mut missing_root_reported = false;

        for statement in &program.statements {
            let (effects, span) = match statement {
                Statement::Expression {
                    expression, span, ..
                } => (
                    self.infer_effects_from_expr(
                        expression,
                        None,
                        &inferred,
                        io_effect,
                        time_effect,
                    ),
                    *span,
                ),
                Statement::Let { value, span, .. }
                | Statement::LetDestructure { value, span, .. }
                | Statement::Assign { value, span, .. } => (
                    self.infer_effects_from_expr(value, None, &inferred, io_effect, time_effect),
                    *span,
                ),
                Statement::Return {
                    value: Some(value),
                    span,
                } => (
                    self.infer_effects_from_expr(value, None, &inferred, io_effect, time_effect),
                    *span,
                ),
                _ => continue,
            };

            if effects.is_empty() {
                continue;
            }

            let mut effect_names: Vec<_> = effects
                .iter()
                .map(|effect| self.sym(*effect).to_string())
                .collect();
            effect_names.sort();
            let effect_names = effect_names.join(", ");

            self.errors.push(
                Diagnostic::make_error_dynamic(
                    "E413",
                    "TOP-LEVEL EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Effectful operation is not allowed at top level (requires: {}).",
                        effect_names
                    ),
                    Some("Move this code into `fn main() with ... { ... }`.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "top-level effectful expression"),
            );

            if !has_main && !missing_root_reported {
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E414",
                        "MISSING MAIN FUNCTION",
                        ErrorType::Compiler,
                        "Effectful program is missing `fn main` root effect handler.",
                        Some(
                            "Define `fn main() with ... { ... }` and move execution there."
                                .to_string(),
                        ),
                        self.file_path.clone(),
                        span,
                    )
                    .with_primary_label(span, "effectful top-level execution"),
                );
                missing_root_reported = true;
            }
        }
    }

    fn validate_main_root_effect_discharge(
        &mut self,
        program: &Program,
        main_state: MainValidationState,
    ) {
        if !(main_state.has_main && main_state.is_unique_main && main_state.is_valid_signature) {
            return;
        }
        let main_symbol = self.interner.intern("main");
        let io_effect = self.interner.intern("IO");
        let time_effect = self.interner.intern("Time");
        let inferred = self.contract_effect_sets();

        let mut main_body = None;
        for statement in &program.statements {
            if let Statement::Function { name, body, .. } = statement
                && *name == main_symbol
            {
                main_body = Some(body);
                break;
            }
        }
        let Some(main_body) = main_body else {
            return;
        };

        let residual =
            self.infer_effects_from_block(main_body, None, &inferred, io_effect, time_effect);
        let mut disallowed: Vec<Symbol> = residual
            .into_iter()
            .filter(|effect| *effect != io_effect && *effect != time_effect)
            .collect();
        if disallowed.is_empty() {
            return;
        }

        disallowed.sort_by_key(|sym| self.sym(*sym).to_string());
        let effects_text = disallowed
            .iter()
            .map(|effect| self.sym(*effect).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.errors.push(
            Diagnostic::make_error_dynamic(
                "E406",
                "UNHANDLED ROOT EFFECT",
                ErrorType::Compiler,
                format!("`fn main` has undischarged effects: {}.", effects_text),
                Some(
                    "Handle these effects explicitly with `... handle Effect { ... }` before returning from `main`."
                        .to_string(),
                ),
                self.file_path.clone(),
                main_body.span,
            )
            .with_primary_label(main_body.span, "undischarged effects at root boundary"),
        );
    }

    fn validate_strict_mode(&mut self, program: &Program, has_main: bool) {
        if !self.strict_mode {
            return;
        }

        if self.strict_require_main && !has_main {
            self.errors.push(
                Diagnostic::make_error_dynamic(
                    "E415",
                    "MISSING MAIN FUNCTION (STRICT)",
                    ErrorType::Compiler,
                    "Strict mode requires `fn main()` for all programs.",
                    Some("Add `fn main() { ... }` as the program entrypoint.".to_string()),
                    self.file_path.clone(),
                    program.span,
                )
                .with_primary_label(program.span, "no `main` entrypoint found"),
            );
        }

        for statement in &program.statements {
            self.validate_strict_mode_statement(statement, None);
        }
    }

    fn validate_strict_mode_statement(
        &mut self,
        statement: &Statement,
        module_name: Option<Symbol>,
    ) {
        match statement {
            Statement::Function {
                is_public,
                name,
                parameters,
                parameter_types,
                return_type,
                effects,
                span,
                ..
            } => {
                if *is_public {
                    if parameter_types.iter().any(Option::is_none) {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E416",
                                "STRICT FUNCTION ANNOTATION REQUIRED",
                                ErrorType::Compiler,
                                format!(
                                    "Public function `{}` must annotate all parameters in strict mode.",
                                    self.sym(*name)
                                ),
                                Some("Add explicit parameter types to the function signature.".to_string()),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_primary_label(*span, "missing parameter type annotations"),
                        );
                    }

                    if return_type.is_none() {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E417",
                                "STRICT RETURN ANNOTATION REQUIRED",
                                ErrorType::Compiler,
                                format!(
                                    "Public function `{}` must declare a return type in strict mode.",
                                    self.sym(*name)
                                ),
                                Some("Add `-> Type` to the function signature.".to_string()),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_primary_label(*span, "missing return type annotation"),
                        );
                    }

                    let function_contract_position = parameter_types
                        .iter()
                        .flatten()
                        .find(|ty| Self::type_expr_contains_function(ty))
                        .map(TypeExpr::span)
                        .or_else(|| {
                            return_type
                                .as_ref()
                                .filter(|ret| Self::type_expr_contains_function(ret))
                                .map(TypeExpr::span)
                        });
                    if let Some(function_span) = function_contract_position {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E424",
                                "STRICT UNSUPPORTED FUNCTION CONTRACT",
                                ErrorType::Compiler,
                                format!(
                                    "Public function `{}` uses function-typed boundary annotations that are not runtime-enforced yet.",
                                    self.sym(*name)
                                ),
                                Some(
                                    "Use concrete boundary types or keep this API internal/private until function-typed runtime contracts are implemented."
                                        .to_string(),
                                ),
                                self.file_path.clone(),
                                function_span,
                            )
                            .with_primary_label(
                                function_span,
                                "function-typed boundary contract is unsupported in strict mode",
                            ),
                        );
                    }

                    let is_effectful = self
                        .lookup_contract(module_name, *name, parameters.len())
                        .is_some_and(|contract| !contract.effects.is_empty());
                    if is_effectful && effects.is_empty() {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E418",
                                "STRICT EFFECT ANNOTATION REQUIRED",
                                ErrorType::Compiler,
                                format!(
                                    "Public effectful function `{}` must declare `with ...` in strict mode.",
                                    self.sym(*name)
                                ),
                                Some("Add explicit `with EffectName` to the function signature.".to_string()),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_primary_label(*span, "missing explicit effect annotation"),
                        );
                    }
                }

                for ty in parameter_types.iter().flatten() {
                    self.error_on_any_type_expr_in_strict(ty);
                }
                if let Some(ret) = return_type {
                    self.error_on_any_type_expr_in_strict(ret);
                }
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.validate_strict_mode_statement(nested, Some(*name));
                }
            }
            _ => {}
        }
    }

    fn error_on_any_type_expr_in_strict(&mut self, ty: &TypeExpr) {
        if !Self::type_expr_contains_any(ty, &self.interner) {
            return;
        }

        let span = ty.span();
        self.errors.push(
            Diagnostic::make_error_dynamic(
                "E423",
                "STRICT ANY TYPE",
                ErrorType::Compiler,
                "Using `Any` in strict mode weakens static guarantees.",
                Some("Replace `Any` with explicit concrete types in strict mode.".to_string()),
                self.file_path.clone(),
                span,
            )
            .with_primary_label(span, "`Any` used here"),
        );
    }

    fn type_expr_contains_any(ty: &TypeExpr, interner: &Interner) -> bool {
        match ty {
            TypeExpr::Named { name, args, .. } => {
                interner.resolve(*name) == "Any"
                    || args
                        .iter()
                        .any(|arg| Self::type_expr_contains_any(arg, interner))
            }
            TypeExpr::Tuple { elements, .. } => elements
                .iter()
                .any(|elem| Self::type_expr_contains_any(elem, interner)),
            TypeExpr::Function { params, ret, .. } => {
                params
                    .iter()
                    .any(|param| Self::type_expr_contains_any(param, interner))
                    || Self::type_expr_contains_any(ret, interner)
            }
        }
    }

    fn type_expr_contains_function(ty: &TypeExpr) -> bool {
        match ty {
            TypeExpr::Function { .. } => true,
            TypeExpr::Named { args, .. } => args.iter().any(Self::type_expr_contains_function),
            TypeExpr::Tuple { elements, .. } => {
                elements.iter().any(Self::type_expr_contains_function)
            }
        }
    }

    fn has_explicit_top_level_main_call(&self, program: &Program, main_symbol: Symbol) -> bool {
        program.statements.iter().any(|statement| {
            matches!(
                statement,
                Statement::Expression {
                    expression: Expression::Call { function, arguments, .. },
                    ..
                } if matches!(function.as_ref(), Expression::Identifier { name, .. } if *name == main_symbol)
                    && arguments.is_empty()
            )
        })
    }

    fn emit_main_entry_call(&mut self) {
        let main_symbol = self.interner.intern("main");
        let Some(main_binding) = self.symbol_table.resolve(main_symbol) else {
            return;
        };
        self.load_symbol(&main_binding);
        self.emit(OpCode::OpCall, &[0]);
        self.emit(OpCode::OpPop, &[]);
    }

    fn is_unit_type_annotation(ty: &TypeExpr, interner: &Interner) -> bool {
        match ty {
            TypeExpr::Named { name, args, .. } if args.is_empty() => {
                matches!(interner.resolve(*name), "Unit" | "None")
            }
            TypeExpr::Tuple { elements, .. } => elements.is_empty(),
            _ => false,
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

    pub(super) fn module_member_function_is_public(
        &self,
        module_name: Symbol,
        member_name: Symbol,
    ) -> Option<bool> {
        self.module_function_visibility
            .get(&(module_name, member_name))
            .copied()
    }

    pub(super) fn module_member_adt_constructor_owner(
        &self,
        module_name: Symbol,
        member_name: Symbol,
    ) -> Option<Symbol> {
        if let Some(owner) = self
            .module_adt_constructors
            .get(&(module_name, member_name))
            .copied()
        {
            return Some(owner);
        }

        let member_text = self.sym(member_name);
        self.module_adt_constructors
            .iter()
            .find_map(|((owner, ctor), adt)| {
                if *owner != module_name {
                    return None;
                }
                let ctor_text = self.sym(*ctor);
                (ctor_text == member_text || ctor_text.rsplit('.').next() == Some(member_text))
                    .then_some(*adt)
            })
    }

    pub(super) fn module_qualifier_text(&self, expr: &Expression) -> Option<String> {
        match expr {
            Expression::Identifier { name, .. } => Some(self.sym(*name).to_string()),
            Expression::MemberAccess { object, member, .. } => Some(format!(
                "{}.{}",
                self.module_qualifier_text(object)?,
                self.sym(*member)
            )),
            _ => None,
        }
    }

    pub(super) fn resolve_module_name_from_expr(&self, expr: &Expression) -> Option<Symbol> {
        if let Expression::Identifier { name, .. } = expr {
            if let Some(target) = self.import_aliases.get(name) {
                return Some(*target);
            }
            if self.imported_modules.contains(name) || self.current_module_prefix == Some(*name) {
                return Some(*name);
            }
            return None;
        }

        let qualifier = self.module_qualifier_text(expr)?;

        if let Some(found) = self
            .imported_modules
            .iter()
            .copied()
            .find(|module| self.sym(*module) == qualifier)
        {
            return Some(found);
        }

        if let Some(current) = self.current_module_prefix
            && self.sym(current) == qualifier
        {
            return Some(current);
        }

        self.module_contracts.keys().find_map(|key| {
            let module = key.module_name?;
            (self.sym(module) == qualifier).then_some(module)
        })
    }

    pub(super) fn effect_declared_ops(&self, effect: Symbol) -> Option<&HashSet<Symbol>> {
        self.effect_ops_registry.get(&effect)
    }

    pub(super) fn effect_op_signature(&self, effect: Symbol, op: Symbol) -> Option<&TypeExpr> {
        self.effect_op_signatures.get(&(effect, op))
    }

    pub(super) fn is_effect_variable(&self, effect: Symbol) -> bool {
        self.sym(effect)
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
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
    pub(super) fn bind_effect_alias(&mut self, name: Symbol, effect: Symbol) {
        if let Some(scope) = self.effect_alias_scopes.last_mut() {
            scope.insert(name, effect);
        }
    }

    #[inline]
    pub(super) fn lookup_effect_alias(&self, name: Symbol) -> Option<Symbol> {
        self.effect_alias_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    pub(super) fn track_effect_alias_for_binding(&mut self, binding: Symbol, value: &Expression) {
        let Expression::Identifier { name, .. } = value else {
            return;
        };

        if let Some(effect) = self.lookup_effect_alias(*name) {
            self.bind_effect_alias(binding, effect);
            return;
        }

        let Some(symbol) = self.resolve_visible_symbol(*name) else {
            return;
        };
        if symbol.symbol_scope != crate::bytecode::symbol_scope::SymbolScope::Base {
            return;
        }

        let base_name = self.sym(*name).to_string();
        let required_name = match base_name.as_str() {
            "print" | "read_file" | "read_lines" | "read_stdin" => Some("IO"),
            "now" | "clock_now" | "now_ms" | "time" => Some("Time"),
            _ => None,
        };
        let Some(required_name) = required_name else {
            return;
        };
        let effect = self.interner.intern(required_name);
        self.bind_effect_alias(binding, effect);
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
    pub fn compile_with_opts(
        &mut self,
        program: &Program,
        optimize: bool,
        analyze: bool,
    ) -> Result<(), Vec<Diagnostic>> {
        // Pointer-identity invariant for HM ExprTypeMap:
        // HM expression IDs are keyed by expression allocation addresses from the
        // Program passed to `compile`. Any AST rewrites must happen before this call.
        // `program_to_compile` is therefore the single transformed Program consumed by
        // both HM inference and PASS 2 codegen validation in one invocation.
        // Apply optimizations only when requested.
        if optimize {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            // Rename pass (currently no-op, reserved for future alpha-conversion)
            let program_to_compile = rename(optimized, HashMap::new());

            // Collect analysis data if requested.
            if analyze {
                self.free_vars = collect_free_vars_in_program(&program_to_compile);
                self.tail_calls = find_tail_calls(&program_to_compile);
            } else {
                self.free_vars.clear();
                self.tail_calls.clear();
            }

            self.compile(&program_to_compile)
        } else {
            // Borrow the original program directly for non-optimized paths.
            if analyze {
                self.free_vars = collect_free_vars_in_program(program);
                self.tail_calls = find_tail_calls(program);
            } else {
                self.free_vars.clear();
                self.tail_calls.clear();
            }
            self.compile(program)
        }
    }

    pub fn compile(&mut self, program: &Program) -> Result<(), Vec<Diagnostic>> {
        // Ensure per-file tracking is clean for each compile pass.
        self.warnings.clear();
        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.imported_module_exclusions.clear();
        self.current_module_prefix = None;
        self.excluded_base_symbols.clear();
        self.function_effects.clear();
        self.handled_effects.clear();
        self.effect_ops_registry.clear();
        self.effect_op_signatures.clear();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.process_base_directives(program);
        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        self.infer_unannotated_function_effects(program);
        self.collect_adt_definitions(program);
        self.collect_effect_declarations(program);
        let main_state = self.validate_main_entrypoint(program);
        self.validate_top_level_effectful_code(program, main_state.has_main);
        self.validate_main_root_effect_discharge(program, main_state);
        self.validate_strict_mode(program, main_state.has_main);

        let main_symbol = self.interner.intern("main");

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
                    // Keep duplicate-main diagnostics canonical via E410 from
                    // `validate_main_entrypoint`, avoid redundant E001 noise.
                    if name == main_symbol {
                        continue;
                    }
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

        tag_diagnostics(&mut self.errors, DiagnosticPhase::Validation);

        // HM type inference pass — runs after predeclaration, before code generation.
        // The resulting TypeEnv is used by `runtime_boundary_expr_type` to enrich identifier
        // type lookup for unannotated bindings.
        //
        // Invariant: `infer_program` and PASS 2 must use the same Program allocation so
        // pointer-keyed expression IDs remain stable.
        //
        // Diagnostics from this pass are guarded by a concrete-types-only filter
        // inside `unify_reporting`: only errors where *both* conflicting types are
        // fully resolved (no free type variables) are emitted. This prevents
        // spurious failures in partially-typed programs where base-function return
        // types are not yet registered in the inference environment.
        let mut hm_diagnostics = {
            let preloaded_member_schemes = self.build_preloaded_hm_member_schemes(program);
            let hm = infer_program(
                program,
                &self.interner,
                Some(self.file_path.clone()),
                preloaded_member_schemes,
                self.effect_op_signatures.clone(),
            );
            self.type_env = hm.type_env;
            self.hm_expr_types = hm.expr_types;
            self.expr_ptr_to_id = hm.expr_ptr_to_id;
            hm.diagnostics
        };
        tag_diagnostics(&mut hm_diagnostics, DiagnosticPhase::TypeInference);

        // PASS 2: Compile all statements
        // Function bodies can now reference any function defined at module level
        let mut pattern_diags = validate_program_patterns(program, &self.file_path, &self.interner);
        tag_diagnostics(&mut pattern_diags, DiagnosticPhase::Validation);
        self.errors.extend(pattern_diags);
        for statement in &program.statements {
            // Continue compilation even if there are errors
            if let Err(err) = self.compile_statement(statement) {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
        }

        if main_state.has_main && !self.has_explicit_top_level_main_call(program, main_symbol) {
            self.emit_main_entry_call();
        }

        // HM no longer emits errors for annotated boundaries (return type,
        // let annotation) — those use `unify_propagate` (silent).
        //
        // For call-site argument mismatches, HM's `infer_call` still reports
        // (it's the only reporter for untyped functions).  When a typed function
        // causes the compiler to emit a per-argument boundary error, the HM
        // call-site error is redundant.  Drop any HM diagnostic that shares the
        // same code + message as an existing compiler error on an overlapping span.
        self.suppress_overlapping_hm_diagnostics(&mut hm_diagnostics);

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

    /// Drop HM diagnostics that are redundant with a compiler boundary error.
    ///
    /// An HM diagnostic is considered redundant when an existing compiler error
    /// has the same error code, the same severity, and overlapping spans.
    /// The message text may differ (HM emits generic "Cannot unify X with Y"
    /// while the compiler emits more specific messages like "matching '+'
    /// operands"), but if code + severity + span overlap they describe the
    /// same semantic issue and the compiler's version is preferred.
    fn suppress_overlapping_hm_diagnostics(&self, hm_diagnostics: &mut Vec<Diagnostic>) {
        if self.errors.is_empty() || hm_diagnostics.is_empty() {
            return;
        }
        let default_file = &self.file_path;
        hm_diagnostics.retain(|hm| {
            !self.errors.iter().any(|existing| {
                existing.code() == hm.code()
                    && existing.severity() == hm.severity()
                    && Self::diagnostic_spans_overlap(existing, hm, default_file)
            })
        });
    }

    fn diagnostic_spans_overlap(a: &Diagnostic, b: &Diagnostic, default_file: &str) -> bool {
        let (Some(a_span), Some(b_span)) = (a.span(), b.span()) else {
            return false;
        };
        let a_file = a.file().unwrap_or(default_file);
        let b_file = b.file().unwrap_or(default_file);
        if a_file != b_file {
            return false;
        }
        Self::spans_overlap(a_span, b_span)
    }

    fn spans_overlap(left: Span, right: Span) -> bool {
        Self::position_leq(left.start, right.end) && Self::position_leq(right.start, left.end)
    }

    fn position_leq(left: Position, right: Position) -> bool {
        left.line < right.line || (left.line == right.line && left.column <= right.column)
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
        self.effect_alias_scopes.push(HashMap::new());
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
        let _ = self.effect_alias_scopes.pop();

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
        self.effect_alias_scopes.push(HashMap::new());
    }

    pub(super) fn leave_block_scope(&mut self) {
        let num_definitions = self.symbol_table.num_definitions;
        if let Some(outer) = self.symbol_table.outer.take() {
            let mut outer = *outer;
            outer.num_definitions = num_definitions;
            self.symbol_table = outer;
        }
        let _ = self.static_type_scopes.pop();
        let _ = self.effect_alias_scopes.pop();
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

    pub(super) fn with_function_context<F, R>(
        &mut self,
        num_params: usize,
        effects: &[EffectExpr],
        f: F,
    ) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.function_param_counts.push(num_params);
        self.function_effects.push(
            effects
                .iter()
                .flat_map(EffectExpr::normalized_names)
                .collect(),
        );
        self.captured_local_indices.push(HashSet::new());
        let result = f(self);
        self.captured_local_indices.pop();
        self.function_effects.pop();
        self.function_param_counts.pop();
        result
    }

    pub(super) fn current_function_param_count(&self) -> Option<usize> {
        self.function_param_counts.last().copied()
    }

    pub(super) fn current_function_effects(&self) -> Option<&[Symbol]> {
        self.function_effects.last().map(Vec::as_slice)
    }

    pub(super) fn with_handled_effect<F, R>(&mut self, effect: Symbol, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.handled_effects.push(effect);
        let result = f(self);
        self.handled_effects.pop();
        result
    }

    pub(super) fn is_effect_available(&self, required: Symbol) -> bool {
        if self.current_function_effects().is_none() && self.handled_effects.is_empty() {
            return true;
        }
        self.current_function_effects()
            .is_some_and(|effects| effects.contains(&required))
            || self.handled_effects.contains(&required)
    }

    pub(super) fn is_effect_available_name(&self, required_name: &str) -> bool {
        if self.current_function_effects().is_none() && self.handled_effects.is_empty() {
            return true;
        }
        self.current_function_effects().is_some_and(|effects| {
            effects
                .iter()
                .any(|effect| self.sym(*effect) == required_name)
        }) || self
            .handled_effects
            .iter()
            .any(|handled| self.sym(*handled) == required_name)
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
