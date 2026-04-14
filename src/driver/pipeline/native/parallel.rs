//! Internal implementation of the parallel native module compilation pipeline.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use crate::{
    bytecode::bytecode_cache::{hash_bytes, hash_cache_key},
    bytecode::compiler::module_interface::{
        build_interface, compute_semantic_config_hash, load_cached_interface,
    },
    cache_paths::{self, CacheLayout},
    core_to_llvm::{
        module_cache::NativeModuleCache, pipeline::compile_ir_to_object, render_module, target,
    },
    diagnostics::{Diagnostic, DiagnosticPhase},
    syntax::{
        interner::Interner,
        module_graph::{ModuleGraph, ModuleKind, ModuleNode},
    },
    types::module_interface::ModuleInterface,
};
use rayon::prelude::*;

use super::native_temp_dir;
use crate::driver::{
    frontend::extract_module_name_and_sym,
    module_compile::{
        build_module_compiler, program_has_user_adt_declarations, tag_module_diagnostics,
    },
    pipeline::parallel_shared::{
        collect_dependency_fingerprints, dependency_changed_paths, emit_progress,
        filter_non_error_diagnostics, interfaces_changed, load_cached_interfaces_for_graph,
        partition_module_batches, progress_name, replay_module_diagnostics_for,
        save_interface_if_enabled, sort_by_path,
    },
};

#[derive(Debug)]
/// Result of compiling or loading a single module during native parallel lowering.
struct NativeParallelModuleResult {
    path: PathBuf,
    object_path: PathBuf,
    compile_failed: bool,
    error_message: Option<String>,
    interface: Option<ModuleInterface>,
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
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    cache_layout: &CacheLayout,
    no_cache: bool,
    force_rebuild: bool,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    base_interner: &Interner,
    export_user_ctor_name_helper: bool,
) -> NativeParallelModuleResult {
    let is_flow_library = node.kind == ModuleKind::FlowStdlib;
    let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
    let source_hash = hash_bytes(module_source.as_bytes());
    let semantic_config_hash =
        compute_semantic_config_hash(!is_flow_library && strict_mode, enable_optimize);
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);

    let native_miss_reason = if !no_cache && !force_rebuild {
        let native_cache = NativeModuleCache::new(cache_layout.native_dir());
        match native_cache.validate(
            &node.path,
            &cache_key,
            cache_layout.root(),
            export_user_ctor_name_helper,
        ) {
            Ok(object_path) => {
                let interface = load_cached_interface(cache_layout.root(), &node.path).ok();
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
    llvm_module.target_triple = Some(target::host_triple());
    llvm_module.data_layout = target::host_data_layout();
    let ll_text = render_module(&llvm_module);

    let dependency_fingerprints = collect_dependency_fingerprints(&node.imports, loaded_interfaces);

    let native_cache = NativeModuleCache::new(cache_layout.native_dir());
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

    if let Err(err) =
        compile_ir_to_object(&ll_text, &object_path, if enable_optimize { 2 } else { 0 })
    {
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

    let interface_changed =
        interfaces_changed(loaded_interfaces.get(&node.path), interface.as_ref());

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
    loaded_interfaces: HashMap<PathBuf, ModuleInterface>,
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
        let loaded_interfaces = if no_cache {
            HashMap::new()
        } else {
            load_cached_interfaces_for_graph(graph, cache_layout.root())
        };

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
        let name = progress_name(
            result.interface.as_ref().map(|i| i.module_name.as_str()),
            &result.path,
        );
        if result.skipped {
            emit_progress(
                self.completed_modules,
                self.total_modules,
                "Cached",
                &name,
                verbose,
                None,
            );
        } else {
            self.any_module_recompiled = true;
            emit_progress(
                self.completed_modules,
                self.total_modules,
                "Linking",
                &name,
                verbose,
                result.miss_reason.as_deref(),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_native_result(
    result: NativeParallelModuleResult,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    build_state: &mut NativeParallelBuildState,
    cache_layout: &CacheLayout,
    no_cache: bool,
    verbose: bool,
    base_interner: &Interner,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), String> {
    if result.compile_failed {
        if let Some(node) = nodes_by_path.get(&result.path) {
            all_diagnostics.extend(replay_module_diagnostics_for(
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
        let replayed = replay_module_diagnostics_for(
            node,
            nodes_by_path,
            &build_state.loaded_interfaces,
            base_interner,
            strict_mode,
            strict_types,
            enable_optimize,
            enable_analyze,
        );
        all_diagnostics.extend(filter_non_error_diagnostics(replayed));
    }

    build_state.record_progress(&result, verbose);
    if result.interface_changed {
        build_state
            .interface_changed_modules
            .insert(result.path.clone());
    }
    if let Some(interface) = result.interface {
        if !result.skipped {
            save_interface_if_enabled(
                no_cache,
                cache_layout.root(),
                &result.path,
                Some(&interface),
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
    base_interner: &Interner,
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
        let batches = partition_module_batches(&level, |node: &&ModuleNode| node.kind);

        for batch in batches {
            let force_rebuild_paths = dependency_changed_paths(
                &batch,
                |node| &node.path,
                |node| {
                    node.imports.iter().any(|dep| {
                        build_state
                            .interface_changed_modules
                            .contains(&dep.target_path)
                    })
                },
            );

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
            sort_by_path(&mut results, |result| &result.path);

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
