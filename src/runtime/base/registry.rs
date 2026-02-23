use crate::runtime::base_function::BaseFunction;

use super::BASE_FUNCTIONS;

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

#[cfg(test)]
mod tests {
    use super::BaseModule;

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
}
