//! Contextual kind checking for surface type expressions.

use std::collections::HashMap;

use crate::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, compiler_errors::*, diagnostic_for, position::Span,
    },
    syntax::{
        Identifier, interner::Interner, program::Program, statement::Statement, type_expr::TypeExpr,
    },
    types::{
        class_env::{ClassDef, ClassEnv},
        class_id::ClassId,
        kind::Kind,
    },
};

#[derive(Debug, Default)]
pub struct KindEnv {
    constructors: HashMap<Identifier, Kind>,
    class_params: HashMap<ClassId, HashMap<Identifier, Kind>>,
    conflicts: Vec<(Identifier, ClassId, Kind, Kind, Identifier, Span)>,
}

impl KindEnv {
    pub fn from_program(program: &Program, class_env: &ClassEnv, interner: &mut Interner) -> Self {
        let mut env = Self::default();
        for name in [
            "Int", "Float", "Bool", "String", "Unit", "Never", "List", "Array", "Map", "Option",
            "Either", "Result",
        ] {
            let symbol = interner.intern(name);
            env.constructors.insert(symbol, builtin_kind(name));
        }
        collect_data_kinds(&program.statements, &mut env.constructors);
        for class in class_env.classes.values() {
            env.infer_class_parameter_kinds(class);
        }
        env
    }

    fn infer_class_parameter_kinds(&mut self, class: &ClassDef) {
        let mut kinds = class
            .type_params
            .iter()
            .copied()
            .map(|name| (name, Kind::Type))
            .collect::<HashMap<_, _>>();
        let mut observed = HashMap::new();
        for method in &class.methods {
            let method_params = method
                .type_params
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            for ty in method
                .param_types
                .iter()
                .chain(std::iter::once(&method.return_type))
            {
                infer_class_parameter_uses(
                    ty,
                    class.name,
                    class.class_id(),
                    &class.type_params,
                    &method_params,
                    &mut kinds,
                    &mut observed,
                    &mut self.conflicts,
                );
            }
        }
        self.class_params.insert(class.class_id(), kinds);
    }

    pub fn validate_program(
        &self,
        program: &Program,
        class_env: &ClassEnv,
        interner: &Interner,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = self
            .conflicts
            .iter()
            .map(|(class_name, _class_id, first, second, parameter, span)| {
                diagnostic_for(&CLASS_PARAMETER_KIND_CONFLICT)
                    .with_span(*span)
                    .with_message(format!(
                        "Class parameter `{}` is used with incompatible kinds `{}` and `{}` in class `{}`.",
                        interner.resolve(*parameter),
                        first,
                        second,
                        interner.resolve(*class_name)
                    ))
            })
            .collect();
        validate_statements(
            &program.statements,
            self,
            class_env,
            interner,
            &mut diagnostics,
        );
        diagnostics
    }

    fn class_parameter_kind(&self, class: &ClassDef, name: Identifier) -> Kind {
        self.class_params
            .get(&class.class_id())
            .and_then(|params| params.get(&name))
            .cloned()
            .unwrap_or(Kind::Type)
    }

    fn constructor_kind(&self, name: Identifier) -> Option<Kind> {
        self.constructors.get(&name).cloned()
    }

    fn has_conflict(&self, class_id: ClassId, parameter: Identifier) -> bool {
        self.conflicts
            .iter()
            .any(|(_, id, _, _, name, _)| *id == class_id && *name == parameter)
    }
}

fn builtin_kind(name: &str) -> Kind {
    match name {
        "List" | "Array" | "Option" => Kind::type1(),
        "Map" | "Either" | "Result" => Kind::type2(),
        _ => Kind::Type,
    }
}

fn collect_data_kinds(statements: &[Statement], constructors: &mut HashMap<Identifier, Kind>) {
    for statement in statements {
        match statement {
            Statement::Data {
                name, type_params, ..
            } => {
                constructors.insert(*name, kind_from_arity(type_params.len()));
            }
            Statement::Module { body, .. } => collect_data_kinds(&body.statements, constructors),
            _ => {}
        }
    }
}

fn kind_from_arity(arity: usize) -> Kind {
    (0..arity).fold(Kind::Type, |result, _| {
        Kind::Arrow(Box::new(Kind::Type), Box::new(result))
    })
}

// The arguments are deliberately explicit: this helper carries the mutable
// inference state and the immutable class context through a recursive type
// expression walk.
#[allow(clippy::too_many_arguments)]
fn infer_class_parameter_uses(
    ty: &TypeExpr,
    class_name: Identifier,
    class_id: ClassId,
    class_params: &[Identifier],
    method_params: &std::collections::HashSet<Identifier>,
    kinds: &mut HashMap<Identifier, Kind>,
    observed: &mut HashMap<Identifier, Kind>,
    conflicts: &mut Vec<(Identifier, ClassId, Kind, Kind, Identifier, Span)>,
) {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            if class_params.contains(name) && !method_params.contains(name) {
                let used = kind_from_arity(args.len());
                if let Some(existing) = observed.get(name).cloned() {
                    if existing != used {
                        conflicts.push((class_name, class_id, existing, used, *name, ty.span()));
                    }
                } else {
                    observed.insert(*name, used.clone());
                    kinds.insert(*name, used);
                }
            }
            for arg in args {
                infer_class_parameter_uses(
                    arg,
                    class_name,
                    class_id,
                    class_params,
                    method_params,
                    kinds,
                    observed,
                    conflicts,
                );
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            for element in elements {
                infer_class_parameter_uses(
                    element,
                    class_name,
                    class_id,
                    class_params,
                    method_params,
                    kinds,
                    observed,
                    conflicts,
                );
            }
        }
        TypeExpr::Function { params, ret, .. } => {
            for param in params {
                infer_class_parameter_uses(
                    param,
                    class_name,
                    class_id,
                    class_params,
                    method_params,
                    kinds,
                    observed,
                    conflicts,
                );
            }
            infer_class_parameter_uses(
                ret,
                class_name,
                class_id,
                class_params,
                method_params,
                kinds,
                observed,
                conflicts,
            );
        }
    }
}

fn validate_statements(
    statements: &[Statement],
    env: &KindEnv,
    class_env: &ClassEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::Module { body, .. } => {
                validate_statements(&body.statements, env, class_env, interner, diagnostics)
            }
            Statement::Data { variants, .. } => {
                for variant in variants {
                    for field in &variant.fields {
                        check_type(
                            field,
                            Some(&Kind::Type),
                            &HashMap::new(),
                            env,
                            interner,
                            diagnostics,
                        );
                    }
                }
            }
            Statement::Class {
                name,
                type_params,
                superclasses,
                methods,
                span,
                ..
            } => {
                let class = class_env.lookup_class(*name);
                let vars = class
                    .map(|class| {
                        type_params
                            .iter()
                            .copied()
                            .map(|param| (param, class.class_id()))
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                if let Some(class) = class {
                    for superclass in superclasses {
                        validate_constraint(
                            superclass,
                            class,
                            class_env,
                            env,
                            interner,
                            diagnostics,
                        );
                    }
                    for method in methods {
                        let mut local = vars.clone();
                        for param in &method.type_params {
                            local.remove(param);
                        }
                        for ty in method
                            .param_types
                            .iter()
                            .chain(std::iter::once(&method.return_type))
                        {
                            check_type(ty, Some(&Kind::Type), &local, env, interner, diagnostics);
                        }
                    }
                } else {
                    let _ = span;
                }
            }
            Statement::Instance {
                class_name,
                type_args,
                context,
                ..
            } => {
                if let Some(class) = class_env.lookup_class(*class_name) {
                    for (index, arg) in type_args.iter().enumerate() {
                        let expected = class
                            .type_params
                            .get(index)
                            .map(|param| env.class_parameter_kind(class, *param))
                            .unwrap_or(Kind::Type);
                        let actual = check_type(
                            arg,
                            Some(&expected),
                            &HashMap::new(),
                            env,
                            interner,
                            diagnostics,
                        );
                        if actual != expected {
                            diagnostics.push(instance_kind_diagnostic(
                                arg.span(),
                                arg,
                                class,
                                &actual,
                                &expected,
                                interner,
                            ));
                        }
                    }
                    for constraint in context {
                        validate_constraint(
                            constraint,
                            class,
                            class_env,
                            env,
                            interner,
                            diagnostics,
                        );
                    }
                }
            }
            Statement::Function {
                parameter_types,
                return_type,
                span,
                ..
            } => {
                for ty in parameter_types.iter().flatten() {
                    check_type(
                        ty,
                        Some(&Kind::Type),
                        &HashMap::new(),
                        env,
                        interner,
                        diagnostics,
                    );
                }
                if let Some(ty) = return_type {
                    check_type(
                        ty,
                        Some(&Kind::Type),
                        &HashMap::new(),
                        env,
                        interner,
                        diagnostics,
                    );
                }
                let _ = span;
            }
            Statement::TypeAlias(alias) => {
                check_type(
                    &alias.body,
                    Some(&Kind::Type),
                    &HashMap::new(),
                    env,
                    interner,
                    diagnostics,
                );
            }
            _ => {}
        }
    }
}

fn validate_constraint(
    constraint: &crate::syntax::type_class::ClassConstraint,
    _owner: &ClassDef,
    class_env: &ClassEnv,
    env: &KindEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(class) = class_env.lookup_class(constraint.class_name) else {
        return;
    };
    for (index, arg) in constraint.type_args.iter().enumerate() {
        let expected = class
            .type_params
            .get(index)
            .map(|param| env.class_parameter_kind(class, *param))
            .unwrap_or(Kind::Type);
        let actual = check_type(
            arg,
            Some(&expected),
            &HashMap::new(),
            env,
            interner,
            diagnostics,
        );
        if actual != expected {
            diagnostics.push(constraint_kind_diagnostic(
                arg.span(),
                arg,
                class,
                &actual,
                &expected,
                interner,
            ));
        }
    }
}

fn check_type(
    ty: &TypeExpr,
    expected: Option<&Kind>,
    locals: &HashMap<Identifier, ClassId>,
    env: &KindEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) -> Kind {
    let actual = match ty {
        TypeExpr::Named { name, args, .. } => {
            let known_head = locals
                .get(name)
                .and_then(|class_id| {
                    env.class_params
                        .get(class_id)
                        .and_then(|params| params.get(name))
                })
                .cloned()
                .or_else(|| env.constructor_kind(*name));
            let Some(head) = known_head else {
                // Imported constructors are validated against their module
                // interface. The local checker does not have their kind table
                // here, so preserve the existing open-world behavior rather
                // than rejecting a valid imported generic type.
                for arg in args {
                    check_type(arg, Some(&Kind::Type), locals, env, interner, diagnostics);
                }
                return Kind::Type;
            };
            let is_conflicted_parameter = locals
                .get(name)
                .is_some_and(|class_id| env.has_conflict(*class_id, *name));
            if args.len() > head.arity() && !is_conflicted_parameter {
                diagnostics.push(arity_diagnostic(
                    ty.span(),
                    *name,
                    head.arity(),
                    args.len(),
                    interner,
                ));
            }
            let mut result = head;
            for arg in args {
                check_type(arg, Some(&Kind::Type), locals, env, interner, diagnostics);
                if let Kind::Arrow(_, rest) = result {
                    result = *rest;
                }
            }
            result
        }
        TypeExpr::Tuple { elements, .. } => {
            for element in elements {
                check_type(
                    element,
                    Some(&Kind::Type),
                    locals,
                    env,
                    interner,
                    diagnostics,
                );
            }
            Kind::Type
        }
        TypeExpr::Function { params, ret, .. } => {
            for param in params {
                check_type(param, Some(&Kind::Type), locals, env, interner, diagnostics);
            }
            check_type(ret, Some(&Kind::Type), locals, env, interner, diagnostics);
            Kind::Type
        }
    };
    if let Some(expected) = expected
        && &actual != expected
        && matches!(expected, Kind::Type)
        && let TypeExpr::Named { name, .. } = ty
        && !locals
            .get(name)
            .is_some_and(|class_id| env.has_conflict(*class_id, *name))
    {
        diagnostics.push(arity_diagnostic(
            ty.span(),
            *name,
            actual.arity(),
            0,
            interner,
        ));
    }
    actual
}

fn arity_diagnostic(
    span: Span,
    name: Identifier,
    expected: usize,
    actual: usize,
    interner: &Interner,
) -> Diagnostic {
    diagnostic_for(&TYPE_CONSTRUCTOR_KIND_ARITY)
        .with_span(span)
        .with_message(format!(
            "Type `{}` expects {} type argument(s), but {} were given.",
            interner.resolve(name),
            expected,
            actual
        ))
}

fn instance_kind_diagnostic(
    span: Span,
    arg: &TypeExpr,
    class: &ClassDef,
    actual: &Kind,
    expected: &Kind,
    interner: &Interner,
) -> Diagnostic {
    diagnostic_for(&INSTANCE_HEAD_KIND_MISMATCH)
        .with_span(span)
        .with_message(format!(
            "Instance head `{}` has kind `{}`, but class `{}` expects a parameter of kind `{}`.",
            arg.display_with(interner),
            actual,
            interner.resolve(class.name),
            expected
        ))
}

fn constraint_kind_diagnostic(
    span: Span,
    arg: &TypeExpr,
    class: &ClassDef,
    actual: &Kind,
    expected: &Kind,
    interner: &Interner,
) -> Diagnostic {
    diagnostic_for(&CONSTRAINT_KIND_MISMATCH)
        .with_span(span)
        .with_message(format!(
            "Constraint `{}` applies class `{}` to a type of kind `{}`, but it expects `{}`.",
            arg.display_with(interner),
            interner.resolve(class.name),
            actual,
            expected
        ))
}
