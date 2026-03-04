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
        for variant in variants {
            self.adt_constructor_types.insert(
                variant.name,
                AdtConstructorTypeInfo {
                    adt_name,
                    type_params: type_params.to_vec(),
                    fields: variant.fields.clone(),
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

    /// Instatiate constructor field and result types with fresh type variables.
    ///
    /// Returns `None` when constructor metadata is unavailable or lowering fails.
    pub(super) fn instantiate_constructor_parts(
        &mut self,
        constructor: Identifier,
    ) -> Option<(Vec<InferType>, InferType)> {
        let info = self.adt_constructor_types.get(&constructor)?.clone();
        let mut type_param_map: HashMap<Identifier, TypeVarId> = HashMap::new();
        for type_param in &info.type_params {
            type_param_map.insert(*type_param, self.env.fresh());
        }

        let field_tys: Vec<InferType> = info
            .fields
            .iter()
            .map(|field| {
                let mut row_var_env = HashMap::new();
                TypeEnv::infer_type_from_type_expr_with_row_vars(
                    field,
                    &type_param_map,
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                )
            })
            .collect::<Option<Vec<_>>>()?;

        let result_ty = if info.type_params.is_empty() {
            InferType::Con(TypeConstructor::Adt(info.adt_name))
        } else {
            let mut args = Vec::with_capacity(info.type_params.len());
            for type_param in &info.type_params {
                let var = type_param_map.get(type_param)?;
                args.push(InferType::Var(*var));
            }
            InferType::App(TypeConstructor::Adt(info.adt_name), args)
        };

        Some((field_tys, result_ty))
    }

    /// Infer constructor call arguments and return instantiated ADT result type.
    ///
    /// Arity mismatches emit constructor-specific diagnostics and return `Any`.
    pub(super) fn infer_constructor_call(
        &mut self,
        constructor: Identifier,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expression(a)).collect();
        let Some((param_tys, result_ty)) = self.instantiate_constructor_parts(constructor) else {
            return InferType::Con(TypeConstructor::Any);
        };
        if arg_tys.len() != param_tys.len() {
            let name_str = self.interner.resolve(constructor).to_string();
            self.errors.push(
                diag_enhanced(&CONSTRUCTOR_ARITY_MISMATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Constructor `{}` expects {} argument(s) but got {}.",
                        name_str,
                        param_tys.len(),
                        arg_tys.len()
                    ))
                    .with_file(self.file_path.clone()),
            );
            return InferType::Con(TypeConstructor::Any);
        }
        for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
            self.unify_reporting(actual, expected, span);
        }
        result_ty.apply_type_subst(&self.subst)
    }
}
