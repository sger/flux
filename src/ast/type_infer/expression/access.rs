use super::*;

impl<'a> InferCtx<'a> {
    /// Infer indexing operations over arrays/lists/maps/tuples.
    pub(super) fn infer_index_expression(
        &mut self,
        left: &Expression,
        index: &Expression,
    ) -> InferType {
        let left_ty = self.infer_expression(left);
        let _index_ty = self.infer_expression(index);
        match left_ty.apply_type_subst(&self.subst) {
            InferType::App(TypeConstructor::Array, args)
            | InferType::App(TypeConstructor::List, args)
                if args.len() == 1 =>
            {
                InferType::App(TypeConstructor::Option, vec![args[0].clone()])
            }
            InferType::App(TypeConstructor::Map, args) if args.len() == 2 => {
                InferType::App(TypeConstructor::Option, vec![args[1].clone()])
            }
            InferType::Tuple(elements) => self.infer_tuple_index_expression(&elements, index),
            other => {
                if self.strict_mode_enabled() {
                    self.emit_strict_inference_error(
                        left.span(),
                        format!(
                            "Index access is only supported on arrays, lists, maps, and tuples in strict mode, but this expression has type `{}`.",
                            self.display_type(&other)
                        ),
                        "Use indexing on Array/List/Map/Tuple values or add a type annotation.",
                    );
                }
                InferType::App(
                    TypeConstructor::Option,
                    vec![self.env.alloc_infer_type_var()],
                )
            }
        }
    }

    /// Infer tuple index result type, including fallback join when index is non-literal.
    fn infer_tuple_index_expression(
        &mut self,
        elements: &[InferType],
        index: &Expression,
    ) -> InferType {
        if let Expression::Integer { value, .. } = index
            && *value >= 0
            && let Some(elem) = elements.get(*value as usize)
        {
            return InferType::App(
                TypeConstructor::Option,
                vec![elem.clone().apply_type_subst(&self.subst)],
            );
        }
        let joined = elements.iter().skip(1).fold(
            elements
                .first()
                .cloned()
                .unwrap_or_else(|| self.env.alloc_infer_type_var()),
            |acc, ty| self.unify_reporting(&acc, ty, index.span()),
        );
        InferType::App(TypeConstructor::Option, vec![joined])
    }

    /// Infer module/member access resolution.
    pub(super) fn infer_member_access_expression(
        &mut self,
        expr: &Expression,
        object: &Expression,
        member: Identifier,
    ) -> InferType {
        if let Expression::Identifier {
            name: module_name, ..
        } = object
            && let Some(scheme) = self
                .module_member_schemes
                .get(&(*module_name, member))
                .cloned()
        {
            let (ty, mapping, constraints) = scheme.instantiate(&mut self.env.counter);
            for &fresh in mapping.values() {
                self.env.record_var_level(fresh);
            }
            self.emit_scheme_constraints(&constraints, expr.span());
            return ty;
        }

        if let Expression::Identifier {
            name: module_name, ..
        } = object
            && *module_name == self.flow_module_symbol
            && self.known_flow_names.contains(&member)
        {
            self.emit_missing_flow_hm_signature(member, expr.span());
        }
        self.infer_expression(object);
        if self.strict_mode_enabled() {
            self.emit_strict_inference_error(
                expr.span(),
                format!(
                    "Strict typing could not resolve member access `{}` on this expression.",
                    expr.display_with(self.interner)
                ),
                "Only imported module member access is currently typed here; add an annotation or use a supported access shape.",
            );
        }
        self.env.alloc_infer_type_var()
    }

    /// Infer tuple field projection by static index.
    pub(super) fn infer_tuple_field_access_expression(
        &mut self,
        object: &Expression,
        index: usize,
    ) -> InferType {
        match self.infer_expression(object).apply_type_subst(&self.subst) {
            InferType::Tuple(elements) => elements.get(index).cloned().unwrap_or_else(|| {
                if self.strict_mode_enabled() {
                    self.emit_strict_inference_error(
                        object.span(),
                        format!("Tuple field .{index} is out of bounds for this tuple expression."),
                        "Use a valid tuple field index for the inferred tuple arity.",
                    );
                }
                self.env.alloc_infer_type_var()
            }),
            other => {
                if self.strict_mode_enabled() {
                    self.emit_strict_inference_error(
                        object.span(),
                        format!(
                            "Tuple field access requires a tuple, but this expression has type `{}`.",
                            self.display_type(&other)
                        ),
                        "Use tuple field access only on tuple values.",
                    );
                }
                self.env.alloc_infer_type_var()
            }
        }
    }
}
