//! Internal implementation of the parallel VM module compilation pipeline.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use crate::{
    bytecode::{
        bytecode_cache::{hash_bytes, hash_cache_key, module_cache::ModuleBytecodeCache},
        compiler::module_interface::{
            build_interface, compute_semantic_config_hash, load_cached_interface,
            load_valid_interface,
        },
        module_linker::{LinkedVmProgram, VmAssemblyContext},
    },
    diagnostics::{Diagnostic, quality::module_skipped_note, render_display_path},
    syntax::module_graph::{ModuleKind, ModuleNode},
    types::module_interface::ModuleInterface,
};
use rayon::prelude::*;

use crate::driver::{
    module_compile::{ModuleBuildState, build_module_compiler},
    pipeline::parallel_shared::{
        collect_dependency_fingerprints, dependency_changed_paths, emit_progress,
        filter_non_error_diagnostics, interfaces_changed, partition_module_batches, progress_name,
        replay_module_diagnostics_for, save_interface_if_enabled, sort_by_path,
    },
    pipeline::vm::{ParallelVmBuild, VmCompileRequest},
};

#[derive(Debug)]
/// Result of compiling or loading a single module during a parallel VM batch.
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

fn can_use_cached_vm_module(
    no_cache: bool,
    force_rebuild: bool,
    has_current_interface: bool,
    is_entry: bool,
    has_cached_artifact: bool,
) -> bool {
    !no_cache && !force_rebuild && (has_current_interface || is_entry) && has_cached_artifact
}

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
        node.kind != ModuleKind::FlowStdlib && request.compile.strict_mode,
        request.compile.enable_optimize,
    );
    let strict_hash = if node.kind == ModuleKind::FlowStdlib {
        hash_bytes(b"strict=0")
    } else {
        hash_bytes(if request.compile.strict_mode {
            b"strict=1"
        } else {
            b"strict=0"
        })
    };
    let cache_key = hash_cache_key(&source_hash, &strict_hash);
    let module_cache = ModuleBytecodeCache::new(request.cache.cache_layout.vm_dir());
    let old_interface = if !request.cache.no_cache {
        load_cached_interface(request.cache.cache_layout.root(), &node.path).ok()
    } else {
        None
    };
    let (current_interface, interface_miss_reason) = if !request.cache.no_cache {
        match load_valid_interface(
            request.cache.cache_layout.root(),
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
            request.cache.cache_layout.root(),
        )
        .is_some();

    if can_use_cached_vm_module(
        request.cache.no_cache,
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
            request.cache.cache_layout.root(),
        ),
    );

    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        request.graph_interner,
        request.compile.strict_mode,
        request.compile.strict_types,
        is_entry,
    );
    let compile_result = compiler.compile_with_opts(
        &node.program,
        request.compile.enable_optimize,
        request.compile.enable_analyze,
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

    let dependency_fingerprints = collect_dependency_fingerprints(&node.imports, loaded_interfaces);

    let interface =
        crate::driver::frontend::extract_module_name_and_sym(&node.program, &compiler.interner)
            .and_then(|(module_name, module_sym)| {
                compiler
                    .lower_aether_report_program(&node.program, request.compile.enable_optimize)
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
            });

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
    if !request.cache.no_cache {
        let _ = module_cache.store(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            &artifact,
            &module_deps,
        );
        save_interface_if_enabled(
            request.cache.no_cache,
            request.cache.cache_layout.root(),
            &node.path,
            interface.as_ref(),
        );
    }

    let new_interface_fingerprint = interface
        .as_ref()
        .map(|iface| iface.interface_fingerprint.clone());
    let interface_changed = interfaces_changed(old_interface.as_ref(), interface.as_ref());

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

struct VmParallelBuildState {
    loaded_interfaces: HashMap<PathBuf, ModuleInterface>,
    module_states: HashMap<PathBuf, ModuleBuildState>,
    failed: HashSet<PathBuf>,
    linker: VmAssemblyContext,
    cached_count: usize,
    completed_modules: usize,
    total_modules: usize,
}

impl VmParallelBuildState {
    fn new(request: &VmCompileRequest<'_>) -> Self {
        Self {
            loaded_interfaces: HashMap::new(),
            module_states: HashMap::new(),
            failed: HashSet::new(),
            linker: VmAssemblyContext::new(request.graph_interner.clone()),
            cached_count: 0,
            completed_modules: 0,
            total_modules: request.graph.topo_order().len(),
        }
    }

    fn dependency_changed(&self, node: &ModuleNode) -> bool {
        node.imports.iter().any(|dep| {
            self.module_states
                .get(&dep.target_path)
                .is_some_and(|state| state.interface_changed)
        })
    }

    fn record_failed_dependency(
        &mut self,
        node: &ModuleNode,
        all_diagnostics: &mut Vec<Diagnostic>,
    ) {
        self.failed.insert(node.path.clone());
        let display = render_display_path(&node.path.to_string_lossy()).into_owned();
        if let Some(dep) = node
            .imports
            .iter()
            .find(|edge| self.failed.contains(&edge.target_path))
        {
            all_diagnostics.push(module_skipped_note(
                display.clone(),
                display,
                dep.name.clone(),
            ));
        }
    }

    fn record_progress(
        &mut self,
        path: &PathBuf,
        interface: Option<&ModuleInterface>,
        skipped: bool,
        verbose: bool,
        miss_reason: Option<&String>,
    ) {
        if skipped {
            self.cached_count += 1;
        }
        self.completed_modules += 1;
        let name = progress_name(interface.map(|iface| iface.module_name.as_str()), path);
        let action = if skipped { "Cached" } else { "Compiling" };
        emit_progress(
            self.completed_modules,
            self.total_modules,
            action,
            &name,
            verbose,
            miss_reason.map(String::as_str),
        );
    }
}

fn compile_batch(
    batch: Vec<ModuleNode>,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    dependency_changed_paths: &HashSet<PathBuf>,
    request: &VmCompileRequest<'_>,
) -> Vec<ParallelModuleResult> {
    let mut parallel_results: Vec<ParallelModuleResult> = batch
        .par_iter()
        .filter(|node| !dependency_changed_paths.contains(&node.path))
        .map(|node| {
            let is_entry = request
                .entry_canonical
                .is_some_and(|entry| entry == &node.path);
            compile_parallel_module(
                node,
                nodes_by_path,
                loaded_interfaces,
                request,
                false,
                is_entry,
            )
        })
        .collect();

    sort_by_path(&mut parallel_results, |result| &result.path);

    let skipped_paths: HashSet<_> = parallel_results
        .iter()
        .map(|result| result.path.clone())
        .collect();
    for node in &batch {
        if dependency_changed_paths.contains(&node.path) && !skipped_paths.contains(&node.path) {
            let is_entry = request
                .entry_canonical
                .is_some_and(|entry| entry == &node.path);
            parallel_results.push(compile_parallel_module(
                node,
                nodes_by_path,
                loaded_interfaces,
                request,
                true,
                is_entry,
            ));
        }
    }
    sort_by_path(&mut parallel_results, |result| &result.path);
    parallel_results
}

fn replay_warnings_if_needed(
    result: &ParallelModuleResult,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    build_state: &VmParallelBuildState,
    request: &VmCompileRequest<'_>,
    all_diagnostics: &mut Vec<Diagnostic>,
) {
    if result.needs_serial_warning_replay
        && let Some(node) = nodes_by_path.get(&result.path)
    {
        let replayed = replay_module_diagnostics_for(
            node,
            nodes_by_path,
            &build_state.loaded_interfaces,
            request.graph_interner,
            request.compile.strict_mode,
            request.compile.strict_types,
            request.compile.enable_optimize,
            request.compile.enable_analyze,
        );
        all_diagnostics.extend(filter_non_error_diagnostics(replayed));
    }
}

fn replay_errors(
    result: &ParallelModuleResult,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    build_state: &VmParallelBuildState,
    request: &VmCompileRequest<'_>,
    all_diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(node) = nodes_by_path.get(&result.path) {
        all_diagnostics.extend(replay_module_diagnostics_for(
            node,
            nodes_by_path,
            &build_state.loaded_interfaces,
            request.graph_interner,
            request.compile.strict_mode,
            request.compile.strict_types,
            request.compile.enable_optimize,
            request.compile.enable_analyze,
        ));
    }
}

fn load_and_link_artifact(
    result: &ParallelModuleResult,
    request: &VmCompileRequest<'_>,
    module_cache: &ModuleBytecodeCache,
    linker: &mut VmAssemblyContext,
) -> Result<(), String> {
    let artifact = module_cache
        .load(
            &result.path,
            &result.cache_key,
            env!("CARGO_PKG_VERSION"),
            request.cache.cache_layout.root(),
        )
        .ok_or_else(|| {
            let reason = module_cache
                .load_failure_reason(
                    &result.path,
                    &result.cache_key,
                    env!("CARGO_PKG_VERSION"),
                    request.cache.cache_layout.root(),
                )
                .unwrap_or_else(|| "unknown".to_string());
            format!(
                "could not load module artifact for {} ({reason})",
                result.path.display()
            )
        })?;
    linker.assemble_module(&artifact)?;
    Ok(())
}

fn handle_result(
    result: ParallelModuleResult,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    build_state: &mut VmParallelBuildState,
    request: &VmCompileRequest<'_>,
    module_cache: &ModuleBytecodeCache,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), String> {
    if result.compile_failed {
        build_state.failed.insert(result.path.clone());
        replay_errors(
            &result,
            nodes_by_path,
            build_state,
            request,
            all_diagnostics,
        );
        return Ok(());
    }

    replay_warnings_if_needed(
        &result,
        nodes_by_path,
        build_state,
        request,
        all_diagnostics,
    );

    build_state.record_progress(
        &result.path,
        result.interface_hit.as_ref(),
        result.skipped,
        request.runtime.verbose,
        result.miss_reason.as_ref(),
    );

    if let Some(interface) = result.interface_hit.clone() {
        build_state
            .loaded_interfaces
            .insert(result.path.clone(), interface);
    }

    let old_interface_fingerprint = result.old_interface_fingerprint.clone();
    let new_interface_fingerprint = result.new_interface_fingerprint.clone();
    let interface_changed = result.interface_changed;
    let skipped = result.skipped;
    build_state.module_states.insert(
        result.path.clone(),
        ModuleBuildState {
            old_interface_fingerprint,
            new_interface_fingerprint,
            interface_changed,
            rebuild_required: !skipped,
            skipped,
        },
    );

    load_and_link_artifact(&result, request, module_cache, &mut build_state.linker)
}

/// Compiles a module graph into linked VM bytecode while preserving dependency order.
pub(crate) fn compile_vm_modules_parallel(
    request: VmCompileRequest<'_>,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<ParallelVmBuild, String> {
    let nodes_by_path: HashMap<PathBuf, ModuleNode> = request
        .graph
        .topo_order()
        .into_iter()
        .map(|node| (node.path.clone(), node.clone()))
        .collect();

    let module_cache = ModuleBytecodeCache::new(request.cache.cache_layout.vm_dir());
    let mut build_state = VmParallelBuildState::new(&request);

    for level in request.graph.topo_levels() {
        let mut ready = Vec::new();
        for node in level {
            if node
                .imports
                .iter()
                .any(|dep| build_state.failed.contains(&dep.target_path))
            {
                build_state.record_failed_dependency(node, all_diagnostics);
                continue;
            }
            ready.push(node.clone());
        }
        if ready.is_empty() {
            continue;
        }

        let batches = partition_module_batches(&ready, |node| node.kind);

        for batch in batches {
            let dependency_changed_paths = dependency_changed_paths(
                &batch,
                |node| &node.path,
                |node| build_state.dependency_changed(node),
            );
            let results = compile_batch(
                batch,
                &nodes_by_path,
                &build_state.loaded_interfaces,
                &dependency_changed_paths,
                &request,
            );
            for result in results {
                handle_result(
                    result,
                    &nodes_by_path,
                    &mut build_state,
                    &request,
                    &module_cache,
                    all_diagnostics,
                )?;
            }
        }
    }

    let compiled_count = build_state.total_modules - build_state.cached_count;
    let LinkedVmProgram {
        bytecode,
        symbol_table,
    } = build_state.linker.finish();
    Ok(ParallelVmBuild {
        bytecode,
        symbol_table,
        cached_count: build_state.cached_count,
        compiled_count,
    })
}

#[cfg(test)]
mod tests {
    use super::{can_use_cached_vm_module, vm_miss_reason};

    #[test]
    fn cached_vm_module_requires_cache_enabled_and_artifact() {
        assert!(can_use_cached_vm_module(false, false, true, false, true));
        assert!(can_use_cached_vm_module(false, false, false, true, true));
        assert!(!can_use_cached_vm_module(true, false, true, false, true));
        assert!(!can_use_cached_vm_module(false, true, true, false, true));
        assert!(!can_use_cached_vm_module(false, false, false, false, true));
        assert!(!can_use_cached_vm_module(false, false, true, false, false));
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
}
