//! Proposal 0152, Phase 2: named-field registry.
//!
//! Populated by `collect_adt_definitions` alongside `AdtRegistry`. Phase 3
//! (type inference) reads this registry to resolve named-constructor
//! arguments, dot access, spread, and named patterns to positional indices.
//!
//! Invariants:
//! - `per_variant[adt][variant][i].index == i` (fields are stored in
//!   declaration order, one entry per positional field).
//! - An ADT appears in `per_variant` only when it has at least one
//!   named-field variant.
//! - `common[adt]` holds fields whose name *and* type appear in every
//!   variant of that ADT. Fields whose types diverge across variants are
//!   excluded and recorded in `conflicts` (for E467 emission).
//!
//! See proposal 0152.
//!
//! Phase 3 of the proposal consumes `per_variant`, `common`, and
//! `conflicts` from type inference. Until then the data-structure
//! accessors carry `#[allow(dead_code)]` to keep clippy quiet.

#![allow(dead_code)]

use std::collections::HashMap;

use crate::{
    diagnostics::position::Span,
    syntax::{data_variant::DataVariant, symbol::Symbol, type_expr::TypeExpr},
};

/// Per-field metadata used by inference when resolving named constructor
/// arguments, dot access, spread targets, and named patterns.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: Symbol,
    pub index: usize,
    pub ty: TypeExpr,
}

/// Records a field whose name is shared across every variant of an ADT but
/// whose declared type diverges. Phase 4 emits E467 from these records.
#[derive(Debug, Clone)]
pub struct FieldConflict {
    pub adt: Symbol,
    pub field: Symbol,
    pub variant1: Symbol,
    pub ty1: TypeExpr,
    pub span1: Span,
    pub variant2: Symbol,
    pub ty2: TypeExpr,
    pub span2: Span,
}

#[derive(Debug, Default)]
pub struct FieldRegistry {
    /// ADT name -> variant name -> ordered fields in declaration order.
    pub per_variant: HashMap<Symbol, HashMap<Symbol, Vec<FieldInfo>>>,
    /// ADT name -> fields common to *all* variants (same name + same type).
    pub common: HashMap<Symbol, Vec<FieldInfo>>,
    /// Shared-name/divergent-type conflicts, one record per offending field.
    pub conflicts: Vec<FieldConflict>,
}

impl FieldRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record every named variant of `adt` and recompute its common-field
    /// intersection. Called once per `data` declaration from the collection
    /// phase. Positional-only ADTs are a no-op.
    pub fn register_adt(&mut self, adt: Symbol, variants: &[DataVariant]) {
        let mut variant_map: HashMap<Symbol, Vec<FieldInfo>> = HashMap::new();
        for variant in variants {
            let Some(names) = variant.field_names.as_ref() else {
                continue;
            };
            // Parser invariant: names.len() == fields.len() for named variants.
            let fields: Vec<FieldInfo> = names
                .iter()
                .zip(variant.fields.iter())
                .enumerate()
                .map(|(index, (name, ty))| FieldInfo {
                    name: *name,
                    index,
                    ty: ty.clone(),
                })
                .collect();
            variant_map.insert(variant.name, fields);
        }

        if variant_map.is_empty() {
            return;
        }

        let common = self.compute_common(adt, variants, &variant_map);
        self.common.insert(adt, common);
        self.per_variant.insert(adt, variant_map);
    }

    /// Returns the positional field list for a given variant of a named-field
    /// ADT, or `None` if the ADT/variant is not registered as named-field.
    pub fn variant_fields(&self, adt: Symbol, variant: Symbol) -> Option<&[FieldInfo]> {
        self.per_variant
            .get(&adt)
            .and_then(|m| m.get(&variant))
            .map(Vec::as_slice)
    }

    /// Returns the fields common to every variant of `adt` (same name, same
    /// type). Empty slice for ADTs with no named variants or no overlap.
    pub fn common_fields(&self, adt: Symbol) -> &[FieldInfo] {
        self.common
            .get(&adt)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn compute_common(
        &mut self,
        adt: Symbol,
        variants: &[DataVariant],
        variant_map: &HashMap<Symbol, Vec<FieldInfo>>,
    ) -> Vec<FieldInfo> {
        // Only consider intersection across variants that are actually
        // named-field. A mix of positional and named variants is rejected
        // earlier (E465) so in practice either all or none are named.
        let named_variants: Vec<&DataVariant> = variants
            .iter()
            .filter(|v| variant_map.contains_key(&v.name))
            .collect();

        if named_variants.is_empty() {
            return Vec::new();
        }

        // Use the first named variant's field list as the candidate set,
        // preserving its declaration order (stable, user-visible order).
        let first = variant_map
            .get(&named_variants[0].name)
            .expect("first named variant is in variant_map");

        let mut out: Vec<FieldInfo> = Vec::new();
        for candidate in first {
            let mut include = true;
            let mut conflict: Option<FieldConflict> = None;

            for other in named_variants.iter().skip(1) {
                let other_fields = variant_map
                    .get(&other.name)
                    .expect("named variant is in variant_map");
                let Some(matched) = other_fields.iter().find(|f| f.name == candidate.name) else {
                    include = false;
                    break;
                };
                if !matched.ty.structural_eq(&candidate.ty) {
                    conflict = Some(FieldConflict {
                        adt,
                        field: candidate.name,
                        variant1: named_variants[0].name,
                        ty1: candidate.ty.clone(),
                        span1: named_variants[0].span,
                        variant2: other.name,
                        ty2: matched.ty.clone(),
                        span2: other.span,
                    });
                    include = false;
                    break;
                }
            }

            if let Some(c) = conflict {
                self.conflicts.push(c);
            } else if include {
                out.push(candidate.clone());
            }
        }

        out
    }
}
