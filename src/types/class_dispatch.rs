//! Type class dispatch — transforms class/instance declarations into callable
//! functions via AST preprocessing (Proposal 0145).
//!
//! For each instance method, generates a mangled function (`__tc_Class_Type_method`)
//! that compiles through the normal pipeline. Polymorphic stubs provide name
//! resolution for HM inference. Monomorphic calls are resolved at compile time
//! via `try_resolve_class_call`; polymorphic calls go through dictionary
//! elaboration (Core-to-Core pass).

use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier,
        block::Block,
        expression::{ExprId, Expression},
        interner::Interner,
        statement::{FunctionTypeParam, Statement},
        type_class::ClassConstraint,
        type_expr::TypeExpr,
    },
    types::class_env::ClassEnv,
};

/// Generate function statements from class/instance declarations.
///
/// Returns a list of new `Statement::Function` to inject into the program:
/// 1. Mangled instance method functions (one per instance method)
/// 2. Dispatch functions for methods with instances (one per class method)
pub fn generate_dispatch_functions(
    statements: &[Statement],
    class_env: &ClassEnv,
    interner: &mut Interner,
) -> Vec<Statement> {
    let mut generated = Vec::new();

    // Collect instance method info grouped by (class_name, method_name)
    let mut dispatch_table: HashSet<(Identifier, Identifier)> =
        HashSet::new();

    generate_from_statements(
        statements,
        class_env,
        interner,
        &mut generated,
        &mut dispatch_table,
    );

    // Generate dispatch functions for each class method.
    // These provide name resolution for the type checker and serve as fallback
    // for cases where compile-time resolution fails. When compile-time resolution
    // succeeds (Phase 4 Step 5), calls are rewritten directly to the mangled
    // instance function during Core lowering, making these dispatch functions
    // dead code for monomorphic call sites.
    let mut sorted_keys: Vec<_> = dispatch_table.iter().copied().collect::<Vec<_>>();
    sorted_keys.sort_by_key(|(c, m)| (c.as_u32(), m.as_u32()));
    for (class_name, method_name) in &sorted_keys {
        if let Some(class_def) = class_env.lookup_class(*class_name)
            && let Some(method_sig) = class_def.methods.iter().find(|m| m.name == *method_name)
        {
            // Polymorphic stub: typed params for HM inference. Body is a panic
            // placeholder — monomorphic calls resolve to __tc_* at compile time,
            // polymorphic calls go through dictionary elaboration.
            generated.push(generate_polymorphic_stub(
                *method_name,
                class_def,
                method_sig,
                interner,
            ));
        }
    }

    // Generate functions for default methods that have no instance override.
    // These are methods with a body in the class declaration (e.g., `neq`).
    generate_default_method_functions(statements, class_env, &dispatch_table, &mut generated);

    // Pre-intern dictionary names (__dict_{Class}_{Type}) for later use
    // by the dictionary elaboration pass (Proposal 0145, Step 5b).
    pre_intern_dict_names(class_env, interner);

    generated
}

/// Pre-intern `__dict_{Class}_{Type}` symbols for each concrete instance.
///
/// Called during Phase 1b so that the dictionary elaboration pass (Core-to-Core,
/// which only has `&Interner`) can find these names via `lookup()`.
fn pre_intern_dict_names(class_env: &ClassEnv, interner: &mut Interner) {
    for instance in &class_env.instances {
        if instance.type_args.is_empty() {
            continue;
        }
        let type_name = instance
            .type_args
            .iter()
            .map(|a| a.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");
        let class_str = interner.resolve(instance.class_name).to_string();
        let dict_name = format!("__dict_{class_str}_{type_name}");
        interner.intern(&dict_name);
    }
}

/// Generate functions for default class methods that have no explicit
/// instance implementation. E.g., `neq` with default body `{ !eq(x, y) }`.
fn generate_default_method_functions(
    statements: &[Statement],
    _class_env: &ClassEnv,
    dispatch_table: &HashSet<(Identifier, Identifier)>,
    generated: &mut Vec<Statement>,
) {
    for stmt in statements {
        match stmt {
            Statement::Class {
                name,
                methods,
                span,
                ..
            } => {
                for method in methods {
                    // Only generate for methods with a default body that have NO instance overrides.
                    if let Some(ref default_body) = method.default_body {
                        let has_instances = dispatch_table.contains(&(*name, method.name));
                        if !has_instances {
                            // Generate a regular function from the default body.
                            generated.push(Statement::Function {
                                is_public: false,
                                fip: None,
                                name: method.name,
                                type_params: vec![],
                                parameters: method.params.clone(),
                                parameter_types: vec![None; method.params.len()],
                                return_type: None,
                                effects: vec![],
                                body: default_body.clone(),
                                span: *span,
                            });
                        }
                    }
                }
            }
            Statement::Module { body, .. } => {
                generate_default_method_functions(
                    &body.statements,
                    _class_env,
                    dispatch_table,
                    generated,
                );
            }
            _ => {}
        }
    }
}

/// Recursively walk statements, generating mangled functions for instance methods.
fn generate_from_statements(
    statements: &[Statement],
    class_env: &ClassEnv,
    interner: &mut Interner,
    generated: &mut Vec<Statement>,
    dispatch_table: &mut HashSet<(Identifier, Identifier)>,
) {
    for stmt in statements {
        match stmt {
            Statement::Instance {
                class_name,
                type_args,
                context,
                methods,
                span,
                ..
            } => {
                let Some(class_def) = class_env.lookup_class(*class_name) else {
                    continue;
                };
                // Determine the head type name(s) for mangling.
                // Multi-param classes join all type args: __tc_Convert_Int_String_convert
                let type_name = if type_args.is_empty() {
                    "Unknown".to_string()
                } else {
                    type_args
                        .iter()
                        .map(|a| a.display_with(interner))
                        .collect::<Vec<_>>()
                        .join("_")
                };

                let class_name_str = interner.resolve(*class_name).to_string();

                for method in methods {
                    let Some(method_sig) = class_def.methods.iter().find(|sig| sig.name == method.name)
                    else {
                        continue;
                    };
                    // Generate mangled name: __tc_ClassName_TypeName_methodName
                    let method_name_str = interner.resolve(method.name).to_string();
                    let mangled = format!("__tc_{class_name_str}_{type_name}_{method_name_str}");
                    let mangled_sym = interner.intern(&mangled);

                    let parameter_types: Vec<Option<TypeExpr>> = method_sig
                        .param_types
                        .iter()
                        .map(|ty| {
                            Some(specialize_type_expr(
                                ty,
                                &class_def.type_params,
                                type_args,
                                interner,
                            ))
                        })
                        .collect();
                    let return_type = Some(specialize_type_expr(
                        &method_sig.return_type,
                        &class_def.type_params,
                        type_args,
                        interner,
                    ));
                    let type_params =
                        build_instance_function_type_params(type_args, context, method_sig, interner);

                    // Create a regular function statement with the mangled name
                    let fn_stmt = Statement::Function {
                        is_public: false,
                        fip: None,
                        name: mangled_sym,
                        type_params,
                        parameters: method.params.clone(),
                        parameter_types,
                        return_type,
                        effects: vec![],
                        body: method.body.clone(),
                        span: *span,
                    };
                    generated.push(fn_stmt);

                    // Record that this (class, method) pair has an instance.
                    dispatch_table.insert((*class_name, method.name));
                }
            }
            Statement::Module { body, .. } => {
                generate_from_statements(
                    &body.statements,
                    class_env,
                    interner,
                    generated,
                    dispatch_table,
                );
            }
            _ => {}
        }
    }
}

fn build_instance_function_type_params(
    instance_type_args: &[TypeExpr],
    context: &[ClassConstraint],
    method_sig: &crate::types::class_env::MethodSig,
    interner: &Interner,
) -> Vec<FunctionTypeParam> {
    let mut ordered = Vec::new();
    for type_arg in instance_type_args {
        collect_free_type_params(type_arg, interner, &mut ordered);
    }
    for constraint in context {
        for type_arg in &constraint.type_args {
            collect_free_type_params(type_arg, interner, &mut ordered);
        }
    }
    for &type_param in &method_sig.type_params {
        if !ordered.contains(&type_param) {
            ordered.push(type_param);
        }
    }
    ordered
        .into_iter()
        .map(|name| FunctionTypeParam {
            name,
            constraints: vec![],
        })
        .collect()
}

fn collect_free_type_params(ty: &TypeExpr, interner: &Interner, out: &mut Vec<Identifier>) {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            if is_type_param_name(*name, interner) && !out.contains(name) {
                out.push(*name);
            }
            for arg in args {
                collect_free_type_params(arg, interner, out);
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            for elem in elements {
                collect_free_type_params(elem, interner, out);
            }
        }
        TypeExpr::Function { params, ret, .. } => {
            for param in params {
                collect_free_type_params(param, interner, out);
            }
            collect_free_type_params(ret, interner, out);
        }
    }
}

fn is_type_param_name(name: Identifier, interner: &Interner) -> bool {
    interner
        .resolve(name)
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
}

fn specialize_type_expr(
    ty: &TypeExpr,
    class_type_params: &[Identifier],
    instance_type_args: &[TypeExpr],
    interner: &Interner,
) -> TypeExpr {
    let subst: HashMap<Identifier, TypeExpr> = class_type_params
        .iter()
        .copied()
        .zip(instance_type_args.iter().cloned())
        .collect();
    substitute_type_expr(ty, &subst, interner)
}

fn substitute_type_expr(
    ty: &TypeExpr,
    subst: &HashMap<Identifier, TypeExpr>,
    interner: &Interner,
) -> TypeExpr {
    match ty {
        TypeExpr::Named { name, args, span } => {
            let substituted_args: Vec<TypeExpr> = args
                .iter()
                .map(|arg| substitute_type_expr(arg, subst, interner))
                .collect();
            if let Some(replacement) = subst.get(name) {
                match replacement {
                    TypeExpr::Named {
                        name: replacement_name,
                        args: replacement_args,
                        ..
                    } => {
                        let mut merged_args: Vec<TypeExpr> = replacement_args.clone();
                        merged_args.extend(substituted_args);
                        TypeExpr::Named {
                            name: *replacement_name,
                            args: merged_args,
                            span: *span,
                        }
                    }
                    other => other.clone(),
                }
            } else {
                let _ = interner;
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
                .map(|elem| substitute_type_expr(elem, subst, interner))
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
                .map(|param| substitute_type_expr(param, subst, interner))
                .collect(),
            ret: Box::new(substitute_type_expr(ret, subst, interner)),
            effects: effects.clone(),
            span: *span,
        },
    }
}

/// Generate a polymorphic dispatch function for a class method.
///
/// Generate a polymorphic type stub for a class method.
///
/// Instead of a runtime `type_of()` chain, emits a properly typed polymorphic
/// function whose body is `panic("No instance")`. HM inference generalizes it
/// (e.g., `∀a. a -> a -> Bool` for `eq`), so each call site instantiates fresh
/// type variables. The body is never executed — Core lowering resolves all
/// monomorphic calls to the mangled instance function at compile time.
fn generate_polymorphic_stub(
    method_name: Identifier,
    class_def: &crate::types::class_env::ClassDef,
    method_sig: &crate::types::class_env::MethodSig,
    interner: &mut Interner,
) -> Statement {
    // Use the class's type parameter plus any per-method type params.
    let mut type_params: Vec<FunctionTypeParam> = class_def
        .type_params
        .iter()
        .map(|name| FunctionTypeParam {
            name: *name,
            constraints: vec![],
        })
        .collect();
    type_params.extend(method_sig.type_params.iter().map(|name| FunctionTypeParam {
        name: *name,
        constraints: vec![],
    }));

    // Generate parameter names: __x0, __x1, ...
    let params: Vec<Identifier> = (0..method_sig.arity)
        .map(|i| interner.intern(&format!("__x{i}")))
        .collect();

    // Use the method's parameter types from the class definition.
    let parameter_types: Vec<Option<crate::syntax::type_expr::TypeExpr>> = method_sig
        .param_types
        .iter()
        .map(|t| Some(t.clone()))
        .collect();

    let return_type = Some(method_sig.return_type.clone());

    let span = Span::default();
    let id = ExprId::UNSET;

    // Body: panic with a descriptive message. This stub exists only to give
    // HM inference a properly typed function signature. Monomorphic calls are
    // resolved directly to __tc_* mangled functions during Core lowering, and
    // polymorphic calls go through dictionary elaboration. The stub body is
    // never executed in well-typed programs.
    let method_display = interner.resolve(method_name).to_string();
    let class_display = interner.resolve(class_def.name).to_string();
    let panic_sym = interner.intern("panic");
    let body_expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: panic_sym,
            span,
            id,
        }),
        arguments: vec![Expression::String {
            value: format!("No instance of {class_display}.{method_display} for the given type"),
            span,
            id,
        }],
        span,
        id,
    };

    Statement::Function {
        is_public: false,
        fip: None,
        name: method_name,
        type_params,
        parameters: params,
        parameter_types,
        return_type,
        effects: vec![],
        body: Block {
            statements: vec![Statement::Expression {
                expression: body_expr,
                has_semicolon: false,
                span,
            }],
            span,
        },
        span,
    }
}

