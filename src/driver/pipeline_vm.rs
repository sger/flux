use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{
    bytecode::{
        bytecode::Bytecode,
        bytecode_cache::{hash_bytes, hash_cache_key, module_cache::ModuleBytecodeCache},
        compiler::module_interface::{
            build_interface, compute_semantic_config_hash, interface_path, load_cached_interface,
            load_valid_interface, module_interface_changed, save_interface,
        },
        module_linker::{LinkedVmProgram, VmAssemblyContext},
        symbol_table::SymbolTable,
    },
    cache_paths::CacheLayout,
    diagnostics::{Diagnostic, Severity, module_skipped_note, render_display_path},
    driver::{
        frontend::extract_module_name_and_sym,
        module_compile::{ModuleBuildState, build_module_compiler, replay_module_diagnostics},
        shared::{module_display_name, progress_line},
    },
    syntax::{
        interner::Interner,
        module_graph::{ModuleGraph, ModuleKind, ModuleNode},
    },
    types::module_interface::{DependencyFingerprint, ModuleInterface},
};

/// Result of compiling and linking a module graph for VM execution.
///
/// The VM pipeline compiles each module into relocatable module bytecode,
/// links the resulting artifacts into one executable bytecode program, and
/// reports how many modules came from cache versus fresh compilation.
pub(crate) struct ParallelVmBuild {
    /// Linked bytecode program ready for VM execution.
    pub(crate) bytecode: Bytecode,
    /// Final symbol table paired with the linked bytecode.
    pub(crate) symbol_table: SymbolTable,
    /// Number of modules whose cached artifacts were reused.
    pub(crate) cached_count: usize,
    /// Number of modules recompiled during this build.
    pub(crate) compiled_count: usize,
}

/// Immutable inputs for the parallel VM module-compilation pipeline.
///
/// This bundles the graph, cache configuration, compiler toggles, and
/// shared interner used while compiling VM artifacts for a whole module graph.
pub(crate) struct VmCompileRequest<'a> {
    /// Fully built module graph in topological order.
    pub(crate) graph: &'a ModuleGraph,
    /// Canonical entry path when one is known. Used to force entry-specific
    /// behaviour such as allowing cached reuse without a current interface hit.
    pub(crate) entry_canonical: Option<&'a PathBuf>,
    /// Shared graph interner used to seed per-module compilers.
    pub(crate) graph_interner: &'a Interner,
    /// Cache layout containing VM and interface cache directories.
    pub(crate) cache_layout: &'a CacheLayout,
    /// Disables module artifact and interface cache reads/writes.
    pub(crate) no_cache: bool,
    /// Enables strict-mode semantics for non-Flow modules.
    pub(crate) strict_mode: bool,
    /// Enables strict type checks for non-Flow modules.
    pub(crate) strict_types: bool,
    /// Enables optimization during module compilation.
    pub(crate) enable_optimize: bool,
    /// Enables analysis passes during module compilation.
    pub(crate) enable_analyze: bool,
    /// Enables verbose cache-miss reporting.
    pub(crate) verbose: bool,
}

/// Per-module result produced by the parallel compilation stage before final
/// artifact replay and linking.
#[derive(Debug)]
struct ParallelModuleResult {
    path: PathBuf,
    needs_serial_warning_replay: bool,
    compile_failed: bool,
    old_interface_fingerprint: Option<String>,
    new_interface_fingerprint: Option<String>,
    interface_changed: bool,
    skipped: bool,
    interface_hit: Option<ModuleInterface>,
    cache_key: [u8; 32],
    miss_reason: Option<String>,
}

/// Returns whether a VM module may be satisfied entirely from cache.
///
/// Cache reuse requires:
/// - cache access to be enabled
/// - no forced rebuild due to upstream interface changes
/// - either a valid current interface or entry-module status
/// - an existing cached VM artifact
fn can_use_cache_vm_module(
    no_cache: bool,
    force_rebuild: bool,
    has_current_interface: bool,
    is_entry: bool,
    has_cached_artifact: bool,
) -> bool {
    !no_cache && !force_rebuild && (has_current_interface || is_entry) && has_cached_artifact
}

/// Chooses the user-facing reason for a VM cache miss.
///
/// Priority order:
/// 1. forced rebuild due to a changed dependency interface
/// 2. current interface validation failure
/// 3. cached artifact load/validation failure
fn vm_miss_reason(
    force_rebuild: bool,
    interface_miss_reason: Option<String>,
    cache_failure_reason: Option<String>,
) -> Option<String> {
    if force_rebuild {
        Some("dependency interface changed".to_string())
    } else {
        interface_miss_reason.or(cache_failure_reason)
    }
}

/// Compiles one module to a relocatable VM artifact, optionally reusing cache.
///
/// This function:
/// - computes cache and interface fingerprints for the module
/// - decides whether an existing artifact can be reused
/// - recompiles the module when cache reuse is not allowed
/// - rebuilds the public interface when possible
/// - stores updated artifact/interface data back into cache when enabled
///
/// It does not link artifacts into a final executable program; that is handled
/// later by [`compile_vm_modules_parallel`].
fn compile_parallel_module(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    request: &VmCompileRequest<'_>,
    force_rebuild: bool,
    is_entry: bool,
) -> ParallelModuleResult {
    let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
    let source_hash = hash_bytes(module_source.as_bytes());
    let semantic_config_hash = compute_semantic_config_hash(
        node.kind != ModuleKind::FlowStdlib && request.strict_mode,
        request.enable_optimize,
    );
    let strict_hash = if node.kind == ModuleKind::FlowStdlib {
        hash_bytes(b"strict=0")
    } else {
        hash_bytes(if request.strict_mode {
            b"strict=1"
        } else {
            b"strict=0"
        })
    };
    let cache_key = hash_cache_key(&source_hash, &strict_hash);
    let module_cache = ModuleBytecodeCache::new(request.cache_layout.vm_dir());
    let old_interface = if !request.no_cache {
        load_cached_interface(request.cache_layout.root(), &node.path).ok()
    } else {
        None
    };
    let (current_interface, interface_miss_reason) = if !request.no_cache {
        match load_valid_interface(
            request.cache_layout.root(),
            &node.path,
            &module_source,
            &semantic_config_hash,
        ) {
            Ok(interface) => (Some(interface), None),
            Err(err) => (None, Some(err.message())),
        }
    } else {
        (None, None)
    };

    let has_cached_artifact = module_cache
        .load(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            request.cache_layout.root(),
        )
        .is_some();

    if can_use_cache_vm_module(
        request.no_cache,
        force_rebuild,
        current_interface.is_some(),
        is_entry,
        has_cached_artifact,
    ) {
        return ParallelModuleResult {
            path: node.path.clone(),
            needs_serial_warning_replay: false,
            compile_failed: false,
            old_interface_fingerprint: current_interface
                .as_ref()
                .map(|i| i.interface_fingerprint.clone()),
            new_interface_fingerprint: current_interface
                .as_ref()
                .map(|i| i.interface_fingerprint.clone()),
            interface_changed: false,
            skipped: true,
            interface_hit: current_interface.clone(),
            cache_key,
            miss_reason: None,
        };
    }

    let miss_reason = vm_miss_reason(
        force_rebuild,
        interface_miss_reason,
        module_cache.load_failure_reason(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            request.cache_layout.root(),
        ),
    );

    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        request.graph_interner,
        request.strict_mode,
        request.strict_types,
        is_entry,
    );
    let compile_result = compiler.compile_with_opts(
        &node.program,
        request.enable_optimize,
        request.enable_analyze,
    );
    let warning_count = compiler.take_warnings().len();

    if compile_result.is_err() {
        return ParallelModuleResult {
            path: node.path.clone(),
            needs_serial_warning_replay: warning_count > 0,
            compile_failed: true,
            old_interface_fingerprint: old_interface
                .as_ref()
                .map(|interface| interface.interface_fingerprint.clone()),
            new_interface_fingerprint: None,
            interface_changed: true,
            skipped: false,
            interface_hit: None,
            cache_key,
            miss_reason,
        };
    }

    let dependency_fingerprints = node
        .imports
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
        .collect::<Vec<_>>();

    let interface = extract_module_name_and_sym(&node.program, &compiler.interner).and_then(
        |(module_name, module_sym)| {
            compiler
                .lower_aether_report_program(&node.program, request.enable_optimize)
                .ok()
                .map(|core| {
                    build_interface(
                        &module_name,
                        module_sym,
                        &source_hash,
                        &semantic_config_hash,
                        core.as_core(),
                        compiler.cached_member_schemes(),
                        &compiler.module_function_visibility,
                        Some(compiler.class_env()),
                        dependency_fingerprints,
                        &compiler.interner,
                    )
                })
        },
    );

    let artifact = compiler.build_relocatable_module_bytecode();
    let module_deps = node
        .imports
        .iter()
        .filter_map(|dep| {
            loaded_interfaces.get(&dep.target_path).map(|interface| {
                (
                    dep.target_path.to_string_lossy().to_string(),
                    interface.interface_fingerprint.clone(),
                )
            })
        })
        .collect::<Vec<_>>();

    if !request.no_cache {
        let _ = module_cache.store(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            &artifact,
            &module_deps,
        );
        if let Some(interface) = interface.as_ref() {
            let iface_path = interface_path(request.cache_layout.root(), &node.path);
            let _ = save_interface(&iface_path, interface);
        }
    }

    let new_interface_fingerprint = interface
        .as_ref()
        .map(|iface| iface.interface_fingerprint.clone());
    let interface_changed = match (&old_interface, &interface) {
        (Some(old), Some(new)) => module_interface_changed(old, new),
        (None, None) => false,
        _ => true,
    };

    ParallelModuleResult {
        path: node.path.clone(),
        needs_serial_warning_replay: warning_count > 0,
        compile_failed: false,
        old_interface_fingerprint: old_interface
            .as_ref()
            .map(|interface| interface.interface_fingerprint.clone()),
        new_interface_fingerprint,
        interface_changed,
        skipped: false,
        interface_hit: interface,
        cache_key,
        miss_reason,
    }
}

/// Compiles a module graph into linked VM bytecode using parallel per-level
/// compilation and cache-aware artifact reuse.
///
/// High-level flow:
/// - traverse graph levels in topological order
/// - skip modules whose dependencies already failed
/// - compile modules in each ready batch in parallel
/// - replay warnings/errors serially when needed for stable diagnostics
/// - load freshly produced or cached module artifacts
/// - link all module artifacts into one `Bytecode` program
///
/// The function mutates `all_diagnostics` with replayed module diagnostics and
/// returns the linked program plus cache hit/miss counts.
pub(crate) fn compile_vm_modules_parallel(
    request: VmCompileRequest<'_>,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<ParallelVmBuild, String> {
    let mut loaded_interfaces: HashMap<PathBuf, ModuleInterface> = HashMap::new();
    let mut module_states: HashMap<PathBuf, ModuleBuildState> = HashMap::new();
    let mut failed: HashSet<PathBuf> = HashSet::new();
    let mut nodes_by_path: HashMap<PathBuf, ModuleNode> = HashMap::new();

    for node in request.graph.topo_order() {
        nodes_by_path.insert(node.path.clone(), node.clone());
    }

    let mut linker = VmAssemblyContext::new(request.graph_interner.clone());
    let module_cache = ModuleBytecodeCache::new(request.cache_layout.vm_dir());
    let total_modules = request.graph.topo_order().len();
    let mut completed_modules = 0usize;
    let mut cached_count = 0usize;

    for level in request.graph.topo_levels().into_iter() {
        let mut ready = Vec::new();
        for node in level {
            if node
                .imports
                .iter()
                .any(|dep| failed.contains(&dep.target_path))
            {
                failed.insert(node.path.clone());
                let display = render_display_path(&node.path.to_string_lossy()).into_owned();
                if let Some(dep) = node
                    .imports
                    .iter()
                    .find(|edge| failed.contains(&edge.target_path))
                {
                    all_diagnostics.push(module_skipped_note(
                        display.clone(),
                        display,
                        dep.name.clone(),
                    ));
                }
                continue;
            }
            ready.push(node.clone());
        }
        if ready.is_empty() {
            continue;
        }

        let (flow_nodes, user_nodes): (Vec<_>, Vec<_>) = ready
            .iter()
            .partition(|node| node.kind == ModuleKind::FlowStdlib);
        let batches: Vec<Vec<&ModuleNode>> = if flow_nodes.is_empty() {
            vec![user_nodes]
        } else if user_nodes.is_empty() {
            vec![flow_nodes]
        } else {
            vec![flow_nodes, user_nodes]
        };

        for batch in batches {
            let dependency_changed = |node: &ModuleNode| {
                node.imports.iter().any(|dep| {
                    module_states
                        .get(&dep.target_path)
                        .is_some_and(|state| state.interface_changed)
                })
            };

            let parallel_results: Vec<ParallelModuleResult> = batch
                .par_iter()
                .filter(|node| !dependency_changed(node))
                .map(|node| {
                    let is_entry = request
                        .entry_canonical
                        .is_some_and(|entry| entry == &node.path);
                    compile_parallel_module(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        &request,
                        false,
                        is_entry,
                    )
                })
                .collect();

            let mut parallel_results = parallel_results;
            parallel_results.sort_by(|left, right| left.path.cmp(&right.path));

            let skipped_paths: HashSet<_> = parallel_results
                .iter()
                .map(|result| result.path.clone())
                .collect();
            for node in &batch {
                if dependency_changed(node) && !skipped_paths.contains(&node.path) {
                    let is_entry = request
                        .entry_canonical
                        .is_some_and(|entry| entry == &node.path);
                    parallel_results.push(compile_parallel_module(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        &request,
                        true,
                        is_entry,
                    ));
                }
            }
            parallel_results.sort_by(|left, right| left.path.cmp(&right.path));

            for result in parallel_results {
                if result.compile_failed {
                    failed.insert(result.path.clone());
                    if let Some(node) = nodes_by_path.get(&result.path) {
                        all_diagnostics.extend(replay_module_diagnostics(
                            node,
                            &nodes_by_path,
                            &loaded_interfaces,
                            request.graph_interner,
                            request.strict_mode,
                            request.strict_types,
                            request.enable_optimize,
                            request.enable_analyze,
                        ));
                    }
                    continue;
                }
                if result.needs_serial_warning_replay
                    && let Some(node) = nodes_by_path.get(&result.path)
                {
                    let replayed = replay_module_diagnostics(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        request.graph_interner,
                        request.strict_mode,
                        request.strict_types,
                        request.enable_optimize,
                        request.enable_analyze,
                    );
                    all_diagnostics.extend(
                        replayed
                            .into_iter()
                            .filter(|diag| diag.severity() != Severity::Error),
                    );
                }

                if let Some(interface) = result.interface_hit.clone() {
                    let name = interface.module_name.clone();
                    if result.skipped {
                        cached_count += 1;
                    }
                    completed_modules += 1;
                    if result.skipped {
                        eprintln!(
                            "{}",
                            progress_line(completed_modules, total_modules, "Cached", &name)
                        );
                    } else {
                        if request.verbose
                            && let Some(reason) = &result.miss_reason
                        {
                            eprintln!("  cache miss ({name}): {reason}");
                        }
                        eprintln!(
                            "{}",
                            progress_line(completed_modules, total_modules, "Compiling", &name)
                        );
                    }
                    loaded_interfaces.insert(result.path.clone(), interface);
                } else {
                    if result.skipped {
                        cached_count += 1;
                    }
                    completed_modules += 1;
                    let name = module_display_name(&result.path);
                    if result.skipped {
                        eprintln!(
                            "{}",
                            progress_line(completed_modules, total_modules, "Cached", &name)
                        );
                    } else {
                        if request.verbose
                            && let Some(reason) = &result.miss_reason
                        {
                            eprintln!("  cache miss ({name}): {reason}");
                        }
                        eprintln!(
                            "{}",
                            progress_line(completed_modules, total_modules, "Compiling", &name)
                        );
                    }
                }

                module_states.insert(
                    result.path.clone(),
                    ModuleBuildState {
                        old_interface_fingerprint: result.old_interface_fingerprint,
                        new_interface_fingerprint: result.new_interface_fingerprint,
                        interface_changed: result.interface_changed,
                        rebuild_required: !result.skipped,
                        skipped: result.skipped,
                    },
                );

                let artifact = module_cache
                    .load(
                        &result.path,
                        &result.cache_key,
                        env!("CARGO_PKG_VERSION"),
                        request.cache_layout.root(),
                    )
                    .ok_or_else(|| {
                        let reason = module_cache
                            .load_failure_reason(
                                &result.path,
                                &result.cache_key,
                                env!("CARGO_PKG_VERSION"),
                                request.cache_layout.root(),
                            )
                            .unwrap_or_else(|| "unknown".to_string());
                        format!(
                            "could not load module artifact for {} ({reason})",
                            result.path.display()
                        )
                    })?;
                linker.assemble_module(&artifact)?;
            }
        }
    }

    let compiled_count = total_modules - cached_count;
    let LinkedVmProgram {
        bytecode,
        symbol_table,
    } = linker.finish();
    Ok(ParallelVmBuild {
        bytecode,
        symbol_table,
        cached_count,
        compiled_count,
    })
}

#[cfg(test)]
mod tests {
    use crate::driver::pipeline_vm::{can_use_cache_vm_module, vm_miss_reason};

    #[test]
    fn cached_vm_module_requires_cache_enabled_and_artifact() {
        assert!(can_use_cache_vm_module(false, false, true, false, true));
        assert!(can_use_cache_vm_module(false, false, false, true, true));
        assert!(!can_use_cache_vm_module(true, false, true, false, true));
        assert!(!can_use_cache_vm_module(false, true, true, false, true));
        assert!(!can_use_cache_vm_module(false, false, false, false, true));
        assert!(!can_use_cache_vm_module(false, false, true, false, false));
    }

    #[test]
    fn vm_miss_reason_prefers_dependency_rebuild_then_interface_then_cache() {
        assert_eq!(
            vm_miss_reason(true, Some("iface".into()), Some("cache".into())),
            Some("dependency interface changed".to_string())
        );
        assert_eq!(
            vm_miss_reason(false, Some("iface".into()), Some("cache".into())),
            Some("iface".to_string())
        );
        assert_eq!(
            vm_miss_reason(false, None, Some("cache".into())),
            Some("cache".to_string())
        );
        assert_eq!(vm_miss_reason(false, None, None), None);
    }

    #[test]
    fn cached_vm_entry_module_may_reuse_artifact_without_current_interface() {
        assert!(can_use_cache_vm_module(false, false, false, true, true));
        assert!(!can_use_cache_vm_module(false, false, false, true, false));
    }

    #[test]
    fn cached_vm_non_entry_module_still_requires_current_interface() {
        assert!(!can_use_cache_vm_module(false, false, false, false, true));
        assert!(can_use_cache_vm_module(false, false, true, false, true));
    }

    #[test]
    fn vm_miss_reason_uses_dependency_message_even_without_other_reasons() {
        assert_eq!(
            vm_miss_reason(true, None, None),
            Some("dependency interface changed".to_string())
        );
    }

    #[test]
    fn vm_miss_reason_falls_back_to_none_when_no_reason_exists() {
        assert_eq!(vm_miss_reason(false, None, None), None);
    }
}
