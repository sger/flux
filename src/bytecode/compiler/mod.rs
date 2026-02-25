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
        CIRCULAR_DEPENDENCY, Diagnostic, DiagnosticBuilder, ErrorType, UNKNOWN_BASE_MEMBER,
        lookup_error_code,
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
mod errors;
mod expression;
mod statement;
mod suggestions;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

#[derive(Debug, Clone)]
struct FunctionEffectSeed {
    key: ContractKey,
    module_name: Option<Symbol>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type: Option<TypeExpr>,
    declared_effects: HashSet<Symbol>,
    body: Block,
    span: Span,
}

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
    // Declared ambient effects for active function scopes innermost last.
    pub(super) function_effects: Vec<Vec<Symbol>>,
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
    pub(super) effect_ops_registry: HashMap<Symbol, HashSet<Symbol>>,
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
            function_effects: Vec::new(),
            captured_local_indices: Vec::new(),
            free_vars: HashSet::new(),
            tail_calls: Vec::new(),
            excluded_base_symbols: HashSet::new(),
            module_contracts: HashMap::new(),
            static_type_scopes: vec![HashMap::new()],
            adt_registry: AdtRegistry::new(),
            effect_ops_registry: HashMap::new(),
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
        self.function_effects.clear();
        self.effect_ops_registry.clear();
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
                name,
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
                    .map(|effect| match effect {
                        EffectExpr::Named { name, .. } => *name,
                    })
                    .collect();
                out.push(FunctionEffectSeed {
                    key: ContractKey {
                        module_name,
                        function_name: *name,
                        arity: parameters.len(),
                    },
                    module_name,
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
                let is_fully_unannotated = !seed.parameter_types.iter().any(Option::is_some)
                    && seed.return_type.is_none();
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
            | Statement::Assign { value, .. } => {
                self.infer_effects_from_expr(value, current_module, inferred, io_effect, time_effect)
            }
            Statement::Return {
                value: Some(value), ..
            } => self.infer_effects_from_expr(value, current_module, inferred, io_effect, time_effect),
            Statement::Expression { expression, .. } => {
                self.infer_effects_from_expr(expression, current_module, inferred, io_effect, time_effect)
            }
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
                let mut effects =
                    self.infer_effects_from_expr(left, current_module, inferred, io_effect, time_effect);
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
                    arguments.len(),
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
                let mut effects =
                    self.infer_effects_from_expr(left, current_module, inferred, io_effect, time_effect);
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
            Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
                self.infer_effects_from_expr(object, current_module, inferred, io_effect, time_effect)
            }
            Expression::Match { scrutinee, arms, .. } => {
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
                let mut effects =
                    self.infer_effects_from_expr(head, current_module, inferred, io_effect, time_effect);
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
                expr,
                effect,
                arms,
                ..
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
        arity: usize,
        current_module: Option<Symbol>,
        inferred: &HashMap<ContractKey, HashSet<Symbol>>,
        io_effect: Symbol,
        time_effect: Symbol,
    ) -> HashSet<Symbol> {
        let mut effects = HashSet::new();
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
                        effects.extend(found.iter().copied());
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
                        effects.extend(found.iter().copied());
                        resolved = true;
                    }
                }
                if !resolved {
                    let name = self.sym(*name);
                    if matches!(name, "print" | "read_file" | "read_lines" | "read_stdin") {
                        effects.insert(io_effect);
                    } else if matches!(name, "now" | "clock_now") {
                        effects.insert(time_effect);
                    }
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                if let Expression::Identifier {
                    name: module_name, ..
                } = object.as_ref()
                {
                    let key = ContractKey {
                        module_name: Some(*module_name),
                        function_name: *member,
                        arity,
                    };
                    if let Some(found) = inferred.get(&key) {
                        effects.extend(found.iter().copied());
                    }
                }
            }
            _ => {}
        }
        effects
    }

    fn validate_main_entrypoint(&mut self, program: &Program) {
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

        if let Some((main_span, param_count, return_type)) = mains.first() {
            if *param_count != 0 {
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

            if let Some(ret) = return_type && !Self::is_unit_type_annotation(ret, &self.interner) {
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
    }

    fn validate_top_level_effectful_code(&mut self, program: &Program) {
        for statement in &program.statements {
            match statement {
                Statement::Expression {
                    expression, span, ..
                } => {
                    if Self::contains_perform_expression(expression) {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E413",
                                "TOP-LEVEL EFFECT",
                                ErrorType::Compiler,
                                "Effectful operation is not allowed at top level.",
                                Some("Move this code into `fn main() with ... { ... }`.".to_string()),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_primary_label(*span, "top-level effectful expression"),
                        );
                    }
                }
                Statement::Let { value, span, .. }
                | Statement::LetDestructure {
                    value, span, ..
                }
                | Statement::Assign { value, span, .. } => {
                    if Self::contains_perform_expression(value) {
                        self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E413",
                                "TOP-LEVEL EFFECT",
                                ErrorType::Compiler,
                                "Effectful operation is not allowed at top level.",
                                Some("Move this code into `fn main() with ... { ... }`.".to_string()),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_primary_label(*span, "top-level effectful expression"),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn contains_perform_expression(expr: &Expression) -> bool {
        match expr {
            Expression::Perform { .. } => true,
            Expression::Prefix { right, .. } => Self::contains_perform_expression(right),
            Expression::Infix { left, right, .. } => {
                Self::contains_perform_expression(left) || Self::contains_perform_expression(right)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                if Self::contains_perform_expression(condition) {
                    return true;
                }
                if consequence
                    .statements
                    .iter()
                    .any(Self::statement_contains_perform)
                {
                    return true;
                }
                alternative
                    .as_ref()
                    .is_some_and(|alt| alt.statements.iter().any(Self::statement_contains_perform))
            }
            Expression::DoBlock { block, .. } => {
                block.statements.iter().any(Self::statement_contains_perform)
            }
            Expression::Function { body, .. } => {
                body.statements.iter().any(Self::statement_contains_perform)
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                Self::contains_perform_expression(function)
                    || arguments.iter().any(Self::contains_perform_expression)
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                elements.iter().any(Self::contains_perform_expression)
            }
            Expression::Index { left, index, .. } => {
                Self::contains_perform_expression(left) || Self::contains_perform_expression(index)
            }
            Expression::Hash { pairs, .. } => pairs.iter().any(|(k, v)| {
                Self::contains_perform_expression(k) || Self::contains_perform_expression(v)
            }),
            Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
                Self::contains_perform_expression(object)
            }
            Expression::Match { scrutinee, arms, .. } => {
                Self::contains_perform_expression(scrutinee)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(Self::contains_perform_expression)
                            || Self::contains_perform_expression(&arm.body)
                    })
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => Self::contains_perform_expression(value),
            Expression::Cons { head, tail, .. } => {
                Self::contains_perform_expression(head) || Self::contains_perform_expression(tail)
            }
            Expression::Handle { expr, arms, .. } => {
                Self::contains_perform_expression(expr)
                    || arms.iter().any(|arm| Self::contains_perform_expression(&arm.body))
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::InterpolatedString { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => false,
        }
    }

    fn statement_contains_perform(statement: &Statement) -> bool {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => Self::contains_perform_expression(value),
            Statement::Return {
                value: Some(value), ..
            } => Self::contains_perform_expression(value),
            Statement::Expression { expression, .. } => Self::contains_perform_expression(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                body.statements.iter().any(Self::statement_contains_perform)
            }
            _ => false,
        }
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

    pub(super) fn effect_declared_ops(&self, effect: Symbol) -> Option<&HashSet<Symbol>> {
        self.effect_ops_registry.get(&effect)
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
        self.function_effects.clear();
        self.module_contracts.clear();
        self.effect_ops_registry.clear();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.process_base_directives(program);
        self.collect_module_contracts(program);
        self.infer_unannotated_function_effects(program);
        self.collect_adt_definitions(program);
        self.collect_effect_declarations(program);
        self.validate_main_entrypoint(program);
        self.validate_top_level_effectful_code(program);

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
                .map(|effect| match effect {
                    EffectExpr::Named { name, .. } => *name,
                })
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
