//! The standard class hierarchy's modules, which every module is given.
//!
//! Proposal 0179 Stage 8 moved `Eq`, `Ord`, `Num`, `Show` and `Semigroup` from
//! Rust registration into Flux source. Before that, the Rust registration put
//! the classes into every module's class environment unconditionally, and the
//! operators rely on it: `==` emits an `Eq` obligation only if a class named
//! `Eq` is in scope, and emits *nothing* otherwise. So the modules declaring
//! them are imported into every module in the graph — the entry file through
//! the ordinary prelude, dependencies through the module graph — rather than
//! only into the entry file.
//!
//! The class modules themselves are excluded, since a module cannot import
//! itself and the five import nothing from each other except what they
//! declare explicitly.

/// `(module name, file name)` for each class-prelude module, in the order
/// they are injected.
pub const FLOW_CLASS_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Eq", "Eq.flx"),
    ("Flow.Ord", "Ord.flx"),
    ("Flow.Num", "Num.flx"),
    ("Flow.Show", "Show.flx"),
    ("Flow.Semigroup", "Semigroup.flx"),
];

/// Whether `module_name` is one of the class-prelude modules.
pub fn is_class_prelude_module(module_name: &str) -> bool {
    FLOW_CLASS_PRELUDE_MODULES
        .iter()
        .any(|(name, _)| *name == module_name)
}

/// The import statements that bring the class prelude into scope, minus any
/// already present, as Flux source.
pub fn class_prelude_import_source<'a>(existing_imports: impl Iterator<Item = &'a str>) -> String {
    let existing: Vec<&str> = existing_imports.collect();
    FLOW_CLASS_PRELUDE_MODULES
        .iter()
        .filter(|(name, _)| !existing.contains(name))
        .map(|(name, _)| format!("import {name} exposing (..)"))
        .collect::<Vec<_>>()
        .join("\n")
}
