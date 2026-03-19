//! Shared metadata collection utilities for native backends (JIT, LLVM).
//!
//! These functions traverse `IrTopLevelItem` trees to collect ADT constructor
//! arities, module names, import aliases, and module-scoped function mappings.
//! Both the Cranelift JIT and LLVM backends use these to avoid duplicating
//! the same IR traversal logic.

use std::collections::HashMap;

use crate::cfg::{FunctionId, IrTopLevelItem};
use crate::syntax::Identifier;

/// Recursively collect ADT constructor arities from all Data declarations,
/// including those nested inside Module items.
pub fn collect_adt_constructors(
    items: &[IrTopLevelItem],
    adt_constructors: &mut HashMap<Identifier, usize>,
) {
    for item in items {
        match item {
            IrTopLevelItem::Data { variants, .. } => {
                for variant in variants {
                    adt_constructors.insert(variant.name, variant.fields.len());
                }
            }
            IrTopLevelItem::Module { body, .. } => {
                collect_adt_constructors(body, adt_constructors);
            }
            _ => {}
        }
    }
}

/// Recursively collect module names and import aliases from top-level items.
pub fn collect_module_metadata(
    items: &[IrTopLevelItem],
    module_names: &mut Vec<Identifier>,
    import_aliases: &mut HashMap<Identifier, Identifier>,
) {
    for item in items {
        match item {
            IrTopLevelItem::Module { name, body, .. } => {
                module_names.push(*name);
                collect_module_metadata(body, module_names, import_aliases);
            }
            IrTopLevelItem::Import { name, alias, .. } => {
                module_names.push(*name);
                if let Some(alias_id) = alias {
                    module_names.push(*alias_id);
                    import_aliases.insert(*alias_id, *name);
                }
            }
            _ => {}
        }
    }
}

/// Recursively collect module-scoped functions, parameterized on the value
/// stored for each function. LLVM stores `usize` (function index), JIT
/// stores its own metadata type.
///
/// After collecting, import aliases are resolved: if module `Foo` is imported
/// as `F`, all `(Foo, fn_name)` entries are duplicated as `(F, fn_name)`.
pub fn collect_module_functions<V: Clone>(
    items: &[IrTopLevelItem],
    current_module: Option<Identifier>,
    resolve_fn: &impl Fn(FunctionId) -> Option<V>,
    module_functions: &mut HashMap<(Identifier, Identifier), V>,
) {
    for item in items {
        match item {
            IrTopLevelItem::Function {
                name, function_id, ..
            } => {
                if let (Some(mod_name), Some(fn_id)) = (current_module, function_id)
                    && let Some(value) = resolve_fn(*fn_id)
                {
                    module_functions.insert((mod_name, *name), value);
                }
            }
            IrTopLevelItem::Module { name, body, .. } => {
                collect_module_functions(body, Some(*name), resolve_fn, module_functions);
            }
            _ => {}
        }
    }
}

/// After collecting module functions, duplicate entries for import aliases.
/// E.g., `import Foo as F` means `(Foo, bar)` is also accessible as `(F, bar)`.
pub fn apply_import_aliases<V: Clone>(
    module_functions: &mut HashMap<(Identifier, Identifier), V>,
    import_aliases: &HashMap<Identifier, Identifier>,
) {
    for (alias_id, original_name) in import_aliases {
        let aliased: Vec<_> = module_functions
            .iter()
            .filter(|((mod_id, _), _)| *mod_id == *original_name)
            .map(|((_, fn_id), val)| ((*alias_id, *fn_id), val.clone()))
            .collect();
        for (key, val) in aliased {
            module_functions.insert(key, val);
        }
    }
}
