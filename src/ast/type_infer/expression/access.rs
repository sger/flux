use super::*;

impl<'a> InferCtx<'a> {
    /// Infer index/member/tuple-field access expressions.
    ///
    /// Returns `None` when the expression is not an access node.
    pub(super) fn infer_access_expression(&mut self, expr: &Expression) -> Option<InferType> {
        let inferred = match expr {
            Expression::Index { left, index, .. } => self.infer_index_expression(left, index),
            Expression::MemberAccess { object, member, .. } => {
                self.infer_member_access_expression(expr, object, *member)
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                self.infer_tuple_field_access_expression(object, *index)
            }
            _ => return None,
        };
        Some(inferred)
    }

    /// Infer indexing operations over arrays/lists/maps/tuples.
    fn infer_index_expression(&mut self, left: &Expression, index: &Expression) -> InferType {
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
            _ => InferType::Con(TypeConstructor::Any),
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
                .unwrap_or(InferType::Con(TypeConstructor::Any)),
            |acc, ty| self.join_types(&acc, ty),
        );
        InferType::App(TypeConstructor::Option, vec![joined])
    }

    /// Infer module/member access resolution.
    fn infer_member_access_expression(
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
            let (ty, _) = scheme.instantiate(&mut self.env.counter);
            return ty;
        }

        if let Expression::Identifier {
            name: module_name, ..
        } = object
            && *module_name == self.base_module_symbol
            && self.known_base_names.contains(&member)
        {
            self.emit_missing_base_hm_signature(member, expr.span());
        }
        self.infer_expression(object);
        InferType::Con(TypeConstructor::Any)
    }

    /// Infer tuple field projection by static index.
    fn infer_tuple_field_access_expression(
        &mut self,
        object: &Expression,
        index: usize,
    ) -> InferType {
        match self.infer_expression(object).apply_type_subst(&self.subst) {
            InferType::Tuple(elements) => elements
                .get(index)
                .cloned()
                .unwrap_or(InferType::Con(TypeConstructor::Any)),
            _ => InferType::Con(TypeConstructor::Any),
        }
    }
}
