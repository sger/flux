use std::collections::HashSet as StdHashSet;

use crate::{
    diagnostics::compiler_errors::{
        NAMED_FIELD_DUPLICATE, NAMED_FIELD_MISSING, NAMED_FIELD_PUN_UNBOUND, NAMED_FIELD_UNKNOWN,
    },
    syntax::expression::NamedFieldInit,
};

use super::*;

impl<'a> InferCtx<'a> {
    /// Predeclare all ADT constructors from the provided statement list.
    ///
    /// This enables constructor references before their textual declaration.
    pub(super) fn predeclare_data_constructors_in_statements(&mut self, statements: &[Statement]) {
        for stmt in statements {
            if let Statement::Data {
                name,
                type_params,
                variants,
                ..
            } = stmt
            {
                self.register_data_constructors(*name, type_params, variants);
            }
        }
    }

    /// Register constructors for an ADT and bind constructor schemes in the type environment.
    pub(super) fn register_data_constructors(
        &mut self,
        adt_name: Identifier,
        type_params: &[Identifier],
        variants: &[DataVariant],
    ) {
        self.adt_type_params.insert(adt_name, type_params.to_vec());
        for variant in variants {
            self.adt_constructor_types.insert(
                variant.name,
                AdtConstructorTypeInfo {
                    adt_name,
                    type_params: type_params.to_vec(),
                    fields: variant.fields.clone(),
                    field_names: variant.field_names.clone(),
                },
            );

            let Some((field_tys, result_ty)) = self.instantiate_constructor_parts(variant.name)
            else {
                continue;
            };
            let ctor_ty = if field_tys.is_empty() {
                result_ty
            } else {
                InferType::Fun(
                    field_tys,
                    Box::new(result_ty),
                    InferEffectRow::closed_empty(),
                )
            };
            let scheme = generalize(&ctor_ty, &HashSet::new());
            self.env.bind(variant.name, scheme);
        }
    }

    /// Instantiate constructor field and result types with fresh type variables.
    ///
    /// Returns `None` when constructor metadata is unavailable or lowering fails.
    pub(super) fn instantiate_constructor_parts(
        &mut self,
        constructor: Identifier,
    ) -> Option<(Vec<InferType>, InferType)> {
        let info = self.adt_constructor_types.get(&constructor)?;
        let type_params = info.type_params.clone();
        let fields = info.fields.clone();
        let adt_name = info.adt_name;

        let mut type_param_map: HashMap<Identifier, TypeVarId> = HashMap::new();
        for type_param in &type_params {
            type_param_map.insert(*type_param, self.env.alloc_type_var_id());
        }

        let field_tys: Vec<InferType> = fields
            .iter()
            .map(|field| {
                let mut row_var_env = HashMap::new();
                TypeEnv::convert_type_expr_rec(
                    field,
                    &type_param_map,
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                )
            })
            .collect::<Option<Vec<_>>>()?;

        let result_ty = if type_params.is_empty() {
            InferType::Con(TypeConstructor::Adt(adt_name))
        } else {
            let mut args = Vec::with_capacity(type_params.len());
            for type_param in &type_params {
                let var = type_param_map.get(type_param)?;
                args.push(InferType::Var(*var));
            }
            InferType::App(TypeConstructor::Adt(adt_name), args)
        };

        Some((field_tys, result_ty))
    }

    /// Infer constructor call arguments and return instantiated ADT result type.
    ///
    /// Arity mismatches emit constructor-specific diagnostics and recover with
    /// a fresh inference variable.
    pub(super) fn infer_constructor_call(
        &mut self,
        constructor: Identifier,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expression(a)).collect();
        let Some((param_tys, result_ty)) = self.instantiate_constructor_parts(constructor) else {
            return self.alloc_fallback_var();
        };
        if arg_tys.len() != param_tys.len() {
            let name_str = self.interner.resolve(constructor).to_string();
            self.errors.push(
                diagnostic_for(&CONSTRUCTOR_ARITY_MISMATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Constructor `{}` expects {} argument(s) but got {}.",
                        name_str,
                        param_tys.len(),
                        arg_tys.len()
                    ))
                    .with_file(self.file_path.clone()),
            );
            return self.alloc_fallback_var();
        }
        for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
            self.unify_reporting(actual, expected, span);
        }
        result_ty.apply_type_subst(&self.subst)
    }

    /// Reorder named-constructor arguments into declaration order and
    /// delegate to the positional inference path.
    ///
    /// Emits E460 for missing fields, E461 for unknown fields, E462 for
    /// duplicates, E466 for punned fields that have no matching binding.
    /// Recovers by returning a fallback inference variable on structural
    /// errors.
    pub(super) fn infer_named_constructor_call(
        &mut self,
        constructor: Identifier,
        fields: &[NamedFieldInit],
        span: Span,
    ) -> InferType {
        let Some(info) = self.adt_constructor_types.get(&constructor).cloned() else {
            return self.alloc_fallback_var();
        };
        let Some(field_names) = info.field_names.clone() else {
            self.report_positional_with_named_syntax(constructor, fields);
            return self.alloc_fallback_var();
        };
        let (by_index, structural_error) =
            self.bucket_named_fields(constructor, fields, &field_names, span);
        if structural_error {
            return self.alloc_fallback_var();
        }
        let Some((param_tys, result_ty)) = self.instantiate_constructor_parts(constructor) else {
            return self.alloc_fallback_var();
        };
        for (idx, slot) in by_index.iter().enumerate() {
            let init = slot.expect("all slots filled after structural checks");
            let expected = &param_tys[idx];
            let actual = match &init.value {
                Some(expr) => self.infer_expression(expr),
                None => self.infer_punned_field(init.name, init.span),
            };
            self.unify_reporting(&actual, expected, init.span);
        }
        result_ty.apply_type_subst(&self.subst)
    }

    /// Emit E461 when a positional-variant constructor is used with
    /// named-field syntax. Anchored on the first field for precise spans.
    fn report_positional_with_named_syntax(
        &mut self,
        constructor: Identifier,
        fields: &[NamedFieldInit],
    ) {
        if let Some(first) = fields.first() {
            let msg = format!(
                "Constructor `{}` was declared with positional fields; \
                 named-field construction is not allowed.",
                self.interner.resolve(constructor)
            );
            self.errors.push(
                diagnostic_for(&NAMED_FIELD_UNKNOWN)
                    .with_span(first.span)
                    .with_message(msg)
                    .with_file(self.file_path.clone()),
            );
        }
    }

    /// Map declared field names to their `NamedFieldInit` slots, emitting
    /// E461/E462/E460 for unknown, duplicate, or missing fields. Returns the
    /// per-index slot vector plus a flag set when any structural error
    /// prevents downstream unification.
    fn bucket_named_fields<'f>(
        &mut self,
        constructor: Identifier,
        fields: &'f [NamedFieldInit],
        field_names: &[Identifier],
        span: Span,
    ) -> (Vec<Option<&'f NamedFieldInit>>, bool) {
        let mut seen: StdHashSet<Identifier> = StdHashSet::new();
        let mut by_index: Vec<Option<&NamedFieldInit>> = vec![None; field_names.len()];
        let mut structural_error = false;
        for init in fields {
            let Some(index) = field_names.iter().position(|n| *n == init.name) else {
                self.emit_field_diag(
                    &NAMED_FIELD_UNKNOWN,
                    init.span,
                    format!(
                        "`{}` has no field named `{}`.",
                        self.interner.resolve(constructor),
                        self.interner.resolve(init.name),
                    ),
                );
                structural_error = true;
                continue;
            };
            if !seen.insert(init.name) {
                self.emit_field_diag(
                    &NAMED_FIELD_DUPLICATE,
                    init.span,
                    format!(
                        "Field `{}` is listed more than once.",
                        self.interner.resolve(init.name)
                    ),
                );
                structural_error = true;
                continue;
            }
            by_index[index] = Some(init);
        }
        for (idx, slot) in by_index.iter().enumerate() {
            if slot.is_none() {
                self.emit_field_diag(
                    &NAMED_FIELD_MISSING,
                    span,
                    format!(
                        "Missing field `{}` in `{}` constructor.",
                        self.interner.resolve(field_names[idx]),
                        self.interner.resolve(constructor),
                    ),
                );
                structural_error = true;
            }
        }
        (by_index, structural_error)
    }

    /// Emit a proposal-0152 field diagnostic with the current file path.
    pub(super) fn emit_field_diag(
        &mut self,
        code: &'static crate::diagnostics::ErrorCode,
        span: Span,
        message: String,
    ) {
        self.errors.push(
            diagnostic_for(code)
                .with_span(span)
                .with_message(message)
                .with_file(self.file_path.clone()),
        );
    }

    /// Resolve a punned field `Foo { x }` against an in-scope binding named
    /// `x`. Emits E466 when no such binding exists.
    pub(super) fn infer_punned_field(&mut self, name: Identifier, span: Span) -> InferType {
        if let Some(scheme) = self.env.lookup(name).cloned() {
            let (ty, mapping, constraints) = scheme.instantiate(&mut self.env.counter);
            let fresh_vars = mapping.values().copied().collect::<Vec<_>>();
            for &fresh in &fresh_vars {
                self.env.record_var_level(fresh);
            }
            self.record_instantiated_expr_vars(fresh_vars);
            self.emit_scheme_constraints(&constraints, span);
            return ty;
        }
        let resolved = self.interner.resolve(name).to_string();
        let msg = format!("No variable named `{resolved}` is in scope for punning.");
        self.errors.push(
            diagnostic_for(&NAMED_FIELD_PUN_UNBOUND)
                .with_span(span)
                .with_message(msg)
                .with_file(self.file_path.clone()),
        );
        self.alloc_fallback_var()
    }

    /// Return all variant names of `adt_name` that are named-field variants.
    /// Sorted by interned name so downstream diagnostics are deterministic.
    pub(super) fn named_variants_of(&self, adt_name: Identifier) -> Vec<Identifier> {
        let mut variants: Vec<Identifier> = self
            .adt_constructor_types
            .iter()
            .filter_map(|(variant, info)| {
                (info.adt_name == adt_name && info.field_names.is_some()).then_some(*variant)
            })
            .collect();
        variants.sort_by_key(|v| self.interner.resolve(*v).to_string());
        variants
    }

    /// Return the field type of `field_name` on a specific variant of an ADT,
    /// freshly instantiated against the same type-parameter substitution as
    /// `adt_ty`.
    pub(super) fn instantiate_variant_field(
        &mut self,
        variant: Identifier,
        field_name: Identifier,
        adt_ty: &InferType,
    ) -> Option<InferType> {
        let info = self.adt_constructor_types.get(&variant)?.clone();
        let field_names = info.field_names.as_ref()?;
        let field_index = field_names.iter().position(|n| *n == field_name)?;

        // Fresh instantiation: allocate vars for each type param, convert
        // field TypeExpr, then unify the variant's result type with adt_ty
        // so the caller's type refines the field type.
        let (field_tys, result_ty) = self.instantiate_constructor_parts(variant)?;
        self.unify_silent(adt_ty, &result_ty);
        Some(field_tys[field_index].apply_type_subst(&self.subst))
    }
}
