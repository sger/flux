//! Shared cache-command helpers and grouped cache display settings.

use std::path::{Path, PathBuf};

use crate::{
    driver::{
        backend::Backend, backend_policy, flags::DriverFlags,
        frontend::load_module_graph_for_cache_info,
    },
    shared::cache_paths::{self, CacheLayout},
    syntax::module_graph::ModuleGraph,
};

/// Grouped cache visibility settings for `cache-info` style commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheDisplaySelection {
    pub(crate) show_vm: bool,
    pub(crate) show_native: bool,
}

/// Shared cache command input settings derived from CLI flags.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CacheCommandInput<'a> {
    pub(crate) path: &'a str,
    pub(crate) extra_roots: &'a [PathBuf],
    pub(crate) cache_dir: Option<&'a Path>,
}

impl CacheDisplaySelection {
    /// Builds cache visibility settings from driver flags.
    pub(crate) fn from_flags(flags: &DriverFlags) -> Self {
        let show_vm = flags.backend.selected == Backend::Vm;
        let show_native = backend_policy::should_show_native_cache(flags)
            && backend_policy::native_cache_available();
        Self {
            show_vm,
            show_native,
        }
    }
}

/// Prints the native cache-unavailable message when the current build cannot inspect it.
pub(crate) fn print_native_cache_unavailable_if_needed(flags: &DriverFlags) {
    if backend_policy::should_show_native_cache(flags) && !backend_policy::native_cache_available()
    {
        println!("{}", backend_policy::native_cache_unavailable_message());
    }
}

/// Resolves the cache layout and canonical entry path used by cache inspection commands.
pub(crate) fn resolve_cache_layout_for_input(input: CacheCommandInput<'_>) -> (&Path, CacheLayout) {
    let entry_path = Path::new(input.path);
    let layout = cache_paths::resolve_cache_layout(entry_path, input.cache_dir);
    (entry_path, layout)
}

/// Loads the module graph for cache inspection, returning a printable error on failure.
pub(crate) fn load_cache_graph(input: CacheCommandInput<'_>) -> Result<ModuleGraph, String> {
    load_module_graph_for_cache_info(input.path, input.extra_roots)
}

#[cfg(test)]
mod tests {
    use super::CacheDisplaySelection;
    use crate::driver::{backend::Backend, test_support::base_flags};

    #[test]
    fn cache_display_selection_matches_backend_policy() {
        let mut flags = base_flags();
        let selection = CacheDisplaySelection::from_flags(&flags);
        assert!(selection.show_vm);

        flags.backend.selected = Backend::Native;
        let selection = CacheDisplaySelection::from_flags(&flags);
        assert!(!selection.show_vm);
    }
}
