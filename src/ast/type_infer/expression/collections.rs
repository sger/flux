use super::*;

impl<'a> InferCtx<'a> {
    /// Infer collection constructors and literals.
    ///
    /// Behavior:
    /// - Dispatches to collection-shape specific helpers.
    ///
    /// Returns:
    /// - `Some(type)` when `expr` belongs to the collection family.
    /// - `None` otherwise.
    pub(super) fn infer_collection_expression(&mut self, expr: &Expression) -> Option<InferType> {
        let inferred = match expr {
            Expression::TupleLiteral { elements, .. } => {
                self.infer_tuple_literal_expression(elements)
            }
            Expression::ListLiteral { elements, span } => {
                self.infer_list_literal_expression(elements, *span)
            }
            Expression::ArrayLiteral { elements, .. } => {
                self.infer_array_literal_expression(elements)
            }
            Expression::EmptyList { .. } => {
                InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()])
            }
            Expression::Hash { pairs, .. } => self.infer_hash_literal_expression(pairs),
            Expression::Cons { head, tail, span } => self.infer_cons_expression(head, tail, *span),
            _ => return None,
        };
        Some(inferred)
    }

    /// Infer tuple literals by inferring each element in order.
    fn infer_tuple_literal_expression(&mut self, elements: &[Expression]) -> InferType {
        let elem_tys: Vec<InferType> = elements.iter().map(|e| self.infer_expression(e)).collect();
        InferType::Tuple(elem_tys)
    }

    /// Infer list literals and unify all elements with the first element type.
    fn infer_list_literal_expression(&mut self, elements: &[Expression], span: Span) -> InferType {
        if elements.is_empty() {
            return InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()]);
        }
        let first = self.infer_expression(&elements[0]);
        for element in elements.iter().skip(1) {
            let ty = self.infer_expression(element);
            self.unify_reporting(&first, &ty, span);
        }
        InferType::App(
            TypeConstructor::List,
            vec![first.apply_type_subst(&self.subst)],
        )
    }

    /// Infer array literals, reducing heterogeneous element sets to `Array<Any>`.
    fn infer_array_literal_expression(&mut self, elements: &[Expression]) -> InferType {
        if elements.is_empty() {
            return InferType::App(TypeConstructor::Array, vec![self.env.fresh_infer_type()]);
        }
        let first = self.infer_expression(&elements[0]);
        let mut homogeneous = true;
        for element in elements.iter().skip(1) {
            let ty = self.infer_expression(element);
            let ty_resolved = ty.apply_type_subst(&self.subst);
            let first_resolved = first.apply_type_subst(&self.subst);
            if ty_resolved != first_resolved {
                homogeneous = false;
                self.unify_reporting(&first, &ty, element.span());
            }
        }
        let elem_ty = if homogeneous {
            first.apply_type_subst(&self.subst)
        } else {
            InferType::Con(TypeConstructor::Any)
        };
        InferType::App(TypeConstructor::Array, vec![elem_ty])
    }

    /// Infer hash literals from the first pair shape, evaluating all pairs for constraints.
    fn infer_hash_literal_expression(&mut self, pairs: &[(Expression, Expression)]) -> InferType {
        if pairs.is_empty() {
            let key = self.env.fresh_infer_type();
            let value = self.env.fresh_infer_type();
            return InferType::App(TypeConstructor::Map, vec![key, value]);
        }
        let key_ty = self.infer_expression(&pairs[0].0);
        let value_ty = self.infer_expression(&pairs[0].1);
        for (key, value) in pairs.iter().skip(1) {
            self.infer_expression(key);
            self.infer_expression(value);
        }
        InferType::App(TypeConstructor::Map, vec![key_ty, value_ty])
    }

    /// Infer cons expressions and constrain the tail to the constructed list type.
    fn infer_cons_expression(
        &mut self,
        head: &Expression,
        tail: &Expression,
        span: Span,
    ) -> InferType {
        let elem_ty = self.infer_expression(head);
        let list_ty = InferType::App(TypeConstructor::List, vec![elem_ty]);
        let tail_ty = self.infer_expression(tail);
        self.unify_reporting(&list_ty, &tail_ty, span);
        list_ty.apply_type_subst(&self.subst)
    }
}
