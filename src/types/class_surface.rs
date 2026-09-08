//! The surface syntax of a class declaration, held apart from the class
//! environment.
//!
//! A `class` declaration carries two unrelated kinds of thing:
//!
//! - what the constraint solver reasons about — the *types* of its methods; and
//! - surface syntax, which only the frontend wants: the [`TypeExpr`] a method
//!   was written with, and the [`Block`] of any default implementation.
//!
//! Keeping both on [`MethodSig`](super::class_env::MethodSig) meant the class
//! environment could not be reasoned about without the AST in scope, and that
//! every construction site — including the four in dictionary elaboration and
//! the one that rebuilds a class from a cached `.flxi` interface — had to name
//! fields it had no surface syntax for.
//!
//! This table holds the surface instead. It is populated from source during
//! collection and read by the passes that genuinely need syntax:
//!
//! | reader | wants |
//! |---|---|
//! | [`class_dispatch`](super::class_dispatch) | `TypeExpr` to put in a generated `Statement::Function` |
//! | [`kind_check`](super::kind_check) | the `Span` on a `TypeExpr`, to anchor a diagnostic |
//! | the `.flxi` interface writer | `TypeExpr`, which is what the format stores |
//! | [`class_dispatch`](super::class_dispatch) | the default `Block` to clone into an instance |
//!
//! Everything else reads the `InferType` on `MethodSig`.
//!
//! # Cross-module reach
//!
//! Entries are recorded only for classes collected from source in the current
//! compilation unit. The interface path deliberately records no default body: a
//! class rebuilt from a `.flxi` entry has method types but no bodies, which is
//! the reach `MethodSig::default_body` had before this table existed. See
//! `docs/known_issues.md#ki-086`.

use std::collections::HashMap;

use crate::{
    syntax::{Identifier, block::Block, type_expr::TypeExpr},
    types::class_id::ClassId,
};

/// The surface syntax one class method was written with.
#[derive(Debug, Clone, Default)]
pub struct MethodSurface {
    /// Value-parameter types in source order, as written.
    pub param_types: Vec<TypeExpr>,
    /// Return type, as written.
    pub return_type: Option<TypeExpr>,
    /// The default implementation, when the class declared one.
    pub default_body: Option<Block>,
}

/// Surface syntax for the classes collected from one compilation unit.
///
/// Keyed by the owning class and the method name.
#[derive(Debug, Clone, Default)]
pub struct ClassSurface {
    methods: HashMap<(ClassId, Identifier), MethodSurface>,
}

impl ClassSurface {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the surface syntax of `method` in `class`, replacing any entry.
    pub fn insert(&mut self, class: ClassId, method: Identifier, surface: MethodSurface) {
        self.methods.insert((class, method), surface);
    }

    /// The surface syntax of `method` in `class`, if it was collected from
    /// source in this unit.
    pub fn get(&self, class: ClassId, method: Identifier) -> Option<&MethodSurface> {
        self.methods.get(&(class, method))
    }

    /// The default body for `method` in `class`, if the class declared one.
    pub fn default_body(&self, class: ClassId, method: Identifier) -> Option<&Block> {
        self.get(class, method)?.default_body.as_ref()
    }

    /// The declared parameter types for `method` in `class`.
    pub fn param_types(&self, class: ClassId, method: Identifier) -> Option<&[TypeExpr]> {
        Some(&self.get(class, method)?.param_types)
    }

    /// The declared return type for `method` in `class`.
    pub fn return_type(&self, class: ClassId, method: Identifier) -> Option<&TypeExpr> {
        self.get(class, method)?.return_type.as_ref()
    }

    /// Whether any class in this unit contributed surface syntax.
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }

    /// Merges `other` into this table. Entries already present win, matching
    /// the precedence the class environment gives locally collected classes
    /// over imported ones.
    pub fn merge_from(&mut self, other: &ClassSurface) {
        for (key, surface) in &other.methods {
            self.methods.entry(*key).or_insert_with(|| surface.clone());
        }
    }
}
