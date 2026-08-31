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
        data_variant::DataVariant,
        expression::{ExprIdGen, Expression},
        interner::Interner,
        lexer::Lexer,
        parser::Parser,
        statement::{FunctionTypeParam, Statement},
        type_class::ClassConstraint,
        type_expr::TypeExpr,
    },
    types::class_env::ClassEnv,
    types::infer_type::InferType,
};

/// Finds the concrete type bound to `target` while matching a scheme pattern
/// against a call-site type.  AST lowering and Core lowering both use this
/// structural operation; keeping it here prevents their dictionary resolvers
/// from silently acquiring different matching rules.
pub(crate) fn match_constraint_type_var(
    pattern: &InferType,
    actual: &InferType,
    target: crate::types::TypeVarId,
) -> Option<InferType> {
    match pattern {
        InferType::Var(var) if *var == target => Some(actual.clone()),
        InferType::App(pattern_ctor, pattern_args) => {
            let InferType::App(actual_ctor, actual_args) = actual else {
                return None;
            };
            if pattern_ctor != actual_ctor || pattern_args.len() != actual_args.len() {
                return None;
            }
            pattern_args
                .iter()
                .zip(actual_args)
                .find_map(|(p, a)| match_constraint_type_var(p, a, target))
        }
        InferType::Tuple(pattern_elems) => {
            let InferType::Tuple(actual_elems) = actual else {
                return None;
            };
            if pattern_elems.len() != actual_elems.len() {
                return None;
            }
            pattern_elems
                .iter()
                .zip(actual_elems)
                .find_map(|(p, a)| match_constraint_type_var(p, a, target))
        }
        InferType::Fun(pattern_params, pattern_ret, _) => {
            let InferType::Fun(actual_params, actual_ret, _) = actual else {
                return None;
            };
            if pattern_params.len() != actual_params.len() {
                return None;
            }
            pattern_params
                .iter()
                .zip(actual_params)
                .find_map(|(p, a)| match_constraint_type_var(p, a, target))
                .or_else(|| match_constraint_type_var(pattern_ret, actual_ret, target))
        }
        InferType::HktApp(pattern_head, pattern_args) => {
            let actual_args = match actual {
                InferType::App(_, args) | InferType::HktApp(_, args) => args,
                _ => return None,
            };
            if pattern_args.len() != actual_args.len() {
                return None;
            }
            if let InferType::Var(var) = pattern_head.as_ref()
                && *var == target
            {
                return match actual {
                    InferType::App(actual_ctor, _) => Some(InferType::Con(actual_ctor.clone())),
                    InferType::HktApp(actual_head, _) => Some(actual_head.as_ref().clone()),
                    _ => None,
                };
            }
            pattern_args
                .iter()
                .zip(actual_args)
                .find_map(|(p, a)| match_constraint_type_var(p, a, target))
        }
        _ => None,
    }
}

/// Options for [`generate_dispatch_functions`].
#[derive(Debug, Clone, Copy)]
pub struct DispatchGenerationOptions {
    /// Synthesize `__tc_*` bodies for built-in instances (`Num<Int>`, ...)
    /// when the program needs them. Stdlib modules must leave this off: the
    /// runner compiles them through one shared interner, and merely interning
    /// `__tc_Num_Int_add` there makes a later user file resolve `add(5, 10)`
    /// to a function that was never generated for it.
    pub include_builtin_instances: bool,
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
    additional_reserved_names: &HashSet<Identifier>,
    options: DispatchGenerationOptions,
) -> Vec<Statement> {
    let mut generated = Vec::new();
    let mut reserved_names = collect_existing_function_names(statements);
    reserved_names.extend(additional_reserved_names.iter().copied());

    // Collect instance method info grouped by (class_name, method_name)
    let mut dispatch_table: HashSet<(Identifier, Identifier)> = HashSet::new();

    // Single source of truth for synthetic [`ExprId`] allocation
    // (Proposal 0167 Part 6). Resuming past the max id already present in
    // `statements` guarantees no collision with parser-assigned ids, and
    // the same allocator threaded through every synthesis site below
    // guarantees no collision *between* generated nodes either.
    let mut synth_expr_ids = ExprIdGen::resuming_past_statements(statements);

    generate_from_statements(
        statements,
        class_env,
        interner,
        &mut generated,
        &mut dispatch_table,
        &mut synth_expr_ids,
    );
    if options.include_builtin_instances && needs_builtin_dispatch_support(statements) {
        generate_builtin_instance_functions(
            statements,
            class_env,
            interner,
            &mut generated,
            &mut dispatch_table,
            &mut reserved_names,
            &mut synth_expr_ids,
        );
    }

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
            if !reserved_names.insert(*method_name) {
                continue;
            }
            // Polymorphic stub: typed params for HM inference. Body is a panic
            // placeholder — monomorphic calls resolve to __tc_* at compile time,
            // polymorphic calls go through dictionary elaboration.
            generated.push(generate_polymorphic_stub(
                *method_name,
                class_def,
                method_sig,
                interner,
                &mut synth_expr_ids,
            ));
        }
    }

    // Generate functions for default methods that have no instance override.
    // These are methods with a body in the class declaration (e.g., `neq`).
    generate_default_method_functions(
        statements,
        class_env,
        &dispatch_table,
        &mut generated,
        &mut reserved_names,
    );

    // Pre-intern dictionary names (__dict_{Class}_{Type}) for later use
    // by the dictionary elaboration pass (Proposal 0145, Step 5b).
    pre_intern_dict_names(class_env, interner);

    generated
}

fn collect_existing_function_names(statements: &[Statement]) -> HashSet<Identifier> {
    let mut names = HashSet::new();
    collect_existing_function_names_into(statements, &mut names);
    names
}

fn collect_existing_function_names_into(statements: &[Statement], names: &mut HashSet<Identifier>) {
    for stmt in statements {
        match stmt {
            Statement::Function { name, body, .. } => {
                names.insert(*name);
                collect_existing_function_names_into(&body.statements, names);
            }
            Statement::Module { body, .. } => {
                collect_existing_function_names_into(&body.statements, names);
            }
            _ => {}
        }
    }
}

fn needs_builtin_dispatch_support(statements: &[Statement]) -> bool {
    statements.iter().any(|stmt| match stmt {
        Statement::Class { .. } | Statement::Instance { .. } => true,
        Statement::Data { deriving, .. } => !deriving.is_empty(),
        Statement::Function {
            type_params, body, ..
        } => {
            type_params.iter().any(|tp| !tp.constraints.is_empty())
                || needs_builtin_dispatch_support(&body.statements)
        }
        Statement::Module { body, .. } => needs_builtin_dispatch_support(&body.statements),
        _ => false,
    })
}

fn generate_builtin_instance_functions(
    statements: &[Statement],
    class_env: &ClassEnv,
    interner: &mut Interner,
    generated: &mut Vec<Statement>,
    dispatch_table: &mut HashSet<(Identifier, Identifier)>,
    reserved_names: &mut HashSet<Identifier>,
    builtin_expr_ids: &mut ExprIdGen,
) {
    let adts = collect_data_declarations(statements, interner);
    for instance in &class_env.instances {
        if instance.span != Span::default() || !instance.method_names.is_empty() {
            continue;
        }
        let Some(class_def) = class_env.lookup_class_by_id(instance.class_id) else {
            continue;
        };
        let type_name = instance
            .type_args
            .iter()
            .map(|a| a.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");
        let class_name_str = interner.resolve(instance.class_name).to_string();

        for method_sig in &class_def.methods {
            let method_name_str = interner.resolve(method_sig.name).to_string();
            let derived_json =
                find_adt_info_for_instance(instance, &adts, interner).and_then(|adt| {
                    derived_json_method_body(adt, &method_name_str, interner, builtin_expr_ids)
                });
            let body = if is_json_codec_class(&class_name_str) {
                let Some(body) = derived_json else {
                    continue;
                };
                body
            } else {
                let Some(body) = builtin_method_body(
                    interner,
                    builtin_expr_ids,
                    &class_name_str,
                    &type_name,
                    &method_name_str,
                ) else {
                    continue;
                };
                body
            };

            let mangled = crate::types::class_env::mangled_method_name(
                &class_name_str,
                &type_name,
                &method_name_str,
            );
            let mangled_sym = interner.intern(&mangled);
            if !reserved_names.insert(mangled_sym) {
                dispatch_table.insert((instance.class_name, method_sig.name));
                continue;
            }
            let mut parameter_types: Vec<Option<TypeExpr>> = vec![None; instance.context.len()];
            parameter_types.extend(method_sig.param_types.iter().map(|ty| {
                Some(specialize_type_expr(
                    ty,
                    &class_def.type_params,
                    &instance.type_args,
                    interner,
                ))
            }));
            let mut params = context_dict_param_names(&instance.context, interner);
            params.extend(builtin_param_names(method_sig.arity, interner));

            generated.push(Statement::Function {
                is_public: false,
                intrinsic: None,
                fip: None,
                name: mangled_sym,
                type_params: build_instance_function_type_params(
                    &instance.type_args,
                    &instance.context,
                    method_sig,
                    interner,
                ),
                parameters: params,
                parameter_types,
                return_type: Some(specialize_type_expr(
                    &method_sig.return_type,
                    &class_def.type_params,
                    &instance.type_args,
                    interner,
                )),
                // Built-in instance bodies are pure intrinsics today; if a
                // built-in class ever gains a `with` clause, this carries it.
                effects: method_sig.effects.clone(),
                body,
                span: Span::default(),
            });
            dispatch_table.insert((instance.class_name, method_sig.name));
        }
    }
}

#[derive(Clone)]
struct DeriveAdtInfo {
    variants: Vec<DataVariant>,
}

fn collect_data_declarations(
    statements: &[Statement],
    interner: &Interner,
) -> HashMap<String, DeriveAdtInfo> {
    let mut out = HashMap::new();
    collect_data_declarations_into(statements, interner, &mut out);
    out
}

fn collect_data_declarations_into(
    statements: &[Statement],
    interner: &Interner,
    out: &mut HashMap<String, DeriveAdtInfo>,
) {
    for stmt in statements {
        match stmt {
            Statement::Data { name, variants, .. } => {
                out.insert(
                    interner.resolve(*name).to_string(),
                    DeriveAdtInfo {
                        variants: variants.clone(),
                    },
                );
            }
            Statement::Module { body, .. } => {
                collect_data_declarations_into(&body.statements, interner, out);
            }
            _ => {}
        }
    }
}

fn find_adt_info_for_instance<'a>(
    instance: &crate::types::class_env::InstanceDef,
    adts: &'a HashMap<String, DeriveAdtInfo>,
    interner: &Interner,
) -> Option<&'a DeriveAdtInfo> {
    let TypeExpr::Named { name, .. } = instance.type_args.first()? else {
        return None;
    };
    adts.get(interner.resolve(*name))
}

fn is_json_codec_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Encode" | "Decode" | "Json.Encode" | "Json.Decode"
    )
}

fn derived_json_method_body(
    adt: &DeriveAdtInfo,
    method_name: &str,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Option<Block> {
    let expr = match method_name {
        "encode" => derived_json_encode_expr(adt, interner),
        "decode" => derived_json_decode_expr(adt, interner),
        _ => return None,
    };
    let body = parse_generated_function_body(&expr, interner)?;
    Some(refresh_block_expr_ids(body, id_gen))
}

fn parse_generated_function_body(body_expr: &str, interner: &mut Interner) -> Option<Block> {
    let source = format!("fn __json_derive_dummy(__x0) {{ {body_expr} }}");
    let mut parser = Parser::new(Lexer::new_with_interner(source.clone(), interner.clone()));
    let program = parser.parse_program();
    *interner = parser.take_interner();
    if !parser.errors.is_empty() {
        return None;
    }
    program.statements.into_iter().find_map(|stmt| match stmt {
        Statement::Function { body, .. } => Some(body),
        _ => None,
    })
}

fn refresh_block_expr_ids(block: Block, id_gen: &mut ExprIdGen) -> Block {
    Block {
        statements: block
            .statements
            .into_iter()
            .map(|stmt| refresh_stmt_expr_ids(stmt, id_gen))
            .collect(),
        span: block.span,
    }
}

fn refresh_stmt_expr_ids(stmt: Statement, id_gen: &mut ExprIdGen) -> Statement {
    match stmt {
        Statement::Let {
            is_public,
            name,
            type_annotation,
            value,
            span,
        } => Statement::Let {
            is_public,
            name,
            type_annotation,
            value: refresh_expr_ids(value, id_gen),
            span,
        },
        Statement::Return { value, span } => Statement::Return {
            value: value.map(|value| refresh_expr_ids(value, id_gen)),
            span,
        },
        Statement::Expression {
            expression,
            has_semicolon,
            span,
        } => Statement::Expression {
            expression: refresh_expr_ids(expression, id_gen),
            has_semicolon,
            span,
        },
        other => other,
    }
}

fn refresh_expr_ids(expr: Expression, id_gen: &mut ExprIdGen) -> Expression {
    match expr {
        Expression::Identifier { name, span, .. } => Expression::Identifier {
            name,
            span,
            id: id_gen.next_id(),
        },
        Expression::Integer { value, span, .. } => Expression::Integer {
            value,
            span,
            id: id_gen.next_id(),
        },
        Expression::Float { value, span, .. } => Expression::Float {
            value,
            span,
            id: id_gen.next_id(),
        },
        Expression::String { value, span, .. } => Expression::String {
            value,
            span,
            id: id_gen.next_id(),
        },
        Expression::InterpolatedString { parts, span, .. } => Expression::InterpolatedString {
            parts: parts
                .into_iter()
                .map(|part| match part {
                    crate::syntax::expression::StringPart::Literal(text) => {
                        crate::syntax::expression::StringPart::Literal(text)
                    }
                    crate::syntax::expression::StringPart::Interpolation(expr) => {
                        crate::syntax::expression::StringPart::Interpolation(Box::new(
                            refresh_expr_ids(*expr, id_gen),
                        ))
                    }
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::Boolean { value, span, .. } => Expression::Boolean {
            value,
            span,
            id: id_gen.next_id(),
        },
        Expression::Prefix {
            operator,
            right,
            span,
            ..
        } => Expression::Prefix {
            operator,
            right: Box::new(refresh_expr_ids(*right, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Infix {
            left,
            operator,
            right,
            span,
            ..
        } => Expression::Infix {
            left: Box::new(refresh_expr_ids(*left, id_gen)),
            operator,
            right: Box::new(refresh_expr_ids(*right, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::If {
            condition,
            consequence,
            alternative,
            span,
            ..
        } => Expression::If {
            condition: Box::new(refresh_expr_ids(*condition, id_gen)),
            consequence: refresh_block_expr_ids(consequence, id_gen),
            alternative: alternative.map(|block| refresh_block_expr_ids(block, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::DoBlock { block, span, .. } => Expression::DoBlock {
            block: refresh_block_expr_ids(block, id_gen),
            span,
            id: id_gen.next_id(),
        },
        Expression::Function {
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => Expression::Function {
            parameters,
            parameter_types,
            return_type,
            effects,
            body: refresh_block_expr_ids(body, id_gen),
            span,
            id: id_gen.next_id(),
        },
        Expression::Call {
            function,
            arguments,
            span,
            ..
        } => Expression::Call {
            function: Box::new(refresh_expr_ids(*function, id_gen)),
            arguments: arguments
                .into_iter()
                .map(|arg| refresh_expr_ids(arg, id_gen))
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::ListLiteral { elements, span, .. } => Expression::ListLiteral {
            elements: elements
                .into_iter()
                .map(|elem| refresh_expr_ids(elem, id_gen))
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::ArrayLiteral { elements, span, .. } => Expression::ArrayLiteral {
            elements: elements
                .into_iter()
                .map(|elem| refresh_expr_ids(elem, id_gen))
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::TupleLiteral { elements, span, .. } => Expression::TupleLiteral {
            elements: elements
                .into_iter()
                .map(|elem| refresh_expr_ids(elem, id_gen))
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::EmptyList { span, .. } => Expression::EmptyList {
            span,
            id: id_gen.next_id(),
        },
        Expression::Index {
            left, index, span, ..
        } => Expression::Index {
            left: Box::new(refresh_expr_ids(*left, id_gen)),
            index: Box::new(refresh_expr_ids(*index, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Hash { pairs, span, .. } => Expression::Hash {
            pairs: pairs
                .into_iter()
                .map(|(key, value)| {
                    (
                        refresh_expr_ids(key, id_gen),
                        refresh_expr_ids(value, id_gen),
                    )
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::MemberAccess {
            object,
            member,
            span,
            ..
        } => Expression::MemberAccess {
            object: Box::new(refresh_expr_ids(*object, id_gen)),
            member,
            span,
            id: id_gen.next_id(),
        },
        Expression::TupleFieldAccess {
            object,
            index,
            span,
            ..
        } => Expression::TupleFieldAccess {
            object: Box::new(refresh_expr_ids(*object, id_gen)),
            index,
            span,
            id: id_gen.next_id(),
        },
        Expression::Match {
            scrutinee,
            arms,
            span,
            ..
        } => Expression::Match {
            scrutinee: Box::new(refresh_expr_ids(*scrutinee, id_gen)),
            arms: arms
                .into_iter()
                .map(|arm| crate::syntax::expression::MatchArm {
                    pattern: arm.pattern,
                    guard: arm.guard.map(|guard| refresh_expr_ids(guard, id_gen)),
                    body: refresh_expr_ids(arm.body, id_gen),
                    span: arm.span,
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::None { span, .. } => Expression::None {
            span,
            id: id_gen.next_id(),
        },
        Expression::Some { value, span, .. } => Expression::Some {
            value: Box::new(refresh_expr_ids(*value, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Left { value, span, .. } => Expression::Left {
            value: Box::new(refresh_expr_ids(*value, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Right { value, span, .. } => Expression::Right {
            value: Box::new(refresh_expr_ids(*value, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Cons {
            head, tail, span, ..
        } => Expression::Cons {
            head: Box::new(refresh_expr_ids(*head, id_gen)),
            tail: Box::new(refresh_expr_ids(*tail, id_gen)),
            span,
            id: id_gen.next_id(),
        },
        Expression::Perform {
            effect,
            operation,
            args,
            span,
            ..
        } => Expression::Perform {
            effect,
            operation,
            args: args
                .into_iter()
                .map(|arg| refresh_expr_ids(arg, id_gen))
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::Handle {
            expr,
            effect,
            parameter,
            arms,
            span,
            ..
        } => Expression::Handle {
            expr: Box::new(refresh_expr_ids(*expr, id_gen)),
            effect,
            parameter: parameter.map(|param| Box::new(refresh_expr_ids(*param, id_gen))),
            arms: arms
                .into_iter()
                .map(|arm| crate::syntax::expression::HandleArm {
                    operation_name: arm.operation_name,
                    resume_param: arm.resume_param,
                    params: arm.params,
                    body: refresh_expr_ids(arm.body, id_gen),
                    span: arm.span,
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::Sealing {
            expr,
            allowed,
            span,
            ..
        } => Expression::Sealing {
            expr: Box::new(refresh_expr_ids(*expr, id_gen)),
            allowed,
            span,
            id: id_gen.next_id(),
        },
        Expression::NamedConstructor {
            name, fields, span, ..
        } => Expression::NamedConstructor {
            name,
            fields: fields
                .into_iter()
                .map(|field| crate::syntax::expression::NamedFieldInit {
                    name: field.name,
                    value: field
                        .value
                        .map(|value| Box::new(refresh_expr_ids(*value, id_gen))),
                    span: field.span,
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
        Expression::Spread {
            base,
            overrides,
            span,
            ..
        } => Expression::Spread {
            base: Box::new(refresh_expr_ids(*base, id_gen)),
            overrides: overrides
                .into_iter()
                .map(|field| crate::syntax::expression::NamedFieldInit {
                    name: field.name,
                    value: field
                        .value
                        .map(|value| Box::new(refresh_expr_ids(*value, id_gen))),
                    span: field.span,
                })
                .collect(),
            span,
            id: id_gen.next_id(),
        },
    }
}

fn derived_json_encode_expr(adt: &DeriveAdtInfo, interner: &Interner) -> String {
    let arms = adt
        .variants
        .iter()
        .map(|variant| {
            let ctor = interner.resolve(variant.name);
            let binders = (0..variant.fields.len())
                .map(|idx| format!("__f{idx}"))
                .collect::<Vec<_>>();
            let pattern = if let Some(names) = &variant.field_names {
                let fields = names
                    .iter()
                    .zip(binders.iter())
                    .map(|(name, binder)| format!("{}: {binder}", interner.resolve(*name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{ctor} {{ {fields} }}")
            } else if binders.is_empty() {
                ctor.to_string()
            } else {
                format!("{ctor}({})", binders.join(", "))
            };
            let fields = if let Some(names) = &variant.field_names {
                let map_expr = names
                    .iter()
                    .enumerate()
                    .map(|(idx, name)| {
                        let binder = &binders[idx];
                        let value = json_encode_value_expr(&variant.fields[idx], binder, interner);
                        format!("\"{}\": {value}", interner.resolve(*name))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Json.object({{{map_expr}}})")
            } else {
                let values = binders
                    .iter()
                    .enumerate()
                    .map(|(idx, binder)| {
                        json_encode_value_expr(&variant.fields[idx], binder, interner)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Json.array([|{values}|])")
            };
            format!(
                "{pattern} -> Json.object({{\"tag\": Json.string(\"{ctor}\"), \"fields\": {fields}}})"
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("match __x0 {{ {arms} }}")
}

fn derived_json_decode_expr(adt: &DeriveAdtInfo, interner: &Interner) -> String {
    let tag_arms = adt
        .variants
        .iter()
        .map(|variant| {
            let ctor = interner.resolve(variant.name);
            format!("\"{ctor}\" -> {}", decode_variant_expr(variant, interner))
        })
        .chain(std::iter::once(
            "_ -> Json.err(\"$.tag\", \"unknown constructor tag\")".to_string(),
        ))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "Json.and_then(Json.as_object(__x0, \"$\"), fn(__obj) {{
            Json.and_then(Json.object_get(__obj, \"tag\", \"$.tag\"), fn(__tag_json) {{
                Json.and_then(Json.as_string(__tag_json, \"$.tag\"), fn(__tag) {{
                    match __tag {{ {tag_arms} }}
                }})
            }})
        }})"
    )
}

fn decode_variant_expr(variant: &DataVariant, interner: &Interner) -> String {
    if let Some(names) = &variant.field_names {
        let body = names
            .iter()
            .enumerate()
            .rev()
            .fold(named_constructor_ok_expr(variant, names, interner), |inner, (idx, name)| {
                let field_name = interner.resolve(*name);
                let decoded = json_decode_value_expr(
                    &variant.fields[idx],
                    &format!("__j{idx}"),
                    &format!("$.fields.{field_name}"),
                    interner,
                );
                format!(
                    "Json.and_then(Json.object_get(__field_obj, \"{field_name}\", \"$.fields.{field_name}\"), fn(__j{idx}) {{
                        Json.and_then({decoded}, fn(__v{idx}) {{ {inner} }})
                    }})"
                )
            });
        format!(
            "Json.and_then(Json.object_get(__obj, \"fields\", \"$.fields\"), fn(__fields_json) {{
                Json.and_then(Json.as_object(__fields_json, \"$.fields\"), fn(__field_obj) {{ {body} }})
            }})"
        )
    } else {
        let expected = variant.fields.len();
        let body = (0..expected).rev().fold(
            positional_constructor_ok_expr(variant, expected, interner),
            |inner, idx| {
                let decoded = json_decode_value_expr(
                    &variant.fields[idx],
                    &format!("__j{idx}"),
                    &format!("$.fields[{idx}]"),
                    interner,
                );
                format!(
                    "Json.and_then(Json.array_get(__field_arr, {idx}, \"$.fields[{idx}]\"), fn(__j{idx}) {{
                        Json.and_then({decoded}, fn(__v{idx}) {{ {inner} }})
                    }})"
                )
            },
        );
        format!(
            "Json.and_then(Json.object_get(__obj, \"fields\", \"$.fields\"), fn(__fields_json) {{
                Json.and_then(Json.as_array(__fields_json, \"$.fields\"), fn(__field_arr) {{
                    if len(__field_arr) == {expected} {{ {body} }} else {{ Json.err(\"$.fields\", \"wrong field count\") }}
                }})
            }})"
        )
    }
}

fn json_encode_value_expr(ty: &TypeExpr, value: &str, interner: &Interner) -> String {
    match json_primitive_type_name(ty, interner).as_deref() {
        Some("String") => format!("Json.string({value})"),
        Some("Bool") => format!("Json.bool({value})"),
        Some("Int") => format!("Json.int({value})"),
        Some("Float") => format!("Json.number({value})"),
        Some("Json") => value.to_string(),
        _ => format!("encode({value})"),
    }
}

fn json_decode_value_expr(ty: &TypeExpr, value: &str, path: &str, interner: &Interner) -> String {
    match json_primitive_type_name(ty, interner).as_deref() {
        Some("String") => format!("Json.as_string({value}, \"{path}\")"),
        Some("Bool") => format!("Json.as_bool({value}, \"{path}\")"),
        Some("Int") => format!("Json.as_int({value}, \"{path}\")"),
        Some("Float") => format!("Json.as_float({value}, \"{path}\")"),
        Some("Json") => format!("Json.ok({value})"),
        _ => format!("decode({value})"),
    }
}

fn json_primitive_type_name(ty: &TypeExpr, interner: &Interner) -> Option<String> {
    let TypeExpr::Named { name, args, .. } = ty else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let name = interner.resolve(*name);
    let short = name.rsplit('.').next().unwrap_or(name);
    matches!(short, "String" | "Bool" | "Int" | "Float" | "Json").then(|| short.to_string())
}

fn positional_constructor_ok_expr(
    variant: &DataVariant,
    arity: usize,
    interner: &Interner,
) -> String {
    let ctor = interner.resolve(variant.name);
    if arity == 0 {
        format!("Json.ok({ctor})")
    } else {
        let args = (0..arity)
            .map(|idx| format!("__v{idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("Json.ok({ctor}({args}))")
    }
}

fn named_constructor_ok_expr(
    variant: &DataVariant,
    names: &[Identifier],
    interner: &Interner,
) -> String {
    let ctor = interner.resolve(variant.name);
    let fields = names
        .iter()
        .enumerate()
        .map(|(idx, name)| format!("{}: __v{idx}", interner.resolve(*name)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Json.ok({ctor} {{ {fields} }})")
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

    // Dictionary *parameter* names, which carry a per-class occurrence suffix
    // so a function holding several dictionaries for one class can tell them
    // apart. Elaboration runs with `&Interner` and can only `lookup`, so an
    // un-interned name silently degrades to the class name and two parameters
    // collide — that was KI-052. A function needing more than a few
    // dictionaries for a single class is pathological, so a small fixed range
    // covers every realistic signature.
    let classes: Vec<Identifier> = class_env.classes.values().map(|class| class.name).collect();
    for class_name in classes {
        let class_str = interner.resolve(class_name).to_string();
        interner.intern(&format!("__dict_{class_str}"));
        for occurrence in 1..MAX_DICT_PARAMS_PER_CLASS {
            interner.intern(&format!("__dict_{class_str}_{occurrence}"));
        }
    }
}

/// How many dictionaries for a single class one signature may hold.
///
/// Only bounds the *pre-interned parameter names*; nothing rejects a signature
/// that exceeds it, it simply would not find its later parameter names.
const MAX_DICT_PARAMS_PER_CLASS: usize = 8;

/// Generate top-level functions for default class methods that have no explicit
/// instance implementation anywhere. E.g., `neq` with default body `{ !eq(x, y) }`.
fn generate_default_method_functions(
    statements: &[Statement],
    _class_env: &ClassEnv,
    dispatch_table: &HashSet<(Identifier, Identifier)>,
    generated: &mut Vec<Statement>,
    reserved_names: &mut HashSet<Identifier>,
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
                        if !has_instances && reserved_names.insert(method.name) {
                            // Generate a regular top-level function from the
                            // default body only when there are no instance
                            // implementations at all for this method.
                            generated.push(Statement::Function {
                                is_public: false,
                                intrinsic: None,
                                fip: None,
                                name: method.name,
                                type_params: vec![],
                                parameters: method.params.clone(),
                                parameter_types: vec![None; method.params.len()],
                                return_type: None,
                                effects: method.effects.clone(),
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
                    reserved_names,
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
    id_gen: &mut ExprIdGen,
) {
    fn resolve_instance_class_def<'a>(
        class_env: &'a ClassEnv,
        class_name: Identifier,
        interner: &Interner,
    ) -> Option<&'a crate::types::class_env::ClassDef> {
        if let Some(class_def) = class_env.lookup_class(class_name) {
            return Some(class_def);
        }

        let wanted = interner.try_resolve(class_name)?;
        let wanted_short = wanted.rsplit('.').next().unwrap_or(wanted);

        class_env.classes.values().find(|class_def| {
            let Some(candidate_short) = interner.try_resolve(class_def.name) else {
                return false;
            };
            if candidate_short == wanted || candidate_short == wanted_short {
                return true;
            }

            class_def
                .module
                .as_identifier()
                .and_then(|module| interner.try_resolve(module))
                .is_some_and(|module| {
                    module == wanted || format!("{module}.{candidate_short}") == wanted
                })
        })
    }

    for stmt in statements {
        match stmt {
            Statement::Instance {
                class_name,
                type_args,
                context,
                methods,
                ..
            } => {
                let Some(class_def) = resolve_instance_class_def(class_env, *class_name, interner)
                else {
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

                let resolved_class_name = class_def.name;
                let class_name_str = interner.resolve(resolved_class_name).to_string();

                let explicit_methods: HashMap<Identifier, _> =
                    methods.iter().map(|m| (m.name, m)).collect();

                for method_sig in &class_def.methods {
                    let explicit_method = explicit_methods.get(&method_sig.name).copied();
                    let body = if let Some(method) = explicit_method {
                        // The generated mangled method is a second AST copy of
                        // the source instance method.  It must not reuse the
                        // source expression IDs: HM stores one inferred type
                        // per ID, and Core lowering uses that map to resolve
                        // contextual class calls.  Reusing IDs lets the
                        // source and generated copies overwrite each other,
                        // which is how an element call such as
                        // `encode(value)` was lowered as a recursive container
                        // call in Flow.Json (KI-051).
                        refresh_block_expr_ids(method.body.clone(), id_gen)
                    } else if let Some(default_body) = &method_sig.default_body {
                        // A default body is cloned into every instance, so each
                        // copy needs its own ExprIds: typed dispatch keys on
                        // `hm_expr_types[expr_id]`, and shared ids would let the
                        // last instance inferred decide dispatch for all of them.
                        refresh_block_expr_ids(default_body.clone(), id_gen)
                    } else {
                        continue;
                    };

                    // Generate mangled name: __tc_ClassName_TypeName_methodName
                    let method_name_str = interner.resolve(method_sig.name).to_string();
                    let mangled = crate::types::class_env::mangled_method_name(
                        &class_name_str,
                        &type_name,
                        &method_name_str,
                    );
                    let mangled_sym = interner.intern(&mangled);

                    let context_params = context_dict_param_names(context, interner);
                    let mut parameters = context_params.clone();
                    let value_parameters = explicit_method
                        .map(|method| method.params.clone())
                        .unwrap_or_else(|| method_sig.param_names.clone());
                    parameters.extend(value_parameters);

                    let mut parameter_types: Vec<Option<TypeExpr>> = vec![None; context.len()];
                    parameter_types.extend(
                        method_sig
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
                            .collect::<Vec<_>>(),
                    );
                    let return_type = Some(specialize_type_expr(
                        &method_sig.return_type,
                        &class_def.type_params,
                        type_args,
                        interner,
                    ));
                    let type_params = build_instance_function_type_params(
                        type_args, context, method_sig, interner,
                    );

                    // Proposal 0151, Phase 4a: forward the instance method's
                    // declared effect row so the synthesized function's
                    // inferred type carries it, and so callers that resolve
                    // through this instance see the row.
                    let inferred_effects = explicit_method
                        .filter(|method| !method.effects.is_empty())
                        .map(|method| method.effects.clone())
                        .unwrap_or_else(|| method_sig.effects.clone());

                    let fn_stmt = Statement::Function {
                        is_public: false,
                        intrinsic: None,
                        fip: None,
                        name: mangled_sym,
                        type_params,
                        parameters,
                        parameter_types,
                        return_type,
                        effects: inferred_effects,
                        body,
                        span: Span::default(),
                    };
                    generated.push(fn_stmt);

                    // Record that this (class, method) pair has an instance.
                    dispatch_table.insert((resolved_class_name, method_sig.name));
                }
            }
            Statement::Module { body, .. } => {
                generate_from_statements(
                    &body.statements,
                    class_env,
                    interner,
                    generated,
                    dispatch_table,
                    id_gen,
                );
            }
            _ => {}
        }
    }
}

fn builtin_param_names(arity: usize, interner: &mut Interner) -> Vec<Identifier> {
    (0..arity)
        .map(|idx| interner.intern(&format!("__x{idx}")))
        .collect()
}

fn context_dict_param_names(
    context: &[ClassConstraint],
    interner: &mut Interner,
) -> Vec<Identifier> {
    let mut seen: HashMap<Identifier, usize> = HashMap::new();
    context
        .iter()
        .map(|constraint| {
            let class_name = interner.resolve(constraint.class_name);
            let count = seen.entry(constraint.class_name).or_insert(0);
            let suffix = if *count == 0 {
                String::new()
            } else {
                format!("_{}", *count)
            };
            *count += 1;
            interner.intern(&format!("__dict_{class_name}{suffix}"))
        })
        .collect()
}

fn builtin_method_body(
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
    class_name: &str,
    type_name: &str,
    method_name: &str,
) -> Option<Block> {
    fn var(id_gen: &mut ExprIdGen, name: Identifier, span: Span) -> Expression {
        Expression::Identifier {
            name,
            span,
            id: id_gen.next_id(),
        }
    }

    fn int(id_gen: &mut ExprIdGen, value: i64, span: Span) -> Expression {
        Expression::Integer {
            value,
            span,
            id: id_gen.next_id(),
        }
    }

    fn infix(
        id_gen: &mut ExprIdGen,
        left: Expression,
        operator: &str,
        right: Expression,
        span: Span,
    ) -> Expression {
        Expression::Infix {
            left: Box::new(left),
            operator: operator.to_string(),
            right: Box::new(right),
            span,
            id: id_gen.next_id(),
        }
    }

    fn ret(expression: Expression, span: Span) -> Block {
        Block {
            statements: vec![Statement::Expression {
                expression,
                has_semicolon: false,
                span,
            }],
            span,
        }
    }

    fn call(
        id_gen: &mut ExprIdGen,
        interner: &mut Interner,
        name: &str,
        arguments: Vec<Expression>,
        span: Span,
    ) -> Expression {
        Expression::Call {
            function: Box::new(Expression::Identifier {
                name: interner.intern(name),
                span,
                id: id_gen.next_id(),
            }),
            arguments,
            span,
            id: id_gen.next_id(),
        }
    }

    let span = Span::default();
    let x = interner.intern("__x0");
    let y = interner.intern("__x1");

    let expression = match (class_name, type_name, method_name) {
        ("Eq", _, "eq") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "==", rhs, span)
        }
        ("Eq", _, "neq") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "!=", rhs, span)
        }
        ("Ord", _, "compare") => {
            let lt_lhs = var(id_gen, x, span);
            let lt_rhs = var(id_gen, y, span);
            let gt_lhs = var(id_gen, x, span);
            let gt_rhs = var(id_gen, y, span);
            Expression::If {
                condition: Box::new(infix(id_gen, lt_lhs, "<", lt_rhs, span)),
                consequence: ret(int(id_gen, -1, span), span),
                alternative: Some(ret(
                    Expression::If {
                        condition: Box::new(infix(id_gen, gt_lhs, ">", gt_rhs, span)),
                        consequence: ret(int(id_gen, 1, span), span),
                        alternative: Some(ret(int(id_gen, 0, span), span)),
                        span,
                        id: id_gen.next_id(),
                    },
                    span,
                )),
                span,
                id: id_gen.next_id(),
            }
        }
        ("Ord", _, "lt") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "<", rhs, span)
        }
        ("Ord", _, "lte") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "<=", rhs, span)
        }
        ("Ord", _, "gt") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, ">", rhs, span)
        }
        ("Ord", _, "gte") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, ">=", rhs, span)
        }
        ("Num", _, "add") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "+", rhs, span)
        }
        ("Num", _, "sub") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "-", rhs, span)
        }
        ("Num", _, "mul") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "*", rhs, span)
        }
        ("Num", _, "div") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            infix(id_gen, lhs, "/", rhs, span)
        }
        ("Show", _, "show") => {
            let arg = var(id_gen, x, span);
            call(id_gen, interner, "to_string", vec![arg], span)
        }
        ("Semigroup", "String", "append") => {
            let lhs = var(id_gen, x, span);
            let rhs = var(id_gen, y, span);
            call(id_gen, interner, "string_concat", vec![lhs, rhs], span)
        }
        _ => return None,
    };

    Some(ret(expression, span))
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
            constraints: context
                .iter()
                .filter(|constraint| {
                    constraint
                        .type_args
                        .iter()
                        .any(|arg| type_expr_mentions_type_param(arg, name, interner))
                })
                .cloned()
                .collect(),
        })
        .collect()
}

fn type_expr_mentions_type_param(ty: &TypeExpr, target: Identifier, interner: &Interner) -> bool {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            (*name == target && is_type_param_name(*name, interner))
                || args
                    .iter()
                    .any(|arg| type_expr_mentions_type_param(arg, target, interner))
        }
        TypeExpr::Tuple { elements, .. } => elements
            .iter()
            .any(|elem| type_expr_mentions_type_param(elem, target, interner)),
        TypeExpr::Function { params, ret, .. } => {
            params
                .iter()
                .any(|param| type_expr_mentions_type_param(param, target, interner))
                || type_expr_mentions_type_param(ret, target, interner)
        }
    }
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
    synth_expr_ids: &mut ExprIdGen,
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

    // Body: panic with a descriptive message. This stub exists only to give
    // HM inference a properly typed function signature. Monomorphic calls are
    // resolved directly to __tc_* mangled functions during Core lowering, and
    // polymorphic calls go through dictionary elaboration. The stub body is
    // never executed in well-typed programs.
    //
    // Each nested AST node receives its own fresh id so HM inference's
    // expr-type map keys stay unique (Proposal 0167 Part 6).
    let method_display = interner.resolve(method_name).to_string();
    let class_display = interner.resolve(class_def.name).to_string();
    let panic_sym = interner.intern("panic");
    let body_expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: panic_sym,
            span,
            id: synth_expr_ids.next_id(),
        }),
        arguments: vec![Expression::String {
            value: format!("No instance of {class_display}.{method_display} for the given type"),
            span,
            id: synth_expr_ids.next_id(),
        }],
        span,
        id: synth_expr_ids.next_id(),
    };

    // The stub body unconditionally calls `panic(...)`, which carries the
    // `Panic` effect. The class method's declared row does not include
    // `Panic` (it is a no-instance fallback, never executed), so we add it
    // to the synthesized stub's row to satisfy the effect checker. Without
    // this, classes whose methods declare a non-empty effect row (e.g.
    // `with Audit`) would emit a spurious E400 for the synthesized stub.
    let mut stub_effects = method_sig.effects.clone();
    let panic_effect_sym = interner.intern(crate::syntax::builtin_effects::PANIC);
    let already_has_panic = stub_effects
        .iter()
        .any(|effect| effect.normalized_names().contains(&panic_effect_sym));
    if !already_has_panic {
        stub_effects.push(crate::syntax::effect_expr::EffectExpr::Named {
            name: panic_effect_sym,
            span,
        });
    }

    Statement::Function {
        is_public: false,
        intrinsic: None,
        fip: None,
        name: method_name,
        type_params,
        parameters: params,
        parameter_types,
        return_type,
        effects: stub_effects,
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
