//! Type class dispatch — transforms class/instance declarations into callable
//! functions via AST preprocessing.
//!
//! For each instance method, generates a mangled function that compiles through
//! the normal pipeline. For methods with multiple instances, generates a runtime
//! dispatch function using `type_of()`.
//!
//! This is the static-dispatch MVP. A future dictionary-passing elaboration
//! (Proposal 0145, Step 5) will replace runtime dispatch with compile-time
//! dictionary arguments for polymorphic code.

use std::collections::HashMap;

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier,
        block::Block,
        expression::{ExprId, Expression},
        interner::Interner,
        statement::{FunctionTypeParam, Statement},
    },
    types::class_env::ClassEnv,
};

/// Information about a single instance method for dispatch generation.
struct InstanceMethodInfo {
    /// The mangled function name (e.g., `__tc_Eq_Int_eq`).
    mangled_name: Identifier,
    /// The type name this instance applies to (e.g., `"Int"`, `"String"`).
    type_name: String,
}

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
    let mut dispatch_table: HashMap<(Identifier, Identifier), Vec<InstanceMethodInfo>> =
        HashMap::new();

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
    let mut sorted_keys: Vec<_> = dispatch_table.keys().collect();
    sorted_keys.sort_by_key(|(c, m)| (c.as_u32(), m.as_u32()));
    for &(class_name, method_name) in &sorted_keys {
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
    dispatch_table: &HashMap<(Identifier, Identifier), Vec<InstanceMethodInfo>>,
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
                        let has_instances = dispatch_table.contains_key(&(*name, method.name));
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
    _class_env: &ClassEnv,
    interner: &mut Interner,
    generated: &mut Vec<Statement>,
    dispatch_table: &mut HashMap<(Identifier, Identifier), Vec<InstanceMethodInfo>>,
) {
    for stmt in statements {
        match stmt {
            Statement::Instance {
                class_name,
                type_args,
                methods,
                span,
                ..
            } => {
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
                    // Generate mangled name: __tc_ClassName_TypeName_methodName
                    let method_name_str = interner.resolve(method.name).to_string();
                    let mangled = format!("__tc_{class_name_str}_{type_name}_{method_name_str}");
                    let mangled_sym = interner.intern(&mangled);

                    // Create a regular function statement with the mangled name
                    let fn_stmt = Statement::Function {
                        is_public: false,
                        fip: None,
                        name: mangled_sym,
                        type_params: vec![],
                        parameters: method.params.clone(),
                        parameter_types: vec![None; method.params.len()],
                        return_type: None,
                        effects: vec![],
                        body: method.body.clone(),
                        span: *span,
                    };
                    generated.push(fn_stmt);

                    // Record for dispatch table
                    dispatch_table
                        .entry((*class_name, method.name))
                        .or_default()
                        .push(InstanceMethodInfo {
                            mangled_name: mangled_sym,
                            type_name: type_name.clone(),
                        });
                }
            }
            Statement::Module { body, .. } => {
                generate_from_statements(
                    &body.statements,
                    _class_env,
                    interner,
                    generated,
                    dispatch_table,
                );
            }
            _ => {}
        }
    }
}

/// Generate a polymorphic dispatch function for a class method.
///
/// Combines:
/// 1. Polymorphic type signature from the class definition (so HM generalizes correctly)
/// 2. Runtime `type_of()` dispatch body (so bytecode execution resolves to the right instance)
///
/// The Core lowering path can further optimize by resolving monomorphic calls at compile time.
#[allow(dead_code)]
fn generate_polymorphic_dispatch(
    method_name: Identifier,
    class_def: &crate::types::class_env::ClassDef,
    method_sig: &crate::types::class_env::MethodSig,
    instances: &[InstanceMethodInfo],
    interner: &mut Interner,
) -> Statement {
    // Type params: class param + method params
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

    let parameter_types: Vec<Option<crate::syntax::type_expr::TypeExpr>> = method_sig
        .param_types
        .iter()
        .map(|t| Some(t.clone()))
        .collect();

    let return_type = Some(method_sig.return_type.clone());

    let span = Span::default();
    let id = ExprId::UNSET;

    // Build the dispatch body: type_of() chain → mangled function calls → panic fallback
    let class_display = interner.resolve(class_def.name).to_string();
    let method_display = interner.resolve(method_name).to_string();
    let type_of_sym = interner.intern("type_of");
    let panic_sym = interner.intern("panic");
    let panic_msg = format!("No instance of {class_display}.{method_display} for the given type");

    let panic_expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: panic_sym,
            span,
            id,
        }),
        arguments: vec![Expression::String {
            value: panic_msg,
            span,
            id,
        }],
        span,
        id,
    };

    let mut body_expr = panic_expr;

    if !instances.is_empty() && !params.is_empty() {
        for inst in instances.iter().rev() {
            let condition = Expression::Infix {
                left: Box::new(Expression::Call {
                    function: Box::new(Expression::Identifier {
                        name: type_of_sym,
                        span,
                        id,
                    }),
                    arguments: vec![Expression::Identifier {
                        name: params[0],
                        span,
                        id,
                    }],
                    span,
                    id,
                }),
                operator: "==".to_string(),
                right: Box::new(Expression::String {
                    value: inst.type_name.clone(),
                    span,
                    id,
                }),
                span,
                id,
            };

            let call_expr = Expression::Call {
                function: Box::new(Expression::Identifier {
                    name: inst.mangled_name,
                    span,
                    id,
                }),
                arguments: params
                    .iter()
                    .map(|p| Expression::Identifier { name: *p, span, id })
                    .collect(),
                span,
                id,
            };

            body_expr = Expression::If {
                condition: Box::new(condition),
                consequence: Block {
                    statements: vec![Statement::Expression {
                        expression: call_expr,
                        has_semicolon: false,
                        span,
                    }],
                    span,
                },
                alternative: Some(Block {
                    statements: vec![Statement::Expression {
                        expression: body_expr,
                        has_semicolon: false,
                        span,
                    }],
                    span,
                }),
                span,
                id,
            };
        }
    }

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

/// Generate a dispatch function that routes to the correct instance at runtime.
///
/// For `eq` with instances for Int and String, generates:
/// ```flux
/// fn eq(x, y) {
///     if type_of(x) == "Int" { __tc_Eq_Int_eq(x, y) }
///     else if type_of(x) == "String" { __tc_Eq_String_eq(x, y) }
///     else { panic("No instance for Eq") }
/// }
/// ```
#[allow(dead_code)]
fn generate_dispatch_function(
    method_name: Identifier,
    instances: &[InstanceMethodInfo],
    arity: usize,
    interner: &mut Interner,
) -> Option<Statement> {
    if instances.is_empty() {
        return None;
    }

    // Generate parameter names: __x0, __x1, ...
    let params: Vec<Identifier> = (0..arity)
        .map(|i| interner.intern(&format!("__x{i}")))
        .collect();

    let span = Span::default();
    let id = ExprId::UNSET;

    // Build the dispatch chain as nested if-else expressions.
    // Start from the last instance (the else/panic branch) and work backwards.
    let type_of_sym = interner.intern("type_of");
    let panic_sym = interner.intern("panic");

    // The panic fallback: panic("No instance for MethodName")
    let method_display = interner.resolve(method_name).to_string();
    let panic_msg = format!("No instance for {method_display}");
    let panic_expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: panic_sym,
            span,
            id,
        }),
        arguments: vec![Expression::String {
            value: panic_msg,
            span,
            id,
        }],
        span,
        id,
    };

    // Build if-else chain from last to first
    let mut current_else = panic_expr;

    for inst in instances.iter().rev() {
        // Condition: type_of(__x0) == "TypeName"
        let condition = Expression::Infix {
            left: Box::new(Expression::Call {
                function: Box::new(Expression::Identifier {
                    name: type_of_sym,
                    span,
                    id,
                }),
                arguments: vec![Expression::Identifier {
                    name: params[0],
                    span,
                    id,
                }],
                span,
                id,
            }),
            operator: "==".to_string(),
            right: Box::new(Expression::String {
                value: inst.type_name.clone(),
                span,
                id,
            }),
            span,
            id,
        };

        // Consequence: call the mangled function with all params
        let call_expr = Expression::Call {
            function: Box::new(Expression::Identifier {
                name: inst.mangled_name,
                span,
                id,
            }),
            arguments: params
                .iter()
                .map(|p| Expression::Identifier { name: *p, span, id })
                .collect(),
            span,
            id,
        };

        current_else = Expression::If {
            condition: Box::new(condition),
            consequence: Block {
                statements: vec![Statement::Expression {
                    expression: call_expr,
                    has_semicolon: false,
                    span,
                }],
                span,
            },
            alternative: Some(Block {
                statements: vec![Statement::Expression {
                    expression: current_else,
                    has_semicolon: false,
                    span,
                }],
                span,
            }),
            span,
            id,
        };
    }

    // Annotate parameters as Any to prevent HM from unifying the dispatch
    // branches' concrete types with each other (e.g., Int vs String).
    let any_name = interner.intern("Any");
    let any_type = crate::syntax::type_expr::TypeExpr::Named {
        name: any_name,
        args: vec![],
        span,
    };
    Some(Statement::Function {
        is_public: false,
        fip: None,
        name: method_name,
        type_params: vec![],
        parameters: params,
        parameter_types: vec![Some(any_type); arity],
        return_type: None,
        effects: vec![],
        body: Block {
            statements: vec![Statement::Expression {
                expression: current_else,
                has_semicolon: false,
                span,
            }],
            span,
        },
        span,
    })
}
