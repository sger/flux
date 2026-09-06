//! Default method bodies, held apart from the class environment.
//!
//! A class declaration carries two unrelated things: the *types* of its
//! methods, which the constraint solver reasons about, and the *code* of any
//! default implementations, which only dispatch generation ever reads. Keeping
//! both on [`MethodSig`](super::class_env::MethodSig) meant every construction
//! site — including the four in dictionary elaboration and the one that rebuilds
//! a class from a cached `.flxi` interface — had to name a field it had no body
//! for, and meant the class environment could not be reasoned about without the
//! surface AST in scope.
//!
//! This table holds the bodies instead. It is populated from source during
//! collection and read once, by
//! [`generate_dispatch_functions`](super::class_dispatch::generate_dispatch_functions),
//! when an instance omits a method the class supplies a default for.
//!
//! # Cross-module reach
//!
//! Bodies are recorded only for classes collected from source in the current
//! compilation unit. The interface path deliberately records none: a class
//! rebuilt from a `.flxi` entry has method types but no bodies, which is the
//! same reach the `MethodSig::default_body` field had before this table
//! existed. See `docs/known_issues.md#ki-086`.

use std::collections::HashMap;

use crate::{
    syntax::{Identifier, block::Block},
    types::class_id::ClassId,
};

/// Default method bodies for the classes collected from one compilation unit.
///
/// Keyed by the owning class and the method name; a method with no default
/// implementation has no entry.
#[derive(Debug, Clone, Default)]
pub struct ClassBodies {
    defaults: HashMap<(ClassId, Identifier), Block>,
}

impl ClassBodies {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `body` as the default implementation of `method` in `class`.
    pub fn insert(&mut self, class: ClassId, method: Identifier, body: Block) {
        self.defaults.insert((class, method), body);
    }

    /// The default body for `method` in `class`, if the class declared one.
    pub fn get(&self, class: ClassId, method: Identifier) -> Option<&Block> {
        self.defaults.get(&(class, method))
    }

    /// Whether any class in this unit declared a default method body.
    pub fn is_empty(&self) -> bool {
        self.defaults.is_empty()
    }

    /// Merges `other` into this table. Entries already present win, matching
    /// the precedence the class environment gives locally collected classes
    /// over imported ones.
    pub fn merge_from(&mut self, other: &ClassBodies) {
        for (key, body) in &other.defaults {
            self.defaults.entry(*key).or_insert_with(|| body.clone());
        }
    }
}
