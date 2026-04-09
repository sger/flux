use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::aether::borrow_infer::{BorrowRegistry, BorrowSignature};
use crate::ast::type_infer::InferProgramConfig;
use crate::bytecode::compiler::effect_rows::EffectRow;
use crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult;
use crate::cfg::{FunctionId, IrFunction, IrInstr, IrProgram, IrTerminator};
use crate::syntax::expression::ExprId;
use crate::types::infer_effect_row::InferEffectRow;
use crate::types::{TypeVarId, infer_type::InferType, scheme::Scheme};
use crate::{
    ast::{
        TailCall, collect_free_vars_in_program,
        desugar_operators_if_needed,
        operator_desugaring_needed,
        type_infer::{InferProgramResult, infer_program},
        type_informed_fold::type_informed_fold,
    },
    bytecode::{
        binding::Binding,
        bytecode::Bytecode,
        bytecode_cache::module_cache::{CachedModuleBinding, CachedModuleBytecode},
        compilation_scope::CompilationScope,
        compiler::{
            adt_registry::AdtRegistry,
            contracts::{ContractKey, FnContract, ModuleContractTable, to_runtime_contract},
        },
        debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation},
        emitted_instruction::EmittedInstruction,
        op_code::{Instructions, OpCode, make},
        symbol_table::SymbolTable,
    },
    diagnostics::{
        CIRCULAR_DEPENDENCY, Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase,
        ErrorType, lookup_error_code,
        position::{Position, Span},
    },
    runtime::{function_contract::FunctionContract, runtime_type::RuntimeType, value::Value},
    syntax::{
        block::Block,
        effect_expr::EffectExpr,
        expression::{Expression, StringPart},
        interner::Interner,
        module_graph::ModuleKind,
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
mod cfg_bytecode;
mod constructor_info;
mod contracts;
mod effect_rows;
mod errors;
mod expression;
mod hm_expr_typer;
pub mod module_interface;
mod passes;
pub(crate) mod pipeline;
mod statement;
mod suggestions;
pub(crate) mod tail_resumptive;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for diag in diags {
        if diag.phase().is_none() {
            diag.phase = Some(phase);
        }
    }
}

fn merge_effect_summary(current: EffectSummary, observed: EffectSummary) -> EffectSummary {
    match (current, observed) {
        (EffectSummary::HasEffects, _) | (_, EffectSummary::HasEffects) => {
            EffectSummary::HasEffects
        }
        (EffectSummary::Unknown, _) | (_, EffectSummary::Unknown) => EffectSummary::Unknown,
        _ => EffectSummary::Pure,
    }
}

#[derive(Default)]
struct AetherDebugDetails {
    call_sites: Vec<String>,
    dups: Vec<String>,
    drops: Vec<String>,
    reuses: Vec<String>,
}

fn collect_aether_debug_details(
    expr: &crate::core::CoreExpr,
    interner: &Interner,
) -> AetherDebugDetails {
    fn walk(expr: &crate::core::CoreExpr, interner: &Interner, details: &mut AetherDebugDetails) {
        use crate::core::CoreExpr;

        match expr {
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
            CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
                walk(body, interner, details);
            }
            CoreExpr::App { func, args, .. } => {
                walk(func, interner, details);
                for arg in args {
                    walk(arg, interner, details);
                }
            }
            CoreExpr::AetherCall {
                func,
                args,
                arg_modes,
                span,
            } => {
                details.call_sites.push(format!(
                    "line {}: {} [{}]",
                    span.start.line,
                    single_line_expr(func, interner),
                    arg_modes
                        .iter()
                        .map(format_borrow_mode)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                walk(func, interner, details);
                for arg in args {
                    walk(arg, interner, details);
                }
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                walk(rhs, interner, details);
                walk(body, interner, details);
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                for (_, rhs) in bindings {
                    walk(rhs, interner, details);
                }
                walk(body, interner, details);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                walk(scrutinee, interner, details);
                for alt in alts {
                    if let Some(guard) = &alt.guard {
                        walk(guard, interner, details);
                    }
                    walk(&alt.rhs, interner, details);
                }
            }
            CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
                for field in fields {
                    walk(field, interner, details);
                }
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                walk(object, interner, details);
            }
            CoreExpr::Perform { args, .. } => {
                for arg in args {
                    walk(arg, interner, details);
                }
            }
            CoreExpr::Handle { body, handlers, .. } => {
                walk(body, interner, details);
                for handler in handlers {
                    walk(&handler.body, interner, details);
                }
            }
            CoreExpr::Dup { var, body, span } => {
                details.dups.push(format!(
                    "line {}: dup {}",
                    span.start.line,
                    format_var_ref(var, interner)
                ));
                walk(body, interner, details);
            }
            CoreExpr::Drop { var, body, span } => {
                details.drops.push(format!(
                    "line {}: drop {}",
                    span.start.line,
                    format_var_ref(var, interner)
                ));
                walk(body, interner, details);
            }
            CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                span,
            } => {
                details.reuses.push(format!(
                    "line {}: reuse {} as {}{}",
                    span.start.line,
                    format_var_ref(token, interner),
                    tag_label(tag, interner),
                    field_mask
                        .map(|mask| format!(" mask=0x{mask:x}"))
                        .unwrap_or_default()
                ));
                for field in fields {
                    walk(field, interner, details);
                }
            }
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                span,
            } => {
                details.reuses.push(format!(
                    "line {}: drop-specialized {}",
                    span.start.line,
                    format_var_ref(scrutinee, interner)
                ));
                walk(unique_body, interner, details);
                walk(shared_body, interner, details);
            }
        }
    }

    let mut details = AetherDebugDetails::default();
    walk(expr, interner, &mut details);
    details
}

fn render_debug_lines(label: &str, lines: &[String]) -> String {
    let mut out = String::new();
    if lines.is_empty() {
        out.push_str(&format!("  {}: none\n", label));
    } else {
        out.push_str(&format!("  {}:\n", label));
        for line in lines {
            out.push_str(&format!("    - {}\n", line));
        }
    }
    out
}

fn single_line_expr(expr: &crate::core::CoreExpr, interner: &Interner) -> String {
    crate::core::display::display_expr_readable(expr, interner)
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_var_ref(var: &crate::core::CoreVarRef, interner: &Interner) -> String {
    let name = interner
        .try_resolve(var.name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("<sym:{}>", var.name.as_u32()));
    match var.binder {
        Some(binder) => format!("{name}#{}", binder.0),
        None => name,
    }
}

fn tag_label(tag: &crate::core::CoreTag, interner: &Interner) -> String {
    match tag {
        crate::core::CoreTag::Nil => "Nil".to_string(),
        crate::core::CoreTag::Cons => "Cons".to_string(),
        crate::core::CoreTag::None => "None".to_string(),
        crate::core::CoreTag::Some => "Some".to_string(),
        crate::core::CoreTag::Left => "Left".to_string(),
        crate::core::CoreTag::Right => "Right".to_string(),
        crate::core::CoreTag::Named(name) => interner
            .try_resolve(*name)
            .map(str::to_string)
            .unwrap_or_else(|| format!("<sym:{}>", name.as_u32())),
    }
}

fn format_borrow_mode(mode: &crate::aether::borrow_infer::BorrowMode) -> &'static str {
    match mode {
        crate::aether::borrow_infer::BorrowMode::Owned => "Owned",
        crate::aether::borrow_infer::BorrowMode::Borrowed => "Borrowed",
    }
}

fn format_borrow_signature(
    signature: Option<&crate::aether::borrow_infer::BorrowSignature>,
) -> String {
    match signature {
        Some(signature) => format!(
            "[{}] ({})",
            signature
                .params
                .iter()
                .map(format_borrow_mode)
                .collect::<Vec<_>>()
                .join(", "),
            match signature.provenance {
                crate::aether::borrow_infer::BorrowProvenance::Inferred => "Inferred",
                crate::aether::borrow_infer::BorrowProvenance::BaseRuntime => "BaseRuntime",
                crate::aether::borrow_infer::BorrowProvenance::Imported => "Imported",
                crate::aether::borrow_infer::BorrowProvenance::Unknown => "Unknown",
            }
        ),
        None => "<none>".to_string(),
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
pub(super) struct MainValidationState {
    pub(super) has_main: bool,
    pub(super) is_unique_main: bool,
    pub(super) is_valid_signature: bool,
}

/// Compile-time handler scope entry for static handler resolution.
///
/// Tracks an active `handle` block's effect, operations, and whether it's
/// tail-resumptive, enabling `OpPerformDirectIndexed` emission.
pub(super) struct HandlerScope {
    pub effect: Symbol,
    pub is_direct: bool,
    pub ops: Vec<Symbol>,
    /// Local variable indices holding arm closures for evidence-passing.
    /// `evidence_locals[i]` is the local index for `ops[i]`.
    /// `None` when evidence-passing is not applicable (non-TR handler).
    pub evidence_locals: Option<Vec<usize>>,
}

pub struct Compiler {
    constants: Vec<Value>,
    pub symbol_table: SymbolTable,
    pub(super) scopes: Vec<CompilationScope>,
    pub(super) scope_index: usize,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub(super) file_path: String,
    pub(super) current_module_kind: ModuleKind,
    imported_files: HashSet<String>,
    pub(super) file_scope_symbols: HashSet<Symbol>,
    pub(super) imported_modules: HashSet<Symbol>,
    pub(super) import_aliases: HashMap<Symbol, Symbol>,
    pub(super) imported_module_exclusions: HashMap<Symbol, HashSet<Symbol>>,
    /// Maps unqualified member name → qualified "Module.member" symbol
    /// for `import Module exposing (member)` or `exposing (..)`.
    pub(super) exposed_bindings: HashMap<Symbol, Symbol>,
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
    // Annotated function-typed parameter effect rows for active function scopes.
    pub(super) function_param_effect_rows: Vec<HashMap<Symbol, effect_rows::EffectRow>>,
    // Effects currently handled by enclosing `handle ...` scopes.
    pub(super) handled_effects: Vec<Symbol>,
    // Compile-time handler scope stack for static handler resolution.
    // Tracks active handle blocks and their operations for OpPerformDirectIndexed.
    pub(super) handler_scopes: Vec<HandlerScope>,
    // For each active function scope track local indexes captured by nested closures.
    pub(super) captured_local_indices: Vec<HashSet<usize>>,
    // Program-level free-variable analysis result for the latest compile pass.
    pub free_vars: HashSet<Symbol>,
    // Program-level tail-position analysis result for the latest optimized compile pass.
    pub tail_calls: Vec<TailCall>,
    analyze_enabled: bool,
    // Conservative per-block local-use counts used to emit consume-style local reads.
    pub(super) consumable_local_use_counts: Vec<HashMap<Symbol, usize>>,
    pub module_contracts: ModuleContractTable,
    pub module_function_visibility: HashMap<(Symbol, Symbol), bool>,
    pub(super) module_adt_constructors: HashMap<(Symbol, Symbol), Symbol>,
    pub(crate) preloaded_imported_globals: HashSet<Symbol>,
    pub(super) static_type_scopes: Vec<HashMap<Symbol, RuntimeType>>,
    pub(super) effect_alias_scopes: Vec<HashMap<Symbol, Symbol>>,
    pub(super) adt_registry: AdtRegistry,
    pub(super) effect_ops_registry: HashMap<Symbol, HashSet<Symbol>>,
    pub(super) effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>,
    /// HM-inferred type environment, populated before PASS 2 by `infer_program`.
    pub(super) type_env: TypeEnv,
    pub(super) hm_expr_types: HashMap<ExprId, InferType>,
    /// Accumulated HM-inferred type schemes for public module members.
    ///
    /// Persists across `set_file_path()` calls so that downstream modules
    /// can use type schemes from previously-compiled modules. Keyed by
    /// `(module_name, member_name)`.
    pub(super) cached_member_schemes: HashMap<(Symbol, Symbol), Scheme>,
    pub(super) cached_member_borrow_signatures: HashMap<(Symbol, Symbol), BorrowSignature>,
    /// True when HM type inference produced diagnostics. Used to block CFG path
    /// for functions in files with type errors (the Core IR may be degenerate).
    pub(super) has_hm_diagnostics: bool,
    pub(super) last_inferred_program: Option<Program>,
    pub(super) ir_function_symbols: HashMap<FunctionId, Symbol>,
    pub(super) inferred_function_effects: HashMap<ContractKey, HashSet<Symbol>>,
    strict_mode: bool,
    strict_types: bool,
    strict_require_main: bool,
    /// When true, run two-phase inference with type-informed optimization
    /// between Phase 1 and Phase 2 (proposal 0077).
    type_optimize: bool,
    /// When true, emit OpEnterCC at function entry for profiling.
    profiling: bool,
    /// Cost centre metadata accumulated during compilation.
    pub cost_centre_infos: Vec<crate::bytecode::vm::profiling::CostCentreInfo>,
    /// Type class environment — populated during collection phase.
    pub(super) class_env: crate::types::class_env::ClassEnv,
    #[cfg(test)]
    pub(super) hm_infer_runs: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ModuleCacheSnapshot {
    constants_len: usize,
    instructions_len: usize,
    global_definitions_len: usize,
}

struct FinalInferenceResult<'a> {
    final_program: Cow<'a, Program>,
    hm_final: InferProgramResult,
}

#[cfg(test)]
mod compiler_test;

impl Compiler {
    fn is_flow_library_file(&self) -> bool {
        self.current_module_kind == ModuleKind::FlowStdlib
    }

    pub fn new() -> Self {
        Self::new_with_file_path("<unknown>")
    }

    pub fn new_with_file_path(file_path: impl Into<String>) -> Self {
        Self::new_with_interner(file_path, Interner::new())
    }

    pub fn new_with_interner(file_path: impl Into<String>, interner: Interner) -> Self {
        let symbol_table = SymbolTable::new();

        Self {
            constants: Vec::new(),
            symbol_table,
            scopes: vec![CompilationScope::new()],
            scope_index: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            file_path: file_path.into(),
            current_module_kind: ModuleKind::User,
            imported_files: HashSet::new(),
            file_scope_symbols: HashSet::new(),
            imported_modules: HashSet::new(),
            import_aliases: HashMap::new(),
            imported_module_exclusions: HashMap::new(),
            exposed_bindings: HashMap::new(),
            current_module_prefix: None,
            current_span: None,
            // Module Constants
            module_constants: HashMap::new(),
            interner,
            in_tail_position: false,
            function_param_counts: Vec::new(),
            function_effects: Vec::new(),
            function_param_effect_rows: Vec::new(),
            handled_effects: Vec::new(),
            handler_scopes: Vec::new(),
            captured_local_indices: Vec::new(),
            free_vars: HashSet::new(),
            tail_calls: Vec::new(),
            analyze_enabled: false,
            consumable_local_use_counts: Vec::new(),
            module_contracts: HashMap::new(),
            module_function_visibility: HashMap::new(),
            module_adt_constructors: HashMap::new(),
            preloaded_imported_globals: HashSet::new(),
            static_type_scopes: vec![HashMap::new()],
            effect_alias_scopes: vec![HashMap::new()],
            adt_registry: AdtRegistry::new(),
            effect_ops_registry: HashMap::new(),
            effect_op_signatures: HashMap::new(),
            type_env: TypeEnv::new(),
            hm_expr_types: HashMap::new(),
            cached_member_schemes: HashMap::new(),
            cached_member_borrow_signatures: HashMap::new(),
            has_hm_diagnostics: false,
            last_inferred_program: None,
            ir_function_symbols: HashMap::new(),
            inferred_function_effects: HashMap::new(),
            strict_mode: false,
            strict_types: false,
            strict_require_main: true,
            type_optimize: false,
            profiling: false,
            cost_centre_infos: Vec::new(),
            class_env: crate::types::class_env::ClassEnv::new(),
            #[cfg(test)]
            hm_infer_runs: 0,
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

    /// Consumes the compiler and returns persistent state for incremental reuse.
    /// Pairs with `new_with_state()` to bootstrap the next compile iteration.
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
        self.exposed_bindings.clear();
        // Auto-expose all Flow library module members (Proposal 0120).
        // This ensures every compilation unit has access to the Flux stdlib
        // without explicit imports, replacing the old base function registry.
        self.auto_expose_flow_modules();
        self.current_module_prefix = None;
        self.current_span = None;
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.type_env = TypeEnv::new();
        self.hm_expr_types.clear();
        self.last_inferred_program = None;
        self.function_effects.clear();
        self.function_param_effect_rows.clear();
        self.handled_effects.clear();
        self.effect_ops_registry.clear();
        self.effect_op_signatures.clear();
    }

    pub fn set_current_module_kind(&mut self, kind: ModuleKind) {
        self.current_module_kind = kind;
    }

    fn run_hm_infer(&mut self, program: &Program) -> InferProgramResult {
        #[cfg(test)]
        {
            self.hm_infer_runs += 1;
        }
        let hm_config = self.build_infer_config(program);
        infer_program(program, &self.interner, hm_config)
    }

    fn infer_final_program<'a>(
        &mut self,
        program: &'a Program,
    ) -> FinalInferenceResult<'a> {
        let hm = self.run_hm_infer(program);
        let pre_desugar_program = if self.type_optimize {
            Cow::Owned(type_informed_fold(program, &hm.type_env, &self.interner))
        } else {
            Cow::Borrowed(program)
        };
        let hm_pre_desugar = if self.type_optimize {
            self.run_hm_infer(pre_desugar_program.as_ref())
        } else {
            hm
        };
        let pre_desugar_expr_types = hm_pre_desugar.expr_types.clone();
        let desugar_changed_program = !self.is_flow_library_file()
            && operator_desugaring_needed(
                pre_desugar_program.as_ref(),
                &pre_desugar_expr_types,
                &self.interner,
            );
        let final_program = if desugar_changed_program {
            desugar_operators_if_needed(
                pre_desugar_program,
                &pre_desugar_expr_types,
                &mut self.interner,
            )
        } else {
            pre_desugar_program
        };
        #[cfg(test)]
        let desugar_changed_program = desugar_changed_program;
        let hm_final = match &final_program {
            _ if !desugar_changed_program => hm_pre_desugar,
            Cow::Owned(_) | Cow::Borrowed(_) => self.run_hm_infer(final_program.as_ref()),
        };
        FinalInferenceResult {
            final_program,
            hm_final,
        }
    }

    /// Auto-expose all public members of Flow library modules.
    ///
    /// Replaces the old base function registry — every compilation unit
    /// gets unqualified access to `map`, `filter`, `assert_eq`, etc.
    /// from `lib/Flow/*.flx` without needing explicit `import ... exposing`.
    fn auto_expose_flow_modules(&mut self) {
        let flow_prefixes: Vec<&str> = vec![
            "Flow.Option",
            "Flow.List",
            "Flow.String",
            "Flow.Numeric",
            "Flow.IO",
            "Flow.Assert",
        ];
        let skip_flow_auto_expose: Vec<(&str, &str)> = vec![
            // Preserve existing primop behavior for unqualified calls.
            ("Flow.List", "concat"),
            ("Flow.List", "delete"),
            ("Flow.List", "sort"),
            ("Flow.List", "sort_by"),
        ];
        // Collect all public members for Flow modules.
        let entries: Vec<(Symbol, Symbol)> = self
            .module_function_visibility
            .iter()
            .filter(|((mod_name, member), is_public)| {
                **is_public && {
                    let module_name = self.interner.try_resolve(*mod_name).unwrap_or("");
                    let member_name = self.interner.try_resolve(*member).unwrap_or("");
                    flow_prefixes.contains(&module_name)
                        && !skip_flow_auto_expose.contains(&(module_name, member_name))
                }
            })
            .map(|((mod_name, member), _)| (*mod_name, *member))
            .collect();
        for (mod_name, member) in entries {
            let qualified = self.interner.intern_join(mod_name, member);
            self.exposed_bindings.insert(member, qualified);
        }
    }

    pub fn set_strict_mode(&mut self, strict_mode: bool) {
        self.strict_mode = strict_mode;
    }

    pub fn set_strict_types(&mut self, enabled: bool) {
        self.strict_types = enabled;
    }

    pub fn set_profiling(&mut self, enabled: bool) {
        self.profiling = enabled;
    }

    fn register_cost_centre(&mut self, name: &str, module: &str) -> u16 {
        let idx = self.cost_centre_infos.len() as u16;
        self.cost_centre_infos
            .push(crate::bytecode::vm::profiling::CostCentreInfo {
                name: name.to_string(),
                module: module.to_string(),
            });
        idx
    }

    pub fn set_strict_require_main(&mut self, strict_require_main: bool) {
        self.strict_require_main = strict_require_main;
    }

    /// Run HM inference for the provided program and return the final expression type map.
    ///
    /// This is intended for non-bytecode backends that still need the same HM
    /// view of the final AST allocation used during code generation.
    pub fn infer_expr_types_for_program(
        &mut self,
        program: &Program,
    ) -> HashMap<ExprId, InferType> {
        let source_program = program;
        #[cfg(test)]
        {
            self.hm_infer_runs = 0;
        }
        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.imported_module_exclusions.clear();
        self.exposed_bindings.clear();
        self.current_module_prefix = None;
        self.current_span = None;
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.module_contracts.clear();
        self.module_function_visibility.clear();
        self.module_adt_constructors.clear();
        self.type_env = TypeEnv::new();
        self.hm_expr_types.clear();
        self.effect_ops_registry.clear();
        self.effect_op_signatures.clear();

        self.collect_module_function_visibility(program);
        self.collect_module_contracts(program);
        self.collect_effect_declarations(program);
        // Auto-expose Flow library modules so HM can resolve Flow functions.
        self.auto_expose_flow_modules();

        let class_augmented;
        let program = if !self.class_env.classes.is_empty() && !self.is_flow_library_file() {
            let extra = crate::types::class_dispatch::generate_dispatch_functions(
                &program.statements,
                &self.class_env,
                &mut self.interner,
            );
            if extra.is_empty() {
                program
            } else {
                let mut statements = extra;
                statements.extend(program.statements.iter().cloned());
                class_augmented = Program {
                    statements,
                    span: program.span,
                };
                &class_augmented
            }
        } else {
            program
        };

        let final_inference = self.infer_final_program(program);
        let final_program = final_inference.final_program;
        let hm_final = final_inference.hm_final;
        self.type_env = hm_final.type_env;
        self.hm_expr_types = hm_final.expr_types;
        self.last_inferred_program = match final_program {
            Cow::Owned(program) => Some(program),
            Cow::Borrowed(_) if !std::ptr::eq(program, source_program) => Some(program.clone()),
            Cow::Borrowed(_) => None,
        };
        self.hm_expr_types.clone()
    }

    pub fn infer_expr_types_for_module_with_preloaded(
        &mut self,
        program: &Program,
    ) -> HashMap<ExprId, InferType> {
        let source_program = program;
        #[cfg(test)]
        {
            self.hm_infer_runs = 0;
        }
        let preloaded_contracts = self.module_contracts.clone();
        let preloaded_visibility = self.module_function_visibility.clone();
        let preloaded_adt_ctors = self.module_adt_constructors.clone();
        let preloaded_effect_ops = self.effect_ops_registry.clone();
        let preloaded_effect_sigs = self.effect_op_signatures.clone();

        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.imported_module_exclusions.clear();
        self.exposed_bindings.clear();
        self.current_module_prefix = None;
        self.current_span = None;
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.module_contracts = preloaded_contracts;
        self.module_function_visibility = preloaded_visibility;
        self.module_adt_constructors = preloaded_adt_ctors;
        self.type_env = TypeEnv::new();
        self.hm_expr_types.clear();
        self.effect_ops_registry = preloaded_effect_ops;
        self.effect_op_signatures = preloaded_effect_sigs;

        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        self.collect_effect_declarations(program);
        self.auto_expose_flow_modules();

        let class_augmented;
        let program = if !self.class_env.classes.is_empty() && !self.is_flow_library_file() {
            let extra = crate::types::class_dispatch::generate_dispatch_functions(
                &program.statements,
                &self.class_env,
                &mut self.interner,
            );
            if extra.is_empty() {
                program
            } else {
                let mut statements = extra;
                statements.extend(program.statements.iter().cloned());
                class_augmented = Program {
                    statements,
                    span: program.span,
                };
                &class_augmented
            }
        } else {
            program
        };

        let final_inference = self.infer_final_program(program);
        let final_program = final_inference.final_program;
        let hm_final = final_inference.hm_final;
        self.type_env = hm_final.type_env;
        self.hm_expr_types = hm_final.expr_types;
        self.last_inferred_program = match final_program {
            Cow::Owned(program) => Some(program),
            Cow::Borrowed(_) if !std::ptr::eq(program, source_program) => Some(program.clone()),
            Cow::Borrowed(_) => None,
        };
        self.hm_expr_types.clone()
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings)
    }

    pub fn cached_member_schemes(&self) -> &HashMap<(Symbol, Symbol), Scheme> {
        &self.cached_member_schemes
    }

    pub fn preload_module_interface(
        &mut self,
        interface: &crate::types::module_interface::ModuleInterface,
    ) {
        // Build symbol remap: translate serialized Symbol IDs to this session's
        // interner IDs. This is necessary because Symbol is a u32 index into an
        // interner that is session-specific.
        let symbol_remap = interface.build_symbol_remap(&mut self.interner);
        let module_name = self.interner.intern(&interface.module_name);
        for (member_name, scheme) in &interface.schemes {
            let member = self.interner.intern(member_name);
            let qualified = self.interner.intern_join(module_name, member);
            if !self.symbol_table.exists_in_current_scope(qualified) {
                self.symbol_table.define(qualified, Span::default());
            }
            self.preloaded_imported_globals.insert(qualified);
            self.module_function_visibility
                .insert((module_name, member), true);
            let remapped = if symbol_remap.is_empty() {
                scheme.clone()
            } else {
                scheme.remap_symbols(&symbol_remap)
            };
            self.cached_member_schemes
                .insert((module_name, member), remapped);
        }
        for (member_name, signature) in &interface.borrow_signatures {
            let member = self.interner.intern(member_name);
            let qualified = self.interner.intern_join(module_name, member);
            if !self.symbol_table.exists_in_current_scope(qualified) {
                self.symbol_table.define(qualified, Span::default());
            }
            self.preloaded_imported_globals.insert(qualified);
            self.module_function_visibility
                .insert((module_name, member), true);
            self.cached_member_borrow_signatures
                .insert((module_name, member), signature.clone());
        }
    }

    pub fn preload_dependency_program(&mut self, program: &Program) {
        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        self.collect_effect_declarations(program);
    }

    pub fn build_native_extern_symbols(
        &self,
        program: &Program,
    ) -> HashMap<String, crate::lir::lower::ImportedNativeSymbol> {
        use crate::syntax::statement::{ImportExposing, Statement};

        let import_bindings = self.collect_import_module_bindings(program);
        let mut symbols = HashMap::new();
        let flow_prefixes = [
            "Flow.Option",
            "Flow.List",
            "Flow.String",
            "Flow.Numeric",
            "Flow.IO",
            "Flow.Assert",
        ];
        let skip_flow_auto_expose = [("Flow.List", "concat"), ("Flow.List", "delete")];

        for ((module_name, member_name), scheme) in &self.cached_member_schemes {
            let module = self.sym(*module_name);
            let member = self.sym(*member_name);
            if !flow_prefixes.contains(&module) || skip_flow_auto_expose.contains(&(module, member))
            {
                continue;
            }
            symbols.entry(member.to_string()).or_insert_with(|| {
                crate::lir::lower::ImportedNativeSymbol {
                    symbol: format!("flux_{}_{}", module.replace('.', "_"), member),
                    arity: Self::native_function_arity(scheme),
                }
            });
        }

        for (binding, target_module) in import_bindings {
            let binding_name = self.sym(binding);
            let target_name = self.sym(target_module);
            for ((module_name, member_name), scheme) in &self.cached_member_schemes {
                if *module_name != target_module {
                    continue;
                }
                let member = self.sym(*member_name);
                symbols.insert(
                    format!("{binding_name}.{member}"),
                    crate::lir::lower::ImportedNativeSymbol {
                        symbol: format!("flux_{}_{}", target_name.replace('.', "_"), member),
                        arity: Self::native_function_arity(scheme),
                    },
                );
            }
        }

        for statement in &program.statements {
            let Statement::Import {
                name: module_name,
                except,
                exposing,
                ..
            } = statement
            else {
                continue;
            };

            if !except.is_empty() {
                for ((mod_name, member_name), scheme) in &self.cached_member_schemes {
                    if *mod_name != *module_name || except.contains(member_name) {
                        continue;
                    }
                    let member = self.sym(*member_name);
                    symbols.insert(
                        member.to_string(),
                        crate::lir::lower::ImportedNativeSymbol {
                            symbol: format!(
                                "flux_{}_{}",
                                self.sym(*module_name).replace('.', "_"),
                                member
                            ),
                            arity: Self::native_function_arity(scheme),
                        },
                    );
                }
                continue;
            }

            match exposing {
                ImportExposing::None => {}
                ImportExposing::All => {
                    for ((mod_name, member_name), scheme) in &self.cached_member_schemes {
                        if *mod_name == *module_name {
                            let member = self.sym(*member_name);
                            symbols.insert(
                                member.to_string(),
                                crate::lir::lower::ImportedNativeSymbol {
                                    symbol: format!(
                                        "flux_{}_{}",
                                        self.sym(*module_name).replace('.', "_"),
                                        member
                                    ),
                                    arity: Self::native_function_arity(scheme),
                                },
                            );
                        }
                    }
                }
                ImportExposing::Names(names) => {
                    for member_name in names {
                        let member = self.sym(*member_name);
                        if let Some(scheme) = self
                            .cached_member_schemes
                            .get(&(*module_name, *member_name))
                        {
                            symbols.insert(
                                member.to_string(),
                                crate::lir::lower::ImportedNativeSymbol {
                                    symbol: format!(
                                        "flux_{}_{}",
                                        self.sym(*module_name).replace('.', "_"),
                                        member
                                    ),
                                    arity: Self::native_function_arity(scheme),
                                },
                            );
                        }
                    }
                }
            }
        }

        symbols
    }

    pub fn build_preloaded_borrow_registry(&self, program: &Program) -> BorrowRegistry {
        use crate::syntax::statement::ImportExposing;

        let import_bindings = self.collect_import_module_bindings(program);
        let mut registry = BorrowRegistry::default();

        for (binding, target_module) in import_bindings {
            for ((mod_name, member), signature) in &self.cached_member_borrow_signatures {
                if *mod_name != target_module {
                    continue;
                }
                registry
                    .by_member_access
                    .insert((binding, *member), signature.clone());
            }
        }

        for stmt in &program.statements {
            let Statement::Import {
                name: module_name,
                except,
                exposing,
                ..
            } = stmt
            else {
                continue;
            };

            if !except.is_empty() {
                for ((mod_name, member), signature) in &self.cached_member_borrow_signatures {
                    if *mod_name == *module_name && !except.contains(member) {
                        registry.by_name.insert(*member, signature.clone());
                    }
                }
                continue;
            }

            match exposing {
                ImportExposing::None => {}
                ImportExposing::All => {
                    for ((mod_name, member), signature) in &self.cached_member_borrow_signatures {
                        if *mod_name == *module_name {
                            registry.by_name.insert(*member, signature.clone());
                        }
                    }
                }
                ImportExposing::Names(names) => {
                    for member in names {
                        if let Some(signature) = self
                            .cached_member_borrow_signatures
                            .get(&(*module_name, *member))
                        {
                            registry.by_name.insert(*member, signature.clone());
                        }
                    }
                }
            }
        }

        registry
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
                self.adt_registry
                    .register_adt(*name, variants, &self.interner);
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

    fn collect_class_declarations(&mut self, program: &Program) {
        // Register built-in classes first so that `deriving` clauses in the
        // program can reference them (Eq, Ord, Num, Show, Semigroup).
        let mut env = crate::types::class_env::ClassEnv::new();
        env.register_builtins(&mut self.interner);
        let diagnostics = env.collect_from_statements(&program.statements, &self.interner);
        self.class_env = env;
        self.warnings.extend(diagnostics);
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
                            type_params: Statement::function_type_param_names(type_params),
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
            let inferred = TypeEnv::convert_type_expr_rec(
                ty_expr,
                &tp_map,
                interner,
                &mut row_var_env,
                &mut next_var,
            )?;
            param_tys.push(inferred);
        }

        let ret_expr = contract.ret.as_ref()?;
        let ret_ty = TypeEnv::convert_type_expr_rec(
            ret_expr,
            &tp_map,
            interner,
            &mut row_var_env,
            &mut next_var,
        )?;
        let effects =
            InferEffectRow::from_effect_exprs(&contract.effects, &mut row_var_env, &mut next_var)
                .ok()?;

        let infer_type = InferType::Fun(param_tys, Box::new(ret_ty), effects);
        let mut forall = infer_type.free_vars().into_iter().collect::<Vec<_>>();
        forall.sort_unstable();
        forall.dedup();
        Some(Scheme {
            forall,
            constraints: Vec::new(),
            infer_type,
        })
    }

    fn native_function_arity(scheme: &Scheme) -> usize {
        match &scheme.infer_type {
            InferType::Fun(params, _, _) => params.len(),
            _ => 0,
        }
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
            // First populate from cached HM-inferred schemes.
            for ((mod_name, member), scheme) in &self.cached_member_schemes {
                if *mod_name != target_module {
                    continue;
                }
                preloaded.insert((binding, *member), scheme.clone());
            }
            // Then supplement with contract-based schemes (for annotated functions).
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
                // Don't override cached scheme if already present.
                if preloaded.contains_key(&(binding, key.function_name)) {
                    continue;
                }
                if let Some(scheme) = Self::scheme_from_contract(contract, &self.interner) {
                    preloaded.insert((binding, key.function_name), scheme);
                }
            }
        }

        preloaded
    }

    /// Build unqualified type schemes for `exposing` imports.
    ///
    /// Returns a map from unqualified member name → Scheme so HM inference
    /// can resolve exposed names without module qualification.
    fn build_exposed_hm_schemes(&self, program: &Program) -> HashMap<Symbol, Scheme> {
        use crate::syntax::statement::ImportExposing;

        let mut exposed = HashMap::new();

        for stmt in &program.statements {
            let Statement::Import {
                name: module_name,
                except,
                exposing,
                ..
            } = stmt
            else {
                continue;
            };

            let members_to_expose: Vec<Symbol> = if !except.is_empty() {
                self.module_function_visibility
                    .iter()
                    .filter(|((mod_name, member), is_public)| {
                        *mod_name == *module_name && **is_public && !except.contains(member)
                    })
                    .map(|((_, member), _)| *member)
                    .collect()
            } else {
                match exposing {
                    ImportExposing::None => continue,
                    ImportExposing::All => self
                        .module_function_visibility
                        .iter()
                        .filter(|((mod_name, _), is_public)| {
                            *mod_name == *module_name && **is_public
                        })
                        .map(|((_, member), _)| *member)
                        .collect(),
                    ImportExposing::Names(names) => names.clone(),
                }
            };

            for member in members_to_expose {
                // Only expose if public
                // First try cached HM-inferred schemes (direct, no roundtrip).
                if let Some(scheme) = self.cached_member_schemes.get(&(*module_name, member)) {
                    exposed.insert(member, scheme.clone());
                    continue;
                }

                if self.module_function_visibility.get(&(*module_name, member)) != Some(&true) {
                    continue;
                }

                // Fallback to contract-based schemes (for annotated functions).
                for (key, contract) in &self.module_contracts {
                    if key.module_name != Some(*module_name) || key.function_name != member {
                        continue;
                    }
                    if let Some(scheme) = Self::scheme_from_contract(contract, &self.interner) {
                        exposed.insert(member, scheme);
                    }
                }
            }
        }
        exposed
    }

    /// Build the `InferProgramConfig` needed by `infer_program`.
    ///
    /// Collects module member schemes and effect signatures.
    /// Can be called multiple times (e.g. for two-phase inference).
    fn build_infer_config(&mut self, program: &Program) -> InferProgramConfig {
        let preloaded_member_schemes = self.build_preloaded_hm_member_schemes(program);
        let flow_module_symbol = self.interner.intern("Flow");

        // Exposed import schemes are used as unqualified identifiers by HM inference.
        let mut exposed_schemes = self.build_exposed_hm_schemes(program);

        // Inject primop type schemes so HM can resolve types for functions
        // that call primops (e.g., lib/Flow/*.flx functions like read_lines).
        self.inject_primop_hm_schemes(&mut exposed_schemes);

        // Auto-inject all cached Flow module member schemes so that every
        // module has access to Flow functions without explicit imports
        // (like Haskell's implicit Prelude import).
        for ((mod_name, member), scheme) in &self.cached_member_schemes {
            let mod_str = self.interner.resolve(*mod_name);
            if mod_str.starts_with("Flow.") {
                // Only inject if not already present (explicit imports take priority).
                exposed_schemes
                    .entry(*member)
                    .or_insert_with(|| scheme.clone());
            }
        }

        let class_env = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(self.class_env.clone())
        };

        InferProgramConfig {
            file_path: Some(self.file_path.as_str().into()),
            preloaded_base_schemes: exposed_schemes,
            preloaded_module_member_schemes: preloaded_member_schemes,
            known_flow_names: HashSet::new(),
            flow_module_symbol,
            class_env,
            preloaded_effect_op_signatures: self.effect_op_signatures.clone(),
        }
    }

    /// Inject HM type schemes for primops so that HM inference can resolve
    /// types in modules that call primops directly (e.g., `lib/Flow/*.flx`).
    ///
    /// Only injects schemes for names not already present in the map
    /// (module-defined functions take priority over primops).
    fn inject_primop_hm_schemes(&mut self, schemes: &mut HashMap<Symbol, Scheme>) {
        use crate::types::infer_effect_row::InferEffectRow;
        use crate::types::type_constructor::TypeConstructor as TC;

        let io_sym = self.interner.intern("IO");

        // Helper closures for common type patterns.
        let con = |tc: TC| InferType::Con(tc);
        let app = |tc: TC, args: Vec<InferType>| InferType::App(tc, args);
        let fun = |params: Vec<InferType>, ret: InferType, eff: InferEffectRow| -> InferType {
            InferType::Fun(params, Box::new(ret), eff)
        };
        let pure = || InferEffectRow::closed_empty();
        let io = || InferEffectRow::closed_from_symbols(vec![io_sym]);
        // Type variables for polymorphic primop signatures.
        // IDs are arbitrary — schemes are instantiated with fresh vars at each use.
        let var_a = || InferType::Var(9000);
        let var_b = || InferType::Var(9001);
        let var_c = || InferType::Var(9002);

        // (name, params, ret, effects, forall_count)
        let primop_sigs: Vec<(&str, Vec<InferType>, InferType, InferEffectRow, usize)> = vec![
            // I/O
            ("print", vec![var_a()], con(TC::Unit), io(), 0),
            ("println", vec![var_a()], con(TC::Unit), io(), 0),
            ("read_file", vec![con(TC::String)], con(TC::String), io(), 0),
            ("read_stdin", vec![], con(TC::String), io(), 0),
            (
                "read_lines",
                vec![con(TC::String)],
                app(TC::Array, vec![con(TC::String)]),
                io(),
                0,
            ),
            (
                "write_file",
                vec![con(TC::String), con(TC::String)],
                con(TC::Unit),
                io(),
                0,
            ),
            ("panic", vec![var_a()], var_b(), pure(), 2),
            // String ops
            (
                "split",
                vec![con(TC::String), con(TC::String)],
                app(TC::Array, vec![con(TC::String)]),
                pure(),
                0,
            ),
            (
                "join",
                vec![app(TC::Array, vec![con(TC::String)]), con(TC::String)],
                con(TC::String),
                pure(),
                0,
            ),
            ("trim", vec![con(TC::String)], con(TC::String), pure(), 0),
            ("upper", vec![con(TC::String)], con(TC::String), pure(), 0),
            ("lower", vec![con(TC::String)], con(TC::String), pure(), 0),
            (
                "starts_with",
                vec![con(TC::String), con(TC::String)],
                con(TC::Bool),
                pure(),
                0,
            ),
            (
                "ends_with",
                vec![con(TC::String), con(TC::String)],
                con(TC::Bool),
                pure(),
                0,
            ),
            (
                "replace",
                vec![con(TC::String), con(TC::String), con(TC::String)],
                con(TC::String),
                pure(),
                0,
            ),
            (
                "chars",
                vec![con(TC::String)],
                app(TC::Array, vec![con(TC::String)]),
                pure(),
                0,
            ),
            (
                "substring",
                vec![con(TC::String), con(TC::Int), con(TC::Int)],
                con(TC::String),
                pure(),
                0,
            ),
            (
                "str_contains",
                vec![con(TC::String), con(TC::String)],
                con(TC::Bool),
                pure(),
                0,
            ),
            ("to_string", vec![var_a()], con(TC::String), pure(), 0),
            // Numeric
            ("abs", vec![var_a()], var_a(), pure(), 0),
            ("min", vec![var_a(), var_a()], var_a(), pure(), 0),
            ("max", vec![var_a(), var_a()], var_a(), pure(), 0),
            ("parse_int", vec![con(TC::String)], con(TC::Int), pure(), 0),
            (
                "parse_ints",
                vec![app(TC::Array, vec![con(TC::String)])],
                app(TC::Array, vec![con(TC::Int)]),
                pure(),
                0,
            ),
            (
                "split_ints",
                vec![con(TC::String), con(TC::String)],
                app(TC::Array, vec![con(TC::Int)]),
                pure(),
                0,
            ),
            // Collection ops
            ("len", vec![var_a()], con(TC::Int), pure(), 0),
            ("push", vec![var_a(), var_b()], var_a(), pure(), 0),
            ("concat", vec![var_a(), var_a()], var_a(), pure(), 0),
            (
                "slice",
                vec![var_a(), con(TC::Int), con(TC::Int)],
                var_a(),
                pure(),
                0,
            ),
            ("reverse", vec![var_a()], var_a(), pure(), 0),
            ("contains", vec![var_a(), var_b()], con(TC::Bool), pure(), 0),
            (
                "range",
                vec![con(TC::Int), con(TC::Int)],
                app(TC::Array, vec![con(TC::Int)]),
                pure(),
                0,
            ),
            // Type checks
            ("type_of", vec![var_a()], con(TC::String), pure(), 0),
            ("is_int", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_float", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_string", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_bool", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_array", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_none", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_some", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_list", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_hash", vec![var_a()], con(TC::Bool), pure(), 0),
            ("is_map", vec![var_a()], con(TC::Bool), pure(), 0),
            // List ops
            ("to_list", vec![var_a()], var_b(), pure(), 0),
            ("to_array", vec![var_a()], var_b(), pure(), 0),
            // Map ops
            ("keys", vec![var_a()], var_b(), pure(), 0),
            ("values", vec![var_a()], var_b(), pure(), 0),
            ("has_key", vec![var_a(), var_b()], con(TC::Bool), pure(), 0),
            ("merge", vec![var_a(), var_a()], var_a(), pure(), 0),
            ("delete", vec![var_a(), var_b()], var_a(), pure(), 0),
            ("put", vec![var_a(), var_b(), var_c()], var_a(), pure(), 0),
            ("get", vec![var_a(), var_b()], var_c(), pure(), 0),
            // Time
            ("now_ms", vec![], con(TC::Int), pure(), 0),
            (
                "time",
                vec![fun(vec![], var_a(), pure())],
                con(TC::Int),
                pure(),
                0,
            ),
            // Sum/Product
            ("sum", vec![var_a()], var_b(), pure(), 0),
            ("product", vec![var_a()], var_b(), pure(), 0),
            // Safe arithmetic (Proposal 0135)
            (
                "safe_div",
                vec![var_a(), var_a()],
                app(TC::Option, vec![var_a()]),
                pure(),
                0,
            ),
            (
                "safe_mod",
                vec![var_a(), var_a()],
                app(TC::Option, vec![var_a()]),
                pure(),
                0,
            ),
        ];

        for (name, params, ret, effects, _forall) in primop_sigs {
            let sym = self.interner.intern(name);
            // Don't override module-defined functions.
            if schemes.contains_key(&sym) {
                continue;
            }
            let infer_type = fun(params, ret, effects);
            let mut forall = infer_type.free_vars().into_iter().collect::<Vec<_>>();
            forall.sort_unstable();
            forall.dedup();
            schemes.insert(
                sym,
                Scheme {
                    forall,
                    constraints: Vec::new(),
                    infer_type,
                },
            );
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
                    type_params: Statement::function_type_param_names(type_params),
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
            self.inferred_function_effects.clear();
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
                let effects = self.infer_effects_from_block(
                    &seed.body,
                    seed.module_name,
                    &inferred,
                    io_effect,
                    time_effect,
                );
                let mut combined_effects = seed.declared_effects.clone();
                combined_effects.extend(effects);
                let entry = inferred.entry(seed.key.clone()).or_default();
                if *entry != combined_effects {
                    *entry = combined_effects;
                    changed = true;
                }
            }
        }

        self.inferred_function_effects = inferred.clone();

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
        &mut self,
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
        &mut self,
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
        &mut self,
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
        &mut self,
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
                        effects.extend(
                            self.resolve_call_effect_row_with_args(
                                found,
                                self.lookup_contract(Some(module_name), *name, arity)
                                    .cloned(),
                                arguments,
                            ),
                        );
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
                            self.lookup_unqualified_contract(*name, arity).cloned(),
                            arguments,
                        ));
                        resolved = true;
                    }
                }
                if !resolved {
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
                        effects.extend(
                            self.resolve_call_effect_row_with_args(
                                found,
                                self.lookup_contract(Some(module_name), *member, arity)
                                    .cloned(),
                                arguments,
                            ),
                        );
                    }
                }
            }
            _ => {}
        }
        effects
    }

    fn resolve_call_effect_row_with_args(
        &mut self,
        raw_effects: &HashSet<Symbol>,
        contract: Option<FnContract>,
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
        let required = EffectRow::from_effect_exprs(&effects_as_expr);

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

                let expected = EffectRow::from_effect_exprs(param_effects);
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
        &mut self,
        argument: &Expression,
        expected_arity: usize,
        inferred_effects: &HashSet<Symbol>,
        call_arguments: &[Expression],
    ) -> Option<crate::bytecode::compiler::effect_rows::EffectRow> {
        use crate::bytecode::compiler::effect_rows::EffectRow;

        match argument {
            Expression::Function { effects, .. } => Some(EffectRow::from_effect_exprs(effects)),
            Expression::Identifier { name, .. } => {
                if let Some(local) = self.current_function_param_effect_row(*name) {
                    return Some(local);
                }

                self.lookup_unqualified_contract(*name, expected_arity)
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
                        EffectRow::from_effect_exprs(&effect_exprs)
                    })
                    .or_else(|| self.infer_argument_effect_row_from_hm(argument))
            }
            Expression::MemberAccess { object, member, .. } => self
                .resolve_module_name_from_expr(object)
                .and_then(|module| self.lookup_contract(Some(module), *member, expected_arity))
                .map(|contract| EffectRow::from_effect_exprs(&contract.effects))
                .or_else(|| self.infer_argument_effect_row_from_hm(argument)),
            _ => {
                let _ = call_arguments;
                self.infer_argument_effect_row_from_hm(argument)
            }
        }
    }

    fn infer_argument_effect_row_from_hm(&mut self, argument: &Expression) -> Option<EffectRow> {
        let HmExprTypeResult::Known(InferType::Fun(_, _, effects)) =
            self.hm_expr_type_strict_path(argument)
        else {
            return None;
        };

        let mut row = EffectRow::default();
        row.atoms.extend(effects.concrete().iter().copied());
        if let Some(tail) = effects.tail() {
            let synthetic = self.interner.intern(&format!("__hm_row_{tail}"));
            row.vars.insert(synthetic);
        }
        Some(row)
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
                    .with_category(DiagnosticCategory::ModuleSystem)
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
                    .with_category(DiagnosticCategory::ModuleSystem)
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
                    .with_category(DiagnosticCategory::ModuleSystem)
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
                .with_category(DiagnosticCategory::ModuleSystem)
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
                    .with_category(DiagnosticCategory::ModuleSystem)
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
            .with_category(DiagnosticCategory::Effects)
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
                .with_category(DiagnosticCategory::ModuleSystem)
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
                            .with_category(DiagnosticCategory::ModuleSystem)
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
                            .with_category(DiagnosticCategory::ModuleSystem)
                            .with_primary_label(*span, "missing return type annotation"),
                        );
                    }
                }

                let missing_effects = self.strict_missing_ambient_effects(
                    module_name,
                    *name,
                    parameters.len(),
                    effects,
                );
                for effect_name in missing_effects {
                    let effect_text = self.sym(effect_name).to_string();
                    self.errors.push(
                        Diagnostic::make_error_dynamic(
                            "E418",
                            "STRICT EFFECT ANNOTATION REQUIRED",
                            ErrorType::Compiler,
                            format!(
                                "Effectful function `{}` must declare `with {}` in strict mode.",
                                self.sym(*name),
                                effect_text,
                            ),
                            Some(format!(
                                "Add explicit `with {}` to the function signature.",
                                effect_text
                            )),
                            self.file_path.clone(),
                            *span,
                        )
                        .with_category(DiagnosticCategory::ModuleSystem)
                        .with_primary_label(*span, "missing explicit effect annotation"),
                    );
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

    fn strict_missing_ambient_effects(
        &self,
        module_name: Option<Symbol>,
        function_name: Symbol,
        arity: usize,
        declared_effects: &[EffectExpr],
    ) -> Vec<Symbol> {
        let key = ContractKey {
            module_name,
            function_name,
            arity,
        };
        let inferred = self
            .inferred_function_effects
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if inferred.is_empty() {
            return Vec::new();
        }

        let declared: HashSet<Symbol> = declared_effects
            .iter()
            .flat_map(EffectExpr::normalized_names)
            .collect();
        let mut missing = Vec::new();

        for effect in inferred {
            let name = self.sym(effect);
            if matches!(name, "IO" | "Time") && !declared.contains(&effect) {
                missing.push(effect);
            }
        }

        missing.sort_by_key(|effect| self.sym(*effect).to_string());
        missing
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
            .with_display_title("Strict Any Type")
            .with_category(DiagnosticCategory::TypeInference)
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
        }
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
                self.analyze_enabled = true;
                self.tail_calls.clear();
            } else {
                self.free_vars.clear();
                self.tail_calls.clear();
                self.analyze_enabled = false;
            }

            // Enable two-phase inference with type-informed optimization (proposal 0077).
            self.type_optimize = true;
            self.compile(&program_to_compile)
        } else {
            // Borrow the original program directly for non-optimized paths.
            if analyze {
                self.free_vars = collect_free_vars_in_program(program);
                self.analyze_enabled = true;
                self.tail_calls.clear();
            } else {
                self.free_vars.clear();
                self.tail_calls.clear();
                self.analyze_enabled = false;
            }
            self.compile(program)
        }
    }

    /// Render the Core IR for the same AST shape consumed by the current
    /// compile configuration. Call this after a successful `compile_with_opts`.
    #[allow(clippy::result_large_err)]
    pub fn dump_core_with_opts(
        &self,
        program: &Program,
        optimize: bool,
        mode: crate::core::display::CoreDisplayMode,
    ) -> Result<String, Diagnostic> {
        let program_to_lower = if !optimize {
            self.last_inferred_program
                .clone()
                .unwrap_or_else(|| program.clone())
        } else {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            rename(optimized, HashMap::new())
        };

        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            &program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            None,
            None,
            class_env_ref,
        );

        // Dictionary elaboration (Proposal 0145, Step 5b):
        // Emit __dict_* CoreDefs and rewrite constrained function bodies.
        if !self.class_env.classes.is_empty() {
            let mut max_id: u32 = 0;
            for def in &core.defs {
                max_id = max_id.max(def.binder.id.0);
            }
            let mut next_id = max_id + 1;
            crate::core::passes::elaborate_dictionaries(
                &mut core,
                &self.class_env,
                &self.type_env,
                &self.interner,
                &mut next_id,
            );
        }

        let preloaded_registry = self.build_preloaded_borrow_registry(&program_to_lower);
        crate::core::passes::run_core_passes_with_interner_and_registry(
            &mut core,
            &self.interner,
            optimize,
            preloaded_registry,
        )?;

        // Collect Aether stats across all definitions
        let mut total_stats = crate::aether::AetherStats::default();
        for def in &core.defs {
            let s = crate::aether::collect_stats(&def.expr);
            total_stats.dups += s.dups;
            total_stats.drops += s.drops;
            total_stats.reuses += s.reuses;
            total_stats.drop_specs += s.drop_specs;
            total_stats.allocs += s.allocs;
        }

        let ir_text = match mode {
            crate::core::display::CoreDisplayMode::Readable => {
                crate::core::display::display_program_readable(&core, &self.interner)
            }
            crate::core::display::CoreDisplayMode::Debug => {
                crate::core::display::display_program_debug(&core, &self.interner)
            }
        };

        if total_stats.dups > 0 || total_stats.drops > 0 || total_stats.reuses > 0 {
            Ok(format!("{}\n── Aether stats ──\n{}", ir_text, total_stats))
        } else {
            Ok(ir_text)
        }
    }

    /// Lower to Core IR, then to LIR, and return a human-readable dump.
    #[allow(clippy::result_large_err)]
    pub fn dump_lir(&self, program: &Program, optimize: bool) -> Result<String, Diagnostic> {
        let program_to_lower = if !optimize {
            self.last_inferred_program
                .clone()
                .unwrap_or_else(|| program.clone())
        } else {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            rename(optimized, HashMap::new())
        };

        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            &program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            None,
            None,
            class_env_ref,
        );
        let preloaded_registry = self.build_preloaded_borrow_registry(&program_to_lower);
        crate::core::passes::run_core_passes_with_interner_and_registry(
            &mut core,
            &self.interner,
            optimize,
            preloaded_registry,
        )?;

        let globals_map = self.build_globals_map();
        let lir = crate::lir::lower::lower_program_with_interner(
            &core,
            Some(&self.interner),
            Some(&globals_map),
        );
        Ok(crate::lir::lower::display_program(&lir))
    }

    /// Lower program through LIR to an LLVM IR module (Proposal 0132 Phase 7).
    /// Returns the `LlvmModule` struct so the caller can inject target triple
    /// and data layout before rendering.
    #[cfg(feature = "core_to_llvm")]
    #[allow(clippy::result_large_err)]
    pub fn lower_to_lir_llvm_module(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::core_to_llvm::LlvmModule, Diagnostic> {
        let program_to_lower = if optimize {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            rename(optimized, HashMap::new())
        } else {
            program.clone()
        };

        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            &program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            None,
            None,
            class_env_ref,
        );
        let preloaded_registry = self.build_preloaded_borrow_registry(&program_to_lower);
        crate::core::passes::run_core_passes_with_interner_and_registry(
            &mut core,
            &self.interner,
            optimize,
            preloaded_registry,
        )?;

        // Pass None for globals_map so ALL functions are lowered to LIR
        // functions (no GetGlobal). In native mode there's no VM globals
        // table, so every function must be compiled into the LLVM module.
        let lir = crate::lir::lower::lower_program_with_interner(&core, Some(&self.interner), None);
        Ok(crate::lir::emit_llvm::emit_llvm_module(&lir))
    }

    /// Lower a single module through LIR to an LLVM IR module while resolving
    /// imported public functions as external symbols rather than merged-program
    /// local binders.
    #[cfg(feature = "core_to_llvm")]
    #[allow(clippy::result_large_err)]
    pub fn lower_to_lir_llvm_module_per_module(
        &mut self,
        program: &Program,
        optimize: bool,
        export_user_ctor_name_helper: bool,
    ) -> Result<crate::core_to_llvm::LlvmModule, Diagnostic> {
        let program_to_lower = if !optimize {
            self.last_inferred_program
                .clone()
                .unwrap_or_else(|| program.clone())
        } else {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            rename(optimized, HashMap::new())
        };

        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            &program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            None,
            None,
            class_env_ref,
        );
        let preloaded_registry = self.build_preloaded_borrow_registry(&program_to_lower);
        crate::core::passes::run_core_passes_with_interner_and_registry(
            &mut core,
            &self.interner,
            optimize,
            preloaded_registry,
        )?;

        let extern_symbols = self.build_native_extern_symbols(&program_to_lower);
        let emit_main = program_to_lower.statements.iter().any(|statement| {
            matches!(
                statement,
                Statement::Function { name, .. } if self.sym(*name) == "main"
            )
        });
        // Derive an entry qualifier from the file path to prevent symbol
        // collisions with C runtime primops. E.g. "examples/day06.flx"
        // yields qualifier "day06", so user's `fn sum` becomes `flux_day06_sum`
        // instead of `flux_sum` (which clashes with libflux_rt.a).
        let entry_qualifier = std::path::Path::new(&self.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.replace(['-', '.', ' '], "_"));
        let lir = crate::lir::lower::lower_program_with_interner_and_externs(
            &core,
            Some(&self.interner),
            None,
            Some(&extern_symbols),
            emit_main,
            entry_qualifier.as_deref(),
        );
        Ok(crate::lir::emit_llvm::emit_llvm_module_with_options(
            &lir,
            false,
            export_user_ctor_name_helper,
        ))
    }

    /// Dump LIR as LLVM IR text (Proposal 0132 Phase 7).
    #[cfg(feature = "core_to_llvm")]
    #[allow(clippy::result_large_err)]
    pub fn dump_lir_llvm(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<String, Diagnostic> {
        let module = self.lower_to_lir_llvm_module(program, optimize)?;
        Ok(crate::core_to_llvm::render_module(&module))
    }

    fn build_globals_map(&self) -> HashMap<String, usize> {
        self.build_globals_map_with_aliases(&[])
    }

    /// Build a string-name → global index map from the compiler's symbol table.
    /// Maps both qualified ("Flow.List.map") and unqualified ("map") names so
    /// the LIR lowerer can resolve external variables regardless of naming.
    /// `extra_aliases` are (alias, module) pairs from the entry module's imports
    /// that weren't processed via CFG compilation.
    fn build_globals_map_with_aliases(
        &self,
        extra_aliases: &[(String, String)],
    ) -> HashMap<String, usize> {
        // Build reverse alias map: "Flow.Array" → ["Array"]
        let mut module_aliases: HashMap<String, Vec<String>> = HashMap::new();
        for (alias_sym, target_sym) in &self.import_aliases {
            let alias = self
                .interner
                .resolve(crate::syntax::Identifier::from(*alias_sym))
                .to_string();
            let target = self
                .interner
                .resolve(crate::syntax::Identifier::from(*target_sym))
                .to_string();
            module_aliases.entry(target).or_default().push(alias);
        }
        for (alias, target) in extra_aliases {
            module_aliases
                .entry(target.clone())
                .or_default()
                .push(alias.clone());
        }

        let mut map = HashMap::new();
        // Sort by global index so later-compiled modules (higher indices)
        // shadow earlier ones for unqualified names. This ensures Flow.Array's
        // `first`/`last`/`contains`/`reverse` shadow Flow.List's versions.
        let mut globals = self.symbol_table.global_definitions();
        globals.sort_by_key(|&(_, idx)| idx);
        for (sym, idx) in globals {
            let name = self
                .interner
                .resolve(crate::syntax::Identifier::from(sym))
                .to_string();
            // Add qualified name (e.g. "Flow.Array.sort")
            map.insert(name.clone(), idx);
            // Add unqualified name (last segment after last '.').
            // Always overwrite: later-compiled modules (e.g. Flow.Array)
            // shadow earlier ones (e.g. Flow.List) for the same unqualified
            // name, matching the CFG compiler's resolution order.
            if let Some(short) = name.rsplit('.').next()
                && short != name
            {
                map.insert(short.to_string(), idx);
            }
            // Add alias-qualified names (e.g. "Array.sort" for "Flow.Array.sort"
            // when "import Flow.Array as Array" is in effect).
            for (module_prefix, aliases) in &module_aliases {
                if let Some(suffix) = name.strip_prefix(module_prefix)
                    && let Some(suffix) = suffix.strip_prefix('.')
                {
                    for alias in aliases {
                        map.entry(format!("{alias}.{suffix}")).or_insert(idx);
                    }
                }
            }
        }
        map
    }

    #[allow(clippy::result_large_err)]
    pub fn lower_aether_report_program(
        &self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::core::CoreProgram, Diagnostic> {
        let program_to_lower = if !optimize {
            self.last_inferred_program
                .clone()
                .unwrap_or_else(|| program.clone())
        } else {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            rename(optimized, HashMap::new())
        };

        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            &program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            None,
            None,
            class_env_ref,
        );
        let preloaded_registry = self.build_preloaded_borrow_registry(&program_to_lower);
        crate::core::passes::run_core_passes_with_interner_and_registry(
            &mut core,
            &self.interner,
            optimize,
            preloaded_registry,
        )?;
        Ok(core)
    }

    /// Render an Aether memory model report showing per-function optimization decisions.
    #[allow(clippy::result_large_err)]
    pub fn render_aether_report(
        &self,
        program: &Program,
        optimize: bool,
        debug: bool,
    ) -> Result<String, Diagnostic> {
        let core = self.lower_aether_report_program(program, optimize)?;
        let fbip_diags = crate::aether::check_fbip::check_fbip(&core, &self.interner);
        let fbip_by_name = fbip_diags
            .diagnostics
            .iter()
            .map(|diag| (diag.function_name.as_str(), diag))
            .collect::<HashMap<_, _>>();

        let mut out = String::new();
        out.push_str("Aether Memory Model Report\n");
        out.push_str("==========================\n\n");

        let mut total = crate::aether::AetherStats::default();

        for def in &core.defs {
            let stats = crate::aether::collect_stats(&def.expr);
            if stats.dups == 0
                && stats.drops == 0
                && stats.reuses == 0
                && stats.drop_specs == 0
                && def.fip.is_none()
            {
                continue;
            }
            let name = self.interner.resolve(def.name);
            let fip_label = match def.fip {
                Some(crate::syntax::statement::FipAnnotation::Fip) => " @fip",
                Some(crate::syntax::statement::FipAnnotation::Fbip) => " @fbip",
                None => "",
            };
            out.push_str(&format!("── fn {}{} ──\n", name, fip_label));
            out.push_str(&format!("  {}\n", stats));
            out.push_str(&format!("  FreshAllocs: {}\n", stats.allocs));

            let verify_errors = crate::aether::verify::verify_contract(&def.expr)
                .err()
                .unwrap_or_default();
            let verify_diags = crate::aether::verify::verify_diagnostics(&def.expr);
            if verify_errors.is_empty() && verify_diags.is_empty() {
                out.push_str("  verifier: ok\n");
            } else {
                out.push_str("  verifier:\n");
                for err in verify_errors {
                    out.push_str(&format!("    - {:?}: {}\n", err.kind, err.message));
                }
                for diag in verify_diags {
                    out.push_str(&format!("    - {:?}: {}\n", diag.kind, diag.message));
                }
            }

            if let Some(fbip_diag) = fbip_by_name.get(name) {
                let fbip_status = if fbip_diag
                    .reasons
                    .contains(&crate::aether::fbip_analysis::FbipFailureReason::NoConstructors)
                    && !matches!(
                        fbip_diag.outcome,
                        crate::aether::fbip_analysis::FbipOutcome::NotProvable
                    ) {
                    "vacuous".to_string()
                } else {
                    match &fbip_diag.outcome {
                        crate::aether::fbip_analysis::FbipOutcome::Fip => "proved (fip)".into(),
                        crate::aether::fbip_analysis::FbipOutcome::Fbip { bound } => {
                            format!("proved (fbip({bound}))")
                        }
                        crate::aether::fbip_analysis::FbipOutcome::NotProvable => {
                            "NotProvable".into()
                        }
                    }
                };
                out.push_str(&format!("  fbip: {}\n", fbip_status));
                for detail in &fbip_diag.details {
                    out.push_str(&format!("    - {}\n", detail));
                }
            }

            if debug {
                out.push_str(&format!(
                    "  borrow signature: {}\n",
                    format_borrow_signature(def.borrow_signature.as_ref())
                ));

                let debug_details = collect_aether_debug_details(&def.expr, &self.interner);
                out.push_str(&render_debug_lines("call sites", &debug_details.call_sites));
                out.push_str(&render_debug_lines("dups", &debug_details.dups));
                out.push_str(&render_debug_lines("drops", &debug_details.drops));
                out.push_str(&render_debug_lines("reuse", &debug_details.reuses));
            }

            let displayed = crate::core::display::display_expr_readable(&def.expr, &self.interner);
            out.push_str(&displayed);
            out.push('\n');

            total.dups += stats.dups;
            total.drops += stats.drops;
            total.reuses += stats.reuses;
            total.drop_specs += stats.drop_specs;
            total.allocs += stats.allocs;
        }

        out.push_str(&format!(
            "\n── Total ──\n  {}\n  FreshAllocs: {}\n",
            total, total.allocs
        ));
        Ok(out)
    }

    /// Dump an Aether memory model report showing per-function optimization decisions.
    #[allow(clippy::result_large_err)]
    pub fn dump_aether_report(
        &self,
        program: &Program,
        optimize: bool,
        debug: bool,
    ) -> Result<String, Diagnostic> {
        self.render_aether_report(program, optimize, debug)
    }

    pub fn compile(&mut self, program: &Program) -> Result<(), Vec<Diagnostic>> {
        self.run_pipeline(program)
    }

    fn register_ir_function_symbols_from_backend(&mut self, functions: &[IrFunction]) {
        for function in functions {
            if let Some(name) = function.name {
                self.ir_function_symbols.insert(function.id, name);
            }
        }
    }

    pub(super) fn lookup_ir_function_symbol_by_raw_id(&self, raw_id: u32) -> Option<Symbol> {
        self.ir_function_symbols
            .iter()
            .find_map(|(function_id, symbol)| (function_id.0 == raw_id).then_some(*symbol))
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

    pub fn module_cache_snapshot(&self) -> ModuleCacheSnapshot {
        ModuleCacheSnapshot {
            constants_len: self.constants.len(),
            instructions_len: self.scopes[self.scope_index].instructions.len(),
            global_definitions_len: self.symbol_table.global_bindings().len(),
        }
    }

    pub fn build_cached_module_bytecode(
        &self,
        snapshot: ModuleCacheSnapshot,
    ) -> CachedModuleBytecode {
        let scope = &self.scopes[self.scope_index];
        let globals = self
            .symbol_table
            .global_bindings()
            .into_iter()
            .skip(snapshot.global_definitions_len)
            .map(|binding| CachedModuleBinding {
                name: self.sym(binding.name).to_string(),
                index: binding.index,
                span: binding.span,
                is_assigned: binding.is_assigned,
                kind: if self.preloaded_imported_globals.contains(&binding.name) {
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Imported
                } else {
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Defined
                },
            })
            .collect();

        let relative_locations = scope
            .locations
            .iter()
            .filter(|location| location.offset >= snapshot.instructions_len)
            .map(|location| InstructionLocation {
                offset: location.offset - snapshot.instructions_len,
                location: location.location.clone(),
            })
            .collect();

        CachedModuleBytecode {
            globals,
            constants: self.constants[snapshot.constants_len..].to_vec(),
            instructions: scope.instructions[snapshot.instructions_len..].to_vec(),
            debug_info: FunctionDebugInfo::new(None, scope.files.clone(), relative_locations)
                .with_effect_summary(scope.effect_summary),
        }
    }

    pub fn build_relocatable_module_bytecode(&self) -> CachedModuleBytecode {
        let scope = &self.scopes[self.scope_index];
        let globals = self
            .symbol_table
            .global_bindings()
            .into_iter()
            .map(|binding| CachedModuleBinding {
                name: self.sym(binding.name).to_string(),
                index: binding.index,
                span: binding.span,
                is_assigned: binding.is_assigned,
                kind: if self.preloaded_imported_globals.contains(&binding.name) {
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Imported
                } else {
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Defined
                },
            })
            .collect();

        CachedModuleBytecode {
            globals,
            constants: self.constants.clone(),
            instructions: scope.instructions.clone(),
            debug_info: FunctionDebugInfo::new(None, scope.files.clone(), scope.locations.clone())
                .with_effect_summary(scope.effect_summary),
        }
    }

    pub fn hydrate_cached_module_bytecode(&mut self, cached: &CachedModuleBytecode) {
        for binding in &cached.globals {
            let symbol = self.interner.intern(&binding.name);
            self.symbol_table.define_global_with_index(
                symbol,
                binding.index,
                binding.span,
                binding.is_assigned,
            );
            if matches!(
                binding.kind,
                crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Imported
            ) {
                self.preloaded_imported_globals.insert(symbol);
            }
            self.file_scope_symbols.insert(symbol);
        }

        self.constants.extend(cached.constants.iter().cloned());

        let base_offset = self.scopes[self.scope_index].instructions.len();
        self.scopes[self.scope_index]
            .instructions
            .extend_from_slice(&cached.instructions);

        let mut file_id_map = HashMap::new();
        for (source_id, file) in cached.debug_info.files.iter().enumerate() {
            let target_id = self.ensure_scope_file(file) as u32;
            file_id_map.insert(source_id as u32, target_id);
        }

        for location in &cached.debug_info.locations {
            let remapped =
                location
                    .location
                    .as_ref()
                    .map(|entry| crate::bytecode::debug_info::Location {
                        file_id: file_id_map
                            .get(&entry.file_id)
                            .copied()
                            .unwrap_or(entry.file_id),
                        span: entry.span,
                    });
            self.scopes[self.scope_index]
                .locations
                .push(InstructionLocation {
                    offset: base_offset + location.offset,
                    location: remapped,
                });
        }

        self.scopes[self.scope_index].effect_summary = merge_effect_summary(
            self.scopes[self.scope_index].effect_summary,
            cached.debug_info.effect_summary,
        );
        self.recompute_last_instructions();
    }

    pub fn imported_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.imported_files.iter().cloned().collect();
        files.sort();
        files
    }

    pub(super) fn current_instructions(&self) -> &Instructions {
        &self.scopes[self.scope_index].instructions
    }

    fn ensure_scope_file(&mut self, file: &str) -> usize {
        let files = &mut self.scopes[self.scope_index].files;
        if let Some((index, _)) = files
            .iter()
            .enumerate()
            .find(|(_, existing)| existing == &file)
        {
            index
        } else {
            files.push(file.to_string());
            files.len() - 1
        }
    }

    fn recompute_last_instructions(&mut self) {
        let instructions = &self.scopes[self.scope_index].instructions;
        let mut previous = EmittedInstruction::default();
        let mut last = EmittedInstruction::default();
        let mut ip = 0;

        while ip < instructions.len() {
            previous = last.clone();
            let op = OpCode::from(instructions[ip]);
            last = EmittedInstruction {
                opcode: Some(op),
                position: ip,
            };
            ip += 1 + crate::bytecode::op_code::operand_widths(op)
                .iter()
                .sum::<usize>();
        }

        self.scopes[self.scope_index].previous_instruction = previous;
        self.scopes[self.scope_index].last_instruction = last;
    }

    fn instruction_len(op: OpCode) -> usize {
        1 + crate::bytecode::op_code::operand_widths(op)
            .iter()
            .sum::<usize>()
    }

    fn previous_instruction_before(&self, target_pos: usize) -> Option<(usize, OpCode)> {
        let instructions = &self.scopes[self.scope_index].instructions;
        let mut ip = 0;
        let mut previous = None;

        while ip < instructions.len() {
            let op = OpCode::from(instructions[ip]);
            if ip == target_pos {
                return previous;
            }
            previous = Some((ip, op));
            ip += Self::instruction_len(op);
        }

        None
    }

    fn decode_local_read_at(&self, pos: usize) -> Option<(usize, usize)> {
        let instructions = &self.scopes[self.scope_index].instructions;
        let op = OpCode::from(instructions[pos]);
        match op {
            OpCode::OpGetLocal => Some((instructions[pos + 1] as usize, 2)),
            OpCode::OpGetLocal0 => Some((0, 1)),
            OpCode::OpGetLocal1 => Some((1, 1)),
            _ => None,
        }
    }

    fn decode_get_local_get_local_at(&self, pos: usize) -> Option<(usize, usize)> {
        let instructions = &self.scopes[self.scope_index].instructions;
        if OpCode::from(instructions[pos]) == OpCode::OpGetLocalGetLocal {
            Some((
                instructions[pos + 1] as usize,
                instructions[pos + 2] as usize,
            ))
        } else {
            None
        }
    }

    fn can_fuse_trailing_region(&self, start: usize, new_len: usize) -> bool {
        let old_len = self.scopes[self.scope_index].instructions.len() - start;
        if new_len > old_len {
            return false;
        }
        // Check all interior positions: both operand bytes of the fused instruction
        // AND removed bytes. A jump target that previously pointed to the start of
        // a constituent instruction would land on an operand byte after fusion.
        for pos in start + 1..start + old_len {
            if self.has_jump_target_at(pos) {
                return false;
            }
        }
        true
    }

    fn rewrite_trailing_region(&mut self, start: usize, new_instruction: Instructions) {
        let first_location = self.scopes[self.scope_index]
            .locations
            .iter()
            .find(|location| location.offset == start)
            .and_then(|location| location.location.clone());

        self.scopes[self.scope_index].instructions.truncate(start);
        self.scopes[self.scope_index]
            .instructions
            .extend_from_slice(&new_instruction);
        self.scopes[self.scope_index]
            .locations
            .retain(|location| location.offset < start);
        self.scopes[self.scope_index]
            .locations
            .push(InstructionLocation {
                offset: start,
                location: first_location,
            });
        self.recompute_last_instructions();
    }

    fn try_fuse_trailing_superinstructions(&mut self) {
        while self.try_fuse_trailing_superinstruction_once() {}
    }

    fn try_fuse_trailing_superinstruction_once(&mut self) -> bool {
        self.try_fuse_trailing_add_sub_locals()
            || self.try_fuse_trailing_constant_add()
            || self.try_fuse_trailing_local_is_adt()
            || self.try_fuse_trailing_set_local_pop()
            || self.try_fuse_trailing_call_arity()
            || self.try_fuse_trailing_tail_call1()
            || self.try_fuse_trailing_get_local_get_local()
    }

    fn try_fuse_trailing_add_sub_locals(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        let fused_op = match last.opcode {
            Some(OpCode::OpAdd) => OpCode::OpAddLocals,
            Some(OpCode::OpSub) => OpCode::OpSubLocals,
            _ => return false,
        };
        let last_pos = last.position;

        if let Some((prev_pos, _)) = self.previous_instruction_before(last_pos) {
            if let Some((a, b)) = self.decode_get_local_get_local_at(prev_pos) {
                let new_instruction = make(fused_op, &[a, b]);
                if self.can_fuse_trailing_region(prev_pos, new_instruction.len()) {
                    self.rewrite_trailing_region(prev_pos, new_instruction);
                    return true;
                }
            }

            if let Some((b, len_b)) = self.decode_local_read_at(prev_pos)
                && let Some((prev_prev_pos, _)) = self.previous_instruction_before(prev_pos)
                && let Some((a, len_a)) = self.decode_local_read_at(prev_prev_pos)
                && prev_prev_pos + len_a == prev_pos
                && prev_pos + len_b == last_pos
            {
                let new_instruction = make(fused_op, &[a, b]);
                if self.can_fuse_trailing_region(prev_prev_pos, new_instruction.len()) {
                    self.rewrite_trailing_region(prev_prev_pos, new_instruction);
                    return true;
                }
            }
        }

        false
    }

    fn try_fuse_trailing_constant_add(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        if last.opcode != Some(OpCode::OpAdd) {
            return false;
        }
        let Some((prev_pos, prev_op)) = self.previous_instruction_before(last.position) else {
            return false;
        };
        if prev_op != OpCode::OpConstant {
            return false;
        }
        let const_idx =
            crate::bytecode::op_code::read_u16(&scope.instructions, prev_pos + 1) as usize;
        let new_instruction = make(OpCode::OpConstantAdd, &[const_idx]);
        if !self.can_fuse_trailing_region(prev_pos, new_instruction.len()) {
            return false;
        }
        self.rewrite_trailing_region(prev_pos, new_instruction);
        true
    }

    fn try_fuse_trailing_local_is_adt(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        if last.opcode != Some(OpCode::OpIsAdt) {
            return false;
        }
        let Some((prev_pos, _)) = self.previous_instruction_before(last.position) else {
            return false;
        };
        let Some((local_idx, _)) = self.decode_local_read_at(prev_pos) else {
            return false;
        };
        let const_idx =
            crate::bytecode::op_code::read_u16(&scope.instructions, last.position + 1) as usize;
        let new_instruction = make(OpCode::OpGetLocalIsAdt, &[local_idx, const_idx]);
        if !self.can_fuse_trailing_region(prev_pos, new_instruction.len()) {
            return false;
        }
        self.rewrite_trailing_region(prev_pos, new_instruction);
        true
    }

    fn try_fuse_trailing_set_local_pop(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        if last.opcode != Some(OpCode::OpPop) {
            return false;
        }
        let Some((prev_pos, prev_op)) = self.previous_instruction_before(last.position) else {
            return false;
        };
        if prev_op != OpCode::OpSetLocal {
            return false;
        }
        let local_idx = scope.instructions[prev_pos + 1] as usize;
        let new_instruction = make(OpCode::OpSetLocalPop, &[local_idx]);
        if !self.can_fuse_trailing_region(prev_pos, new_instruction.len()) {
            return false;
        }
        self.rewrite_trailing_region(prev_pos, new_instruction);
        true
    }

    fn try_fuse_trailing_call_arity(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        if last.opcode != Some(OpCode::OpCall) {
            return false;
        }
        let fused_op = match scope.instructions[last.position + 1] {
            0 => OpCode::OpCall0,
            1 => OpCode::OpCall1,
            2 => OpCode::OpCall2,
            _ => return false,
        };
        let new_instruction = make(fused_op, &[]);
        if !self.can_fuse_trailing_region(last.position, new_instruction.len()) {
            return false;
        }
        self.rewrite_trailing_region(last.position, new_instruction);
        true
    }

    fn try_fuse_trailing_tail_call1(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        if last.opcode != Some(OpCode::OpTailCall) || scope.instructions[last.position + 1] != 1 {
            return false;
        }
        let new_instruction = make(OpCode::OpTailCall1, &[]);
        if !self.can_fuse_trailing_region(last.position, new_instruction.len()) {
            return false;
        }
        self.rewrite_trailing_region(last.position, new_instruction);
        true
    }

    fn try_fuse_trailing_get_local_get_local(&mut self) -> bool {
        let scope = &self.scopes[self.scope_index];
        let last = scope.last_instruction.clone();
        let Some((b, len_b)) = self.decode_local_read_at(last.position) else {
            return false;
        };
        let Some((prev_pos, _)) = self.previous_instruction_before(last.position) else {
            return false;
        };
        let Some((a, len_a)) = self.decode_local_read_at(prev_pos) else {
            return false;
        };
        if prev_pos + len_a != last.position {
            return false;
        }
        let new_instruction = make(OpCode::OpGetLocalGetLocal, &[a, b]);
        if !self.can_fuse_trailing_region(prev_pos, new_instruction.len()) {
            return false;
        }
        if len_a + len_b < new_instruction.len() {
            return false;
        }
        self.rewrite_trailing_region(prev_pos, new_instruction);
        true
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
            Some(
                OpCode::OpGetLocal0
                | OpCode::OpGetLocal1
                | OpCode::OpConsumeLocal0
                | OpCode::OpConsumeLocal1,
            ) => prev_pos + 1 == pop_pos,
            Some(OpCode::OpConsumeLocal) => prev_pos + 2 == pop_pos,
            _ => false,
        };

        if adjacent && !self.has_jump_target_at(pop_pos) {
            match prev_op {
                Some(OpCode::OpGetLocal | OpCode::OpConsumeLocal) => {
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
                Some(OpCode::OpGetLocal0 | OpCode::OpConsumeLocal0) => {
                    self.scopes[self.scope_index].instructions[prev_pos] =
                        OpCode::OpReturnLocal as u8;
                    self.scopes[self.scope_index].instructions[pop_pos] = 0u8;
                    self.scopes[self.scope_index].last_instruction.opcode =
                        Some(OpCode::OpReturnLocal);
                    self.scopes[self.scope_index].last_instruction.position = prev_pos;
                    return;
                }
                Some(OpCode::OpGetLocal1 | OpCode::OpConsumeLocal1) => {
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

    pub(super) fn replace_last_local_read_with_return(&mut self) -> bool {
        let last = self.scopes[self.scope_index].last_instruction.clone();
        let pos = last.position;

        match last.opcode {
            Some(OpCode::OpGetLocal | OpCode::OpConsumeLocal) => {
                let local_idx = self.scopes[self.scope_index].instructions[pos + 1] as usize;
                self.replace_instruction(pos, make(OpCode::OpReturnLocal, &[local_idx]));
                self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnLocal);
                true
            }
            Some(OpCode::OpGetLocal0 | OpCode::OpConsumeLocal0) => {
                // Expanding a 1-byte opcode to 2 bytes shifts all subsequent
                // positions.  If any jump targets the byte right after this
                // instruction it would land on the new operand byte instead of
                // a valid opcode.  Bail out and let the caller emit
                // OpReturnValue instead.
                if self.scopes[self.scope_index].instructions.len() == pos + 1
                    && self.has_jump_target_at(pos + 1)
                {
                    return false;
                }
                self.scopes[self.scope_index].instructions[pos] = OpCode::OpReturnLocal as u8;
                if self.scopes[self.scope_index].instructions.len() == pos + 1 {
                    self.scopes[self.scope_index].instructions.push(0u8);
                } else {
                    self.scopes[self.scope_index].instructions[pos + 1] = 0u8;
                }
                self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnLocal);
                true
            }
            Some(OpCode::OpGetLocal1 | OpCode::OpConsumeLocal1) => {
                // Same guard as OpGetLocal0 — avoid corrupting jump targets.
                if self.scopes[self.scope_index].instructions.len() == pos + 1
                    && self.has_jump_target_at(pos + 1)
                {
                    return false;
                }
                self.scopes[self.scope_index].instructions[pos] = OpCode::OpReturnLocal as u8;
                if self.scopes[self.scope_index].instructions.len() == pos + 1 {
                    self.scopes[self.scope_index].instructions.push(1u8);
                } else {
                    self.scopes[self.scope_index].instructions[pos + 1] = 1u8;
                }
                self.scopes[self.scope_index].last_instruction.opcode = Some(OpCode::OpReturnLocal);
                true
            }
            _ => false,
        }
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
                OpCode::OpJump
                | OpCode::OpJumpNotTruthy
                | OpCode::OpJumpTruthy
                | OpCode::OpCmpEqJumpNotTruthy
                | OpCode::OpCmpNeJumpNotTruthy
                | OpCode::OpCmpGtJumpNotTruthy
                | OpCode::OpCmpLeJumpNotTruthy
                | OpCode::OpCmpGeJumpNotTruthy => {
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

    pub(super) fn with_consumable_local_use_counts<F, R>(
        &mut self,
        counts: HashMap<Symbol, usize>,
        f: F,
    ) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.consumable_local_use_counts.push(counts);
        let result = f(self);
        self.consumable_local_use_counts.pop();
        result
    }

    pub(super) fn current_consumable_local_use_counts(&self) -> Option<&HashMap<Symbol, usize>> {
        self.consumable_local_use_counts.last()
    }

    pub(super) fn with_function_context_with_param_effect_rows<F, R>(
        &mut self,
        num_params: usize,
        effects: &[EffectExpr],
        param_effect_rows: HashMap<Symbol, effect_rows::EffectRow>,
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
        self.function_param_effect_rows.push(param_effect_rows);
        self.captured_local_indices.push(HashSet::new());
        let result = f(self);
        self.captured_local_indices.pop();
        self.function_param_effect_rows.pop();
        self.function_effects.pop();
        self.function_param_counts.pop();
        result
    }

    pub(super) fn current_function_effects(&self) -> Option<&[Symbol]> {
        self.function_effects.last().map(Vec::as_slice)
    }

    pub(super) fn current_function_param_effect_row(
        &self,
        name: Symbol,
    ) -> Option<effect_rows::EffectRow> {
        self.function_param_effect_rows
            .last()
            .and_then(|rows| rows.get(&name).cloned())
    }

    pub(super) fn build_param_effect_rows(
        &self,
        parameters: &[Symbol],
        parameter_types: &[Option<TypeExpr>],
    ) -> HashMap<Symbol, effect_rows::EffectRow> {
        let mut rows = HashMap::new();
        for (index, param) in parameters.iter().enumerate() {
            let Some(Some(TypeExpr::Function { effects, .. })) = parameter_types.get(index) else {
                continue;
            };
            rows.insert(*param, effect_rows::EffectRow::from_effect_exprs(effects));
        }
        rows
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

    /// Try to resolve a perform target at compile time.
    ///
    /// Searches the handler scope stack (innermost first) for a tail-resumptive
    /// handler matching the given effect and operation. Returns
    /// `Some((depth, arm_index))` if found, where depth is the distance from
    /// the top of the runtime handler stack (0 = innermost).
    pub(super) fn resolve_handler_statically(
        &self,
        effect: Symbol,
        op: Symbol,
    ) -> Option<(usize, usize)> {
        // Search from innermost handler outward.
        for (i, scope) in self.handler_scopes.iter().rev().enumerate() {
            if scope.effect == effect {
                if !scope.is_direct {
                    // Found the handler but it's not tail-resumptive —
                    // can't use indexed direct dispatch.
                    return None;
                }
                if let Some(arm_idx) = scope.ops.iter().position(|&o| o == op) {
                    return Some((i, arm_idx));
                }
                // Effect matches but operation not found — shouldn't happen
                // (validated earlier), fall through to runtime dispatch.
                return None;
            }
        }
        None
    }

    /// Try to resolve a perform target to an evidence local variable.
    ///
    /// Returns `Some(local_index)` if the target handler has evidence locals
    /// for this operation, enabling direct `OpGetLocal` + `OpCall` dispatch.
    pub(super) fn resolve_evidence_local(&self, effect: Symbol, op: Symbol) -> Option<usize> {
        for scope in self.handler_scopes.iter().rev() {
            if scope.effect == effect {
                if let (Some(ev_locals), Some(arm_idx)) = (
                    &scope.evidence_locals,
                    scope.ops.iter().position(|&o| o == op),
                ) {
                    return Some(ev_locals[arm_idx]);
                }
                return None;
            }
        }
        None
    }

    /// Emit bytecode that pushes an identity closure `fn(x) -> x` onto the stack.
    ///
    /// Used as the `resume` parameter for evidence-passing performs. The closure
    /// is compiled as a constant `OpReturnLocal(0)` function, shared across all
    /// evidence performs in the same compilation unit.
    pub(super) fn emit_identity_closure(&mut self) {
        use crate::bytecode::op_code::OpCode;
        use crate::runtime::value::Value;
        use std::rc::Rc;

        let instructions = vec![OpCode::OpReturnLocal as u8, 0];
        let func = Rc::new(crate::runtime::compiled_function::CompiledFunction::new(
            instructions,
            1,    // arity = 1
            1,    // num_locals = 1 (the parameter)
            None, // no name
        ));
        let fn_idx = self.add_constant(Value::Function(func));
        // Emit OpClosure with 0 free variables.
        self.emit(OpCode::OpClosure, &[fn_idx, 0]);
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

    pub(super) fn is_flow_module_symbol(&self, name: Symbol) -> bool {
        self.sym(name) == "Flow"
    }

    pub(super) fn resolve_visible_symbol(&mut self, name: Symbol) -> Option<Binding> {
        self.symbol_table.resolve(name)
    }

    pub(super) fn resolve_library_primop(
        name: &str,
        arity: usize,
    ) -> Option<crate::core::CorePrimOp> {
        match (name.rsplit('.').next().unwrap_or(name), arity) {
            ("sort", 1) => Some(crate::core::CorePrimOp::Sort),
            ("sort_by", 2) => Some(crate::core::CorePrimOp::SortBy),
            _ => None,
        }
    }
}

pub(super) fn collect_tail_calls_from_ir(program: &IrProgram) -> Vec<TailCall> {
    let mut tail_calls = Vec::new();
    for function in program.functions() {
        // Build a map from BlockId to block for fast lookup.
        let block_map: std::collections::HashMap<_, _> =
            function.blocks.iter().map(|b| (b.id, b)).collect();

        for block in &function.blocks {
            match &block.terminator {
                // Explicit tail-call terminator emitted by the IR lowering for
                // self-tail-calls or direct tail-position calls at the statement level.
                IrTerminator::TailCall { metadata, .. } => {
                    if let Some(span) = metadata.span {
                        tail_calls.push(TailCall { span });
                    }
                }
                // Pattern produced by `lower_if_expression` for tail calls inside
                // if-branches: the call result is the last instruction and is passed
                // directly as the sole arg to a jump whose target block immediately
                // returns it (merge block with one param and no instructions).
                IrTerminator::Jump(target_id, jump_args, _) => {
                    let Some(last_instr) = block.instrs.last() else {
                        continue;
                    };
                    let IrInstr::Call {
                        dest: call_dest,
                        metadata,
                        ..
                    } = last_instr
                    else {
                        continue;
                    };
                    if jump_args != &[*call_dest] {
                        continue;
                    }
                    let Some(target_block) = block_map.get(target_id) else {
                        continue;
                    };
                    if !target_block.instrs.is_empty() || target_block.params.len() != 1 {
                        continue;
                    }
                    let merge_param = target_block.params[0].var;
                    if matches!(
                        &target_block.terminator,
                        IrTerminator::Return(ret_var, _) if *ret_var == merge_param
                    ) && let Some(span) = metadata.span
                    {
                        tail_calls.push(TailCall { span });
                    }
                }
                _ => {}
            }
        }
    }
    tail_calls
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
