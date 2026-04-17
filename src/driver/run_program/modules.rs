use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate as flux;
use crate::driver::{
    backend::Backend,
    frontend::extract_module_name_and_sym,
    module_compile::{ModuleBuildState, effective_module_strictness, log_interface_diff},
    shared::{DriverCacheConfig, DriverCompileConfig, DriverRuntimeConfig, sort_stdlib_first},
    support::shared::{module_display_name, progress_line, short_hash, tag_diagnostics},
};
use flux::{
    bytecode::bytecode_cache::{hash_bytes, hash_cache_key, module_cache::ModuleBytecodeCache},
    diagnostics::{Diagnostic, DiagnosticPhase, quality::module_skipped_note, render_display_path},
    shared::cache_paths::cache_key_filename,
    syntax::module_graph::{ModuleGraph, ModuleKind, ModuleNode},
};

pub(crate) struct CompileModulesRequest<'a> {
    pub(crate) graph: &'a ModuleGraph,
    pub(crate) entry_path: &'a Path,
    pub(crate) failed_modules: &'a HashSet<PathBuf>,
    pub(crate) compiler: &'a mut flux::bytecode::compiler::Compiler,
    pub(crate) cache: DriverCacheConfig<'a>,
    pub(crate) compile: DriverCompileConfig,
    pub(crate) runtime: DriverRuntimeConfig,
    pub(crate) allow_cached_module_bytecode: bool,
    pub(crate) backend: Backend,
    pub(crate) strict_hash: [u8; 32],
    pub(crate) entry_has_errors: bool,
    pub(crate) all_diagnostics: &'a mut Vec<Diagnostic>,
}

pub(crate) fn compile_modules(request: CompileModulesRequest<'_>) {
    let entry_canonical = std::fs::canonicalize(request.entry_path).ok();
    let entry_module_kind = request
        .graph
        .entry_node()
        .map(|node| node.kind)
        .unwrap_or_default();
    let mut preloaded_interfaces: HashSet<PathBuf> = HashSet::new();
    let mut loaded_interfaces: HashMap<PathBuf, flux::types::module_interface::ModuleInterface> =
        HashMap::new();
    let mut module_states: HashMap<PathBuf, ModuleBuildState> = HashMap::new();
    let module_cache = ModuleBytecodeCache::new(request.cache.cache_layout.vm_dir());

    let nodes_by_path: HashMap<PathBuf, ModuleNode> = request
        .graph
        .topo_order()
        .iter()
        .map(|node| (node.path.clone(), (*node).clone()))
        .collect();
    let mut ordered_nodes = request.graph.topo_order();
    sort_stdlib_first(&mut ordered_nodes, |node| node.kind);

    let seq_total = ordered_nodes.len();
    let mut seq_completed = 0usize;
    let mut failed: HashSet<PathBuf> = request.failed_modules.clone();
    if request.entry_has_errors
        && let Ok(canon) = std::fs::canonicalize(request.entry_path)
    {
        failed.insert(canon);
    }

    for node in ordered_nodes {
        if request.entry_has_errors
            && let Some(ref canon) = entry_canonical
            && &node.path == canon
        {
            continue;
        }

        let failed_dep = node
            .imports
            .iter()
            .find(|e| failed.contains(&e.target_path));
        if let Some(dep) = failed_dep {
            failed.insert(node.path.clone());
            let display = render_display_path(&node.path.to_string_lossy()).into_owned();
            request.all_diagnostics.push(module_skipped_note(
                display.clone(),
                display,
                dep.name.clone(),
            ));
            continue;
        }

        for dep in &node.imports {
            if let Some(interface) = loaded_interfaces.get(&dep.target_path) {
                if preloaded_interfaces.insert(dep.target_path.clone()) {
                    request.compiler.preload_module_interface(interface);
                    if request.runtime.verbose {
                        eprintln!(
                            "interface: hit {} [abi:{}]",
                            interface.module_name,
                            short_hash(&interface.interface_fingerprint)
                        );
                    }
                }
                continue;
            }
            if request.cache.no_cache {
                continue;
            }
            let Ok(dep_source) = std::fs::read_to_string(&dep.target_path) else {
                if request.runtime.verbose {
                    eprintln!(
                        "interface: miss {} (reason: source not readable)",
                        dep.target_path.display()
                    );
                }
                continue;
            };
            let dep_semantic_config_hash = {
                let dep_kind = nodes_by_path
                    .get(&dep.target_path)
                    .map(|node| node.kind)
                    .unwrap_or_default();
                let strict_mode = effective_module_strictness(
                    dep_kind,
                    entry_module_kind,
                    request.compile.strict_mode,
                );
                flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
                    strict_mode,
                    request.compile.enable_optimize,
                )
            };
            match flux::bytecode::compiler::module_interface::load_valid_interface(
                request.cache.cache_layout.root(),
                &dep.target_path,
                &dep_source,
                &dep_semantic_config_hash,
            ) {
                Ok(interface) => {
                    request.compiler.preload_module_interface(&interface);
                    preloaded_interfaces.insert(dep.target_path.clone());
                    if request.runtime.verbose {
                        eprintln!(
                            "interface: hit {} [abi:{}]",
                            interface.module_name,
                            short_hash(&interface.interface_fingerprint)
                        );
                    }
                    loaded_interfaces.insert(dep.target_path.clone(), interface);
                }
                Err(err) if request.runtime.verbose => {
                    eprintln!(
                        "interface: miss {} (reason: {})",
                        dep.target_path.display(),
                        err.message()
                    );
                }
                Err(_) => {}
            }
        }
        for dep in &node.imports {
            if let Some(dep_node) = nodes_by_path.get(&dep.target_path) {
                request
                    .compiler
                    .preload_dependency_program(&dep_node.program);
            }
        }
        if node.kind != ModuleKind::FlowStdlib {
            for (path, dep_node) in &nodes_by_path {
                if !node.imports.iter().any(|dep| &dep.target_path == path)
                    && dep_node.kind == ModuleKind::FlowStdlib
                {
                    request
                        .compiler
                        .preload_dependency_program(&dep_node.program);
                }
            }
        }

        request
            .compiler
            .set_file_path(node.path.to_string_lossy().to_string());
        request.compiler.set_current_module_kind(node.kind);
        let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
        let module_strict_mode =
            effective_module_strictness(node.kind, entry_module_kind, request.compile.strict_mode);
        request.compiler.set_strict_mode(module_strict_mode);
        let module_semantic_config_hash =
            flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
                module_strict_mode,
                request.compile.enable_optimize,
            );
        let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
        let module_source_hash = hash_bytes(module_source.as_bytes());
        let module_strict_hash = request.strict_hash;
        let module_cache_key = hash_cache_key(&module_source_hash, &module_strict_hash);
        let old_interface = if !request.cache.no_cache {
            flux::bytecode::compiler::module_interface::load_cached_interface(
                request.cache.cache_layout.root(),
                &node.path,
            )
            .ok()
        } else {
            None
        };
        let must_rebuild_due_to_dependency = node.imports.iter().any(|dep| {
            module_states
                .get(&dep.target_path)
                .is_some_and(|state| state.interface_changed)
        });
        let current_interface = if !request.cache.no_cache {
            match flux::bytecode::compiler::module_interface::load_valid_interface(
                request.cache.cache_layout.root(),
                &node.path,
                &module_source,
                &module_semantic_config_hash,
            ) {
                Ok(interface) => Some(interface),
                Err(err) => {
                    if request.runtime.verbose && !is_entry_module {
                        eprintln!(
                            "interface: miss {} (reason: {})",
                            node.path.display(),
                            err.message()
                        );
                    }
                    None
                }
            }
        } else {
            None
        };

        let can_skip_semantic = !request.cache.no_cache
            && !must_rebuild_due_to_dependency
            && current_interface.is_some();
        let has_vm_cache = can_skip_semantic
            && request.allow_cached_module_bytecode
            && module_cache
                .load(
                    &node.path,
                    &module_cache_key,
                    env!("CARGO_PKG_VERSION"),
                    request.cache.cache_layout.root(),
                )
                .is_some();
        let has_vm_cache_entry = !request.cache.no_cache
            && is_entry_module
            && !must_rebuild_due_to_dependency
            && request.allow_cached_module_bytecode
            && module_cache
                .load(
                    &node.path,
                    &module_cache_key,
                    env!("CARGO_PKG_VERSION"),
                    request.cache.cache_layout.root(),
                )
                .is_some();
        let skip_for_llvm = can_skip_semantic && request.backend == Backend::Native;
        let llvm_entry_marker = request.cache.cache_layout.vm_dir().join(cache_key_filename(
            &node.path,
            &module_cache_key,
            "fxs",
        ));
        let skip_llvm_entry = request.backend == Backend::Native
            && is_entry_module
            && !request.cache.no_cache
            && llvm_entry_marker.exists();

        if has_vm_cache || skip_for_llvm || has_vm_cache_entry || skip_llvm_entry {
            if let Some(interface) = current_interface.as_ref() {
                request.compiler.preload_module_interface(interface);
                loaded_interfaces.insert(node.path.clone(), interface.clone());
                preloaded_interfaces.insert(node.path.clone());
            }
            if (has_vm_cache || has_vm_cache_entry)
                && let Some(cached) = module_cache.load(
                    &node.path,
                    &module_cache_key,
                    env!("CARGO_PKG_VERSION"),
                    request.cache.cache_layout.root(),
                )
            {
                request.compiler.hydrate_cached_module_bytecode(&cached);
            }
            let display_name = current_interface
                .as_ref()
                .map(|i| i.module_name.clone())
                .unwrap_or_else(|| module_display_name(&node.path));
            seq_completed += 1;
            eprintln!(
                "{}",
                progress_line(seq_completed, seq_total, "Cached", &display_name,)
            );
            module_states.insert(
                node.path.clone(),
                ModuleBuildState {
                    old_interface_fingerprint: current_interface
                        .as_ref()
                        .map(|i| i.interface_fingerprint.clone()),
                    new_interface_fingerprint: current_interface
                        .as_ref()
                        .map(|i| i.interface_fingerprint.clone()),
                    interface_changed: false,
                    rebuild_required: false,
                    skipped: true,
                },
            );
            continue;
        } else if !request.cache.no_cache
            && !must_rebuild_due_to_dependency
            && request.runtime.verbose
        {
            if request.allow_cached_module_bytecode {
                let reason = module_cache
                    .load_failure_reason(
                        &node.path,
                        &module_cache_key,
                        env!("CARGO_PKG_VERSION"),
                        request.cache.cache_layout.root(),
                    )
                    .unwrap_or_else(|| "not eligible".to_string());
                eprintln!("module-cache: miss (reason: {reason})");
            } else if current_interface.is_none() && !is_entry_module {
                eprintln!(
                    "interface: miss {} (no valid interface)",
                    node.path.display()
                );
            }
        } else if request.runtime.verbose
            && !request.cache.no_cache
            && must_rebuild_due_to_dependency
        {
            eprintln!("module-cache: miss (reason: dependency interface changed)");
        }
        request.compiler.set_strict_require_main(is_entry_module);
        let module_snapshot = request.compiler.module_cache_snapshot();
        let compile_result = request.compiler.compile_with_opts(
            &node.program,
            request.compile.enable_optimize,
            request.compile.enable_analyze,
        );
        let mut compiler_warnings = request.compiler.take_warnings();
        tag_diagnostics(&mut compiler_warnings, DiagnosticPhase::Validation);
        for diag in &mut compiler_warnings {
            if diag.file().is_none() {
                diag.set_file(node.path.to_string_lossy().to_string());
            }
        }
        request.all_diagnostics.append(&mut compiler_warnings);

        if let Err(mut diags) = compile_result {
            failed.insert(node.path.clone());
            tag_diagnostics(&mut diags, DiagnosticPhase::TypeCheck);
            for diag in &mut diags {
                if diag.file().is_none() {
                    diag.set_file(node.path.to_string_lossy().to_string());
                }
            }
            request.all_diagnostics.append(&mut diags);
            continue;
        }

        {
            seq_completed += 1;
            let name = module_display_name(&node.path);
            eprintln!(
                "{}",
                progress_line(seq_completed, seq_total, "Compiling", &name)
            );
        }

        let module_deps: Vec<(String, String)> = node
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
            .collect();

        if !request.cache.no_cache && request.allow_cached_module_bytecode {
            let cached_module = request
                .compiler
                .build_cached_module_bytecode(module_snapshot);
            if let Err(e) = module_cache.store(
                &node.path,
                &module_cache_key,
                env!("CARGO_PKG_VERSION"),
                &cached_module,
                &module_deps,
            ) && request.runtime.verbose
            {
                eprintln!(
                    "warning: could not write module cache file for {}: {e}",
                    node.path.display()
                );
            }
        }
        if !request.cache.no_cache && request.backend == Backend::Native && is_entry_module {
            let marker_path = request.cache.cache_layout.vm_dir().join(cache_key_filename(
                &node.path,
                &module_cache_key,
                "fxs",
            ));
            if let Some(parent) = marker_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker_path, b"");
        }

        if let Some((module_name, module_sym)) =
            extract_module_name_and_sym(&node.program, &request.compiler.interner)
        {
            match request
                .compiler
                .lower_aether_report_program(&node.program, request.compile.enable_optimize)
            {
                Ok(core) => {
                    let dependency_fingerprints = node
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
                    let exported_runtime_contracts = request.compiler.exported_runtime_contracts();
                    let interface = flux::bytecode::compiler::module_interface::build_interface(
                        &module_name,
                        module_sym,
                        &module_source_hash,
                        &module_semantic_config_hash,
                        core.as_core(),
                        request.compiler.cached_member_schemes(),
                        &exported_runtime_contracts,
                        &request.compiler.module_function_visibility,
                        Some(request.compiler.class_env()),
                        dependency_fingerprints,
                        &request.compiler.interner,
                    );
                    request.compiler.preload_module_interface(&interface);
                    loaded_interfaces.insert(node.path.clone(), interface.clone());
                    preloaded_interfaces.insert(node.path.clone());
                    let interface_changed = old_interface.as_ref().is_none_or(|old| {
                        flux::bytecode::compiler::module_interface::module_interface_changed(
                            old, &interface,
                        )
                    });
                    if request.runtime.verbose && interface_changed {
                        if let Some(old) = old_interface.as_ref() {
                            log_interface_diff(old, &interface);
                        } else {
                            eprintln!("  interface: new (no previous interface)");
                        }
                    }
                    module_states.insert(
                        node.path.clone(),
                        ModuleBuildState {
                            old_interface_fingerprint: old_interface
                                .as_ref()
                                .map(|interface| interface.interface_fingerprint.clone()),
                            new_interface_fingerprint: Some(
                                interface.interface_fingerprint.clone(),
                            ),
                            interface_changed,
                            rebuild_required: true,
                            skipped: false,
                        },
                    );
                    if !request.cache.no_cache {
                        let iface_path = flux::bytecode::compiler::module_interface::interface_path(
                            request.cache.cache_layout.root(),
                            &node.path,
                        );
                        if let Err(e) = flux::bytecode::compiler::module_interface::save_interface(
                            &iface_path,
                            &interface,
                        ) {
                            if request.runtime.verbose {
                                eprintln!(
                                    "warning: could not write interface file {}: {e}",
                                    iface_path.display()
                                );
                            }
                        } else if request.runtime.verbose {
                            eprintln!(
                                "interface: stored {} [abi:{}]",
                                interface.module_name,
                                short_hash(&interface.interface_fingerprint)
                            );
                        }
                    }
                }
                Err(e) if request.runtime.verbose => {
                    module_states.insert(
                        node.path.clone(),
                        ModuleBuildState {
                            old_interface_fingerprint: old_interface
                                .as_ref()
                                .map(|interface| interface.interface_fingerprint.clone()),
                            new_interface_fingerprint: None,
                            interface_changed: true,
                            rebuild_required: true,
                            skipped: false,
                        },
                    );
                    eprintln!(
                        "warning: could not build interface for {}: {}",
                        node.path.display(),
                        e.message().unwrap_or("unknown Core lowering error")
                    );
                }
                Err(_) => {
                    module_states.insert(
                        node.path.clone(),
                        ModuleBuildState {
                            old_interface_fingerprint: old_interface
                                .as_ref()
                                .map(|interface| interface.interface_fingerprint.clone()),
                            new_interface_fingerprint: None,
                            interface_changed: true,
                            rebuild_required: true,
                            skipped: false,
                        },
                    );
                }
            }
        } else {
            module_states.insert(
                node.path.clone(),
                ModuleBuildState {
                    old_interface_fingerprint: None,
                    new_interface_fingerprint: None,
                    interface_changed: false,
                    rebuild_required: true,
                    skipped: false,
                },
            );
        }
    }
}
