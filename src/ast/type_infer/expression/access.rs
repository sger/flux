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
            _other => InferType::App(
                TypeConstructor::Option,
                vec![self.env.alloc_infer_type_var()],
            ),
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
            let fresh_vars = mapping.values().copied().collect::<Vec<_>>();
            for &fresh in &fresh_vars {
                self.env.record_var_level(fresh);
            }
            self.record_instantiated_expr_vars(fresh_vars);
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
        self.alloc_fallback_var()
    }

    /// Infer tuple field projection by static index.
    pub(super) fn infer_tuple_field_access_expression(
        &mut self,
        object: &Expression,
        index: usize,
    ) -> InferType {
        let object_ty = self.infer_expression(object);
        match object_ty.apply_type_subst(&self.subst) {
            InferType::Tuple(elements) => elements
                .get(index)
                .cloned()
                .unwrap_or_else(|| self.alloc_fallback_var()),
            InferType::Var(_) => {
                // Delay projection failure for unresolved tuple-typed values by
                // constraining them to a tuple shape. This lets later call-site
                // unification discharge local helper projections like `pair.0`
                // instead of poisoning the expression with a fallback hole.
                let arity = std::cmp::max(index + 1, 2);
                let elements: Vec<InferType> =
                    (0..arity).map(|_| self.env.alloc_infer_type_var()).collect();
                let projected = elements[index].clone();
                let tuple_shape = InferType::Tuple(elements);
                self.unify_silent(&object_ty, &tuple_shape);
                projected.apply_type_subst(&self.subst)
            }
            _other => self.alloc_fallback_var(),
        }
    }
}
