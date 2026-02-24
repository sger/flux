use std::collections::{HashMap, HashSet};

use crate::{
    runtime::runtime_type::RuntimeType,
    syntax::{Identifier, interner::Interner, type_expr::TypeExpr},
    types::{
        TypeVarId, infer_type::InferType, scheme::Scheme, type_constructor::TypeConstructor,
        type_subst::TypeSubst,
    },
};

/// Scoped type environment mapping identifiers to their type schemes.
///
/// Supports nested scopes (function bodies, let bindings) and tracks a fresh
/// type-variable counter used throughout the inference pass.
#[derive(Debug, Clone)]
pub struct TypeEnv {
    scopes: Vec<HashMap<Identifier, Scheme>>,
    pub counter: u32,
}

impl TypeEnv {
    pub fn new() -> Self {
        TypeEnv {
            scopes: vec![HashMap::new()],
            counter: 0,
        }
    }

    /// Allocate a fresh type variable.
    pub fn fresh(&mut self) -> TypeVarId {
        let v = self.counter;
        self.counter += 1;
        v
    }

    /// Allocate a fresh `InferType::Var`.
    pub fn fresh_infer_type(&mut self) -> InferType {
        InferType::Var(self.fresh())
    }

    /// Bind a name to a scheme in the current (innermost) scope.
    pub fn bind(&mut self, name: Identifier, scheme: Scheme) {
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name, scheme);
    }

    /// Look up a name, searching from innermost to outermost scope.
    pub fn lookup(&self, name: Identifier) -> Option<&Scheme> {
        for scope in self.scopes.iter().rev() {
            if let Some(s) = scope.get(&name) {
                return Some(s);
            }
        }
        None
    }

    /// Push a new empty scope.
    pub fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the innermost scope.
    pub fn leave_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// All free type variables present anywhere in the environment.
    ///
    /// Used by `generalize` to avoid quantifying variables that are still
    /// constrained by the surrounding context.
    pub fn free_vars(&self) -> HashSet<TypeVarId> {
        let mut set = HashSet::new();
        for scope in &self.scopes {
            for scheme in scope.values() {
                set.extend(scheme.free_vars());
            }
        }
        set
    }

    // -------------------------------------------------------------------------
    // Bridge helpers
    // -------------------------------------------------------------------------

    /// Convert a `TypeExpr` (surface annotation AST) to `Ty`.
    ///
    /// `type_params` maps the names of generic type parameters declared on the
    /// current function (`fn f<T>`) to their allocated `TyVar` numbers.
    ///
    /// Returns `None` only for unknown named types that aren't type parameters.
    pub fn infer_type_from_type_expr(
        type_expr: &TypeExpr,
        type_params: &HashMap<Identifier, TypeVarId>,
        interner: &Interner,
    ) -> Option<InferType> {
        match type_expr {
            TypeExpr::Named { name, args, .. } => {
                let name_str = interner.resolve(*name);
                // Check if this name is a generic type parameter
                if let Some(&v) = type_params.get(name) {
                    // Generic params should not have args: `T<X>` is nonsensical
                    return Some(InferType::Var(v));
                }

                // Resolve the constructor
                let con = match name_str {
                    "Int" => TypeConstructor::Int,
                    "Float" => TypeConstructor::Float,
                    "Bool" => TypeConstructor::Bool,
                    "String" => TypeConstructor::String,
                    "Unit" | "None" => TypeConstructor::Unit,
                    "Never" => TypeConstructor::Never,
                    "Any" => TypeConstructor::Any,
                    "List" => TypeConstructor::List,
                    "Array" => TypeConstructor::Array,
                    "Map" => TypeConstructor::Map,
                    "Option" => TypeConstructor::Option,
                    "Either" => TypeConstructor::Either,
                    _ => TypeConstructor::Adt(*name),
                };

                // Nullary types: no args
                if args.is_empty() {
                    return Some(InferType::Con(con));
                }

                // Parametric: App(con, arg_tys)
                let args_tys: Option<Vec<InferType>> = args
                    .iter()
                    .map(|a| Self::infer_type_from_type_expr(a, type_params, interner))
                    .collect();
                Some(InferType::App(con, args_tys?))
            }
            TypeExpr::Tuple { elements, .. } => {
                let elem_tys: Option<Vec<InferType>> = elements
                    .iter()
                    .map(|e| Self::infer_type_from_type_expr(e, type_params, interner))
                    .collect();
                Some(InferType::Tuple(elem_tys?))
            }
            TypeExpr::Function { params, ret, .. } => {
                let param_tys: Option<Vec<InferType>> = params
                    .iter()
                    .map(|p| Self::infer_type_from_type_expr(p, type_params, interner))
                    .collect();
                let ret_ty = Self::infer_type_from_type_expr(ret, type_params, interner)?;
                Some(InferType::Fun(param_tys?, Box::new(ret_ty)))
            }
        }
    }

    /// Convert a `RuntimeType` (VM boundary type) to `InferType`.
    pub fn infer_type_from_runtime(runtime_type: &RuntimeType) -> InferType {
        match runtime_type {
            RuntimeType::Any => InferType::Con(TypeConstructor::Any),
            RuntimeType::Int => InferType::Con(TypeConstructor::Int),
            RuntimeType::Float => InferType::Con(TypeConstructor::Float),
            RuntimeType::Bool => InferType::Con(TypeConstructor::Bool),
            RuntimeType::String => InferType::Con(TypeConstructor::String),
            RuntimeType::Unit => InferType::Con(TypeConstructor::Unit),
            RuntimeType::Option(inner) => InferType::App(
                TypeConstructor::Option,
                vec![Self::infer_type_from_runtime(inner)],
            ),
            RuntimeType::Array(inner) => InferType::App(
                TypeConstructor::Array,
                vec![Self::infer_type_from_runtime(inner)],
            ),
            RuntimeType::Map(k, v) => InferType::App(
                TypeConstructor::Map,
                vec![
                    Self::infer_type_from_runtime(k),
                    Self::infer_type_from_runtime(v),
                ],
            ),
            RuntimeType::Tuple(elems) => {
                InferType::Tuple(elems.iter().map(Self::infer_type_from_runtime).collect())
            }
        }
    }

    /// Convert a concrete `Ty` back to `RuntimeType` for the VM boundary check system.
    ///
    /// Returns `RuntimeType::Any` for type variables (unresolved / gradual).
    pub fn to_runtime(infer_type: &InferType, type_subst: &TypeSubst) -> RuntimeType {
        let resolved = infer_type.apply_type_subst(type_subst);

        match &resolved {
            InferType::Con(c) => match c {
                TypeConstructor::Int => RuntimeType::Int,
                TypeConstructor::Float => RuntimeType::Float,
                TypeConstructor::Bool => RuntimeType::Bool,
                TypeConstructor::String => RuntimeType::String,
                TypeConstructor::Unit | TypeConstructor::Never => RuntimeType::Unit,
                TypeConstructor::Any
                | TypeConstructor::List
                | TypeConstructor::Array
                | TypeConstructor::Map
                | TypeConstructor::Option
                | TypeConstructor::Either
                | TypeConstructor::Adt(_) => RuntimeType::Any,
            },
            InferType::App(con, args) => match con {
                TypeConstructor::Option if args.len() == 1 => {
                    RuntimeType::Option(Box::new(Self::to_runtime(&args[0], type_subst)))
                }
                TypeConstructor::Array if args.len() == 1 => {
                    RuntimeType::Array(Box::new(Self::to_runtime(&args[0], type_subst)))
                }
                TypeConstructor::Map if args.len() == 2 => RuntimeType::Map(
                    Box::new(Self::to_runtime(&args[0], type_subst)),
                    Box::new(Self::to_runtime(&args[1], type_subst)),
                ),
                _ => RuntimeType::Any,
            },
            InferType::Tuple(elems) => RuntimeType::Tuple(
                elems
                    .iter()
                    .map(|e| Self::to_runtime(e, type_subst))
                    .collect(),
            ),
            // Functions and unresolved vars become Any in the runtime
            InferType::Fun(..) | InferType::Var(_) => RuntimeType::Any,
        }
    }
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        diagnostics::position::Span,
        runtime::runtime_type::RuntimeType,
        syntax::{interner::Interner, type_expr::TypeExpr},
        types::{
            infer_type::InferType, scheme::Scheme, type_constructor::TypeConstructor,
            type_subst::TypeSubst,
        },
    };

    use super::TypeEnv;

    fn infer_var(id: u32) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    #[test]
    fn fresh_and_fresh_infer_type_increment_counter() {
        let mut env = TypeEnv::new();
        assert_eq!(env.fresh(), 0);
        assert_eq!(env.fresh_infer_type(), infer_var(1));
        assert_eq!(env.counter, 2);
    }

    #[test]
    fn bind_lookup_and_scope_shadowing() {
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let mut env = TypeEnv::new();
        env.bind(x, Scheme::mono(int()));
        assert_eq!(env.lookup(x).expect("bound in outer").infer_type, int());

        env.enter_scope();
        env.bind(x, Scheme::mono(InferType::Con(TypeConstructor::Bool)));
        assert_eq!(
            env.lookup(x).expect("shadowed in inner").infer_type,
            InferType::Con(TypeConstructor::Bool)
        );

        env.leave_scope();
        assert_eq!(env.lookup(x).expect("back to outer").infer_type, int());
    }

    #[test]
    fn leave_scope_keeps_global_scope_intact() {
        let mut env = TypeEnv::new();
        env.leave_scope();
        assert_eq!(env.counter, 0);
        assert!(env.free_vars().is_empty());
    }

    #[test]
    fn free_vars_aggregates_and_respects_scheme_quantifiers() {
        let mut interner = Interner::new();
        let a = interner.intern("a");
        let b = interner.intern("b");

        let mut env = TypeEnv::new();
        env.bind(
            a,
            Scheme {
                forall: vec![0],
                infer_type: InferType::Fun(vec![infer_var(0)], Box::new(infer_var(1))),
            },
        );
        env.bind(b, Scheme::mono(InferType::Tuple(vec![infer_var(2), int()])));

        let free = env.free_vars();
        assert_eq!(free.len(), 2);
        assert!(free.contains(&1));
        assert!(free.contains(&2));
        assert!(!free.contains(&0));
    }

    #[test]
    fn infer_type_from_type_expr_supports_generics_and_named_types() {
        let mut interner = Interner::new();
        let t = interner.intern("T");
        let option = interner.intern("Option");
        let string = interner.intern("String");

        let type_expr = TypeExpr::Function {
            params: vec![TypeExpr::Named {
                name: t,
                args: vec![],
                span: Span::default(),
            }],
            ret: Box::new(TypeExpr::Named {
                name: option,
                args: vec![TypeExpr::Named {
                    name: string,
                    args: vec![],
                    span: Span::default(),
                }],
                span: Span::default(),
            }),
            effects: vec![],
            span: Span::default(),
        };

        let type_params = HashMap::from([(t, 77_u32)]);
        let got = TypeEnv::infer_type_from_type_expr(&type_expr, &type_params, &interner)
            .expect("type expression should convert");
        let expected = InferType::Fun(
            vec![infer_var(77)],
            Box::new(InferType::App(
                TypeConstructor::Option,
                vec![InferType::Con(TypeConstructor::String)],
            )),
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn infer_type_runtime_round_trip_for_collections() {
        let inferred = TypeEnv::infer_type_from_runtime(&RuntimeType::Map(
            Box::new(RuntimeType::String),
            Box::new(RuntimeType::Array(Box::new(RuntimeType::Int))),
        ));
        let runtime = TypeEnv::to_runtime(&inferred, &TypeSubst::empty());

        assert_eq!(
            runtime,
            RuntimeType::Map(
                Box::new(RuntimeType::String),
                Box::new(RuntimeType::Array(Box::new(RuntimeType::Int)))
            )
        );
    }
}
