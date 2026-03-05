use super::*;

impl<'a> InferCtx<'a> {
    /// Infer anonymous function expressions.
    pub(super) fn infer_lambda_expression(&mut self, input: LambdaInferInput<'_>) -> InferType {
        self.env.enter_scope();
        let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();
        let type_params: HashMap<Identifier, TypeVarId> = HashMap::new();

        let param_tys = self.infer_and_bind_parameter_types(
            &type_params,
            &mut row_var_env,
            input.parameters,
            input.parameter_types,
        );

        let ambient_effect_row = if input.effects.is_empty() {
            InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), self.env.fresh())
        } else {
            Self::infer_effect_row(input.effects, &mut row_var_env, &mut self.env.counter)
        };

        let declared_effect_row = ambient_effect_row.clone();
        let body_ty =
            self.with_ambient_effect_row(ambient_effect_row, |ctx| ctx.infer_block(input.body));
        let ret_ty = self.infer_return_type_with_optional_annotation(
            &type_params,
            &mut row_var_env,
            input.return_type,
            &body_ty,
        );

        let final_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|ty| ty.apply_type_subst(&self.subst))
            .collect();
        self.env.leave_scope();

        InferType::Fun(
            final_param_tys,
            Box::new(ret_ty),
            declared_effect_row.apply_row_subst(&self.subst),
        )
    }
}
