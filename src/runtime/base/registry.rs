use crate::runtime::base_function::BaseFunction;

use super::BASE_FUNCTIONS;

/// Canonical Base fastcall allowlist used by VM/JIT lowering (`OpCallBase`).
pub const BASE_FASTCALL_ALLOWLIST: &[&str] = &[
    "all",
    "any",
    "chars",
    "contains",
    "count",
    "delete",
    "ends_with",
    "filter",
    "find",
    "first",
    "flat_map",
    "flatten",
    "fold",
    "is_array",
    "is_bool",
    "is_float",
    "is_hash",
    "is_int",
    "is_map",
    "is_none",
    "is_some",
    "is_string",
    "keys",
    "last",
    "len",
    "lower",
    "map",
    "merge",
    "parse_int",
    "parse_ints",
    "replace",
    "rest",
    "reverse",
    "slice",
    "sort",
    "sort_by",
    "split_ints",
    "starts_with",
    "to_string",
    "trim",
    "type_of",
    "upper",
    "values",
    "zip",
];

/// Synthetic Base module registry facade.
///
/// Base is the language-level prelude surface. It is backed by the canonical
/// Base function registry and preserves stable Base function indices.
#[derive(Debug, Clone, Copy, Default)]
pub struct BaseModule;

impl BaseModule {
    /// Creates a Base registry view.
    pub fn new() -> Self {
        Self
    }

    /// Returns all Base names in deterministic index order.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        BASE_FUNCTIONS.iter().map(|b| b.name)
    }

    /// Returns the Base entry count.
    pub fn len(self) -> usize {
        BASE_FUNCTIONS.len()
    }

    /// Returns true when the Base registry has no entries.
    pub fn is_empty(self) -> bool {
        BASE_FUNCTIONS.is_empty()
    }

    /// Returns the Base entry for a given index.
    pub fn by_index(self, index: usize) -> Option<&'static BaseFunction> {
        BASE_FUNCTIONS.get(index)
    }

    /// Returns the index for a given Base name.
    pub fn index_of(self, name: &str) -> Option<usize> {
        BASE_FUNCTIONS.iter().position(|b| b.name == name)
    }
}

/// Looks up a Base function by name.
pub fn get_base_function(name: &str) -> Option<&'static BaseFunction> {
    BASE_FUNCTIONS.iter().find(|b| b.name == name)
}

/// Looks up a Base function index by name.
pub fn get_base_function_index(name: &str) -> Option<usize> {
    BASE_FUNCTIONS.iter().position(|b| b.name == name)
}

/// Looks up a Base function by index.
pub fn get_base_function_by_index(index: usize) -> Option<&'static BaseFunction> {
    BASE_FUNCTIONS.get(index)
}

/// Returns true when a Base function name is allowlisted for `OpCallBase` fastcall lowering.
pub fn is_base_fastcall_allowlisted(name: &str) -> bool {
    BASE_FASTCALL_ALLOWLIST.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        BASE_FASTCALL_ALLOWLIST, BASE_FUNCTIONS, BaseModule, is_base_fastcall_allowlisted,
    };

    #[test]
    fn base_registry_names_are_index_stable() {
        let base = BaseModule::new();
        for (idx, name) in base.names().enumerate() {
            assert_eq!(base.index_of(name), Some(idx));
            let entry = base.by_index(idx).expect("base entry must exist");
            assert_eq!(entry.name, name);
        }
        assert_eq!(base.by_index(base.len()), None);
    }

    #[test]
    fn base_fastcall_allowlist_contains_only_registered_base_names() {
        let registered: BTreeSet<_> = BASE_FUNCTIONS.iter().map(|b| b.name).collect();
        for name in BASE_FASTCALL_ALLOWLIST {
            assert!(
                registered.contains(name),
                "allowlisted name '{}' is not a registered Base function",
                name
            );
            assert!(
                is_base_fastcall_allowlisted(name),
                "allowlisted name '{}' must return true",
                name
            );
        }
    }

    #[test]
    fn base_fastcall_allowlist_is_sorted() {
        assert!(
            BASE_FASTCALL_ALLOWLIST
                .windows(2)
                .all(|pair| pair[0] <= pair[1]),
            "BASE_FASTCALL_ALLOWLIST must remain sorted for binary_search"
        );
    }

    #[test]
    fn base_fastcall_classification_is_explicit_and_total() {
        let allowlisted: BTreeSet<_> = BASE_FASTCALL_ALLOWLIST.iter().copied().collect();
        let explicitly_non_allowlisted: BTreeSet<_> = [
            "print",
            "push",
            "concat",
            "split",
            "join",
            "substring",
            "has_key",
            "abs",
            "min",
            "max",
            "hd",
            "tl",
            "is_list",
            "to_list",
            "to_array",
            "put",
            "get",
            "list",
            "read_file",
            "read_lines",
            "read_stdin",
            "now_ms",
            "time",
            "range",
            "sum",
            "product",
            "assert_eq",
            "assert_neq",
            "assert_true",
            "assert_false",
            "assert_throws",
        ]
        .into_iter()
        .collect();

        let registered: BTreeSet<_> = BASE_FUNCTIONS.iter().map(|b| b.name).collect();
        let classified: BTreeSet<_> = allowlisted
            .union(&explicitly_non_allowlisted)
            .copied()
            .collect();

        assert_eq!(
            classified, registered,
            "every Base function must be classified as allowlisted or explicitly non-allowlisted"
        );
        assert!(
            allowlisted.is_disjoint(&explicitly_non_allowlisted),
            "a Base function cannot be both allowlisted and explicitly non-allowlisted"
        );

        for name in explicitly_non_allowlisted {
            assert!(
                !is_base_fastcall_allowlisted(name),
                "non-allowlisted name '{}' unexpectedly returns true",
                name
            );
        }
    }
}
