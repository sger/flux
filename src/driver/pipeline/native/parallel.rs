//! Internal implementation of the parallel native module compilation pipeline.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use crate as flux;
use crate::{
    bytecode::bytecode_cache::{hash_bytes, hash_cache_key},
    cache_paths::{self, CacheLayout},
    diagnostics::{Diagnostic, DiagnosticPhase, Severity},
    syntax::module_graph::{ModuleGraph, ModuleKind, ModuleNode},
};
use rayon::prelude::*;

use super::native_temp_dir;
use crate::driver::{
    frontend::extract_module_name_and_sym,
    module_compile::{
        build_module_compiler, program_has_user_adt_declarations, replay_module_diagnostics,
        tag_module_diagnostics,
    },
    support::shared::{module_display_name, progress_line},
};

#[derive(Debug)]
/// Result of compiling or loading a single module during native parallel lowering.
struct NativeParallelModuleResult {
    path: PathBuf,
    object_path: PathBuf,
    compile_failed: bool,
    error_message: Option<String>,
    interface: Option<flux::types::module_interface::ModuleInterface>,
    skipped: bool,
    interface_changed: bool,
    miss_reason: Option<String>,
}

fn native_cache_hit_is_usable(interface_present: bool, module_kind: ModuleKind) -> bool {
    interface_present || module_kind != ModuleKind::FlowStdlib
}

fn native_miss_reason(
    no_cache: bool,
    force_rebuild: bool,
    validation_error: Option<String>,
) -> Option<String> {
    if no_cache {
        None
    } else if force_rebuild {
        Some("dependency interface changed".to_string())
    } else {
        validation_error
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_parallel_native_module(
    node: &ModuleNode,
    is_entry_module: bool,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    cache_layout: &CacheLayout,
    no_cache: bool,
    force_rebuild: bool,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    base_interner: &flux::syntax::interner::Interner,
    export_user_ctor_name_helper: bool,
) -> NativeParallelModuleResult {
    let is_flow_library = node.kind == ModuleKind::FlowStdlib;
    let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
    let source_hash = hash_bytes(module_source.as_bytes());
    let semantic_config_hash =
        flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
            !is_flow_library && strict_mode,
            enable_optimize,
        );
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);

    let native_miss_reason = if !no_cache && !force_rebuild {
        let native_cache =
            flux::core_to_llvm::module_cache::NativeModuleCache::new(cache_layout.native_dir());
        match native_cache.validate(
            &node.path,
            &cache_key,
            cache_layout.root(),
            export_user_ctor_name_helper,
        ) {
            Ok(object_path) => {
                let interface = flux::bytecode::compiler::module_interface::load_cached_interface(
                    cache_layout.root(),
                    &node.path,
                )
                .ok();
                if native_cache_hit_is_usable(interface.is_some(), node.kind) {
                    return NativeParallelModuleResult {
                        path: node.path.clone(),
                        object_path,
                        compile_failed: false,
                        error_message: None,
                        interface,
                        skipped: true,
                        interface_changed: false,
                        miss_reason: None,
                    };
                }
                Some("interface missing for library module".to_string())
            }
            Err(err) => Some(err.message()),
        }
    } else {
        native_miss_reason(no_cache, force_rebuild, None)
    };

    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        base_interner,
        strict_mode,
        strict_types,
        false,
    );
    compiler.set_file_path(node.path.to_string_lossy().to_string());
    if is_flow_library {
        compiler.set_strict_mode(false);
        compiler.set_strict_types(false);
    }

    let compile_result = compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
    let _ = compiler.take_warnings();
    if let Err(mut diags) = compile_result {
        tag_module_diagnostics(&mut diags, DiagnosticPhase::TypeCheck, &node.path);
        return NativeParallelModuleResult {
            path: node.path.clone(),
            object_path: PathBuf::new(),
            compile_failed: true,
            error_message: None,
            interface: None,
            skipped: false,
            interface_changed: true,
            miss_reason: native_miss_reason,
        };
    }

    let llvm_module = match compiler.lower_to_lir_llvm_module_per_module(
        &node.program,
        enable_optimize,
        export_user_ctor_name_helper,
        is_entry_module,
    ) {
        Ok(module) => module,
        Err(mut diag) => {
            diag.set_file(node.path.to_string_lossy().to_string());
            return NativeParallelModuleResult {
                path: node.path.clone(),
                object_path: PathBuf::new(),
                compile_failed: true,
                error_message: Some(format!(
                    "native lowering failed for {}: {}",
                    node.path.display(),
                    diag.title()
                )),
                interface: None,
                skipped: false,
                interface_changed: true,
                miss_reason: native_miss_reason,
            };
        }
    };

    let mut llvm_module = llvm_module;
    llvm_module.target_triple = Some(flux::core_to_llvm::target::host_triple());
    llvm_module.data_layout = flux::core_to_llvm::target::host_data_layout();
    let ll_text = flux::core_to_llvm::render_module(&llvm_module);

    let dependency_fingerprints: Vec<_> = node
        .imports
        .iter()
        .filter_map(|dep| {
            loaded_interfaces.get(&dep.target_path).map(|interface| {
                flux::types::module_interface::DependencyFingerprint {
                    module_name: interface.module_name.clone(),
                    source_path: dep.target_path.to_string_lossy().to_string(),
                    interface_fingerprint: interface.interface_fingerprint.clone(),
                }
            })
        })
        .collect();

    let native_cache =
        flux::core_to_llvm::module_cache::NativeModuleCache::new(cache_layout.native_dir());
    let object_path = if no_cache {
        let dir = native_temp_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(cache_paths::cache_key_filename(
            &node.path,
            &cache_key,
            if cfg!(windows) { "obj" } else { "o" },
        ))
    } else {
        match native_cache.store(
            &node.path,
            &cache_key,
            dependency_fingerprints.clone(),
            enable_optimize,
            export_user_ctor_name_helper,
        ) {
            Ok(path) => path,
            Err(_) => cache_layout
                .native_dir()
                .join(cache_paths::cache_key_filename(
                    &node.path,
                    &cache_key,
                    if cfg!(windows) { "obj" } else { "o" },
                )),
        }
    };

    if let Err(err) = flux::core_to_llvm::pipeline::compile_ir_to_object(
        &ll_text,
        &object_path,
        if enable_optimize { 2 } else { 0 },
    ) {
        return NativeParallelModuleResult {
            path: node.path.clone(),
            object_path: PathBuf::new(),
            compile_failed: true,
            error_message: Some(format!(
                "native module compilation failed for {}: {err}",
                node.path.display()
            )),
            interface: None,
            skipped: false,
            interface_changed: true,
            miss_reason: native_miss_reason,
        };
    }

    let interface = extract_module_name_and_sym(&node.program, &compiler.interner).and_then(
        |(module_name, module_sym)| {
            compiler
                .lower_aether_report_program(&node.program, enable_optimize)
                .ok()
                .map(|core| {
                    flux::bytecode::compiler::module_interface::build_interface(
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

    let interface_changed = match (&loaded_interfaces.get(&node.path), &interface) {
        (Some(old), Some(new)) => {
            flux::bytecode::compiler::module_interface::module_interface_changed(old, new)
        }
        _ => true,
    };

    NativeParallelModuleResult {
        path: node.path.clone(),
        object_path,
        compile_failed: false,
        error_message: None,
        interface,
        skipped: false,
        interface_changed,
        miss_reason: native_miss_reason,
    }
}

struct NativeParallelBuildState {
    loaded_interfaces: HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    object_paths: Vec<PathBuf>,
    interface_changed_modules: HashSet<PathBuf>,
    any_module_recompiled: bool,
    completed_modules: usize,
    total_modules: usize,
}

impl NativeParallelBuildState {
    fn new(
        graph: &ModuleGraph,
        cache_layout: &CacheLayout,
        no_cache: bool,
    ) -> NativeParallelBuildState {
        let mut loaded_interfaces = HashMap::new();
        if !no_cache {
            for node in graph.topo_order() {
                if let Ok(interface) =
                    flux::bytecode::compiler::module_interface::load_cached_interface(
                        cache_layout.root(),
                        &node.path,
                    )
                {
                    loaded_interfaces.insert(node.path.clone(), interface);
                }
            }
        }

        NativeParallelBuildState {
            loaded_interfaces,
            object_paths: Vec::new(),
            interface_changed_modules: HashSet::new(),
            any_module_recompiled: false,
            completed_modules: 0,
            total_modules: graph.topo_order().len(),
        }
    }

    fn record_progress(&mut self, result: &NativeParallelModuleResult, verbose: bool) {
        self.completed_modules += 1;
        let name = result
            .interface
            .as_ref()
            .map(|i| i.module_name.clone())
            .unwrap_or_else(|| module_display_name(&result.path));
        if result.skipped {
            eprintln!(
                "{}",
                progress_line(self.completed_modules, self.total_modules, "Cached", &name)
            );
        } else {
            self.any_module_recompiled = true;
            if verbose && let Some(reason) = &result.miss_reason {
                eprintln!("  cache miss ({name}): {reason}");
            }
            eprintln!(
                "{}",
                progress_line(self.completed_modules, self.total_modules, "Linking", &name)
            );
        }
    }
}

fn replay_native_diagnostics(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    base_interner: &flux::syntax::interner::Interner,
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

#[allow(clippy::too_many_arguments)]
fn handle_native_result(
    result: NativeParallelModuleResult,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    build_state: &mut NativeParallelBuildState,
    cache_layout: &CacheLayout,
    no_cache: bool,
    verbose: bool,
    base_interner: &flux::syntax::interner::Interner,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), String> {
    if result.compile_failed {
        if let Some(node) = nodes_by_path.get(&result.path) {
            all_diagnostics.extend(replay_native_diagnostics(
                node,
                nodes_by_path,
                &build_state.loaded_interfaces,
                base_interner,
                strict_mode,
                strict_types,
                enable_optimize,
                enable_analyze,
            ));
        }
        return Err(result.error_message.unwrap_or_else(|| {
            format!(
                "native module compilation failed for {}",
                result.path.display()
            )
        }));
    }

    if !result.skipped
        && let Some(node) = nodes_by_path.get(&result.path)
    {
        let replayed = replay_native_diagnostics(
            node,
            nodes_by_path,
            &build_state.loaded_interfaces,
            base_interner,
            strict_mode,
            strict_types,
            enable_optimize,
            enable_analyze,
        );
        all_diagnostics.extend(
            replayed
                .into_iter()
                .filter(|diag| diag.severity() != Severity::Error),
        );
    }

    build_state.record_progress(&result, verbose);
    if result.interface_changed {
        build_state
            .interface_changed_modules
            .insert(result.path.clone());
    }
    if let Some(interface) = result.interface {
        if !result.skipped && !no_cache {
            let interface_path = flux::bytecode::compiler::module_interface::interface_path(
                cache_layout.root(),
                &result.path,
            );
            let _ = flux::bytecode::compiler::module_interface::save_interface(
                &interface_path,
                &interface,
            );
        }
        build_state
            .loaded_interfaces
            .insert(result.path.clone(), interface);
    }
    build_state.object_paths.push(result.object_path);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// Compiles a module graph into native object files using parallel per-level batches.
pub(crate) fn compile_native_modules_parallel(
    graph: &ModuleGraph,
    cache_layout: &CacheLayout,
    no_cache: bool,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    verbose: bool,
    base_interner: &flux::syntax::interner::Interner,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<(Vec<PathBuf>, bool), String> {
    let entry_path = graph
        .entry_node()
        .map(|node| node.path.clone())
        .ok_or_else(|| "native module graph is missing an entry node".to_string())?;
    let nodes_by_path: HashMap<PathBuf, ModuleNode> = graph
        .topo_order()
        .into_iter()
        .map(|node| (node.path.clone(), node.clone()))
        .collect();
    let user_ctor_helper_owner = graph
        .topo_order()
        .into_iter()
        .find(|node| program_has_user_adt_declarations(&node.program))
        .map(|node| node.path.clone());
    let mut build_state = NativeParallelBuildState::new(graph, cache_layout, no_cache);

    for level in graph.topo_levels() {
        let (flow_nodes, user_nodes): (Vec<_>, Vec<_>) = level
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
            let force_rebuild_paths: HashSet<PathBuf> = batch
                .iter()
                .filter(|node| {
                    node.imports.iter().any(|dep| {
                        build_state
                            .interface_changed_modules
                            .contains(&dep.target_path)
                    })
                })
                .map(|node| node.path.clone())
                .collect();

            let mut results: Vec<_> = batch
                .par_iter()
                .map(|node| {
                    compile_parallel_native_module(
                        node,
                        node.path == entry_path,
                        &nodes_by_path,
                        &build_state.loaded_interfaces,
                        cache_layout,
                        no_cache,
                        force_rebuild_paths.contains(&node.path),
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                        base_interner,
                        user_ctor_helper_owner
                            .as_ref()
                            .is_some_and(|owner| owner == &node.path),
                    )
                })
                .collect();
            results.sort_by(|left, right| left.path.cmp(&right.path));

            for result in results {
                handle_native_result(
                    result,
                    &nodes_by_path,
                    &mut build_state,
                    cache_layout,
                    no_cache,
                    verbose,
                    base_interner,
                    strict_mode,
                    strict_types,
                    enable_optimize,
                    enable_analyze,
                    all_diagnostics,
                )?;
            }
        }
    }

    build_state.object_paths.sort();
    Ok((build_state.object_paths, build_state.any_module_recompiled))
}

#[cfg(test)]
mod tests {
    use super::{native_cache_hit_is_usable, native_miss_reason};
    use crate::syntax::module_graph::ModuleKind;

    #[test]
    fn native_cache_hit_rules_depend_on_module_kind() {
        assert!(native_cache_hit_is_usable(true, ModuleKind::FlowStdlib));
        assert!(!native_cache_hit_is_usable(false, ModuleKind::FlowStdlib));
        assert!(native_cache_hit_is_usable(false, ModuleKind::User));
    }

    #[test]
    fn native_miss_reason_respects_no_cache_and_force_rebuild() {
        assert_eq!(native_miss_reason(true, false, Some("stale".into())), None);
        assert_eq!(
            native_miss_reason(false, true, Some("stale".into())),
            Some("dependency interface changed".to_string())
        );
        assert_eq!(
            native_miss_reason(false, false, Some("stale".into())),
            Some("stale".to_string())
        );
        assert_eq!(native_miss_reason(false, false, None), None);
    }
}
