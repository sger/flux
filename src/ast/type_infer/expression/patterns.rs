use crate::syntax::expression::Pattern;

use super::*;

/// Classifies the type-constraining family of a match pattern.
///
/// Used to identify a shared scrutinee type across match arms so that a
/// concrete expected type can be propagated before arm bodies are inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatternFamily {
    Option,
    Either,
    List,
    Tuple(usize),
    Adt(Identifier),
    NonConstraining,
    UnknownOrMixed,
}

impl<'a> InferCtx<'a> {
    /// Bind variables introduced by `pattern` using the resolved scrutinee type.
    ///
    /// Behavior:
    /// - Resolves `scrutinee_ty` through current substitution.
    /// - Dispatches to family-specific binders.
    ///
    /// Side effects:
    /// - Mutates local type environment with new bindings.
    /// - May mutate substitution through unification.
    /// - May append diagnostics when tuple binding conflicts are concrete.
    ///
    /// Diagnostics:
    /// - Emits tuple-shape mismatch diagnostics for concrete scrutinee conflicts.
    ///
    /// Returns:
    /// - No return value; updates inference context state in place.
    pub(in crate::ast::type_infer) fn bind_pattern_variables(
        &mut self,
        pattern: &Pattern,
        scrutinee_ty: &InferType,
        span: Span,
    ) {
        let resolved_scrutinee = scrutinee_ty.apply_type_subst(&self.subst);
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.env.bind(*name, Scheme::mono(resolved_scrutinee));
            }
            Pattern::Wildcard { .. } => {}
            Pattern::Literal { expression, .. } => {
                let literal_ty = self.infer_expression(expression);
                self.unify_reporting(&resolved_scrutinee, &literal_ty, span);
            }
            Pattern::None { .. } | Pattern::Some { .. } => {
                self.bind_option_pattern_variables(pattern, &resolved_scrutinee, span)
            }
            Pattern::Left { .. } | Pattern::Right { .. } => {
                self.bind_either_pattern_variables(pattern, &resolved_scrutinee, span)
            }
            Pattern::EmptyList { .. } => {
                let elem = self.env.alloc_infer_type_var();
                let expected = InferType::App(TypeConstructor::List, vec![elem]);
                self.unify_reporting(&resolved_scrutinee, &expected, span);
            }
            Pattern::Cons { head, tail, .. } => {
                self.bind_list_pattern_variables(head, tail, &resolved_scrutinee, span)
            }
            Pattern::Tuple { elements, .. } => {
                self.bind_tuple_pattern_variables(elements, &resolved_scrutinee, span)
            }
            Pattern::Constructor { fields, .. } => {
                self.bind_constructor_pattern_variables(pattern, fields, &resolved_scrutinee, span)
            }
        }
    }

    /// Bind `Option` family patterns (`None`/`Some`).
    fn bind_option_pattern_variables(
        &mut self,
        pattern: &Pattern,
        resolved_scrutinee: &InferType,
        span: Span,
    ) {
        match pattern {
            Pattern::None { .. } => {
                let inner = self.env.alloc_infer_type_var();
                let expected = InferType::App(TypeConstructor::Option, vec![inner]);
                self.unify_reporting(resolved_scrutinee, &expected, span);
            }
            Pattern::Some { pattern, .. } => {
                let inner = self.env.alloc_infer_type_var();
                let expected = InferType::App(TypeConstructor::Option, vec![inner.clone()]);
                let unified = self.unify_reporting(resolved_scrutinee, &expected, span);
                let inner_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Option, args) if args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => inner.apply_type_subst(&self.subst),
                };
                self.bind_pattern_variables(pattern, &inner_ty, span);
            }
            _ => {}
        }
    }

    /// Bind `Either` family patterns (`Left`/`right`).
    fn bind_either_pattern_variables(
        &mut self,
        pattern: &Pattern,
        resolved_scrutinee: &InferType,
        span: Span,
    ) {
        match pattern {
            Pattern::Left { pattern, .. } => {
                let left = self.env.alloc_infer_type_var();
                let right = self.env.alloc_infer_type_var();
                let expected = InferType::App(TypeConstructor::Either, vec![left.clone(), right]);
                let unified = self.unify_reporting(resolved_scrutinee, &expected, span);
                let left_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[0].clone()
                    }
                    _ => left.apply_type_subst(&self.subst),
                };
                self.bind_pattern_variables(pattern, &left_ty, span);
            }
            Pattern::Right { pattern, .. } => {
                let left = self.env.alloc_infer_type_var();
                let right = self.env.alloc_infer_type_var();
                let expected = InferType::App(TypeConstructor::Either, vec![left, right.clone()]);
                let unified = self.unify_reporting(resolved_scrutinee, &expected, span);
                let right_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[1].clone()
                    }
                    _ => right.apply_type_subst(&self.subst),
                };
                self.bind_pattern_variables(pattern, &right_ty, span);
            }
            _ => {}
        }
    }

    /// Bind `Cons` list patterns (`head :: tail`).
    fn bind_list_pattern_variables(
        &mut self,
        head: &Pattern,
        tail: &Pattern,
        resolved_scrutinee: &InferType,
        span: Span,
    ) {
        let elem = self.env.alloc_infer_type_var();
        let list_ty = InferType::App(TypeConstructor::List, vec![elem.clone()]);
        let unified = self.unify_reporting(resolved_scrutinee, &list_ty, span);
        let element_ty = match unified.apply_type_subst(&self.subst) {
            InferType::App(TypeConstructor::List, args) if args.len() == 1 => args[0].clone(),
            _ => elem.apply_type_subst(&self.subst),
        };
        self.bind_pattern_variables(head, &element_ty, span);
        self.bind_pattern_variables(tail, &list_ty, span);
    }

    /// Bind tuple patterns and propagate tuple member types when available.
    fn bind_tuple_pattern_variables(
        &mut self,
        elements: &[Pattern],
        resolved_scrutinee: &InferType,
        span: Span,
    ) {
        let tuple_shape = InferType::Tuple(
            elements
                .iter()
                .map(|_| self.env.alloc_infer_type_var())
                .collect(),
        );
        let unified = self.unify_reporting(resolved_scrutinee, &tuple_shape, span);
        if let InferType::Tuple(component_types) = unified.apply_type_subst(&self.subst) {
            for (elem, elem_ty) in elements.iter().zip(component_types.iter()) {
                self.bind_pattern_variables(elem, elem_ty, span);
            }
            return;
        }

        if Self::is_fully_concrete(resolved_scrutinee) {
            let expected = self.display_type(&tuple_shape.apply_type_subst(&self.subst));
            let actual = self.display_type(resolved_scrutinee);
            self.errors.push(type_unification_error(
                self.file_path.clone(),
                span,
                &expected,
                &actual,
            ));
        }
        for elem in elements {
            let fallback = self.alloc_fallback_var();
            self.bind_pattern_variables(elem, &fallback, span);
        }
    }

    /// Bind ADT constructor patterns and propagate field types into pattern bindings.
    fn bind_constructor_pattern_variables(
        &mut self,
        pattern: &Pattern,
        fields: &[Pattern],
        resolved_scrutinee: &InferType,
        span: Span,
    ) {
        if let Pattern::Constructor { name, .. } = pattern
            && let Some((field_ty, result_ty)) = self.instantiate_constructor_parts(*name)
        {
            self.unify_reporting(resolved_scrutinee, &result_ty, span);
            if field_ty.len() == fields.len() {
                for (field, field_ty) in fields.iter().zip(field_ty.iter()) {
                    self.bind_pattern_variables(field, field_ty, span);
                }
            } else {
                if self.strict_mode_enabled() {
                    self.emit_strict_inference_error(
                        pattern.span(),
                        format!(
                            "Pattern `{}` uses the wrong number of constructor fields in strict mode.",
                            pattern.display_with(self.interner)
                        ),
                        "Match the constructor arity exactly in the pattern.",
                    );
                }
                for field in fields {
                    let fallback = self.alloc_fallback_var();
                    self.bind_pattern_variables(field, &fallback, span);
                }
            }
            return;
        }
        if self.strict_mode_enabled() {
            self.emit_strict_inference_error(
                pattern.span(),
                format!(
                    "Could not resolve constructor pattern `{}` in strict mode.",
                    pattern.display_with(self.interner)
                ),
                "Make sure the constructor exists and is in scope before using it in a pattern.",
            );
        }
        for field in fields {
            let fallback = self.alloc_fallback_var();
            self.bind_pattern_variables(field, &fallback, span);
        }
    }

    /// Classify the type-constraining family represented by a pattern.
    pub(super) fn pattern_family(&self, pattern: &Pattern) -> PatternFamily {
        match pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } | Pattern::Literal { .. } => {
                PatternFamily::NonConstraining
            }
            Pattern::None { .. } | Pattern::Some { .. } => PatternFamily::Option,
            Pattern::Left { .. } | Pattern::Right { .. } => PatternFamily::Either,
            Pattern::EmptyList { .. } | Pattern::Cons { .. } => PatternFamily::List,
            Pattern::Tuple { elements, .. } => PatternFamily::Tuple(elements.len()),
            Pattern::Constructor { name, .. } => self
                .adt_constructor_types
                .get(name)
                .map(|info| PatternFamily::Adt(info.adt_name))
                .unwrap_or(PatternFamily::UnknownOrMixed),
        }
    }

    /// Compute a shared constraining family across match arms when possible.
    pub(super) fn shared_pattern_family(&self, arms: &[MatchArm]) -> Option<PatternFamily> {
        let mut family: Option<PatternFamily> = None;
        for arm in arms {
            let arm_family = self.pattern_family(&arm.pattern);
            match arm_family {
                PatternFamily::NonConstraining => {}
                PatternFamily::UnknownOrMixed => return None,
                _ => match &family {
                    None => family = Some(arm_family),
                    Some(existing) if *existing == arm_family => {}
                    Some(_) => return None,
                },
            }
        }
        family
    }

    /// Build the expected scrutinee type for a concrete pattern family.
    pub(super) fn expected_type_for_pattern_family(
        &mut self,
        family: &PatternFamily,
    ) -> Option<InferType> {
        match family {
            PatternFamily::Option => Some(InferType::App(
                TypeConstructor::Option,
                vec![self.env.alloc_infer_type_var()],
            )),
            PatternFamily::Either => Some(InferType::App(
                TypeConstructor::Either,
                vec![
                    self.env.alloc_infer_type_var(),
                    self.env.alloc_infer_type_var(),
                ],
            )),
            PatternFamily::List => Some(InferType::App(
                TypeConstructor::List,
                vec![self.env.alloc_infer_type_var()],
            )),
            PatternFamily::Tuple(arity) => Some(InferType::Tuple(
                (0..*arity)
                    .map(|_| self.env.alloc_infer_type_var())
                    .collect(),
            )),
            PatternFamily::Adt(adt_name) => {
                let type_params = self.adt_type_params.get(adt_name)?;
                if type_params.is_empty() {
                    Some(InferType::Con(TypeConstructor::Adt(*adt_name)))
                } else {
                    Some(InferType::App(
                        TypeConstructor::Adt(*adt_name),
                        type_params
                            .iter()
                            .map(|_| self.env.alloc_infer_type_var())
                            .collect(),
                    ))
                }
            }
            PatternFamily::NonConstraining | PatternFamily::UnknownOrMixed => None,
        }
    }
}
