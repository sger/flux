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

    /// The scheme of `module.member`, however the module was named.
    ///
    /// Schemes preloaded for an importing file are keyed by the import binding
    /// **as written** (`build_preloaded_hm_member_schemes`), while schemes
    /// captured from a module body are keyed by its declared name. Both spellings
    /// must be tried: resolving the alias *instead of* consulting the written
    /// name drops every preloaded entry, which is silent — the member degrades
    /// to a fallback variable and surfaces later as E430 at the call site, or,
    /// where the lookup is a guard rather than a source of types, as a check
    /// that simply stops firing.
    pub(super) fn module_member_scheme(
        &self,
        module_name: Identifier,
        member: Identifier,
    ) -> Option<&crate::types::scheme::Scheme> {
        self.module_member_schemes
            .get(&(module_name, member))
            .or_else(|| {
                self.module_aliases
                    .get(&module_name)
                    .and_then(|target| self.module_member_schemes.get(&(*target, member)))
            })
    }

    /// Whether `module.member` resolves to a known member under either spelling.
    pub(super) fn knows_module_member(&self, module_name: Identifier, member: Identifier) -> bool {
        self.module_member_scheme(module_name, member).is_some()
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
            // Preloaded schemes are keyed by the import binding as written
            // (`build_preloaded_hm_member_schemes`), while schemes captured
            // from a module body are keyed by its declared name. Try the name
            // as written first, then the module the alias resolves to, so
            // neither form of key is missed — resolving the alias *instead of*
            // consulting the written name drops every preloaded entry and the
            // member degrades to a fallback variable, which surfaces as E430
            // at the call site rather than here.
            && let Some(scheme) = self.module_member_scheme(*module_name, member).cloned()
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
        // Named-field dot access (Proposal 0152). If the object's inferred
        // type is an ADT with named-field variants, try resolving `member`
        // against those fields. Returns the field type when it is common to
        // all variants with the same type; otherwise lifts to Option<T>.
        let object_ty = self.infer_expression(object);
        if let Some(field_ty) = self.resolve_named_field_access(&object_ty, member, expr.span()) {
            return field_ty;
        }
        if !self.is_field_predicate_receiver(object, &object_ty) {
            return self.alloc_fallback_var();
        }
        self.emit_field_predicate(&object_ty, member, expr.span())
    }

    /// Whether `object` is a receiver a field predicate may be emitted for
    /// (Proposal 0184).
    ///
    /// The predicate replaces the hole for a receiver whose type is *not yet
    /// known*, which is the case 0184 is about. A receiver whose type is
    /// already settled and is still not a named-field ADT is a different
    /// situation, and keeps the behaviour it had.
    ///
    /// That distinction matters because `a.b` is also the syntax for reaching
    /// into a module, and an unresolved module member falls through to the
    /// field path — an import that is missing, private, or misspelled, already
    /// reported as E011/E012/E013. Such a receiver either has no type or has a
    /// settled non-record one (`Lock.lock(..)`, where `Lock` also names a
    /// constructor), and neither is a field access to report on.
    ///
    /// The two tests are not redundant. A name with no value binding is a
    /// module path, not a receiver: a module referred to from inside its own
    /// body (`Parse.here(..)` in `Flow.Toml.Parse`) has no binding and no type,
    /// so the type test alone would take it for an unknown record.
    fn is_field_predicate_receiver(&self, object: &Expression, object_ty: &InferType) -> bool {
        if !matches!(object_ty.apply_type_subst(&self.subst), InferType::Var(_)) {
            return false;
        }
        if let Expression::Identifier { name, .. } = object
            && self.env.lookup(*name).is_none()
        {
            return false;
        }
        true
    }

    /// Record that `object` must have a field `member`, and return that
    /// field's type (Proposal 0184).
    ///
    /// Reached when the receiver's type is not yet a known named-field ADT.
    /// Before 0184 this allocated a *fallback* variable — one
    /// `resolve_binding_schemes` excludes from every scheme's `forall`, so it
    /// could never be quantified and could only be filled in by unifying the
    /// enclosing definition with a call site. That made field access depend on
    /// the definition staying monomorphic, and left the obligation with no
    /// terminal state.
    ///
    /// The predicate says the same thing in a form the solver can act on:
    /// `__field.member<Receiver, Field>`. Discharging it *determines* the field
    /// type, which is GHC's functional dependency `x r -> a` on
    /// `HasField x r a`, so the type propagates where the hole could not.
    ///
    /// The field type is an ordinary variable, not a fallback one: it is
    /// resolved by solving rather than by call-site unification.
    fn emit_field_predicate(
        &mut self,
        object_ty: &InferType,
        member: Identifier,
        span: Span,
    ) -> InferType {
        let Some(module) = self.field_predicate_module else {
            return self.alloc_fallback_var();
        };
        let field_ty = self.env.alloc_infer_type_var();
        self.emit_class_constraint_args_for_id(
            crate::types::class_id::ClassId::new(module, member),
            member,
            vec![object_ty.clone(), field_ty.clone()],
            span,
            constraint::WantedClassConstraintOrigin::FieldAccess,
        );
        field_ty
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
                let elements: Vec<InferType> = (0..arity)
                    .map(|_| self.env.alloc_infer_type_var())
                    .collect();
                let projected = elements[index].clone();
                let tuple_shape = InferType::Tuple(elements);
                self.unify_silent(&object_ty, &tuple_shape);
                projected.apply_type_subst(&self.subst)
            }
            _other => self.alloc_fallback_var(),
        }
    }
}
