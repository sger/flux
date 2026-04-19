//! Proposal 0152, Phase 3: inference for dot access on named-field ADTs
//! and for `{ ...base, ... }` spread expressions.

use crate::{
    diagnostics::compiler_errors::{
        NAMED_FIELD_NOT_ON_TYPE, NAMED_FIELD_TYPE_DIVERGES, NAMED_FIELD_UNKNOWN,
        SPREAD_NON_NAMED_ADT, SPREAD_UNKNOWN_VARIANT,
    },
    syntax::expression::{Expression, NamedFieldInit},
};

use super::*;

impl<'a> InferCtx<'a> {
    /// Resolve `.field` on an ADT value. Returns `Some(ty)` when the member
    /// names a named field on every variant of the value's ADT (same type),
    /// or `Some(Option<ty>)` when the member is only present on some
    /// variants. Returns `None` when `object_ty` isn't a named-field ADT;
    /// the caller then falls back to existing behavior.
    pub(super) fn resolve_named_field_access(
        &mut self,
        object_ty: &InferType,
        member: Identifier,
        span: Span,
    ) -> Option<InferType> {
        let resolved = object_ty.apply_type_subst(&self.subst);
        let adt_name = adt_name_of(&resolved)?;
        let variants = self.named_variants_of(adt_name);
        if variants.is_empty() {
            return None;
        }
        let mut present: Vec<(Identifier, InferType)> = Vec::new();
        let mut missing_any = false;
        for variant in &variants {
            match self.instantiate_variant_field(*variant, member, &resolved) {
                Some(ty) => present.push((*variant, ty)),
                None => missing_any = true,
            }
        }
        if present.is_empty() {
            self.emit_field_diag(
                &NAMED_FIELD_NOT_ON_TYPE,
                span,
                format!(
                    "Type `{}` has no field `{}`.",
                    self.interner.resolve(adt_name),
                    self.interner.resolve(member),
                ),
            );
            return Some(self.alloc_fallback_var());
        }
        let resolved_field = self.merge_field_types(adt_name, member, &present, span);
        if missing_any {
            Some(InferType::App(
                TypeConstructor::Option,
                vec![resolved_field],
            ))
        } else {
            Some(resolved_field)
        }
    }

    /// Unify field types across variants, emitting E467 on concrete
    /// divergence. Returns the unified type after applying the substitution.
    fn merge_field_types(
        &mut self,
        adt_name: Identifier,
        member: Identifier,
        present: &[(Identifier, InferType)],
        span: Span,
    ) -> InferType {
        let mut iter = present.iter();
        let (first_variant, first_ty) = iter.next().expect("present is non-empty");
        let first_variant = *first_variant;
        let mut merged = first_ty.clone();
        let first_resolved = first_ty.apply_type_subst(&self.subst);
        for (variant, ty) in iter {
            let ty_resolved = ty.apply_type_subst(&self.subst);
            if first_resolved.is_concrete()
                && ty_resolved.is_concrete()
                && first_resolved != ty_resolved
            {
                self.emit_field_diag(
                    &NAMED_FIELD_TYPE_DIVERGES,
                    span,
                    format!(
                        "Field `{}` has inconsistent types across variants of `{}` (`{}` in `{}`, `{}` in `{}`).",
                        self.interner.resolve(member),
                        self.interner.resolve(adt_name),
                        self.display_type(&first_resolved),
                        self.interner.resolve(first_variant),
                        self.display_type(&ty_resolved),
                        self.interner.resolve(*variant),
                    ),
                );
                break;
            }
            merged = self.unify_silent(&merged, ty);
        }
        merged.apply_type_subst(&self.subst)
    }

    /// Infer a `{ ...base, field: value, ... }` spread expression.
    pub(super) fn infer_spread_expression(
        &mut self,
        base: &Expression,
        overrides: &[NamedFieldInit],
        span: Span,
    ) -> InferType {
        let base_ty = self.infer_expression(base);
        let resolved = base_ty.apply_type_subst(&self.subst);
        let Some(variant) = self.resolve_spread_variant(base, &resolved, span) else {
            return self.alloc_fallback_var();
        };
        let info = match self.adt_constructor_types.get(&variant).cloned() {
            Some(i) => i,
            None => return self.alloc_fallback_var(),
        };
        let Some(field_names) = info.field_names.clone() else {
            return self.alloc_fallback_var();
        };
        let Some((param_tys, result_ty)) = self.instantiate_constructor_parts(variant) else {
            return self.alloc_fallback_var();
        };
        self.unify_silent(&resolved, &result_ty);
        self.infer_spread_overrides(variant, &field_names, &param_tys, overrides);
        result_ty.apply_type_subst(&self.subst)
    }

    /// Determine which named-field variant `base` refers to. Emits E464 when
    /// the base type is not a named-field ADT and E468 when a multi-variant
    /// ADT's variant cannot be determined statically.
    fn resolve_spread_variant(
        &mut self,
        base: &Expression,
        resolved: &InferType,
        span: Span,
    ) -> Option<Identifier> {
        let Some(adt_name) = adt_name_of(resolved) else {
            self.emit_field_diag(
                &SPREAD_NON_NAMED_ADT,
                span,
                format!(
                    "Cannot spread a value of type `{}`; spread requires a named-field data type.",
                    self.display_type(resolved)
                ),
            );
            return None;
        };
        let variants = self.named_variants_of(adt_name);
        if variants.is_empty() {
            self.emit_field_diag(
                &SPREAD_NON_NAMED_ADT,
                span,
                format!(
                    "Cannot spread a value of type `{}`; spread requires a named-field data type.",
                    self.display_type(resolved)
                ),
            );
            return None;
        }
        if variants.len() == 1 {
            return Some(variants[0]);
        }
        if let Expression::NamedConstructor { name, .. } = base {
            return Some(*name);
        }
        self.emit_field_diag(
            &SPREAD_UNKNOWN_VARIANT,
            span,
            format!(
                "Cannot spread `{}` because its variant is not statically known.",
                self.display_type(resolved)
            ),
        );
        None
    }

    /// Type-check each spread override against the known variant's field
    /// types, emitting E461 for any override that names an unknown field.
    fn infer_spread_overrides(
        &mut self,
        variant: Identifier,
        field_names: &[Identifier],
        param_tys: &[InferType],
        overrides: &[NamedFieldInit],
    ) {
        for override_init in overrides {
            let Some(index) = field_names.iter().position(|n| *n == override_init.name) else {
                self.emit_field_diag(
                    &NAMED_FIELD_UNKNOWN,
                    override_init.span,
                    format!(
                        "`{}` has no field named `{}`.",
                        self.interner.resolve(variant),
                        self.interner.resolve(override_init.name),
                    ),
                );
                continue;
            };
            let expected = &param_tys[index];
            let actual = match &override_init.value {
                Some(expr) => self.infer_expression(expr),
                None => self.infer_punned_field(override_init.name, override_init.span),
            };
            self.unify_reporting(&actual, expected, override_init.span);
        }
    }
}

/// Extract the ADT identifier from an `InferType`, handling both the
/// zero-argument (`Con`) and applied (`App`) forms.
fn adt_name_of(ty: &InferType) -> Option<Identifier> {
    match ty {
        InferType::Con(TypeConstructor::Adt(n)) => Some(*n),
        InferType::App(TypeConstructor::Adt(n), _) => Some(*n),
        _ => None,
    }
}
