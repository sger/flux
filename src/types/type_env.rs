use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::position::Span,
    runtime::runtime_type::RuntimeType,
    syntax::{Identifier, interner::Interner, type_expr::TypeExpr},
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
        type_constructor::TypeConstructor, type_subst::TypeSubst,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTypeLoweringIssue {
    UnresolvedTypeVariable,
    OpenFunctionEffects,
    UnsupportedNominalType,
    UnsupportedHigherKindedType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTypeLoweringError {
    issue: RuntimeTypeLoweringIssue,
}

impl RuntimeTypeLoweringError {
    fn new(issue: RuntimeTypeLoweringIssue) -> Self {
        Self { issue }
    }

    pub fn issue(&self) -> &RuntimeTypeLoweringIssue {
        &self.issue
    }
}

/// Scoped type environment mapping identifiers to their type schemes.
///
/// Uses a shadow-stack design for O(1) lookup: each name maps to a stack of
/// bindings, with the top entry being the currently visible one. Scope
/// markers track which names were bound at each scope level so `leave_scope`
/// can efficiently restore the previous state.
///
/// Tracks a scope level counter shared with level-based generalization:
/// type variables allocated at a deeper level than the generalization point
/// are quantified without scanning the environment.
#[derive(Debug, Clone)]
pub struct TypeEnv {
    /// Shadow stack: each name maps to a stack of bindings (top = visible).
    bindings: HashMap<Identifier, Vec<TypeBindingEntry>>,
    /// Names bound at each scope level, used for cleanup on `leave_scope`.
    scope_markers: Vec<Vec<Identifier>>,
    /// Current scope depth. Incremented on `enter_scope`, decremented on
    /// `leave_scope`. Used for level-based generalization.
    level: u32,
    /// Allocation level for each type variable. Variables with level > the
    /// generalization point are quantified by `generalization_at_level`.
    var_levels: HashMap<TypeVarId, u32>,
    pub(crate) counter: u32,
}

#[derive(Debug, Clone)]
struct TypeBindingEntry {
    scheme: Scheme,
    def_span: Option<Span>,
}

impl TypeEnv {
    pub fn new() -> Self {
        TypeEnv {
            bindings: HashMap::new(),
            scope_markers: vec![Vec::new()],
            level: 0,
            var_levels: HashMap::new(),
            counter: 0,
        }
    }

    /// Current scope level.
    pub fn level(&self) -> u32 {
        self.level
    }

    /// Allocate a fresh type variable id and record its allocation level.
    pub fn alloc_type_var_id(&mut self) -> TypeVarId {
        let var = self.counter;
        self.counter += 1;
        self.var_levels.insert(var, self.level);
        var
    }

    /// Allocate a fresh `InferType::Var`.
    pub fn alloc_infer_type_var(&mut self) -> InferType {
        InferType::Var(self.alloc_type_var_id())
    }

    /// Record the allocation level for a type variable that was created
    /// externally (e.g. by `Scheme::instantiate`).
    pub fn record_var_level(&mut self, var: TypeVarId) {
        self.var_levels.insert(var, self.level);
    }

    /// Bind a name to a scheme in the current (innermost) scope.
    pub fn bind(&mut self, name: Identifier, scheme: Scheme) {
        self.bind_with_span(name, scheme, None);
    }

    /// Bind a name to a scheme and optional definition span in the current scope.
    pub fn bind_with_span(&mut self, name: Identifier, scheme: Scheme, def_span: Option<Span>) {
        self.bindings
            .entry(name)
            .or_default()
            .push(TypeBindingEntry { scheme, def_span });
        if let Some(marker) = self.scope_markers.last_mut() {
            marker.push(name);
        }
    }

    /// Look up a name O(1) via shadow stack top.
    pub fn lookup(&self, name: Identifier) -> Option<&Scheme> {
        self.bindings.get(&name)?.last().map(|e| &e.scheme)
    }

    /// Iterate over all currently visible bindings (top of each shadow stack).
    pub fn visible_bindings(&self) -> impl Iterator<Item = (Identifier, &Scheme)> {
        self.bindings
            .iter()
            .filter_map(|(name, entries)| entries.last().map(|e| (*name, &e.scheme)))
    }

    /// Look up a name's definition span O(1) via shadow stack top.
    pub fn lookup_span(&self, name: Identifier) -> Option<Span> {
        self.bindings.get(&name)?.last().and_then(|e| e.def_span)
    }

    /// Push a new empty scope and bump the level.
    pub fn enter_scope(&mut self) {
        self.level += 1;
        self.scope_markers.push(Vec::new());
    }

    /// Pop the innermost scope, restoring shadowed bindings.
    pub fn leave_scope(&mut self) {
        if let Some(names) = self.scope_markers.pop() {
            for name in names {
                if let Some(stack) = self.bindings.get_mut(&name) {
                    stack.pop();
                    if stack.is_empty() {
                        self.bindings.remove(&name);
                    }
                }
            }
        }
        self.level = self.level.saturating_sub(1);
    }

    /// All free type variables in currently visible bindings.
    pub fn free_vars(&self) -> HashSet<TypeVarId> {
        let mut set = HashSet::new();
        for stack in self.bindings.values() {
            if let Some(entry) = stack.last() {
                set.extend(entry.scheme.free_vars());
            }
        }
        set
    }

    /// Level-based generalization: quantify all free type variables whose
    /// allocation level is strictly greater than the current environment level.
    ///
    /// This replaces `generalize(ty, &env.free_vars())` with an O(type-size)
    /// operation independent of environment size.
    pub fn generalize_at_level(&self, ty: &InferType) -> Scheme {
        let level = self.level;
        let mut forall: Vec<TypeVarId> = ty
            .free_vars()
            .into_iter()
            .filter(|v| self.var_levels.get(v).copied().unwrap_or(0) > level)
            .collect();
        forall.sort_unstable();
        Scheme {
            forall,
            constraints: Vec::new(),
            infer_type: ty.clone(),
        }
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
        let mut row_var_env = HashMap::new();
        let mut row_var_counter: u32 = 0;
        Self::convert_type_expr_rec(
            type_expr,
            type_params,
            interner,
            &mut row_var_env,
            &mut row_var_counter,
        )
    }

    pub fn convert_type_expr_rec(
        type_expr: &TypeExpr,
        type_params: &HashMap<Identifier, TypeVarId>,
        interner: &Interner,
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        row_var_counter: &mut u32,
    ) -> Option<InferType> {
        match type_expr {
            TypeExpr::Named { name, args, .. } => {
                let name_str = interner.resolve(*name);
                // Check if this name is a generic type parameter
                if let Some(&v) = type_params.get(name) {
                    if args.is_empty() {
                        return Some(InferType::Var(v));
                    }
                    // HKT application: f<a> where f is a type param → HktApp(Var(f), [a])
                    let arg_tys: Option<Vec<InferType>> = args
                        .iter()
                        .map(|a| {
                            Self::convert_type_expr_rec(
                                a,
                                type_params,
                                interner,
                                row_var_env,
                                row_var_counter,
                            )
                        })
                        .collect();
                    return Some(InferType::HktApp(Box::new(InferType::Var(v)), arg_tys?));
                }

                // Resolve the constructor
                let con = match name_str {
                    "Int" => TypeConstructor::Int,
                    "Float" => TypeConstructor::Float,
                    "Bool" => TypeConstructor::Bool,
                    "String" => TypeConstructor::String,
                    "Unit" | "None" => TypeConstructor::Unit,
                    "Never" => TypeConstructor::Never,
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
                    .map(|a| {
                        Self::convert_type_expr_rec(
                            a,
                            type_params,
                            interner,
                            row_var_env,
                            row_var_counter,
                        )
                    })
                    .collect();
                Some(InferType::App(con, args_tys?))
            }
            TypeExpr::Tuple { elements, .. } => {
                if elements.is_empty() {
                    return Some(InferType::Con(TypeConstructor::Unit));
                }
                let elem_tys: Option<Vec<InferType>> = elements
                    .iter()
                    .map(|e| {
                        Self::convert_type_expr_rec(
                            e,
                            type_params,
                            interner,
                            row_var_env,
                            row_var_counter,
                        )
                    })
                    .collect();
                Some(InferType::Tuple(elem_tys?))
            }
            TypeExpr::Function {
                params,
                ret,
                effects,
                ..
            } => {
                let param_tys: Option<Vec<InferType>> = params
                    .iter()
                    .map(|p| {
                        Self::convert_type_expr_rec(
                            p,
                            type_params,
                            interner,
                            row_var_env,
                            row_var_counter,
                        )
                    })
                    .collect();
                let ret_ty = Self::convert_type_expr_rec(
                    ret,
                    type_params,
                    interner,
                    row_var_env,
                    row_var_counter,
                )?;
                let effect_row =
                    InferEffectRow::from_effect_exprs(effects, row_var_env, row_var_counter)
                        .ok()?;
                Some(InferType::Fun(param_tys?, Box::new(ret_ty), effect_row))
            }
        }
    }

    /// Convert a `RuntimeType` (VM boundary type) to `InferType`.
    pub fn try_infer_type_from_runtime(
        runtime_type: &RuntimeType,
    ) -> Result<InferType, RuntimeTypeLoweringError> {
        Ok(match runtime_type {
            RuntimeType::Int => InferType::Con(TypeConstructor::Int),
            RuntimeType::Float => InferType::Con(TypeConstructor::Float),
            RuntimeType::Bool => InferType::Con(TypeConstructor::Bool),
            RuntimeType::String => InferType::Con(TypeConstructor::String),
            RuntimeType::Unit => InferType::Con(TypeConstructor::Unit),
            RuntimeType::Option(inner) => InferType::App(
                TypeConstructor::Option,
                vec![Self::try_infer_type_from_runtime(inner)?],
            ),
            RuntimeType::List(inner) => InferType::App(
                TypeConstructor::List,
                vec![Self::try_infer_type_from_runtime(inner)?],
            ),
            RuntimeType::Either(left, right) => InferType::App(
                TypeConstructor::Either,
                vec![
                    Self::try_infer_type_from_runtime(left)?,
                    Self::try_infer_type_from_runtime(right)?,
                ],
            ),
            RuntimeType::Array(inner) => InferType::App(
                TypeConstructor::Array,
                vec![Self::try_infer_type_from_runtime(inner)?],
            ),
            RuntimeType::Map(k, v) => InferType::App(
                TypeConstructor::Map,
                vec![
                    Self::try_infer_type_from_runtime(k)?,
                    Self::try_infer_type_from_runtime(v)?,
                ],
            ),
            RuntimeType::Tuple(elems) => InferType::Tuple(
                elems
                    .iter()
                    .map(Self::try_infer_type_from_runtime)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            RuntimeType::Function {
                params,
                ret,
                effects,
            } => InferType::Fun(
                params
                    .iter()
                    .map(Self::try_infer_type_from_runtime)
                    .collect::<Result<Vec<_>, _>>()?,
                Box::new(Self::try_infer_type_from_runtime(ret)?),
                InferEffectRow::closed_from_symbols(effects.iter().copied()),
            ),
        })
    }

    /// Convert a concrete `Ty` back to `RuntimeType` for the VM boundary check system.
    ///
    /// The checked form preserves every representable runtime type and
    /// distinguishes unresolved variables, open function
    /// effects, and currently unsupported nominal/HKT shapes.
    pub fn try_to_runtime(
        infer_type: &InferType,
        type_subst: &TypeSubst,
    ) -> Result<RuntimeType, RuntimeTypeLoweringError> {
        let resolved = infer_type.apply_type_subst(type_subst);

        match &resolved {
            InferType::Con(c) => match c {
                TypeConstructor::Int => Ok(RuntimeType::Int),
                TypeConstructor::Float => Ok(RuntimeType::Float),
                TypeConstructor::Bool => Ok(RuntimeType::Bool),
                TypeConstructor::String => Ok(RuntimeType::String),
                TypeConstructor::Unit | TypeConstructor::Never => Ok(RuntimeType::Unit),
                TypeConstructor::List
                | TypeConstructor::Array
                | TypeConstructor::Map
                | TypeConstructor::Option
                | TypeConstructor::Either
                | TypeConstructor::Adt(_) => Err(RuntimeTypeLoweringError::new(
                    RuntimeTypeLoweringIssue::UnsupportedNominalType,
                )),
            },
            InferType::App(con, args) => match con {
                TypeConstructor::Option if args.len() == 1 => Ok(RuntimeType::Option(Box::new(
                    Self::try_to_runtime(&args[0], type_subst)?,
                ))),
                TypeConstructor::List if args.len() == 1 => Ok(RuntimeType::List(Box::new(
                    Self::try_to_runtime(&args[0], type_subst)?,
                ))),
                TypeConstructor::Either if args.len() == 2 => Ok(RuntimeType::Either(
                    Box::new(Self::try_to_runtime(&args[0], type_subst)?),
                    Box::new(Self::try_to_runtime(&args[1], type_subst)?),
                )),
                TypeConstructor::Array if args.len() == 1 => Ok(RuntimeType::Array(Box::new(
                    Self::try_to_runtime(&args[0], type_subst)?,
                ))),
                TypeConstructor::Map if args.len() == 2 => Ok(RuntimeType::Map(
                    Box::new(Self::try_to_runtime(&args[0], type_subst)?),
                    Box::new(Self::try_to_runtime(&args[1], type_subst)?),
                )),
                _ => Err(RuntimeTypeLoweringError::new(
                    RuntimeTypeLoweringIssue::UnsupportedNominalType,
                )),
            },
            InferType::Tuple(elems) => Ok(RuntimeType::Tuple(
                elems
                    .iter()
                    .map(|e| Self::try_to_runtime(e, type_subst))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            InferType::Fun(params, ret, effects) => {
                let resolved_effects = effects.apply_row_subst(type_subst);
                if resolved_effects.tail().is_some() {
                    return Err(RuntimeTypeLoweringError::new(
                        RuntimeTypeLoweringIssue::OpenFunctionEffects,
                    ));
                }
                let mut effect_set = resolved_effects
                    .concrete()
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                effect_set.sort_by_key(|sym| sym.as_u32());
                effect_set.dedup();
                Ok(RuntimeType::Function {
                    params: params
                        .iter()
                        .map(|param| Self::try_to_runtime(param, type_subst))
                        .collect::<Result<Vec<_>, _>>()?,
                    ret: Box::new(Self::try_to_runtime(ret, type_subst)?),
                    effects: effect_set,
                })
            }
            InferType::Var(_) => Err(RuntimeTypeLoweringError::new(
                RuntimeTypeLoweringIssue::UnresolvedTypeVariable,
            )),
            InferType::HktApp(..) => Err(RuntimeTypeLoweringError::new(
                RuntimeTypeLoweringIssue::UnsupportedHigherKindedType,
            )),
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
            infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
            type_constructor::TypeConstructor, type_env::RuntimeTypeLoweringIssue,
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
    fn alloc_helpers_increment_counter() {
        let mut env = TypeEnv::new();
        assert_eq!(env.alloc_type_var_id(), 0);
        assert_eq!(env.alloc_infer_type_var(), infer_var(1));
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
                constraints: vec![],
                infer_type: InferType::Fun(
                    vec![infer_var(0)],
                    Box::new(infer_var(1)),
                    InferEffectRow::closed_empty(),
                ),
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
    fn lookup_span_tracks_bound_definition_span() {
        let mut interner = Interner::new();
        let f = interner.intern("f");
        let mut env = TypeEnv::new();
        let def_span = Span::new(
            crate::diagnostics::position::Position::new(2, 1),
            crate::diagnostics::position::Position::new(2, 10),
        );
        env.bind_with_span(f, Scheme::mono(int()), Some(def_span));
        assert_eq!(env.lookup_span(f), Some(def_span));
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
            InferEffectRow::closed_empty(),
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn alloc_records_var_level() {
        let mut env = TypeEnv::new();
        let v0 = env.alloc_type_var_id(); // level 0
        env.enter_scope();
        let v1 = env.alloc_type_var_id(); // level 1
        env.enter_scope();
        let v2 = env.alloc_type_var_id(); // level 2

        assert_eq!(env.level(), 2);
        assert_eq!(*env.var_levels.get(&v0).unwrap(), 0);
        assert_eq!(*env.var_levels.get(&v1).unwrap(), 1);
        assert_eq!(*env.var_levels.get(&v2).unwrap(), 2);
    }

    #[test]
    fn generalize_at_level_quantifies_deep_vars_only() {
        let mut env = TypeEnv::new();
        let v0 = env.alloc_type_var_id(); // level 0
        env.enter_scope();
        let v1 = env.alloc_type_var_id(); // level 1

        // At level 1, generalize a type containing both vars.
        // Only v0 (level 0) should remain free; v1 (level 1) is at the current
        // level, not strictly greater, so it should NOT be quantified.
        let ty = InferType::Fun(
            vec![infer_var(v0)],
            Box::new(infer_var(v1)),
            InferEffectRow::closed_empty(),
        );
        let scheme = env.generalize_at_level(&ty);
        assert!(scheme.forall.is_empty());

        // Leave scope back to level 0 — now v1 (level 1) > 0, so it should
        // be quantified.
        env.leave_scope();
        let scheme2 = env.generalize_at_level(&ty);
        assert_eq!(scheme2.forall, vec![v1]);
    }

    #[test]
    fn infer_type_runtime_round_trip_for_collections() {
        let inferred = TypeEnv::try_infer_type_from_runtime(&RuntimeType::Map(
            Box::new(RuntimeType::String),
            Box::new(RuntimeType::Array(Box::new(RuntimeType::Int))),
        ))
        .expect("runtime type should convert back to infer type");
        let runtime = TypeEnv::try_to_runtime(&inferred, &TypeSubst::empty())
            .expect("collection runtime lowering should succeed");

        assert_eq!(
            runtime,
            RuntimeType::Map(
                Box::new(RuntimeType::String),
                Box::new(RuntimeType::Array(Box::new(RuntimeType::Int)))
            )
        );
    }

    #[test]
    fn try_to_runtime_lowers_closed_function_types() {
        let ty = InferType::Fun(
            vec![InferType::Con(TypeConstructor::Int)],
            Box::new(InferType::Con(TypeConstructor::Bool)),
            InferEffectRow::closed_empty(),
        );

        let runtime = TypeEnv::try_to_runtime(&ty, &TypeSubst::empty()).expect("function lowers");

        assert_eq!(
            runtime,
            RuntimeType::Function {
                params: vec![RuntimeType::Int],
                ret: Box::new(RuntimeType::Bool),
                effects: vec![]
            }
        )
    }

    #[test]
    fn try_to_runtime_rejects_open_function_effect_rows() {
        let ty = InferType::Fun(
            vec![InferType::Con(TypeConstructor::Int)],
            Box::new(InferType::Con(TypeConstructor::Bool)),
            InferEffectRow::open_from_symbols([], 42),
        );

        let err =
            TypeEnv::try_to_runtime(&ty, &TypeSubst::empty()).expect_err("open row should fail");

        assert_eq!(err.issue(), &RuntimeTypeLoweringIssue::OpenFunctionEffects);
    }

    #[test]
    fn try_to_runtime_rejects_hkt_apps() {
        let ty = InferType::HktApp(
            Box::new(InferType::Var(0)),
            vec![InferType::Con(TypeConstructor::Int)],
        );

        let err = TypeEnv::try_to_runtime(&ty, &TypeSubst::empty()).expect_err("hkt should fail");

        assert_eq!(
            err.issue(),
            &RuntimeTypeLoweringIssue::UnsupportedHigherKindedType
        );
    }

    #[test]
    fn try_infer_type_from_runtime_accepts_function() {
        let inferred = TypeEnv::try_infer_type_from_runtime(&RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        })
        .expect("runtime function type should convert back into infer type");

        assert_eq!(
            inferred,
            InferType::Fun(
                vec![InferType::Con(TypeConstructor::Int)],
                Box::new(InferType::Con(TypeConstructor::Bool)),
                InferEffectRow::closed_empty(),
            )
        );
    }
}
