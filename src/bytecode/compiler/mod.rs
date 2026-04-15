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
        TailCall, collect_free_vars_in_program, desugar_operators_if_needed,
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
        ErrorType, diagnostic_for, lookup_error_code,
        position::{Position, Span},
    },
    runtime::{function_contract::FunctionContract, runtime_type::RuntimeType, value::Value},
    syntax::{
        Identifier,
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

fn remap_identifier(id: Identifier, remap: &HashMap<Symbol, Symbol>) -> Identifier {
    remap.get(&id).copied().unwrap_or(id)
}

fn remap_effect_expr(effect: &EffectExpr, remap: &HashMap<Symbol, Symbol>) -> EffectExpr {
    match effect {
        EffectExpr::Named { name, span } => EffectExpr::Named {
            name: remap_identifier(*name, remap),
            span: *span,
        },
        EffectExpr::RowVar { name, span } => EffectExpr::RowVar {
            name: remap_identifier(*name, remap),
            span: *span,
        },
        EffectExpr::Add { left, right, span } => EffectExpr::Add {
            left: Box::new(remap_effect_expr(left, remap)),
            right: Box::new(remap_effect_expr(right, remap)),
            span: *span,
        },
        EffectExpr::Subtract { left, right, span } => EffectExpr::Subtract {
            left: Box::new(remap_effect_expr(left, remap)),
            right: Box::new(remap_effect_expr(right, remap)),
            span: *span,
        },
    }
}

fn remap_type_expr(ty: &TypeExpr, remap: &HashMap<Symbol, Symbol>) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, args, span } => TypeExpr::Named {
            name: remap_identifier(*name, remap),
            args: args.iter().map(|arg| remap_type_expr(arg, remap)).collect(),
            span: *span,
        },
        TypeExpr::Tuple { elements, span } => TypeExpr::Tuple {
            elements: elements
                .iter()
                .map(|elem| remap_type_expr(elem, remap))
                .collect(),
            span: *span,
        },
        TypeExpr::Function {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Function {
            params: params
                .iter()
                .map(|param| remap_type_expr(param, remap))
                .collect(),
            ret: Box::new(remap_type_expr(ret, remap)),
            effects: effects
                .iter()
                .map(|effect| remap_effect_expr(effect, remap))
                .collect(),
            span: *span,
        },
    }
}

fn remap_class_constraint(
    constraint: &crate::syntax::type_class::ClassConstraint,
    remap: &HashMap<Symbol, Symbol>,
) -> crate::syntax::type_class::ClassConstraint {
    crate::syntax::type_class::ClassConstraint {
        class_name: remap_identifier(constraint.class_name, remap),
        type_args: constraint
            .type_args
            .iter()
            .map(|arg| remap_type_expr(arg, remap))
            .collect(),
        span: constraint.span,
    }
}

fn imported_class_def_from_entry(
    entry: &crate::types::module_interface::PublicClassEntry,
    remap: &HashMap<Symbol, Symbol>,
    interner: &mut Interner,
) -> Option<crate::types::class_env::ClassDef> {
    let module_sym = interner.intern(&entry.class_module);
    let class_sym = interner.intern(&entry.name);
    let module = crate::types::class_id::ModulePath::from_identifier(module_sym);
    let methods = entry
        .methods
        .iter()
        .map(|method| crate::types::class_env::MethodSig {
            name: remap_identifier(method.name, remap),
            type_params: method
                .type_params
                .iter()
                .map(|tp| remap_identifier(*tp, remap))
                .collect(),
            param_types: method
                .param_types
                .iter()
                .map(|ty| remap_type_expr(ty, remap))
                .collect(),
            return_type: remap_type_expr(&method.return_type, remap),
            arity: method.param_types.len(),
            effects: method
                .effects
                .iter()
                .map(|effect| remap_effect_expr(effect, remap))
                .collect(),
        })
        .collect::<Vec<_>>();

    if methods.is_empty() && !entry.method_names.is_empty() {
        return None;
    }

    Some(crate::types::class_env::ClassDef {
        name: class_sym,
        module,
        is_public: true,
        type_params: entry
            .type_params
            .iter()
            .map(|tp| remap_identifier(*tp, remap))
            .collect(),
        superclasses: entry
            .superclasses
            .iter()
            .map(|constraint| remap_class_constraint(constraint, remap))
            .collect(),
        methods,
        default_methods: entry
            .default_methods
            .iter()
            .map(|name| remap_identifier(*name, remap))
            .collect(),
        span: Span::default(),
    })
}

fn imported_instance_def_from_entry(
    entry: &crate::types::module_interface::PublicInstanceEntry,
    remap: &HashMap<Symbol, Symbol>,
    interner: &mut Interner,
    imported_classes: &HashMap<crate::types::class_id::ClassId, crate::types::class_env::ClassDef>,
) -> Option<crate::types::class_env::InstanceDef> {
    let class_module =
        crate::types::class_id::ModulePath::from_identifier(interner.intern(&entry.class_module));
    let class_name = interner.intern(&entry.class_name);
    let class_id = crate::types::class_id::ClassId::new(class_module, class_name);
    imported_classes.get(&class_id)?;
    Some(crate::types::class_env::InstanceDef {
        class_name,
        class_id,
        instance_module: crate::types::class_id::ModulePath::from_identifier(
            interner.intern(&entry.instance_module),
        ),
        is_public: true,
        type_args: entry
            .type_args
            .iter()
            .map(|ty| remap_type_expr(ty, remap))
            .collect(),
        context: entry
            .context
            .iter()
            .map(|constraint| remap_class_constraint(constraint, remap))
            .collect(),
        method_names: entry
            .methods
            .iter()
            .map(|method| remap_identifier(method.name, remap))
            .collect(),
        method_effects: entry
            .methods
            .iter()
            .map(|method| {
                (
                    remap_identifier(method.name, remap),
                    method
                        .effects
                        .iter()
                        .map(|effect| remap_effect_expr(effect, remap))
                        .collect(),
                )
            })
            .collect(),
        span: Span::default(),
    })
}

fn remap_public_instance_entry(
    entry: &crate::types::module_interface::PublicInstanceEntry,
    remap: &HashMap<Symbol, Symbol>,
) -> crate::types::module_interface::PublicInstanceEntry {
    crate::types::module_interface::PublicInstanceEntry {
        class_module: entry.class_module.clone(),
        class_name: entry.class_name.clone(),
        instance_module: entry.instance_module.clone(),
        head_type_repr: entry.head_type_repr.clone(),
        type_args: entry
            .type_args
            .iter()
            .map(|ty| remap_type_expr(ty, remap))
            .collect(),
        context: entry
            .context
            .iter()
            .map(|constraint| remap_class_constraint(constraint, remap))
            .collect(),
        methods: entry
            .methods
            .iter()
            .map(
                |method| crate::types::module_interface::PublicInstanceMethodEntry {
                    name: remap_identifier(method.name, remap),
                    effects: method
                        .effects
                        .iter()
                        .map(|effect| remap_effect_expr(effect, remap))
                        .collect(),
                },
            )
            .collect(),
        pinned_row_placeholder: entry.pinned_row_placeholder.clone(),
    }
}

fn build_public_class_method_scheme(
    class_def: &crate::types::class_env::ClassDef,
    method: &crate::types::class_env::MethodSig,
    interner: &Interner,
) -> Scheme {
    let mut type_params = HashMap::new();
    let mut next_var: TypeVarId = 0;
    for &name in &class_def.type_params {
        type_params.insert(name, next_var);
        next_var += 1;
    }
    for &name in &method.type_params {
        type_params.insert(name, next_var);
        next_var += 1;
    }
    let mut row_var_env = HashMap::new();
    let mut row_var_counter = next_var;
    let param_tys: Vec<InferType> = method
        .param_types
        .iter()
        .map(|ty| {
            TypeEnv::convert_type_expr_rec(
                ty,
                &type_params,
                interner,
                &mut row_var_env,
                &mut row_var_counter,
            )
            .expect("public class method param type should convert")
        })
        .collect();
    let ret_ty = TypeEnv::convert_type_expr_rec(
        &method.return_type,
        &type_params,
        interner,
        &mut row_var_env,
        &mut row_var_counter,
    )
    .expect("public class method return type should convert");
    let effect_row =
        InferEffectRow::from_effect_exprs(&method.effects, &mut row_var_env, &mut row_var_counter)
            .expect("public class method effects should convert");
    crate::types::scheme::generalize(
        &InferType::Fun(param_tys, Box::new(ret_ty), effect_row),
        &HashSet::new(),
    )
}

fn substitute_type_expr_for_instance(
    ty: &TypeExpr,
    subst: &HashMap<Identifier, TypeExpr>,
) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, args, span } => {
            let substituted_args: Vec<TypeExpr> = args
                .iter()
                .map(|arg| substitute_type_expr_for_instance(arg, subst))
                .collect();
            if let Some(replacement) = subst.get(name) {
                if let TypeExpr::Named {
                    name: replacement_name,
                    args: replacement_args,
                    ..
                } = replacement
                {
                    let mut merged_args = replacement_args.clone();
                    merged_args.extend(substituted_args);
                    TypeExpr::Named {
                        name: *replacement_name,
                        args: merged_args,
                        span: *span,
                    }
                } else {
                    replacement.clone()
                }
            } else {
                TypeExpr::Named {
                    name: *name,
                    args: substituted_args,
                    span: *span,
                }
            }
        }
        TypeExpr::Tuple { elements, span } => TypeExpr::Tuple {
            elements: elements
                .iter()
                .map(|elem| substitute_type_expr_for_instance(elem, subst))
                .collect(),
            span: *span,
        },
        TypeExpr::Function {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Function {
            params: params
                .iter()
                .map(|param| substitute_type_expr_for_instance(param, subst))
                .collect(),
            ret: Box::new(substitute_type_expr_for_instance(ret, subst)),
            effects: effects.clone(),
            span: *span,
        },
    }
}

fn specialize_type_expr_for_instance(
    ty: &TypeExpr,
    class_type_params: &[Identifier],
    instance_type_args: &[TypeExpr],
) -> TypeExpr {
    let subst: HashMap<Identifier, TypeExpr> = class_type_params
        .iter()
        .copied()
        .zip(instance_type_args.iter().cloned())
        .collect();
    substitute_type_expr_for_instance(ty, &subst)
}

fn preload_imported_instance_schemes(
    symbol_table: &mut SymbolTable,
    preloaded_imported_globals: &mut HashSet<Symbol>,
    out: &mut HashMap<Symbol, Scheme>,
    native_symbols: &mut HashMap<Symbol, String>,
    instance_def: &crate::types::class_env::InstanceDef,
    class_def: &crate::types::class_env::ClassDef,
    interner: &mut Interner,
) {
    let type_key = instance_def
        .type_args
        .iter()
        .map(|arg| arg.display_with(interner))
        .collect::<Vec<_>>()
        .join("_");
    let class_str = interner.resolve(instance_def.class_name).to_string();
    let method_effects: HashMap<Identifier, Vec<EffectExpr>> =
        instance_def.method_effects.iter().cloned().collect();
    let module_qualifier = instance_def
        .instance_module
        .as_identifier()
        .map(|sym| interner.resolve(sym))
        .and_then(|module| module.rsplit('.').next())
        .filter(|segment| !segment.is_empty())
        .unwrap_or("module")
        .replace('.', "_");

    for method in &class_def.methods {
        let method_str = interner.resolve(method.name).to_string();
        let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
        let mangled_sym = interner.intern(&mangled);
        if !symbol_table.exists_in_current_scope(mangled_sym) {
            symbol_table.define(mangled_sym, Span::default());
        }
        preloaded_imported_globals.insert(mangled_sym);
        native_symbols.insert(mangled_sym, format!("flux_{module_qualifier}_{mangled}"));
        let specialized_param_types = method
            .param_types
            .iter()
            .map(|ty| {
                specialize_type_expr_for_instance(
                    ty,
                    &class_def.type_params,
                    &instance_def.type_args,
                )
            })
            .collect::<Vec<_>>();
        let specialized_return_type = specialize_type_expr_for_instance(
            &method.return_type,
            &class_def.type_params,
            &instance_def.type_args,
        );
        let effects = method_effects
            .get(&method.name)
            .cloned()
            .filter(|effects| !effects.is_empty())
            .unwrap_or_else(|| method.effects.clone());
        let specialized_method = crate::types::class_env::MethodSig {
            name: method.name,
            type_params: method.type_params.clone(),
            param_types: specialized_param_types,
            return_type: specialized_return_type,
            arity: method.arity,
            effects,
        };
        out.insert(
            mangled_sym,
            build_public_class_method_scheme(
                &crate::types::class_env::ClassDef {
                    type_params: vec![],
                    methods: vec![],
                    default_methods: vec![],
                    ..class_def.clone()
                },
                &specialized_method,
                interner,
            ),
        );
    }
}

fn merge_imported_public_instances(
    env: &mut crate::types::class_env::ClassEnv,
    imported_instances: &[crate::types::class_env::InstanceDef],
    diagnostics: &mut Vec<Diagnostic>,
    interner: &Interner,
) {
    for imported in imported_instances {
        let duplicate = env.instances.iter().find(|existing| {
            existing.class_id == imported.class_id
                && existing.type_args.len() == imported.type_args.len()
                && existing
                    .type_args
                    .iter()
                    .zip(imported.type_args.iter())
                    .all(|(a, b)| a.structural_eq(b))
        });
        if let Some(existing) = duplicate {
            let display_class = interner.resolve(imported.class_name);
            let display_type: Vec<String> = imported
                .type_args
                .iter()
                .map(|t| t.display_with(interner))
                .collect();
            let existing_module = existing
                .instance_module
                .as_identifier()
                .map(|id| interner.resolve(id).to_string())
                .unwrap_or_else(|| "<prelude>".to_string());
            let imported_module = imported
                .instance_module
                .as_identifier()
                .map(|id| interner.resolve(id).to_string())
                .unwrap_or_else(|| "<prelude>".to_string());
            diagnostics.push(
                diagnostic_for(&crate::diagnostics::compiler_errors::DUPLICATE_INSTANCE)
                    .with_span(imported.span)
                    .with_message(format!(
                        "Duplicate instance for `{display_class}<{}>`.",
                        display_type.join(", ")
                    ))
                    .with_hint_text(format!(
                        "Another instance of `{display_class}<{}>` already lives in module \
                         `{existing_module}`; imported public instance came from `{imported_module}`.",
                        display_type.join(", ")
                    )),
            );
            continue;
        }
        env.instances.push(imported.clone());
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_pending_imported_public_instances(
    imported_public_classes: &HashMap<
        crate::types::class_id::ClassId,
        crate::types::class_env::ClassDef,
    >,
    pending_entries: &mut Vec<crate::types::module_interface::PublicInstanceEntry>,
    imported_public_instances: &mut Vec<crate::types::class_env::InstanceDef>,
    imported_instance_method_schemes: &mut HashMap<Symbol, Scheme>,
    imported_instance_method_native_symbols: &mut HashMap<Symbol, String>,
    symbol_table: &mut SymbolTable,
    preloaded_imported_globals: &mut HashSet<Symbol>,
    interner: &mut Interner,
) {
    let mut still_pending = Vec::new();
    for entry in pending_entries.drain(..) {
        if let Some(instance_def) = imported_instance_def_from_entry(
            &entry,
            &HashMap::new(),
            interner,
            imported_public_classes,
        ) {
            if let Some(class_def) = imported_public_classes.get(&instance_def.class_id) {
                preload_imported_instance_schemes(
                    symbol_table,
                    preloaded_imported_globals,
                    imported_instance_method_schemes,
                    imported_instance_method_native_symbols,
                    &instance_def,
                    class_def,
                    interner,
                );
            }
            imported_public_instances.push(instance_def);
        } else {
            still_pending.push(entry);
        }
    }
    *pending_entries = still_pending;
}

#[derive(Default)]
struct AetherDebugDetails {
    call_sites: Vec<String>,
    dups: Vec<String>,
    drops: Vec<String>,
    reuses: Vec<String>,
}

fn collect_aether_debug_details(
    expr: &crate::aether::AetherExpr,
    interner: &Interner,
) -> AetherDebugDetails {
    fn walk(
        expr: &crate::aether::AetherExpr,
        interner: &Interner,
        details: &mut AetherDebugDetails,
    ) {
        use crate::aether::AetherExpr;

        match expr {
            AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
            AetherExpr::Lam { body, .. } | AetherExpr::Return { value: body, .. } => {
                walk(body, interner, details);
            }
            AetherExpr::App { func, args, .. } => {
                walk(func, interner, details);
                for arg in args {
                    walk(arg, interner, details);
                }
            }
            AetherExpr::AetherCall {
                func,
                args,
                arg_modes,
                span,
            } => {
                details.call_sites.push(format!(
                    "line {}: {} [{}]",
                    span.start.line,
                    crate::aether::display::single_line_expr(func, interner),
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
            AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
                walk(rhs, interner, details);
                walk(body, interner, details);
            }
            AetherExpr::LetRecGroup { bindings, body, .. } => {
                for (_, rhs) in bindings {
                    walk(rhs, interner, details);
                }
                walk(body, interner, details);
            }
            AetherExpr::Case {
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
            AetherExpr::Con { fields, .. } | AetherExpr::PrimOp { args: fields, .. } => {
                for field in fields {
                    walk(field, interner, details);
                }
            }
            AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
                walk(object, interner, details);
            }
            AetherExpr::Perform { args, .. } => {
                for arg in args {
                    walk(arg, interner, details);
                }
            }
            AetherExpr::Handle { body, handlers, .. } => {
                walk(body, interner, details);
                for handler in handlers {
                    walk(&handler.body, interner, details);
                }
            }
            AetherExpr::Dup { var, body, span } => {
                details.dups.push(format!(
                    "line {}: dup {}",
                    span.start.line,
                    crate::aether::display::format_var_ref(var, interner)
                ));
                walk(body, interner, details);
            }
            AetherExpr::Drop { var, body, span } => {
                details.drops.push(format!(
                    "line {}: drop {}",
                    span.start.line,
                    crate::aether::display::format_var_ref(var, interner)
                ));
                walk(body, interner, details);
            }
            AetherExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                span,
            } => {
                details.reuses.push(format!(
                    "line {}: reuse {} as {}{}",
                    span.start.line,
                    crate::aether::display::format_var_ref(token, interner),
                    crate::aether::display::tag_label(tag, interner),
                    field_mask
                        .map(|mask| format!(" mask=0x{mask:x}"))
                        .unwrap_or_default()
                ));
                for field in fields {
                    walk(field, interner, details);
                }
            }
            AetherExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                span,
            } => {
                details.reuses.push(format!(
                    "line {}: drop-specialized {}",
                    span.start.line,
                    crate::aether::display::format_var_ref(scrutinee, interner)
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
    preloaded_effect_ops_registry: HashMap<Symbol, HashSet<Symbol>>,
    preloaded_effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>,
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
    pub(super) ir_function_symbols: HashMap<FunctionId, Symbol>,
    pub(super) inferred_function_effects: HashMap<ContractKey, HashSet<Symbol>>,
    strict_mode: bool,
    strict_inference: bool,
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
    /// Imported `public class` entries reconstructed from preloaded module interfaces.
    imported_public_classes:
        HashMap<crate::types::class_id::ClassId, crate::types::class_env::ClassDef>,
    /// Imported `public instance` entries reconstructed from preloaded module interfaces.
    imported_public_instances: Vec<crate::types::class_env::InstanceDef>,
    /// Imported `public instance` entries waiting for their class interface to load.
    pending_imported_public_instance_entries:
        Vec<crate::types::module_interface::PublicInstanceEntry>,
    /// Imported monomorphic `__tc_*` schemes rebuilt from public instance metadata.
    imported_instance_method_schemes: HashMap<Symbol, Scheme>,
    imported_instance_method_native_symbols: HashMap<Symbol, String>,
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
    effective_program: Cow<'a, Program>,
    hm_final: InferProgramResult,
}

#[derive(Clone, Copy)]
enum LoweringPreparationMode {
    Fresh,
    WithPreloaded,
}

#[cfg(test)]
mod compiler_test;

impl Compiler {
    fn is_flow_library_file(&self) -> bool {
        self.current_module_kind == ModuleKind::FlowStdlib
    }

    pub(super) fn inject_generated_dispatch_functions(
        &self,
        program: &Program,
        generated: Vec<Statement>,
    ) -> Program {
        let module_count = program
            .statements
            .iter()
            .filter(|stmt| matches!(stmt, Statement::Module { .. }))
            .count();

        if module_count == 1 {
            let (top_level_generated, module_generated): (Vec<_>, Vec<_>) =
                generated.into_iter().partition(|stmt| match stmt {
                    Statement::Function { name, .. } => self.sym(*name).starts_with("__tc_"),
                    _ => false,
                });
            let statements = program
                .statements
                .iter()
                .cloned()
                .enumerate()
                .flat_map(|(idx, stmt)| {
                    let mut emitted = Vec::new();
                    if idx == 0 {
                        emitted.extend(top_level_generated.clone());
                    }
                    match stmt {
                        Statement::Module { name, body, span } => {
                            let mut module_statements = module_generated.clone();
                            module_statements.extend(body.statements.iter().cloned());
                            emitted.push(Statement::Module {
                                name,
                                body: crate::syntax::block::Block {
                                    statements: module_statements,
                                    span: body.span,
                                },
                                span,
                            });
                        }
                        other => emitted.push(other),
                    }
                    emitted
                })
                .collect();
            Program {
                statements,
                span: program.span,
            }
        } else {
            let mut statements = generated;
            statements.extend(program.statements.iter().cloned());
            Program {
                statements,
                span: program.span,
            }
        }
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
            preloaded_effect_ops_registry: HashMap::new(),
            preloaded_effect_op_signatures: HashMap::new(),
            type_env: TypeEnv::new(),
            hm_expr_types: HashMap::new(),
            cached_member_schemes: HashMap::new(),
            cached_member_borrow_signatures: HashMap::new(),
            has_hm_diagnostics: false,
            ir_function_symbols: HashMap::new(),
            inferred_function_effects: HashMap::new(),
            strict_mode: false,
            strict_inference: false,
            strict_require_main: true,
            type_optimize: false,
            profiling: false,
            cost_centre_infos: Vec::new(),
            class_env: crate::types::class_env::ClassEnv::new(),
            imported_public_classes: HashMap::new(),
            imported_public_instances: Vec::new(),
            pending_imported_public_instance_entries: Vec::new(),
            imported_instance_method_schemes: HashMap::new(),
            imported_instance_method_native_symbols: HashMap::new(),
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

    fn infer_final_program<'a>(&mut self, program: &'a Program) -> FinalInferenceResult<'a> {
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
        let effective_program = if desugar_changed_program {
            desugar_operators_if_needed(
                pre_desugar_program,
                &pre_desugar_expr_types,
                &mut self.interner,
            )
        } else {
            pre_desugar_program
        };
        #[cfg(test)]
        let _ = desugar_changed_program;
        let hm_final = match &effective_program {
            _ if !desugar_changed_program => hm_pre_desugar,
            Cow::Owned(_) | Cow::Borrowed(_) => self.run_hm_infer(effective_program.as_ref()),
        };
        FinalInferenceResult {
            effective_program,
            hm_final,
        }
    }

    fn apply_hm_final(&mut self, hm_final: &InferProgramResult) {
        self.type_env = hm_final.type_env.clone();
        self.hm_expr_types = hm_final.expr_types.clone();
    }

    #[allow(clippy::result_large_err)]
    fn lower_core_from_program(
        &self,
        program_to_lower: &Program,
        optimize: bool,
        elaborate_dictionaries: bool,
    ) -> Result<crate::core::CoreProgram, Diagnostic> {
        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let mut core = crate::core::lower_ast::lower_program_ast_with_class_env(
            program_to_lower,
            &self.hm_expr_types,
            Some(&self.interner),
            Some(&self.type_env),
            None,
            class_env_ref,
        );

        if elaborate_dictionaries && !self.class_env.classes.is_empty() {
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

        let preloaded_registry = self.build_preloaded_borrow_registry(program_to_lower);
        let _ = preloaded_registry;
        crate::core::passes::run_core_passes_with_interner(&mut core, &self.interner, optimize)?;
        Ok(core)
    }

    #[allow(clippy::result_large_err)]
    fn lower_aether_from_program(
        &self,
        program_to_lower: &Program,
        optimize: bool,
        elaborate_dictionaries: bool,
    ) -> Result<crate::aether::AetherProgram, Diagnostic> {
        let core =
            self.lower_core_from_program(program_to_lower, optimize, elaborate_dictionaries)?;
        let preloaded_registry = self.build_preloaded_borrow_registry(program_to_lower);
        let (aether, _) = crate::aether::lower_core_to_aether_program(
            &core,
            Some(&self.interner),
            preloaded_registry,
        )?;
        Ok(aether)
    }

    #[allow(clippy::result_large_err)]
    fn prepare_core_program(
        &mut self,
        program: &Program,
        optimize: bool,
        elaborate_dictionaries: bool,
    ) -> Result<crate::core::CoreProgram, Diagnostic> {
        if optimize {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            let program_to_lower = rename(optimized, HashMap::new());
            return self.lower_core_from_program(&program_to_lower, true, elaborate_dictionaries);
        }

        let prepared = self.prepare_program_for_lowering(program);
        self.apply_hm_final(&prepared.hm_final);
        self.lower_core_from_program(
            prepared.effective_program.as_ref(),
            false,
            elaborate_dictionaries,
        )
    }

    #[allow(clippy::result_large_err)]
    fn prepare_backend_core_program(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::aether::AetherProgram, Diagnostic> {
        if optimize {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(program.clone());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            let program_to_lower = rename(optimized, HashMap::new());
            return self.lower_aether_from_program(&program_to_lower, true, true);
        }

        let prepared = self.prepare_program_for_lowering(program);
        self.apply_hm_final(&prepared.hm_final);
        self.lower_aether_from_program(prepared.effective_program.as_ref(), false, true)
    }

    #[allow(clippy::result_large_err)]
    fn prepare_backend_core_program_with_preloaded(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::aether::AetherProgram, Diagnostic> {
        if optimize {
            return self.prepare_backend_core_program(program, true);
        }

        let prepared = self.prepare_program_for_lowering_with_preloaded(program);
        self.apply_hm_final(&prepared.hm_final);
        self.lower_aether_from_program(prepared.effective_program.as_ref(), false, true)
    }

    fn prepare_program_for_lowering_internal<'a>(
        &mut self,
        program: &'a Program,
        mode: LoweringPreparationMode,
    ) -> FinalInferenceResult<'a> {
        #[cfg(test)]
        {
            self.hm_infer_runs = 0;
        }
        let (
            preloaded_contracts,
            preloaded_visibility,
            preloaded_adt_ctors,
            preloaded_effect_ops,
            preloaded_effect_sigs,
        ) = match mode {
            LoweringPreparationMode::Fresh => (
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            ),
            LoweringPreparationMode::WithPreloaded => (
                self.module_contracts.clone(),
                self.module_function_visibility.clone(),
                self.module_adt_constructors.clone(),
                self.effect_ops_registry.clone(),
                self.effect_op_signatures.clone(),
            ),
        };

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
        if matches!(mode, LoweringPreparationMode::WithPreloaded) {
            self.collect_module_adt_constructors(program);
        }
        self.collect_module_contracts(program);
        self.collect_effect_declarations(program);
        self.auto_expose_flow_modules();

        if !self.class_env.classes.is_empty() && !self.is_flow_library_file() {
            let additional_reserved_names = self
                .preloaded_imported_globals
                .iter()
                .copied()
                .filter(|name| {
                    !self
                        .interner
                        .try_resolve(*name)
                        .is_some_and(|resolved| resolved.starts_with("__tc_"))
                })
                .collect::<HashSet<_>>();
            let extra = crate::types::class_dispatch::generate_dispatch_functions(
                &program.statements,
                &self.class_env,
                &mut self.interner,
                &additional_reserved_names,
            );
            if extra.is_empty() {
                self.infer_final_program(program)
            } else {
                let class_augmented = self.inject_generated_dispatch_functions(program, extra);
                let final_inference = self.infer_final_program(&class_augmented);
                FinalInferenceResult {
                    effective_program: Cow::Owned(final_inference.effective_program.into_owned()),
                    hm_final: final_inference.hm_final,
                }
            }
        } else {
            self.infer_final_program(program)
        }
    }

    fn prepare_program_for_lowering<'a>(
        &mut self,
        program: &'a Program,
    ) -> FinalInferenceResult<'a> {
        self.prepare_program_for_lowering_internal(program, LoweringPreparationMode::Fresh)
    }

    fn prepare_program_for_lowering_with_preloaded<'a>(
        &mut self,
        program: &'a Program,
    ) -> FinalInferenceResult<'a> {
        self.prepare_program_for_lowering_internal(program, LoweringPreparationMode::WithPreloaded)
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
        let skip_flow_auto_expose: Vec<(&str, &str)> = vec![];
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

    pub fn set_strict_inference(&mut self, strict_inference: bool) {
        self.strict_inference = strict_inference;
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
        let prepared = self.prepare_program_for_lowering(program);
        self.apply_hm_final(&prepared.hm_final);
        self.hm_expr_types.clone()
    }

    pub fn infer_expr_types_for_module_with_preloaded(
        &mut self,
        program: &Program,
    ) -> HashMap<ExprId, InferType> {
        let prepared = self.prepare_program_for_lowering_with_preloaded(program);
        self.apply_hm_final(&prepared.hm_final);
        self.hm_expr_types.clone()
    }

    pub fn take_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.warnings)
    }

    pub fn cached_member_schemes(&self) -> &HashMap<(Symbol, Symbol), Scheme> {
        &self.cached_member_schemes
    }

    /// Proposal 0151, Phase 2: read-only access to the collected class
    /// environment so that `build_interface` can extract `public class`
    /// and `public instance` entries owned by the current module.
    pub fn class_env(&self) -> &crate::types::class_env::ClassEnv {
        &self.class_env
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

        for class_entry in &interface.public_classes {
            if let Some(class_def) =
                imported_class_def_from_entry(class_entry, &symbol_remap, &mut self.interner)
            {
                let class_id = class_def.class_id();
                for method in &class_def.methods {
                    let qualified = self.interner.intern_join(module_name, method.name);
                    if !self.symbol_table.exists_in_current_scope(qualified) {
                        self.symbol_table.define(qualified, Span::default());
                    }
                    self.preloaded_imported_globals.insert(qualified);
                    let scheme =
                        build_public_class_method_scheme(&class_def, method, &self.interner);
                    self.cached_member_schemes
                        .insert((module_name, method.name), scheme);
                    self.module_function_visibility
                        .insert((module_name, method.name), true);
                }
                self.imported_public_classes.insert(class_id, class_def);
            }
        }

        for instance_entry in &interface.public_instances {
            let remapped_entry = remap_public_instance_entry(instance_entry, &symbol_remap);
            if let Some(instance_def) = imported_instance_def_from_entry(
                &remapped_entry,
                &HashMap::new(),
                &mut self.interner,
                &self.imported_public_classes,
            ) {
                if let Some(class_def) = self.imported_public_classes.get(&instance_def.class_id) {
                    preload_imported_instance_schemes(
                        &mut self.symbol_table,
                        &mut self.preloaded_imported_globals,
                        &mut self.imported_instance_method_schemes,
                        &mut self.imported_instance_method_native_symbols,
                        &instance_def,
                        class_def,
                        &mut self.interner,
                    );
                }
                self.imported_public_instances.push(instance_def);
            } else {
                self.pending_imported_public_instance_entries
                    .push(remapped_entry);
            }
        }
        resolve_pending_imported_public_instances(
            &self.imported_public_classes,
            &mut self.pending_imported_public_instance_entries,
            &mut self.imported_public_instances,
            &mut self.imported_instance_method_schemes,
            &mut self.imported_instance_method_native_symbols,
            &mut self.symbol_table,
            &mut self.preloaded_imported_globals,
            &mut self.interner,
        );
    }

    pub fn preload_dependency_program(&mut self, program: &Program) {
        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        for statement in &program.statements {
            self.collect_effect_declarations_from_stmt(statement);
        }
        self.preloaded_effect_ops_registry = self.effect_ops_registry.clone();
        self.preloaded_effect_op_signatures = self.effect_op_signatures.clone();
    }

    pub fn build_native_extern_symbols(
        &self,
        program: &Program,
    ) -> HashMap<String, crate::lir::lower::ImportedNativeSymbol> {
        use crate::syntax::statement::{ImportExposing, Statement};

        fn collect_local_function_names(
            statements: &[Statement],
            out: &mut HashSet<String>,
            interner: &Interner,
        ) {
            for statement in statements {
                match statement {
                    Statement::Function { name, .. } => {
                        out.insert(interner.resolve(*name).to_string());
                    }
                    Statement::Module { body, .. } => {
                        collect_local_function_names(&body.statements, out, interner);
                    }
                    _ => {}
                }
            }
        }

        let mut import_bindings = self.collect_import_module_bindings(program);
        for (&alias, &target) in &self.import_aliases {
            import_bindings.insert(alias, target);
        }
        for &module in &self.imported_modules {
            import_bindings.entry(module).or_insert(module);
        }
        let mut symbols = HashMap::new();
        let flow_prefixes = [
            "Flow.Option",
            "Flow.List",
            "Flow.String",
            "Flow.Numeric",
            "Flow.IO",
            "Flow.Assert",
        ];
        let skip_flow_auto_expose: [(&str, &str); 0] = [];
        let mut local_function_names = HashSet::new();
        collect_local_function_names(
            &program.statements,
            &mut local_function_names,
            &self.interner,
        );

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

            for class_def in self.imported_public_classes.values() {
                if class_def.module.as_identifier() != Some(target_module) {
                    continue;
                }
                for method in &class_def.methods {
                    let member = self.sym(method.name);
                    symbols
                        .entry(format!("{binding_name}.{member}"))
                        .or_insert_with(|| crate::lir::lower::ImportedNativeSymbol {
                            symbol: format!("flux_{}_{}", target_name.replace('.', "_"), member),
                            arity: method.arity,
                        });
                }
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
                    for class_def in self.imported_public_classes.values() {
                        if class_def.module.as_identifier() != Some(*module_name) {
                            continue;
                        }
                        for method in &class_def.methods {
                            let member = self.sym(method.name);
                            symbols.entry(member.to_string()).or_insert_with(|| {
                                crate::lir::lower::ImportedNativeSymbol {
                                    symbol: format!(
                                        "flux_{}_{}",
                                        self.sym(*module_name).replace('.', "_"),
                                        member
                                    ),
                                    arity: method.arity,
                                }
                            });
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
                        for class_def in self.imported_public_classes.values() {
                            if class_def.module.as_identifier() != Some(*module_name) {
                                continue;
                            }
                            for method in &class_def.methods {
                                if method.name != *member_name {
                                    continue;
                                }
                                symbols.entry(member.to_string()).or_insert_with(|| {
                                    crate::lir::lower::ImportedNativeSymbol {
                                        symbol: format!(
                                            "flux_{}_{}",
                                            self.sym(*module_name).replace('.', "_"),
                                            member
                                        ),
                                        arity: method.arity,
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }

        for (&name, scheme) in &self.imported_instance_method_schemes {
            let mangled = self.sym(name);
            if local_function_names.contains(mangled) {
                continue;
            }
            let native_symbol = self
                .imported_instance_method_native_symbols
                .get(&name)
                .cloned()
                .unwrap_or_else(|| format!("flux_{mangled}"));
            symbols.entry(mangled.to_string()).or_insert_with(|| {
                crate::lir::lower::ImportedNativeSymbol {
                    symbol: native_symbol,
                    arity: Self::native_function_arity(scheme),
                }
            });
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
        self.effect_ops_registry = self.preloaded_effect_ops_registry.clone();
        self.effect_op_signatures = self.preloaded_effect_op_signatures.clone();
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
        env.classes.extend(self.imported_public_classes.clone());
        let diagnostics = env.collect_from_statements(&program.statements, &self.interner);
        merge_imported_public_instances(
            &mut env,
            &self.imported_public_instances,
            &mut self.warnings,
            &self.interner,
        );
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

        // Imported instance method schemes are hidden implementation details,
        // but HM needs them in scope so resolved class-call effect propagation
        // can look up the instantiated `__tc_*` row in downstream modules.
        for (&name, scheme) in &self.imported_instance_method_schemes {
            exposed_schemes
                .entry(name)
                .or_insert_with(|| scheme.clone());
        }

        let class_env = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(self.class_env.clone())
        };

        InferProgramConfig {
            file_path: Some(self.file_path.as_str().into()),
            strict_inference: self.strict_inference,
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
            ("array_push", vec![var_a(), var_b()], var_a(), pure(), 0),
            ("array_concat", vec![var_a(), var_a()], var_a(), pure(), 0),
            (
                "array_slice",
                vec![var_a(), con(TC::Int), con(TC::Int)],
                var_a(),
                pure(),
                0,
            ),
            ("array_reverse", vec![var_a()], var_a(), pure(), 0),
            (
                "array_contains",
                vec![var_a(), var_b()],
                con(TC::Bool),
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
            ("map_keys", vec![var_a()], var_b(), pure(), 0),
            ("map_values", vec![var_a()], var_b(), pure(), 0),
            ("map_has", vec![var_a(), var_b()], con(TC::Bool), pure(), 0),
            ("map_merge", vec![var_a(), var_a()], var_a(), pure(), 0),
            ("map_delete", vec![var_a(), var_b()], var_a(), pure(), 0),
            (
                "map_set",
                vec![var_a(), var_b(), var_c()],
                var_a(),
                pure(),
                0,
            ),
            ("map_get", vec![var_a(), var_b()], var_c(), pure(), 0),
            ("map_size", vec![var_a()], con(TC::Int), pure(), 0),
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
        if self.strict_mode && self.strict_require_main && !has_main {
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
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                span,
                ..
            } => {
                if self.strict_mode && *is_public {
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

                if self.strict_mode {
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
                }

                let allowed_type_params: HashSet<Symbol> =
                    type_params.iter().map(|param| param.name).collect();
                for ty in parameter_types.iter().flatten() {
                    self.error_on_unknown_type_expr(ty, &allowed_type_params);
                }
                if let Some(ret) = return_type {
                    self.error_on_unknown_type_expr(ret, &allowed_type_params);
                }
            }
            Statement::Let {
                type_annotation: Some(annotation),
                ..
            } => {
                self.error_on_unknown_type_expr(annotation, &HashSet::new());
            }
            Statement::Module { name, body, .. } => {
                for nested in &body.statements {
                    self.validate_strict_mode_statement(nested, Some(*name));
                }
            }
            _ => {}
        }
    }

    fn error_on_unknown_type_expr(&mut self, ty: &TypeExpr, allowed_type_params: &HashSet<Symbol>) {
        match ty {
            TypeExpr::Named { name, args, span } => {
                for arg in args {
                    self.error_on_unknown_type_expr(arg, allowed_type_params);
                }

                if self.is_known_annotation_type(*name, allowed_type_params) {
                    return;
                }

                let type_name = self.sym(*name).to_string();
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E423",
                        "UNKNOWN TYPE",
                        ErrorType::Compiler,
                        format!("I can't find a type named `{type_name}`."),
                        Some(
                            "Use a built-in type, an in-scope type parameter, or a declared ADT."
                                .to_string(),
                        ),
                        self.file_path.clone(),
                        *span,
                    )
                    .with_display_title("Unknown Type")
                    .with_category(DiagnosticCategory::TypeInference)
                    .with_primary_label(*span, "unknown type used here"),
                );
            }
            TypeExpr::Tuple { elements, .. } => {
                for element in elements {
                    self.error_on_unknown_type_expr(element, allowed_type_params);
                }
            }
            TypeExpr::Function { params, ret, .. } => {
                for param in params {
                    self.error_on_unknown_type_expr(param, allowed_type_params);
                }
                self.error_on_unknown_type_expr(ret, allowed_type_params);
            }
        }
    }

    fn is_known_annotation_type(
        &self,
        name: Symbol,
        allowed_type_params: &HashSet<Symbol>,
    ) -> bool {
        if allowed_type_params.contains(&name) {
            return true;
        }

        matches!(
            self.sym(name),
            "Int"
                | "Float"
                | "Bool"
                | "String"
                | "Unit"
                | "None"
                | "Never"
                | "List"
                | "Array"
                | "Map"
                | "Option"
                | "Either"
        ) || self.adt_registry.lookup_adt(name).is_some()
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
            let short = self.sym(*name);
            if let Some(found) = self
                .imported_modules
                .iter()
                .copied()
                .find(|module| self.sym(*module).rsplit('.').next() == Some(short))
            {
                return Some(found);
            }
            if let Some(current) = self.current_module_prefix
                && self.sym(current).rsplit('.').next() == Some(short)
            {
                return Some(current);
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
        // Program passed to `compile`. All AST rewrites must happen before this call.
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
        &mut self,
        program: &Program,
        optimize: bool,
        mode: crate::core::display::CoreDisplayMode,
    ) -> Result<String, Diagnostic> {
        let core = self.prepare_core_program(program, optimize, true)?;

        let ir_text = match mode {
            crate::core::display::CoreDisplayMode::Readable => {
                crate::core::display::display_program_readable(&core, &self.interner)
            }
            crate::core::display::CoreDisplayMode::Debug => {
                crate::core::display::display_program_debug(&core, &self.interner)
            }
        };
        Ok(ir_text)
    }

    /// Lower to Core IR, then to LIR, and return a human-readable dump.
    #[allow(clippy::result_large_err)]
    pub fn dump_lir(&mut self, program: &Program, optimize: bool) -> Result<String, Diagnostic> {
        let aether = self.prepare_backend_core_program_with_preloaded(program, optimize)?;
        let globals_map = self.build_globals_map();
        let lir = crate::lir::lower::lower_aether_program_with_interner(
            &aether,
            Some(&self.interner),
            Some(&globals_map),
        );
        Ok(crate::lir::lower::display_program(&lir))
    }

    /// Lower to Core IR, then to CFG IR, and return a human-readable dump.
    #[allow(clippy::result_large_err)]
    pub fn dump_cfg(&mut self, program: &Program, optimize: bool) -> Result<String, Diagnostic> {
        let prepared = self.prepare_program_for_lowering(program);
        self.apply_hm_final(&prepared.hm_final);
        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let (mut ir_program, _) = crate::cfg::lower_program_to_ir_typed(
            prepared.effective_program.as_ref(),
            &self.hm_expr_types,
            Some(&self.interner),
            optimize,
            Some(&self.type_env),
            class_env_ref,
        )?;
        crate::cfg::run_ir_pass_pipeline(&mut ir_program, &crate::cfg::IrPassContext)?;
        Ok(ir_program.to_string())
    }

    /// Lower program through LIR to an LLVM IR module (Proposal 0132 Phase 7).
    /// Returns the `LlvmModule` struct so the caller can inject target triple
    /// and data layout before rendering.
    #[cfg(feature = "llvm")]
    #[allow(clippy::result_large_err)]
    pub fn lower_to_lir_llvm_module(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::llvm::LlvmModule, Diagnostic> {
        let aether = self.prepare_backend_core_program_with_preloaded(program, optimize)?;

        // Pass None for globals_map so ALL functions are lowered to LIR
        // functions (no GetGlobal). In native mode there's no VM globals
        // table, so every function must be compiled into the LLVM module.
        let lir = crate::lir::lower::lower_aether_program_with_interner(
            &aether,
            Some(&self.interner),
            None,
        );
        Ok(crate::lir::emit_llvm::emit_llvm_module(&lir))
    }

    /// Lower a single module through LIR to an LLVM IR module while resolving
    /// imported public functions as external symbols rather than merged-program
    /// local binders.
    #[cfg(feature = "llvm")]
    #[allow(clippy::result_large_err)]
    pub fn lower_to_lir_llvm_module_per_module(
        &mut self,
        program: &Program,
        optimize: bool,
        export_user_ctor_name_helper: bool,
        emit_entry_main: bool,
    ) -> Result<crate::llvm::LlvmModule, Diagnostic> {
        let _ = self.phase_collection(program);
        let prepared = self.prepare_program_for_lowering_with_preloaded(program);
        self.apply_hm_final(&prepared.hm_final);
        let effective_program = prepared.effective_program;

        let (effective_program, aether) = if optimize {
            use crate::ast::{constant_fold_with_interner, desugar, rename};
            let desugared = desugar(effective_program.into_owned());
            let optimized = constant_fold_with_interner(desugared, &self.interner);
            let program_to_lower = rename(optimized, HashMap::new());
            let aether = self.lower_aether_from_program(&program_to_lower, true, true)?;
            (Cow::Owned(program_to_lower), aether)
        } else {
            let aether = self.lower_aether_from_program(effective_program.as_ref(), false, true)?;
            (effective_program, aether)
        };

        let extern_symbols = self.build_native_extern_symbols(effective_program.as_ref());
        let has_named_main = effective_program.statements.iter().any(|statement| {
            matches!(
                statement,
                Statement::Function { name, .. } if self.sym(*name) == "main"
            )
        });
        let emit_main = emit_entry_main || has_named_main;
        // Derive an entry qualifier from the file path to prevent symbol
        // collisions with C runtime primops. E.g. "examples/day06.flx"
        // yields qualifier "day06", so user's `fn sum` becomes `flux_day06_sum`
        // instead of `flux_sum` (which clashes with libflux_rt.a).
        let entry_qualifier = std::path::Path::new(&self.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.replace(['-', '.', ' '], "_"));
        let lir = crate::lir::lower::lower_aether_program_with_interner_and_externs(
            &aether,
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
    #[cfg(feature = "llvm")]
    #[allow(clippy::result_large_err)]
    pub fn dump_lir_llvm(
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<String, Diagnostic> {
        let module = self.lower_to_lir_llvm_module(program, optimize)?;
        Ok(crate::llvm::render_module(&module))
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
        // Sort by global index so later-defined globals overwrite earlier
        // unqualified names. Module members from non-prelude modules remain
        // qualified/alias-qualified only.
        let mut globals = self.symbol_table.global_definitions();
        globals.sort_by_key(|&(_, idx)| idx);
        let flow_prelude_modules = [
            "Flow.Option",
            "Flow.List",
            "Flow.String",
            "Flow.Numeric",
            "Flow.IO",
            "Flow.Assert",
        ];
        for (sym, idx) in globals {
            let name = self
                .interner
                .resolve(crate::syntax::Identifier::from(sym))
                .to_string();
            // Add qualified name (e.g. "Flow.Array.sort")
            map.insert(name.clone(), idx);
            if let Some((module, short)) = name.rsplit_once('.')
                && flow_prelude_modules.contains(&module)
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
        &mut self,
        program: &Program,
        optimize: bool,
    ) -> Result<crate::aether::AetherProgram, Diagnostic> {
        self.prepare_backend_core_program_with_preloaded(program, optimize)
    }

    /// Render an Aether-native ownership report showing per-function lowering decisions.
    #[allow(clippy::result_large_err)]
    pub fn render_aether_report(
        &mut self,
        program: &Program,
        optimize: bool,
        debug: bool,
    ) -> Result<String, Diagnostic> {
        let aether = self.lower_aether_report_program(program, optimize)?;
        let fbip_diags = crate::aether::check_fbip::check_fbip_aether(&aether, &self.interner);
        let fbip_by_name = fbip_diags
            .diagnostics
            .iter()
            .map(|diag| (diag.function_name.as_str(), diag))
            .collect::<HashMap<_, _>>();

        let mut out = String::new();
        out.push_str("Aether Ownership Report\n");
        out.push_str("=======================\n\n");

        let mut total = crate::aether::AetherStats::default();

        for def in aether.defs() {
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

            let verify_errors = crate::aether::verify::verify_contract_aether(&def.expr)
                .err()
                .unwrap_or_default();
            let verify_diags = crate::aether::verify::verify_diagnostics_aether(&def.expr);
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

            let displayed =
                crate::aether::display::display_expr_readable(&def.expr, &self.interner);
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

    /// Dump an Aether-native ownership report showing per-function lowering decisions.
    #[allow(clippy::result_large_err)]
    pub fn dump_aether_report(
        &mut self,
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
        let constants = self.constants[snapshot.constants_len..].to_vec();
        let instructions = scope.instructions[snapshot.instructions_len..].to_vec();
        let referenced_globals =
            self.referenced_global_indices_in_artifact(&instructions, &constants);
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
            .filter(|binding| {
                matches!(
                    binding.kind,
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Defined
                ) || referenced_globals.contains(&binding.index)
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
            constants,
            instructions,
            debug_info: FunctionDebugInfo::new(None, scope.files.clone(), relative_locations)
                .with_effect_summary(scope.effect_summary),
        }
    }

    pub fn build_relocatable_module_bytecode(&self) -> CachedModuleBytecode {
        let scope = &self.scopes[self.scope_index];
        let constants = self.constants.clone();
        let instructions = scope.instructions.clone();
        let referenced_globals =
            self.referenced_global_indices_in_artifact(&instructions, &constants);
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
            .filter(|binding| {
                matches!(
                    binding.kind,
                    crate::bytecode::bytecode_cache::module_cache::CachedModuleBindingKind::Defined
                ) || referenced_globals.contains(&binding.index)
            })
            .collect();

        CachedModuleBytecode {
            globals,
            constants,
            instructions,
            debug_info: FunctionDebugInfo::new(None, scope.files.clone(), scope.locations.clone())
                .with_effect_summary(scope.effect_summary),
        }
    }

    fn referenced_global_indices_in_artifact(
        &self,
        instructions: &[u8],
        constants: &[Value],
    ) -> HashSet<usize> {
        let mut referenced = HashSet::new();
        self.collect_referenced_global_indices(instructions, constants, &mut referenced);
        referenced
    }

    fn collect_referenced_global_indices(
        &self,
        instructions: &[u8],
        constants: &[Value],
        referenced: &mut HashSet<usize>,
    ) {
        let mut ip = 0usize;
        while ip < instructions.len() {
            let op = crate::bytecode::op_code::OpCode::from(instructions[ip]);
            match op {
                crate::bytecode::op_code::OpCode::OpGetGlobal
                | crate::bytecode::op_code::OpCode::OpSetGlobal => {
                    referenced
                        .insert(crate::bytecode::op_code::read_u16(instructions, ip + 1) as usize);
                }
                _ => {}
            }
            ip += 1 + crate::bytecode::op_code::operand_widths(op)
                .iter()
                .sum::<usize>();
        }

        for constant in constants {
            if let Value::Function(function) = constant {
                self.collect_referenced_global_indices(&function.instructions, &[], referenced);
            }
        }
    }

    fn remap_cached_constant_symbols(&mut self, value: &Value) -> Value {
        match value {
            Value::HandlerDescriptor(desc) => Value::HandlerDescriptor(std::rc::Rc::new(
                crate::runtime::handler_descriptor::HandlerDescriptor {
                    effect: self.interner.intern(&desc.effect_name),
                    effect_name: desc.effect_name.clone(),
                    ops: desc
                        .op_names
                        .iter()
                        .map(|name| self.interner.intern(name))
                        .collect(),
                    op_names: desc.op_names.clone(),
                    is_discard: desc.is_discard,
                },
            )),
            Value::PerformDescriptor(desc) => Value::PerformDescriptor(std::rc::Rc::new(
                crate::runtime::perform_descriptor::PerformDescriptor {
                    effect: self.interner.intern(&desc.effect_name),
                    op: self.interner.intern(&desc.op_name),
                    effect_name: desc.effect_name.clone(),
                    op_name: desc.op_name.clone(),
                },
            )),
            Value::Array(values) => Value::Array(std::rc::Rc::new(
                values
                    .iter()
                    .map(|value| self.remap_cached_constant_symbols(value))
                    .collect(),
            )),
            Value::Tuple(values) => Value::Tuple(std::rc::Rc::new(
                values
                    .iter()
                    .map(|value| self.remap_cached_constant_symbols(value))
                    .collect(),
            )),
            Value::Some(value) => {
                Value::Some(std::rc::Rc::new(self.remap_cached_constant_symbols(value)))
            }
            Value::Left(value) => {
                Value::Left(std::rc::Rc::new(self.remap_cached_constant_symbols(value)))
            }
            Value::Right(value) => {
                Value::Right(std::rc::Rc::new(self.remap_cached_constant_symbols(value)))
            }
            Value::Cons(cell) => crate::runtime::cons_cell::ConsCell::cons(
                self.remap_cached_constant_symbols(&cell.head),
                self.remap_cached_constant_symbols(&cell.tail),
            ),
            Value::Adt(adt) => Value::Adt(std::rc::Rc::new(crate::runtime::value::AdtValue {
                constructor: adt.constructor.clone(),
                fields: crate::runtime::value::AdtFields::from_vec(
                    adt.fields
                        .iter()
                        .map(|value| self.remap_cached_constant_symbols(value))
                        .collect(),
                ),
            })),
            _ => value.clone(),
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

        let remapped_constants = cached
            .constants
            .iter()
            .map(|value| self.remap_cached_constant_symbols(value))
            .collect::<Vec<_>>();
        self.constants.extend(remapped_constants);

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
