//! Shared helpers for the VM and native parallel pipeline implementations.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    bytecode::compiler::module_interface::{interface_path, module_interface_changed, save_interface},
    diagnostics::{Diagnostic, Severity},
    syntax::module_graph::{ImportEdge, ModuleKind},
    types::module_interface::{DependencyFingerprint, ModuleInterface},
};

#[cfg(feature = "core_to_llvm")]
use crate::{
    bytecode::compiler::module_interface::load_cached_interface,
    syntax::module_graph::ModuleGraph,
};

use crate::driver::{
    module_compile::replay_module_diagnostics,
    support::shared::{module_display_name, progress_line},
};

/// Splits a level of modules into Flow stdlib and user batches, preserving the existing
/// "stdlib first" behavior used by both maintained backends.
pub(crate) fn partition_module_batches<T: Clone>(
    items: &[T],
    kind_of: impl Fn(&T) -> ModuleKind,
) -> Vec<Vec<T>> {
    let (flow_nodes, user_nodes): (Vec<_>, Vec<_>) = items
        .iter()
        .partition(|item| kind_of(item) == ModuleKind::FlowStdlib);
    let flow_nodes: Vec<_> = flow_nodes.into_iter().cloned().collect();
    let user_nodes: Vec<_> = user_nodes.into_iter().cloned().collect();
    if flow_nodes.is_empty() {
        vec![user_nodes]
    } else if user_nodes.is_empty() {
        vec![flow_nodes]
    } else {
        vec![flow_nodes, user_nodes]
    }
}

/// Collects the paths for items whose dependencies require a rebuild.
pub(crate) fn dependency_changed_paths<T>(
    items: &[T],
    path_of: impl Fn(&T) -> &PathBuf,
    is_changed: impl Fn(&T) -> bool,
) -> HashSet<PathBuf> {
    items
        .iter()
        .filter(|item| is_changed(item))
        .map(|item| path_of(item).clone())
        .collect()
}

/// Sorts result collections by module path to keep output deterministic after parallel work.
pub(crate) fn sort_by_path<T>(items: &mut [T], path_of: impl Fn(&T) -> &PathBuf) {
    items.sort_by(|left, right| path_of(left).cmp(path_of(right)));
}

/// Chooses the user-facing module name for progress output.
pub(crate) fn progress_name(interface_name: Option<&str>, path: &Path) -> String {
    interface_name
        .map(str::to_string)
        .unwrap_or_else(|| module_display_name(path))
}

/// Emits cache miss details and a progress line for the current module.
pub(crate) fn emit_progress(
    completed: usize,
    total: usize,
    action: &str,
    name: &str,
    verbose: bool,
    miss_reason: Option<&str>,
) {
    if action != "Cached"
        && verbose
        && let Some(reason) = miss_reason
    {
        eprintln!("  cache miss ({name}): {reason}");
    }
    eprintln!("{}", progress_line(completed, total, action, name));
}

/// Removes error diagnostics when only warnings/notes should be replayed.
pub(crate) fn filter_non_error_diagnostics(diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .filter(|diag| diag.severity() != Severity::Error)
        .collect()
}

/// Replays module diagnostics using the shared compiler replay path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_module_diagnostics_for(
    node: &crate::syntax::module_graph::ModuleNode,
    nodes_by_path: &HashMap<PathBuf, crate::syntax::module_graph::ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    base_interner: &crate::syntax::interner::Interner,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
) -> Vec<Diagnostic> {
    replay_module_diagnostics(
        node,
        nodes_by_path,
        loaded_interfaces,
        base_interner,
        strict_mode,
        strict_types,
        enable_optimize,
        enable_analyze,
    )
}

/// Builds dependency fingerprint metadata for the currently loaded interfaces.
pub(crate) fn collect_dependency_fingerprints(
    imports: &[ImportEdge],
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
) -> Vec<DependencyFingerprint> {
    imports
        .iter()
        .filter_map(|dep| {
            loaded_interfaces
                .get(&dep.target_path)
                .map(|interface| DependencyFingerprint {
                    module_name: interface.module_name.clone(),
                    source_path: dep.target_path.to_string_lossy().to_string(),
                    interface_fingerprint: interface.interface_fingerprint.clone(),
                })
        })
        .collect()
}

/// Persists a module interface when caching is enabled and the interface is available.
pub(crate) fn save_interface_if_enabled(
    no_cache: bool,
    cache_root: &Path,
    module_path: &Path,
    interface: Option<&ModuleInterface>,
) {
    if no_cache {
        return;
    }
    if let Some(interface) = interface {
        let iface_path = interface_path(cache_root, module_path);
        let _ = save_interface(&iface_path, interface);
    }
}

/// Compares two module interfaces using the shared change-detection policy.
pub(crate) fn interfaces_changed(
    old: Option<&ModuleInterface>,
    new: Option<&ModuleInterface>,
) -> bool {
    match (old, new) {
        (Some(old), Some(new)) => module_interface_changed(old, new),
        (None, None) => false,
        _ => true,
    }
}

#[cfg(feature = "core_to_llvm")]
/// Loads cached interfaces for every module in a graph.
pub(crate) fn load_cached_interfaces_for_graph(
    graph: &ModuleGraph,
    cache_root: &Path,
) -> HashMap<PathBuf, ModuleInterface> {
    let mut loaded_interfaces = HashMap::new();
    for node in graph.topo_order() {
        if let Ok(interface) = load_cached_interface(cache_root, &node.path) {
            loaded_interfaces.insert(node.path.clone(), interface);
        }
    }
    loaded_interfaces
}

#[cfg(test)]
mod tests {
    use super::{
        dependency_changed_paths, filter_non_error_diagnostics, partition_module_batches,
        progress_name, sort_by_path,
    };
    use crate::{diagnostics::Diagnostic, syntax::module_graph::ModuleKind};
    use std::path::PathBuf;

    #[derive(Clone)]
    struct TestNode {
        path: PathBuf,
        kind: ModuleKind,
        changed: bool,
    }

    #[test]
    fn partitions_stdlib_before_user_batches() {
        let nodes = vec![
            TestNode {
                path: PathBuf::from("user_a.flx"),
                kind: ModuleKind::User,
                changed: false,
            },
            TestNode {
                path: PathBuf::from("flow_a.flx"),
                kind: ModuleKind::FlowStdlib,
                changed: false,
            },
            TestNode {
                path: PathBuf::from("user_b.flx"),
                kind: ModuleKind::User,
                changed: false,
            },
        ];

        let batches = partition_module_batches(&nodes, |node| node.kind);

        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].path, PathBuf::from("flow_a.flx"));
        assert_eq!(batches[1].len(), 2);
    }

    #[test]
    fn dependency_changed_paths_collects_only_changed_items() {
        let nodes = vec![
            TestNode {
                path: PathBuf::from("a.flx"),
                kind: ModuleKind::User,
                changed: true,
            },
            TestNode {
                path: PathBuf::from("b.flx"),
                kind: ModuleKind::User,
                changed: false,
            },
        ];

        let changed = dependency_changed_paths(&nodes, |node| &node.path, |node| node.changed);

        assert!(changed.contains(&PathBuf::from("a.flx")));
        assert!(!changed.contains(&PathBuf::from("b.flx")));
    }

    #[test]
    fn sort_by_path_produces_deterministic_order() {
        let mut nodes = vec![
            TestNode {
                path: PathBuf::from("z.flx"),
                kind: ModuleKind::User,
                changed: false,
            },
            TestNode {
                path: PathBuf::from("a.flx"),
                kind: ModuleKind::User,
                changed: false,
            },
        ];

        sort_by_path(&mut nodes, |node| &node.path);

        assert_eq!(nodes[0].path, PathBuf::from("a.flx"));
        assert_eq!(nodes[1].path, PathBuf::from("z.flx"));
    }

    #[test]
    fn progress_name_prefers_interface_name_then_path_fallback() {
        assert_eq!(
            progress_name(Some("Flow.List"), PathBuf::from("list.flx").as_path()),
            "Flow.List"
        );
        assert_eq!(
            progress_name(None, PathBuf::from("list.flx").as_path()),
            "list"
        );
    }

    #[test]
    fn filter_non_error_diagnostics_keeps_only_non_errors() {
        let diags = vec![
            Diagnostic::warning("warn"),
            Diagnostic::warning("warn2"),
            crate::ice!("boom"),
        ];

        let filtered = filter_non_error_diagnostics(diags);

        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|diag| diag.severity() != crate::diagnostics::Severity::Error)
        );
    }
}
