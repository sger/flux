/// Unit tests for the HM type inference engine (src/types/).
// ============================================================================
// Helper constructors
// ============================================================================
use std::collections::{HashMap, HashSet};

use flux::ast::type_infer::{InferProgramConfig, infer_program};
use flux::syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement};
use flux::types::infer_effect_row::InferEffectRow;
use flux::types::infer_type::InferType;
use flux::types::scheme::{Scheme, generalize};
use flux::types::type_constructor::TypeConstructor;
use flux::types::type_env::TypeEnv;
use flux::types::type_subst::TypeSubst;
use flux::types::unify::unify;
use flux::types::unify_error::UnifyErrorKind;

fn int() -> InferType {
    InferType::Con(TypeConstructor::Int)
}

fn float() -> InferType {
    InferType::Con(TypeConstructor::Float)
}

fn string() -> InferType {
    InferType::Con(TypeConstructor::String)
}

fn bool_() -> InferType {
    InferType::Con(TypeConstructor::Bool)
}

fn var(v: u32) -> InferType {
    InferType::Var(v)
}

fn list(t: InferType) -> InferType {
    InferType::App(TypeConstructor::List, vec![t])
}

fn option(t: InferType) -> InferType {
    InferType::App(TypeConstructor::Option, vec![t])
}

fn fun(params: Vec<InferType>, ret: InferType) -> InferType {
    InferType::Fun(params, Box::new(ret), InferEffectRow::closed_empty())
}

fn fun_with_effects(
    params: Vec<InferType>,
    ret: InferType,
    effects: Vec<flux::syntax::Identifier>,
) -> InferType {
    InferType::Fun(params, Box::new(ret), InferEffectRow::closed_from_symbols(effects))
}

fn tuple(elems: Vec<InferType>) -> InferType {
    InferType::Tuple(elems)
}

fn infer_program_from_source(
    source: &str,
) -> (flux::ast::type_infer::InferProgramResult, flux::syntax::program::Program) {
    infer_program_from_source_with_class_env(source, None)
}

fn infer_program_from_source_with_class_env(
    source: &str,
    class_env: Option<flux::types::class_env::ClassEnv>,
) -> (flux::ast::type_infer::InferProgramResult, flux::syntax::program::Program) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut effect_op_sigs = HashMap::new();
    fn effect_op_scheme(
        interner: &flux::syntax::interner::Interner,
        type_expr: &flux::syntax::type_expr::TypeExpr,
    ) -> Scheme {
        fn collect_implicit_type_params(
            interner: &flux::syntax::interner::Interner,
            ty: &flux::syntax::type_expr::TypeExpr,
            out: &mut HashSet<flux::syntax::Identifier>,
        ) {
            match ty {
                flux::syntax::type_expr::TypeExpr::Named { name, args, .. } => {
                    if interner
                        .resolve(*name)
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_lowercase())
                    {
                        out.insert(*name);
                    }
                    for arg in args {
                        collect_implicit_type_params(interner, arg, out);
                    }
                }
                flux::syntax::type_expr::TypeExpr::Tuple { elements, .. } => {
                    for element in elements {
                        collect_implicit_type_params(interner, element, out);
                    }
                }
                flux::syntax::type_expr::TypeExpr::Function { params, ret, .. } => {
                    for param in params {
                        collect_implicit_type_params(interner, param, out);
                    }
                    collect_implicit_type_params(interner, ret, out);
                }
            }
        }
        let flux::syntax::type_expr::TypeExpr::Function { params, ret, effects, .. } = type_expr
        else {
            panic!("effect operation signature must be function-shaped");
        };
        let mut implicit_type_params = HashSet::new();
        collect_implicit_type_params(interner, type_expr, &mut implicit_type_params);
        let mut sorted_type_params = implicit_type_params.into_iter().collect::<Vec<_>>();
        sorted_type_params.sort_by_key(|sym| sym.as_u32());
        let type_params = sorted_type_params
            .into_iter()
            .enumerate()
            .map(|(idx, sym)| (sym, idx as u32))
            .collect::<HashMap<_, _>>();
        let mut row_var_env = HashMap::new();
        let mut next_var = type_params.len() as u32;
        let param_tys = params
            .iter()
            .map(|param| {
                TypeEnv::convert_type_expr_rec(
                    param,
                    &type_params,
                    interner,
                    &mut row_var_env,
                    &mut next_var,
                )
                .expect("effect op param should lower")
            })
            .collect();
        let ret_ty = TypeEnv::convert_type_expr_rec(
            ret,
            &type_params,
            interner,
            &mut row_var_env,
            &mut next_var,
        )
        .expect("effect op return should lower");
        let effect_row =
            InferEffectRow::from_effect_exprs(effects, &mut row_var_env, &mut next_var)
                .expect("effect op effect row should lower");
        generalize(&InferType::Fun(param_tys, Box::new(ret_ty), effect_row), &HashSet::new())
    }
    fn collect_effect_sigs(
        statements: &[Statement],
        out: &mut HashMap<(flux::syntax::Identifier, flux::syntax::Identifier), Scheme>,
        interner: &flux::syntax::interner::Interner,
    ) {
        for statement in statements {
            match statement {
                Statement::EffectDecl { name, ops, .. } => {
                    for op in ops {
                        out.insert((*name, op.name), effect_op_scheme(interner, &op.type_expr));
                    }
                }
                Statement::Module { body, .. } => {
                    collect_effect_sigs(&body.statements, out, interner)
                }
                _ => {}
            }
        }
    }
    collect_effect_sigs(&program.statements, &mut effect_op_sigs, &interner);
    let mut interner_for_base = interner.clone();
    let base_symbol = interner_for_base.intern("Flow");
    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: Some("<test>".into()),

            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            task_module_bindings: HashSet::new(),
            task_spawn_exposed: false,
            known_flow_names: HashSet::new(),
            flow_module_symbol: base_symbol,
            preloaded_effect_op_signatures: effect_op_sigs,
            effect_row_aliases: HashMap::new(),
            class_env,
            preloaded_adt_data: Vec::new(),
        },
    );
    (result, program)
}

fn has_diagnostic_code(result: &flux::ast::type_infer::InferProgramResult, code: &str) -> bool {
    result.diagnostics.iter().any(|diag| diag.code() == Some(code))
}

fn has_diagnostic_message_fragment(
    result: &flux::ast::type_infer::InferProgramResult,
    fragment: &str,
) -> bool {
    result.diagnostics.iter().any(|diag| diag.message().is_some_and(|m| m.contains(fragment)))
}

mod annotations;
mod class_env_and_patterns;
mod effects;
mod hm_diagnostics;
mod type_machinery;
