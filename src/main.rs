use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

use rayon::prelude::*;

use flux::syntax::program::Program;
use flux::{
    ast::{collect_free_vars_in_program, find_tail_calls},
    bytecode::vm::{
        VM,
        test_runner::{collect_test_functions, print_test_report, run_tests},
    },
    bytecode::{
        bytecode_cache::{hash_bytes, hash_cache_key, module_cache::ModuleBytecodeCache},
        compiler::Compiler,
        module_linker::{LinkedVmProgram, VmAssemblyContext},
        op_code::disassemble,
    },
    cache_paths::{self, CacheLayout},
    diagnostics::{
        DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticPhase, DiagnosticsAggregator,
        quality::module_skipped_note, render_diagnostics_json, render_display_path,
    },
    runtime::value::Value,
    syntax::{
        formatter::format_source,
        lexer::Lexer,
        linter::Linter,
        module_graph::{ModuleGraph, ModuleNode},
        parser::Parser,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticOutputFormat {
    Text,
    Json,
    JsonCompact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreDumpMode {
    None,
    Readable,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AetherDumpMode {
    None,
    Summary,
    Debug,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceBackend {
    Vm,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
struct ModuleBuildState {
    old_interface_fingerprint: Option<String>,
    new_interface_fingerprint: Option<String>,
    interface_changed: bool,
    rebuild_required: bool,
    skipped: bool,
}

struct ParallelVmBuild {
    bytecode: flux::bytecode::bytecode::Bytecode,
    symbol_table: flux::bytecode::symbol_table::SymbolTable,
    cached_count: usize,
    compiled_count: usize,
}

#[derive(Debug)]
struct ParallelModuleResult {
    path: PathBuf,
    needs_serial_warning_replay: bool,
    compile_failed: bool,
    old_interface_fingerprint: Option<String>,
    new_interface_fingerprint: Option<String>,
    interface_changed: bool,
    skipped: bool,
    interface_hit: Option<flux::types::module_interface::ModuleInterface>,
    cache_key: [u8; 32],
    /// Human-readable reason why the module was recompiled (populated when
    /// the cache miss path is taken).
    miss_reason: Option<String>,
}

#[cfg(feature = "core_to_llvm")]
#[derive(Debug)]
struct NativeParallelModuleResult {
    path: PathBuf,
    object_path: PathBuf,
    compile_failed: bool,
    error_message: Option<String>,
    interface: Option<flux::types::module_interface::ModuleInterface>,
    skipped: bool,
    interface_changed: bool,
    /// Human-readable reason why the module was recompiled.
    miss_reason: Option<String>,
}

fn is_flow_library_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("lib/Flow/") || text.contains("lib\\Flow\\")
}

#[cfg(feature = "core_to_llvm")]
fn program_has_user_adt_declarations(program: &Program) -> bool {
    fn block_has_user_adt_declarations(block: &flux::syntax::block::Block) -> bool {
        block
            .statements
            .iter()
            .any(statement_has_user_adt_declarations)
    }

    fn statement_has_user_adt_declarations(statement: &flux::syntax::statement::Statement) -> bool {
        match statement {
            flux::syntax::statement::Statement::Data { .. } => true,
            flux::syntax::statement::Statement::Module { body, .. } => {
                block_has_user_adt_declarations(body)
            }
            _ => false,
        }
    }

    program
        .statements
        .iter()
        .any(statement_has_user_adt_declarations)
}

fn tag_module_diagnostics(diags: &mut Vec<Diagnostic>, phase: DiagnosticPhase, path: &Path) {
    tag_diagnostics(diags, phase);
    for diag in diags {
        if diag.file().is_none() {
            diag.set_file(path.to_string_lossy().to_string());
        }
    }
}

fn build_module_compiler(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    base_interner: &flux::syntax::interner::Interner,
    strict_mode: bool,
    strict_types: bool,
    is_entry_module: bool,
) -> Compiler {
    let mut compiler = Compiler::new_with_interner(
        node.path.to_string_lossy().to_string(),
        base_interner.clone(),
    );
    compiler.set_strict_require_main(is_entry_module);
    compiler.set_strict_mode(!is_flow_library_path(&node.path) && strict_mode);
    compiler.set_strict_types(!is_flow_library_path(&node.path) && strict_types);
    for dep in &node.imports {
        if let Some(interface) = loaded_interfaces.get(&dep.target_path) {
            compiler.preload_module_interface(interface);
        }
        if let Some(dep_node) = nodes_by_path.get(&dep.target_path) {
            compiler.preload_dependency_program(&dep_node.program);
        }
    }
    // Auto-prelude: ensure Flow library interfaces and AST visibility are
    // available to all non-Flow modules, even without explicit import edges.
    // The sequential (--no-cache) path achieves this via a shared compiler
    // instance; the parallel path needs explicit preloading.
    // Flow-to-Flow dependencies must use explicit imports.
    if !is_flow_library_path(&node.path) {
        for (path, interface) in loaded_interfaces {
            if !node.imports.iter().any(|dep| &dep.target_path == path)
                && is_flow_library_path(path)
            {
                compiler.preload_module_interface(interface);
            }
        }
        for (path, dep_node) in nodes_by_path {
            if !node.imports.iter().any(|dep| &dep.target_path == path)
                && is_flow_library_path(path)
            {
                compiler.preload_dependency_program(&dep_node.program);
            }
        }
    }
    if is_flow_library_path(&node.path) {
        compiler.set_strict_mode(false);
        compiler.set_strict_types(false);
    }
    compiler
}

#[allow(clippy::too_many_arguments)]
fn replay_module_diagnostics(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    base_interner: &flux::syntax::interner::Interner,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
) -> Vec<Diagnostic> {
    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        base_interner,
        strict_mode,
        strict_types,
        false,
    );
    let compile_result = compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
    let mut diagnostics = compiler.take_warnings();
    tag_module_diagnostics(&mut diagnostics, DiagnosticPhase::Validation, &node.path);
    if let Err(mut diags) = compile_result {
        tag_module_diagnostics(&mut diags, DiagnosticPhase::TypeCheck, &node.path);
        diagnostics.extend(diags);
    }
    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn compile_parallel_module(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, flux::types::module_interface::ModuleInterface>,
    cache_layout: &CacheLayout,
    no_cache: bool,
    force_rebuild: bool,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    is_entry: bool,
    base_interner: &flux::syntax::interner::Interner,
) -> ParallelModuleResult {
    let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
    let source_hash = hash_bytes(module_source.as_bytes());
    let semantic_config_hash =
        flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
            !is_flow_library_path(&node.path) && strict_mode,
            enable_optimize,
        );
    let strict_hash = if is_flow_library_path(&node.path) {
        hash_bytes(b"strict=0")
    } else {
        hash_bytes(if strict_mode {
            b"strict=1"
        } else {
            b"strict=0"
        })
    };
    let cache_key = hash_cache_key(&source_hash, &strict_hash);
    let module_cache = ModuleBytecodeCache::new(cache_layout.vm_dir());
    let old_interface = if !no_cache {
        flux::bytecode::compiler::module_interface::load_cached_interface(
            cache_layout.root(),
            &node.path,
        )
        .ok()
    } else {
        None
    };
    let (current_interface, interface_miss_reason) = if !no_cache {
        match flux::bytecode::compiler::module_interface::load_valid_interface(
            cache_layout.root(),
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

    if !no_cache
        && !force_rebuild
        && (current_interface.is_some() || is_entry)
        && module_cache
            .load(
                &node.path,
                &cache_key,
                env!("CARGO_PKG_VERSION"),
                cache_layout.root(),
            )
            .is_some()
    {
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

    // Determine the miss reason: interface failure takes priority, then
    // check whether the bytecode artifact itself is stale.
    let miss_reason = if force_rebuild {
        Some("dependency interface changed".to_string())
    } else if let Some(reason) = interface_miss_reason {
        Some(reason)
    } else {
        module_cache.load_failure_reason(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            cache_layout.root(),
        )
    };

    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        base_interner,
        strict_mode,
        strict_types,
        is_entry,
    );
    let compile_result = compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
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
            loaded_interfaces.get(&dep.target_path).map(|interface| {
                flux::types::module_interface::DependencyFingerprint {
                    module_name: interface.module_name.clone(),
                    source_path: dep.target_path.to_string_lossy().to_string(),
                    interface_fingerprint: interface.interface_fingerprint.clone(),
                }
            })
        })
        .collect::<Vec<_>>();

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
                        &core,
                        compiler.cached_member_schemes(),
                        &compiler.module_function_visibility,
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
    if !no_cache {
        let _ = module_cache.store(
            &node.path,
            &cache_key,
            env!("CARGO_PKG_VERSION"),
            &artifact,
            &module_deps,
        );
        if let Some(interface) = interface.as_ref() {
            let iface_path = flux::bytecode::compiler::module_interface::interface_path(
                cache_layout.root(),
                &node.path,
            );
            let _ =
                flux::bytecode::compiler::module_interface::save_interface(&iface_path, interface);
        }
    }

    let new_interface_fingerprint = interface
        .as_ref()
        .map(|iface| iface.interface_fingerprint.clone());
    let interface_changed = match (&old_interface, &interface) {
        (Some(old), Some(new)) => {
            flux::bytecode::compiler::module_interface::module_interface_changed(old, new)
        }
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

#[allow(clippy::too_many_arguments)]
fn compile_vm_modules_parallel(
    graph: &ModuleGraph,
    entry_canonical: Option<&PathBuf>,
    graph_interner: &flux::syntax::interner::Interner,
    cache_layout: &CacheLayout,
    no_cache: bool,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    verbose: bool,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<ParallelVmBuild, String> {
    let mut loaded_interfaces: HashMap<PathBuf, flux::types::module_interface::ModuleInterface> =
        HashMap::new();
    let mut module_states: HashMap<PathBuf, ModuleBuildState> = HashMap::new();
    let mut failed: HashSet<PathBuf> = HashSet::new();
    let mut nodes_by_path: HashMap<PathBuf, ModuleNode> = HashMap::new();
    for node in graph.topo_order() {
        nodes_by_path.insert(node.path.clone(), node.clone());
    }

    let mut linker = VmAssemblyContext::new(graph_interner.clone());
    let module_cache = ModuleBytecodeCache::new(cache_layout.vm_dir());
    let total_modules = graph.topo_order().len();
    let mut completed_modules = 0usize;
    let mut cached_count = 0usize;

    for level in graph.topo_levels().into_iter() {
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

        // Split each topo level into Flow library modules and user modules.
        // Flow modules must be processed first so their interfaces are in
        // `loaded_interfaces` before user modules compile — the auto-prelude
        // makes Flow functions available to all modules, but user modules
        // have no explicit import edges to them in the module graph.
        let (flow_nodes, user_nodes): (Vec<_>, Vec<_>) = ready
            .iter()
            .partition(|node| is_flow_library_path(&node.path));

        // Sub-batches to process: Flow first, then user modules.
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
                    let is_entry = entry_canonical.is_some_and(|entry| entry == &node.path);
                    compile_parallel_module(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        cache_layout,
                        no_cache,
                        false,
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                        is_entry,
                        graph_interner,
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
                    let is_entry = entry_canonical.is_some_and(|entry| entry == &node.path);
                    let result = compile_parallel_module(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        cache_layout,
                        no_cache,
                        true,
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                        is_entry,
                        graph_interner,
                    );
                    parallel_results.push(result);
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
                            graph_interner,
                            strict_mode,
                            strict_types,
                            enable_optimize,
                            enable_analyze,
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
                        graph_interner,
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                    );
                    all_diagnostics.extend(
                        replayed
                            .into_iter()
                            .filter(|diag| diag.severity() != flux::diagnostics::Severity::Error),
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
                        if verbose && let Some(reason) = &result.miss_reason {
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
                        if verbose && let Some(reason) = &result.miss_reason {
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
                        cache_layout.root(),
                    )
                    .ok_or_else(|| {
                        format!(
                            "could not load module artifact for {}",
                            result.path.display()
                        )
                    })?;
                linker.assemble_module(&artifact)?;
            }
        } // end for batch in batches
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

#[cfg(feature = "core_to_llvm")]
fn native_temp_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("flux_native_{}_{}", std::process::id(), stamp))
}

#[cfg(feature = "core_to_llvm")]
fn compile_native_support_object(
    cache_layout: &CacheLayout,
    no_cache: bool,
    enable_optimize: bool,
    program_has_main: bool,
) -> Result<PathBuf, String> {
    let object_path = if no_cache {
        let dir = native_temp_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(if cfg!(windows) {
            "flux_support.obj"
        } else {
            "flux_support.o"
        })
    } else {
        let dir = cache_layout.native_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(if enable_optimize {
            if cfg!(windows) {
                "flux_support_O2.obj"
            } else {
                "flux_support_O2.o"
            }
        } else if cfg!(windows) {
            "flux_support_O0.obj"
        } else {
            "flux_support_O0.o"
        })
    };

    // The support object is deterministic (empty LirProgram + runtime stubs),
    // so reuse the cached .o if it already exists. Skip cache when emitting
    // a flux_main stub (no-main programs) to avoid reusing a stub-less object.
    if !no_cache && program_has_main && object_path.exists() {
        return Ok(object_path);
    }

    let lir = flux::lir::LirProgram::new();
    let mut llvm_module = flux::lir::emit_llvm::emit_llvm_module_with_options(&lir, true, false);
    // If no module defines fn main(), emit an empty flux_main stub so that
    // libflux_rt.a links successfully. This allows module-only .flx files
    // to be compiled with --native.
    if !program_has_main {
        llvm_module
            .functions
            .push(flux::lir::emit_llvm::flux_main_stub());
    }
    llvm_module.target_triple = Some(flux::core_to_llvm::target::host_triple());
    llvm_module.data_layout = flux::core_to_llvm::target::host_data_layout();
    let ll_text = flux::core_to_llvm::render_module(&llvm_module);
    flux::core_to_llvm::pipeline::compile_ir_to_object(
        &ll_text,
        &object_path,
        if enable_optimize { 2 } else { 0 },
    )
    .map_err(|err| format!("native support object compilation failed: {err}"))?;
    Ok(object_path)
}

#[cfg(feature = "core_to_llvm")]
#[allow(clippy::too_many_arguments)]
fn compile_parallel_native_module(
    node: &ModuleNode,
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
    let is_flow_library = is_flow_library_path(&node.path);
    let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
    let source_hash = hash_bytes(module_source.as_bytes());
    let semantic_config_hash =
        flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
            !is_flow_library && strict_mode,
            enable_optimize,
        );
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);

    // Check native artifact cache before doing any compilation.
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
                // Try to load a cached interface. Entry modules (containing main)
                // may not have one, but their .o can still be cached.
                let interface = flux::bytecode::compiler::module_interface::load_cached_interface(
                    cache_layout.root(),
                    &node.path,
                )
                .ok();
                // Library modules require a valid interface to be cached;
                // entry modules (non-library) can be cached without one.
                if interface.is_some() || !is_flow_library_path(&node.path) {
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
    } else if force_rebuild {
        Some("dependency interface changed".to_string())
    } else {
        None
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
                        &core,
                        compiler.cached_member_schemes(),
                        &compiler.module_function_visibility,
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

#[cfg(feature = "core_to_llvm")]
#[allow(clippy::too_many_arguments)]
fn compile_native_modules_parallel(
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
    let mut loaded_interfaces: HashMap<PathBuf, flux::types::module_interface::ModuleInterface> =
        HashMap::new();
    let mut nodes_by_path: HashMap<PathBuf, ModuleNode> = HashMap::new();
    for node in graph.topo_order() {
        nodes_by_path.insert(node.path.clone(), node.clone());
    }
    let user_ctor_helper_owner = graph
        .topo_order()
        .into_iter()
        .find(|node| program_has_user_adt_declarations(&node.program))
        .map(|node| node.path.clone());
    // Pre-load valid interfaces from cache so dependency fingerprints are
    // available for the first level's cache validation.
    if !no_cache {
        for node in graph.topo_order() {
            if let Ok(interface) = flux::bytecode::compiler::module_interface::load_cached_interface(
                cache_layout.root(),
                &node.path,
            ) {
                loaded_interfaces.insert(node.path.clone(), interface);
            }
        }
    }
    let mut object_paths = Vec::new();
    let mut any_module_recompiled = false;
    // Track which modules had interface changes so dependents can be forced
    // to rebuild even if their own source didn't change.
    let mut interface_changed_modules: HashSet<PathBuf> = HashSet::new();
    let total_native_modules = graph.topo_order().len();
    let mut completed_native = 0usize;

    for level in graph.topo_levels().into_iter() {
        // Split each level into Flow library modules and user modules.
        // Flow modules must be processed first so their interfaces are
        // available for user modules via auto-prelude.
        let (flow_nodes, user_nodes): (Vec<_>, Vec<_>) = level
            .iter()
            .partition(|node| is_flow_library_path(&node.path));
        let batches: Vec<Vec<&ModuleNode>> = if flow_nodes.is_empty() {
            vec![user_nodes]
        } else if user_nodes.is_empty() {
            vec![flow_nodes]
        } else {
            vec![flow_nodes, user_nodes]
        };

        for batch in batches {
            let mut results: Vec<_> = batch
                .par_iter()
                .map(|node| {
                    let force_rebuild = node
                        .imports
                        .iter()
                        .any(|dep| interface_changed_modules.contains(&dep.target_path));
                    compile_parallel_native_module(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        cache_layout,
                        no_cache,
                        force_rebuild,
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
                if result.compile_failed {
                    if let Some(node) = nodes_by_path.get(&result.path) {
                        all_diagnostics.extend(replay_module_diagnostics(
                            node,
                            &nodes_by_path,
                            &loaded_interfaces,
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
                    let replayed = replay_module_diagnostics(
                        node,
                        &nodes_by_path,
                        &loaded_interfaces,
                        base_interner,
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                    );
                    all_diagnostics.extend(
                        replayed
                            .into_iter()
                            .filter(|diag| diag.severity() != flux::diagnostics::Severity::Error),
                    );
                }
                completed_native += 1;
                {
                    let name = result
                        .interface
                        .as_ref()
                        .map(|i| i.module_name.clone())
                        .unwrap_or_else(|| module_display_name(&result.path));
                    if result.skipped {
                        eprintln!(
                            "{}",
                            progress_line(completed_native, total_native_modules, "Cached", &name)
                        );
                    } else {
                        any_module_recompiled = true;
                        if verbose && let Some(reason) = &result.miss_reason {
                            eprintln!("  cache miss ({name}): {reason}");
                        }
                        eprintln!(
                            "{}",
                            progress_line(completed_native, total_native_modules, "Linking", &name)
                        );
                    }
                }
                if result.interface_changed {
                    interface_changed_modules.insert(result.path.clone());
                }
                if let Some(interface) = result.interface {
                    if !result.skipped {
                        let interface_path =
                            flux::bytecode::compiler::module_interface::interface_path(
                                cache_layout.root(),
                                &result.path,
                            );
                        if !no_cache {
                            let _ = flux::bytecode::compiler::module_interface::save_interface(
                                &interface_path,
                                &interface,
                            );
                        }
                    }
                    loaded_interfaces.insert(result.path.clone(), interface);
                }
                object_paths.push(result.object_path);
            }
        } // end for batch in batches
    }

    object_paths.sort();
    Ok((object_paths, any_module_recompiled))
}

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    let leak_detector = args.iter().any(|arg| arg == "--leak-detector");
    let trace = args.iter().any(|arg| arg == "--trace");
    let trace_aether = args.iter().any(|arg| arg == "--trace-aether");
    let profiling = args.iter().any(|arg| arg == "--prof");
    // Profiling requires fresh compilation (no cached bytecode) since
    // OpEnterCC instructions are only emitted when profiling is enabled.
    let no_cache = args.iter().any(|arg| arg == "--no-cache") || profiling;
    let roots_only = args.iter().any(|arg| arg == "--roots-only");
    let enable_optimize = args.iter().any(|arg| arg == "--optimize" || arg == "-O");
    let enable_analyze = args.iter().any(|arg| arg == "--analyze" || arg == "-A");
    let show_stats = args.iter().any(|arg| arg == "--stats");
    let test_mode = args.iter().any(|arg| arg == "--test");
    let strict_mode = args.iter().any(|arg| arg == "--strict");
    let strict_types = args.iter().any(|arg| arg == "--strict-types");
    let all_errors = args.iter().any(|arg| arg == "--all-errors");
    let dump_aether = if args.iter().any(|arg| arg == "--dump-aether=debug") {
        AetherDumpMode::Debug
    } else if args.iter().any(|arg| arg == "--dump-aether") {
        AetherDumpMode::Summary
    } else {
        AetherDumpMode::None
    };
    let dump_lir = args.iter().any(|arg| arg == "--dump-lir");
    #[cfg(feature = "core_to_llvm")]
    let dump_lir_llvm = args.iter().any(|arg| arg == "--dump-lir-llvm");
    #[cfg(not(feature = "core_to_llvm"))]
    let dump_lir_llvm = false;
    #[cfg(feature = "native")]
    let use_core_to_llvm = args
        .iter()
        .any(|arg| arg == "--core-to-llvm" || arg == "--native");
    #[cfg(not(feature = "native"))]
    let use_core_to_llvm = false;
    let emit_llvm = args.iter().any(|arg| arg == "--emit-llvm");
    let emit_binary = args.iter().any(|arg| arg == "--emit-binary");
    let mut roots = Vec::new();
    if verbose {
        args.retain(|arg| arg != "--verbose");
    }
    if leak_detector {
        args.retain(|arg| arg != "--leak-detector");
    }
    if trace {
        args.retain(|arg| arg != "--trace");
    }
    if trace_aether {
        args.retain(|arg| arg != "--trace-aether");
    }
    if no_cache {
        args.retain(|arg| arg != "--no-cache");
    }
    if profiling {
        args.retain(|arg| arg != "--prof");
    }
    if roots_only {
        args.retain(|arg| arg != "--roots-only");
    }
    if enable_optimize {
        args.retain(|arg| arg != "--optimize" && arg != "-O");
    }
    if enable_analyze {
        args.retain(|arg| arg != "--analyze" && arg != "-A");
    }
    if show_stats {
        args.retain(|arg| arg != "--stats");
    }
    if test_mode {
        args.retain(|arg| arg != "--test");
    }
    if args.iter().any(|arg| arg == "--strict") {
        args.retain(|arg| arg != "--strict");
    }
    if strict_types {
        args.retain(|arg| arg != "--strict-types");
    }
    args.retain(|arg| arg != "--no-strict");
    if all_errors {
        args.retain(|arg| arg != "--all-errors");
    }
    if dump_aether != AetherDumpMode::None {
        args.retain(|arg| arg != "--dump-aether" && arg != "--dump-aether=debug");
    }
    if dump_lir {
        args.retain(|arg| arg != "--dump-lir");
    }
    if dump_lir_llvm {
        args.retain(|arg| arg != "--dump-lir-llvm");
    }
    if use_core_to_llvm {
        args.retain(|arg| arg != "--core-to-llvm" && arg != "--native");
    }
    if emit_llvm {
        args.retain(|arg| arg != "--emit-llvm");
    }
    if emit_binary {
        args.retain(|arg| arg != "--emit-binary");
    }
    let cache_dir = match extract_cache_dir(&mut args) {
        Some(value) => value,
        None => return,
    };
    let output_path = extract_output_path(&mut args);
    let dump_core = match extract_dump_core_mode(&mut args) {
        Some(value) => value,
        None => return,
    };
    let diagnostics_format = match extract_diagnostic_format(&mut args) {
        Some(value) => value,
        None => return,
    };
    let max_errors = match extract_max_errors(&mut args) {
        Some(value) => value,
        None => return,
    };
    let test_filter = match extract_test_filter(&mut args) {
        Some(value) => value,
        None => return,
    };
    if !extract_roots(&mut args, &mut roots) {
        return;
    }

    if trace_aether
        && (!matches!(dump_core, CoreDumpMode::None)
            || dump_aether != AetherDumpMode::None
            || test_mode)
    {
        eprintln!(
            "Error: --trace-aether only supports normal program execution. Use --dump-aether for report-only output."
        );
        return;
    }

    if args.len() < 2 {
        print_help();
        return;
    }

    if is_flx_file(&args[1]) {
        if test_mode {
            run_test_file(
                &args[1],
                roots_only,
                enable_optimize,
                enable_analyze,
                max_errors,
                &roots,
                cache_dir.as_deref(),
                test_filter.as_deref(),
                strict_mode,
                strict_types,
                diagnostics_format,
                all_errors,
                use_core_to_llvm,
            );
        } else {
            run_file(
                &args[1],
                verbose,
                leak_detector,
                trace,
                no_cache,
                roots_only,
                enable_optimize,
                enable_analyze,
                max_errors,
                &roots,
                cache_dir.as_deref(),
                show_stats,
                trace_aether,
                strict_mode,
                strict_types,
                profiling,
                diagnostics_format,
                all_errors,
                dump_core,
                dump_aether,
                dump_lir,
                dump_lir_llvm,
                use_core_to_llvm,
                emit_llvm,
                emit_binary,
                output_path.clone(),
            );
        }
        return;
    }

    match args[1].as_str() {
        "-h" | "--help" | "help" => {
            print_help();
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: flux run <file.flx>");
                return;
            }

            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            if test_mode {
                run_test_file(
                    &args[2],
                    roots_only,
                    enable_optimize,
                    enable_analyze,
                    max_errors,
                    &roots,
                    cache_dir.as_deref(),
                    test_filter.as_deref(),
                    strict_mode,
                    strict_types,
                    diagnostics_format,
                    all_errors,
                    use_core_to_llvm,
                );
            } else {
                run_file(
                    &args[2],
                    verbose,
                    leak_detector,
                    trace,
                    no_cache,
                    roots_only,
                    enable_optimize,
                    enable_analyze,
                    max_errors,
                    &roots,
                    cache_dir.as_deref(),
                    show_stats,
                    trace_aether,
                    strict_mode,
                    strict_types,
                    profiling,
                    diagnostics_format,
                    all_errors,
                    dump_core,
                    dump_aether,
                    dump_lir,
                    dump_lir_llvm,
                    use_core_to_llvm,
                    emit_llvm,
                    emit_binary,
                    output_path,
                );
            }
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Usage: flux tokens <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_tokens(&args[2]);
        }
        "bytecode" => {
            if args.len() < 3 {
                eprintln!("Usage: flux bytecode <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_bytecode(
                &args[2],
                enable_optimize,
                enable_analyze,
                max_errors,
                strict_mode,
                strict_types,
                diagnostics_format,
            );
        }
        "lint" => {
            if args.len() < 3 {
                eprintln!("Usage: flux lint <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            lint_file(&args[2], max_errors, diagnostics_format);
        }
        "fmt" => {
            if args.len() < 3 {
                eprintln!("Usage: flux fmt [--check] <file.flx>");
                return;
            }
            let check = args.iter().any(|arg| arg == "--check");
            let file = if check { &args[3] } else { &args[2] };
            if check && args.len() < 4 {
                eprintln!("Usage: flux fmt --check <file.flx>");
                return;
            }
            if !is_flx_file(file) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    file
                );
                return;
            }
            fmt_file(file, check);
        }
        "cache-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux cache-info <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_cache_info(&args[2], &roots, cache_dir.as_deref());
        }
        "module-cache-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux module-cache-info <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_module_cache_info(&args[2], &roots, cache_dir.as_deref());
        }
        "native-cache-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux native-cache-info <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_native_cache_info(&args[2], &roots, cache_dir.as_deref());
        }
        "clean" => {
            let entry = if args.len() >= 3 && is_flx_file(&args[2]) {
                Path::new(&args[2])
            } else {
                Path::new(".")
            };
            let layout = flux::cache_paths::resolve_cache_layout(entry, cache_dir.as_deref());
            let root = layout.root();
            if root.exists() {
                match std::fs::remove_dir_all(root) {
                    Ok(()) => println!("Removed cache: {}", root.display()),
                    Err(e) => eprintln!("Failed to remove cache {}: {e}", root.display()),
                }
            } else {
                println!("No cache found at {}", root.display());
            }
        }
        "interface-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux interface-info <file.flxi>");
                return;
            }
            if !args[2].ends_with(".flxi") {
                eprintln!(
                    "Error: expected a `.flxi` file, got `{}`. Pass a Flux interface file like `path/to/module.flxi`.",
                    args[2]
                );
                return;
            }
            show_interface_info_file(&args[2]);
        }
        "analyze-free-vars" | "free-vars" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-free-vars <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            analyze_free_vars(&args[2], max_errors, diagnostics_format);
        }
        "analyze-tail-calls" | "analyze-tails-calls" | "tail-calls" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-tail-calls <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            analyze_tail_calls(&args[2], max_errors, diagnostics_format);
        }
        "parity-check" => {
            let parity_args: Vec<String> = args[2..].to_vec();
            flux::parity::cli::run_parity_check(&parity_args);
        }
        _ => {
            eprintln!(
                "Error: unknown command or invalid input `{}`. Pass a `.flx` file or a valid subcommand.",
                args[1]
            );
        }
    }
}

fn print_help() {
    println!(
        "\
Flux CLI

Usage:
  flux <file.flx>
  flux run <file.flx>
  flux tokens <file.flx>
  flux bytecode <file.flx>
  flux lint <file.flx>
  flux fmt [--check] <file.flx>
  flux cache-info <file.flx>
  flux module-cache-info <file.flx>
  flux native-cache-info <file.flx>
  flux interface-info <file.flxi>
  flux clean [<file.flx>]
  flux analyze-free-vars <file.flx>
  flux analyze-tail-calls <file.flx>
  flux parity-check <file-or-dir> [--ways vm,llvm] [--root <path> ...]
  flux <file.flx> --root <path> [--root <path> ...]
  flux run <file.flx> --root <path> [--root <path> ...]

Flags:
  --verbose          Show cache status (hit/miss/store)
  --trace            Print VM instruction trace
  --trace-aether     Print Aether report plus backend/execution path, then run
  --test             Run test_* functions and report results
  --test-filter <s>  Only run tests whose names contain <s>
  --leak-detector    Print approximate allocation stats after run
  --no-cache         Disable bytecode cache for this run
  --cache-dir <dir>  Override cache root (default: nearest Cargo.toml target/flux, else .flux/cache)
  --optimize, -O     Enable AST optimizations (desugar + constant fold)
  --analyze, -A      Enable analysis passes (free vars + tail calls)
  --format <f>       Diagnostics format: text|json|json-compact (default: text)
  --max-errors <n>   Limit displayed errors (default: 50)
  --root <path>      Add a module root (can be repeated)
  --roots-only       Use only explicitly provided --root values
  --stats            Print execution analytics (parse/compile/execute times, module info)
  --prof             Print per-function profiling report (call counts, time, allocations)
  --strict           Enable strict type/effect boundary checks
  --all-errors       Show diagnostics from all phases (disable stage-aware filtering)
  --dump-core        Lower to Flux Core IR, print a readable dump, and exit
  --dump-core=debug  Lower to Flux Core IR, print a raw debug dump, and exit
  --dump-aether      Show Aether memory model report (per-function reuse/drop stats)
  --dump-aether=debug
                    Show detailed Aether debug report (borrow signatures, call modes, dup/drop, reuse)
  --native           Compile via Core IR → LLVM text IR → native binary (requires LLVM tools)
  --core-to-llvm     Alias for --native
  --emit-llvm        Emit LLVM IR text (.ll) to stdout (with --native)
  --emit-binary      Compile to native binary via opt + llc + cc (with --native)
  -o <path>          Output path for --emit-llvm or --emit-binary
  -h, --help         Show this help message

Optimization & Analysis:
  --optimize         Apply transformations (faster bytecode)
  --analyze          Collect analysis data (free vars, tail calls)
  -O -A              Both optimization and analysis
"
    );
}

#[allow(clippy::too_many_arguments)]
fn run_file(
    path: &str,
    verbose: bool,
    leak_detector: bool,
    trace: bool,
    no_cache: bool,
    roots_only: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    extra_roots: &[std::path::PathBuf],
    cache_dir: Option<&Path>,
    show_stats: bool,
    trace_aether: bool,
    strict_mode: bool,
    strict_types: bool,
    profiling: bool,
    diagnostics_format: DiagnosticOutputFormat,
    all_errors: bool,
    dump_core: CoreDumpMode,
    dump_aether: AetherDumpMode,
    dump_lir: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] dump_lir_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] use_core_to_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] emit_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] emit_binary: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] output_path: Option<String>,
) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let entry_path = Path::new(path);
            let cache_layout = cache_paths::resolve_cache_layout(entry_path, cache_dir);
            let strict_hash = hash_bytes(if strict_mode {
                b"strict=1"
            } else {
                b"strict=0"
            });

            let parse_start = Instant::now();
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            // --- Collect all diagnostics into a single pool ---
            let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
            let mut parse_warnings = parser.take_warnings();
            tag_diagnostics(&mut parse_warnings, DiagnosticPhase::Parse);
            for diag in &mut parse_warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }
            all_diagnostics.append(&mut parse_warnings);

            // Entry file parse errors: collect but do NOT exit early.
            let entry_has_errors = !parser.errors.is_empty();
            if entry_has_errors {
                tag_diagnostics(&mut parser.errors, DiagnosticPhase::Parse);
                for diag in &mut parser.errors {
                    if diag.file().is_none() {
                        diag.set_file(path.to_string());
                    }
                }
                all_diagnostics.append(&mut parser.errors);
            }

            // Auto-import Flow library modules (Proposal 0120/0121 Phase 4).
            // Dump/analysis surfaces should see the same enriched program that
            // normal compilation executes, otherwise `--dump-core` and related
            // commands become semantically inconsistent with real runs.
            // Only skip the injection for `--trace-aether`, which is intended
            // to show the direct execution path without extra dump-only noise.
            let mut program = program;
            if !trace_aether {
                inject_flow_prelude(&mut program, &mut parser, use_core_to_llvm);
            }

            let interner = parser.take_interner();
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);

            // --- Build module graph (always returns, may have diagnostics) ---
            let graph_result =
                ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
            let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;
            let mut graph_diags = graph_result.diagnostics;
            tag_diagnostics(&mut graph_diags, DiagnosticPhase::ModuleGraph);
            all_diagnostics.extend(graph_diags);

            // Track all failed modules (parse + validation failures from graph).
            let mut failed: HashSet<PathBuf> = graph_result.failed_modules;
            if entry_has_errors && let Ok(canon) = std::fs::canonicalize(entry_path) {
                failed.insert(canon);
            }

            let module_count = graph_result.graph.module_count();
            let is_multimodule = module_count > 1;
            let graph = graph_result.graph;

            // Warm the toolchain info cache before compile timing starts.
            #[cfg(feature = "core_to_llvm")]
            if verbose && (use_core_to_llvm || emit_binary) {
                let _ = flux::core_to_llvm::pipeline::toolchain_info();
            }

            // --- Compile valid modules, suppress cascade ---
            let compile_start = Instant::now();
            let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
            compiler.set_strict_mode(strict_mode);
            compiler.set_strict_types(strict_types);
            if profiling {
                compiler.set_profiling(true);
            }
            let entry_canonical = std::fs::canonicalize(entry_path).ok();
            let mut preloaded_interfaces: HashSet<PathBuf> = HashSet::new();
            let mut loaded_interfaces: HashMap<
                PathBuf,
                flux::types::module_interface::ModuleInterface,
            > = HashMap::new();
            let mut module_states: HashMap<PathBuf, ModuleBuildState> = HashMap::new();
            let module_cache = ModuleBytecodeCache::new(cache_layout.vm_dir());
            let allow_cached_module_bytecode = !use_core_to_llvm
                && !emit_llvm
                && !emit_binary
                && matches!(dump_core, CoreDumpMode::None)
                && dump_aether == AetherDumpMode::None
                && !dump_lir
                && !dump_lir_llvm
                && !trace_aether;

            if is_multimodule && allow_cached_module_bytecode && !no_cache {
                let build = match compile_vm_modules_parallel(
                    &graph,
                    entry_canonical.as_ref(),
                    &compiler.interner,
                    &cache_layout,
                    no_cache,
                    strict_mode,
                    strict_types,
                    enable_optimize,
                    enable_analyze,
                    verbose,
                    &mut all_diagnostics,
                ) {
                    Ok(build) => build,
                    Err(err) => {
                        eprintln!("parallel VM compilation failed: {err}");
                        std::process::exit(1);
                    }
                };

                let report = DiagnosticsAggregator::new(&all_diagnostics)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(true)
                    .with_max_errors(Some(max_errors))
                    .with_stage_filtering(!all_errors)
                    .report();
                if report.counts.errors > 0 {
                    emit_diagnostics(
                        &all_diagnostics,
                        Some(path),
                        Some(source.as_str()),
                        true,
                        max_errors,
                        diagnostics_format,
                        all_errors,
                        true,
                    );
                    std::process::exit(1);
                }
                if !all_diagnostics.is_empty() {
                    emit_diagnostics(
                        &all_diagnostics,
                        Some(path),
                        Some(source.as_str()),
                        true,
                        max_errors,
                        diagnostics_format,
                        all_errors,
                        true,
                    );
                }

                let compile_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
                let bytecode = build.bytecode;
                let globals_count = build.symbol_table.num_definitions;
                let functions_count = count_bytecode_functions(&bytecode.constants);
                let instruction_bytes = bytecode.instructions.len();

                eprintln!("[cfg→vm] Running via CFG → bytecode VM backend...");
                let mut vm = VM::new(bytecode);
                vm.set_trace(trace);
                let exec_start = Instant::now();
                if let Err(err) = vm.run() {
                    eprintln!("{}", err);
                    std::process::exit(1);
                }
                let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                if leak_detector {
                    print_leak_stats();
                }
                if show_stats {
                    print_stats(&RunStats {
                        parse_ms: Some(parse_ms),
                        compile_ms: Some(compile_ms),
                        compile_backend: Some("bytecode"),
                        execute_ms,
                        execute_backend: "vm",
                        cached: false,
                        module_count: Some(module_count),
                        cached_module_count: Some(build.cached_count),
                        compiled_module_count: Some(build.compiled_count),
                        source_lines: source.lines().count(),
                        globals_count: Some(globals_count),
                        functions_count: Some(functions_count),
                        instruction_bytes: Some(instruction_bytes),
                    });
                }
                return;
            }

            // Sort topo_order to compile Flow library modules first.
            // This ensures all modules can access Flow functions (map, filter, etc.)
            // without explicit imports — like Haskell's implicit Prelude.
            let mut ordered_nodes = graph.topo_order();
            ordered_nodes.sort_by_key(|node| {
                let is_flow = node.path.to_string_lossy().contains("lib/Flow/")
                    || node.path.to_string_lossy().contains("lib\\Flow\\");
                if is_flow { 0 } else { 1 }
            });

            let seq_total = ordered_nodes.len();
            let mut seq_completed = 0usize;

            for node in ordered_nodes {
                // Skip entry if it had parse errors (it is in topo_order but
                // should not be compiled).
                if entry_has_errors
                    && let Some(ref canon) = entry_canonical
                    && &node.path == canon
                {
                    continue;
                }

                // Cascade suppression: skip if any dependency failed.
                let failed_dep = node
                    .imports
                    .iter()
                    .find(|e| failed.contains(&e.target_path));
                if let Some(dep) = failed_dep {
                    failed.insert(node.path.clone());
                    let display = render_display_path(&node.path.to_string_lossy()).into_owned();
                    all_diagnostics.push(module_skipped_note(
                        display.clone(),
                        display,
                        dep.name.clone(),
                    ));
                    continue;
                }

                if !no_cache {
                    for dep in &node.imports {
                        if let Some(interface) = loaded_interfaces.get(&dep.target_path) {
                            if preloaded_interfaces.insert(dep.target_path.clone()) {
                                compiler.preload_module_interface(interface);
                                if verbose {
                                    eprintln!(
                                        "interface: hit {} [abi:{}]",
                                        interface.module_name,
                                        short_hash(&interface.interface_fingerprint)
                                    );
                                }
                            }
                            continue;
                        }
                        let Ok(dep_source) = std::fs::read_to_string(&dep.target_path) else {
                            if verbose {
                                eprintln!(
                                    "interface: miss {} (reason: source not readable)",
                                    dep.target_path.display()
                                );
                            }
                            continue;
                        };
                        let dep_is_flow_library =
                            dep.target_path.to_string_lossy().contains("lib/Flow/")
                                || dep.target_path.to_string_lossy().contains("lib\\Flow\\");
                        let dep_semantic_config_hash =
                            flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
                                !dep_is_flow_library && strict_mode,
                                enable_optimize,
                            );
                        match flux::bytecode::compiler::module_interface::load_valid_interface(
                            cache_layout.root(),
                            &dep.target_path,
                            &dep_source,
                            &dep_semantic_config_hash,
                        ) {
                            Ok(interface) => {
                                compiler.preload_module_interface(&interface);
                                preloaded_interfaces.insert(dep.target_path.clone());
                                if verbose {
                                    eprintln!(
                                        "interface: hit {} [abi:{}]",
                                        interface.module_name,
                                        short_hash(&interface.interface_fingerprint)
                                    );
                                }
                                loaded_interfaces.insert(dep.target_path.clone(), interface);
                            }
                            Err(err) if verbose => {
                                eprintln!(
                                    "interface: miss {} (reason: {})",
                                    dep.target_path.display(),
                                    err.message()
                                );
                            }
                            Err(_) => {}
                        }
                    }
                }

                compiler.set_file_path(node.path.to_string_lossy().to_string());
                let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
                let is_flow_library = node.path.to_string_lossy().contains("lib/Flow/")
                    || node.path.to_string_lossy().contains("lib\\Flow\\");
                let module_semantic_config_hash =
                    flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
                        !is_flow_library && strict_mode,
                        enable_optimize,
                    );
                let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
                let module_source_hash = hash_bytes(module_source.as_bytes());
                let module_strict_hash = if is_flow_library {
                    hash_bytes(b"strict=0")
                } else {
                    strict_hash
                };
                let module_cache_key = hash_cache_key(&module_source_hash, &module_strict_hash);
                let old_interface = if !no_cache {
                    flux::bytecode::compiler::module_interface::load_cached_interface(
                        cache_layout.root(),
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
                let current_interface = if !no_cache {
                    match flux::bytecode::compiler::module_interface::load_valid_interface(
                        cache_layout.root(),
                        &node.path,
                        &module_source,
                        &module_semantic_config_hash,
                    ) {
                        Ok(interface) => Some(interface),
                        Err(err) => {
                            // Entry modules have no `module` declaration so
                            // they never produce an interface — don't log
                            // the expected "not found" miss.
                            if verbose && !is_entry_module {
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

                // Skip semantic compilation if interface is valid and no
                // dependency changed. For VM, also requires a valid .fxm
                // bytecode cache. For LLVM, the interface alone is sufficient
                // since native artifacts are cached separately.
                let can_skip_semantic =
                    !no_cache && !must_rebuild_due_to_dependency && current_interface.is_some();

                let has_vm_cache = can_skip_semantic
                    && allow_cached_module_bytecode
                    && module_cache
                        .load(
                            &node.path,
                            &module_cache_key,
                            env!("CARGO_PKG_VERSION"),
                            cache_layout.root(),
                        )
                        .is_some();

                // Entry modules have no interface, but can still skip via
                // bytecode-only cache when the source hash hasn't changed.
                let has_vm_cache_entry = !no_cache
                    && is_entry_module
                    && !must_rebuild_due_to_dependency
                    && allow_cached_module_bytecode
                    && module_cache
                        .load(
                            &node.path,
                            &module_cache_key,
                            env!("CARGO_PKG_VERSION"),
                            cache_layout.root(),
                        )
                        .is_some();

                // LLVM path: skip if interface is valid (native artifacts
                // are validated separately in compile_native_modules_parallel).
                let skip_for_llvm = can_skip_semantic && use_core_to_llvm;

                // LLVM entry modules have no interface but can still skip
                // semantic compilation when their compilation marker file
                // exists. Uses a dedicated `.fxs` (flux-semantic) marker
                // instead of `.fxm` to avoid cross-backend cache pollution —
                // a `.fxm` written during an LLVM session contains global
                // indices incompatible with the VM backend.
                let llvm_entry_marker = cache_layout.vm_dir().join(
                    cache_paths::cache_key_filename(&node.path, &module_cache_key, "fxs"),
                );
                let skip_llvm_entry =
                    use_core_to_llvm && is_entry_module && !no_cache && llvm_entry_marker.exists();

                if has_vm_cache || skip_for_llvm || has_vm_cache_entry || skip_llvm_entry {
                    if let Some(interface) = current_interface.as_ref() {
                        compiler.preload_module_interface(interface);
                        loaded_interfaces.insert(node.path.clone(), interface.clone());
                        preloaded_interfaces.insert(node.path.clone());
                    }
                    if has_vm_cache || has_vm_cache_entry {
                        // Re-load for hydration (load() was consumed above for the check).
                        if let Some(cached) = module_cache.load(
                            &node.path,
                            &module_cache_key,
                            env!("CARGO_PKG_VERSION"),
                            cache_layout.root(),
                        ) {
                            compiler.hydrate_cached_module_bytecode(&cached);
                        }
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
                } else if !no_cache && !must_rebuild_due_to_dependency && verbose {
                    if allow_cached_module_bytecode {
                        let reason = module_cache
                            .load_failure_reason(
                                &node.path,
                                &module_cache_key,
                                env!("CARGO_PKG_VERSION"),
                                cache_layout.root(),
                            )
                            .unwrap_or_else(|| "not eligible".to_string());
                        eprintln!("module-cache: miss (reason: {reason})");
                    } else if current_interface.is_none() && verbose && !is_entry_module {
                        eprintln!(
                            "interface: miss {} (no valid interface)",
                            node.path.display()
                        );
                    }
                } else if !no_cache && must_rebuild_due_to_dependency && verbose {
                    eprintln!("module-cache: miss (reason: dependency interface changed)");
                }
                compiler.set_strict_require_main(is_entry_module);
                // Disable strict mode for Flow library modules — they use
                // polymorphic signatures that strict mode can't validate yet.
                if is_flow_library {
                    compiler.set_strict_mode(false);
                    compiler.set_strict_types(false);
                }
                let module_snapshot = compiler.module_cache_snapshot();
                let compile_result =
                    compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
                if is_flow_library {
                    compiler.set_strict_mode(strict_mode);
                    compiler.set_strict_types(strict_types);
                }
                let mut compiler_warnings = compiler.take_warnings();
                tag_diagnostics(&mut compiler_warnings, DiagnosticPhase::Validation);
                for diag in &mut compiler_warnings {
                    if diag.file().is_none() {
                        diag.set_file(node.path.to_string_lossy().to_string());
                    }
                }
                all_diagnostics.append(&mut compiler_warnings);

                if let Err(mut diags) = compile_result {
                    failed.insert(node.path.clone());
                    tag_diagnostics(&mut diags, DiagnosticPhase::TypeCheck);
                    for diag in &mut diags {
                        if diag.file().is_none() {
                            diag.set_file(node.path.to_string_lossy().to_string());
                        }
                    }
                    all_diagnostics.append(&mut diags);
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

                // Store bytecode cache for VM runs.
                if !no_cache && allow_cached_module_bytecode {
                    let cached_module = compiler.build_cached_module_bytecode(module_snapshot);
                    if let Err(e) = module_cache.store(
                        &node.path,
                        &module_cache_key,
                        env!("CARGO_PKG_VERSION"),
                        &cached_module,
                        &module_deps,
                    ) && verbose
                    {
                        eprintln!(
                            "warning: could not write module cache file for {}: {e}",
                            node.path.display()
                        );
                    }
                }
                // For LLVM entry modules, write a lightweight semantic
                // marker (.fxs) so the next LLVM run can skip re-compilation.
                // This is separate from .fxm to avoid cross-backend pollution.
                if !no_cache && use_core_to_llvm && is_entry_module {
                    let marker_path = cache_layout.vm_dir().join(cache_paths::cache_key_filename(
                        &node.path,
                        &module_cache_key,
                        "fxs",
                    ));
                    if let Some(parent) = marker_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&marker_path, b"");
                }

                // Save module interface (.flxi) when available.
                // Entry modules have no `module` declaration, so
                // `extract_module_name_and_sym` returns None — the block
                // naturally won't execute for them.
                if !no_cache
                    && let Some((module_name, module_sym)) =
                        extract_module_name_and_sym(&node.program, &compiler.interner)
                {
                    match compiler.lower_aether_report_program(&node.program, enable_optimize) {
                        Ok(core) => {
                            let dependency_fingerprints = node
                                .imports
                                .iter()
                                .filter_map(|dep| {
                                    loaded_interfaces.get(&dep.target_path).map(|interface| {
                                        flux::types::module_interface::DependencyFingerprint {
                                            module_name: interface.module_name.clone(),
                                            source_path: dep
                                                .target_path
                                                .to_string_lossy()
                                                .to_string(),
                                            interface_fingerprint: interface
                                                .interface_fingerprint
                                                .clone(),
                                        }
                                    })
                                })
                                .collect();
                            let interface =
                                flux::bytecode::compiler::module_interface::build_interface(
                                    &module_name,
                                    module_sym,
                                    &module_source_hash,
                                    &module_semantic_config_hash,
                                    &core,
                                    compiler.cached_member_schemes(),
                                    &compiler.module_function_visibility,
                                    dependency_fingerprints,
                                    &compiler.interner,
                                );
                            compiler.preload_module_interface(&interface);
                            loaded_interfaces.insert(node.path.clone(), interface.clone());
                            preloaded_interfaces.insert(node.path.clone());
                            let interface_changed = old_interface.as_ref().is_none_or(|old| {
                                flux::bytecode::compiler::module_interface::module_interface_changed(
                                    old, &interface,
                                )
                            });
                            if verbose && interface_changed {
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
                            let iface_path =
                                flux::bytecode::compiler::module_interface::interface_path(
                                    cache_layout.root(),
                                    &node.path,
                                );
                            if let Err(e) =
                                flux::bytecode::compiler::module_interface::save_interface(
                                    &iface_path,
                                    &interface,
                                )
                            {
                                if verbose {
                                    eprintln!(
                                        "warning: could not write interface file {}: {e}",
                                        iface_path.display()
                                    );
                                }
                            } else if verbose {
                                eprintln!(
                                    "interface: stored {} [abi:{}]",
                                    interface.module_name,
                                    short_hash(&interface.interface_fingerprint)
                                );
                            }
                        }
                        Err(e) if verbose => {
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

            // --- One unified report ---
            if !all_diagnostics.is_empty() {
                let report = DiagnosticsAggregator::new(&all_diagnostics)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(is_multimodule)
                    .with_max_errors(Some(max_errors))
                    .with_stage_filtering(!all_errors)
                    .report();
                if report.counts.errors > 0 {
                    emit_diagnostics(
                        &all_diagnostics,
                        Some(path),
                        Some(source.as_str()),
                        is_multimodule,
                        max_errors,
                        diagnostics_format,
                        all_errors,
                        true,
                    );
                    std::process::exit(1);
                }
                emit_diagnostics(
                    &all_diagnostics,
                    Some(path),
                    Some(source.as_str()),
                    is_multimodule,
                    max_errors,
                    diagnostics_format,
                    all_errors,
                    true,
                );
            }

            // Build merged program from all modules for dump/analysis surfaces that
            // need whole-program visibility.
            let merged_program = if is_multimodule
                && (dump_aether != AetherDumpMode::None
                    || !matches!(dump_core, CoreDumpMode::None)
                    || dump_lir
                    || dump_lir_llvm)
            {
                let mut merged = Program::new();
                for node in graph.topo_order() {
                    merged.statements.extend(node.program.statements.clone());
                }
                merged
            } else {
                program.clone()
            };

            if dump_aether != AetherDumpMode::None {
                match compiler.dump_aether_report(
                    &merged_program,
                    enable_optimize,
                    dump_aether == AetherDumpMode::Debug,
                ) {
                    Ok(report) => println!("{report}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            if !matches!(dump_core, CoreDumpMode::None) {
                let dumped = compiler.dump_core_with_opts(
                    &merged_program,
                    enable_optimize,
                    match dump_core {
                        CoreDumpMode::Readable => flux::core::display::CoreDisplayMode::Readable,
                        CoreDumpMode::Debug => flux::core::display::CoreDisplayMode::Debug,
                        CoreDumpMode::None => unreachable!("checked above"),
                    },
                );
                match dumped {
                    Ok(dumped) => println!("{dumped}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            if dump_lir {
                let dumped = compiler.dump_lir(&merged_program, enable_optimize);
                match dumped {
                    Ok(dumped) => println!("{dumped}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            // --- LIR → LLVM IR dump (Proposal 0132 Phase 7) ---
            #[cfg(feature = "core_to_llvm")]
            if dump_lir_llvm {
                match compiler.dump_lir_llvm(&merged_program, enable_optimize) {
                    Ok(ir_text) => println!("{ir_text}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            // --- LIR → LLVM native execution path (Proposal 0132) ---
            #[cfg(feature = "core_to_llvm")]
            if use_core_to_llvm || emit_llvm || emit_binary {
                if emit_llvm {
                    // Keep merged LLVM IR emission as the debug surface in Phase 5.
                    let mut native_program = Program::new();
                    for node in graph.topo_order() {
                        native_program
                            .statements
                            .extend(node.program.statements.clone());
                    }
                    compiler.infer_expr_types_for_program(&native_program);
                    eprintln!("[lir→llvm] Compiling via LIR → LLVM native backend...");
                    let mut llvm_module =
                        match compiler.lower_to_lir_llvm_module(&native_program, enable_optimize) {
                            Ok(m) => m,
                            Err(diag) => {
                                emit_diagnostics(
                                    &[diag],
                                    Some(path),
                                    Some(source.as_str()),
                                    is_multimodule,
                                    max_errors,
                                    diagnostics_format,
                                    all_errors,
                                    true,
                                );
                                std::process::exit(1);
                            }
                        };
                    llvm_module.target_triple = Some(flux::core_to_llvm::target::host_triple());
                    llvm_module.data_layout = flux::core_to_llvm::target::host_data_layout();
                    let ll_text = flux::core_to_llvm::render_module(&llvm_module);
                    if let Some(ref out) = output_path {
                        if let Err(e) = std::fs::write(out, &ll_text) {
                            eprintln!("Failed to write LLVM IR: {e}");
                            std::process::exit(1);
                        }
                        println!("Emitted LLVM IR: {out}");
                    } else {
                        println!("{ll_text}");
                    }
                    return;
                }

                let frontend_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
                let runtime_lib_dir = locate_runtime_lib_dir();
                if verbose {
                    eprintln!(
                        "[lir→llvm] toolchain: {}",
                        flux::core_to_llvm::pipeline::toolchain_info()
                    );
                }
                eprintln!("[lir→llvm] Compiling via per-module LLVM native backend...");
                let native_modules_start = Instant::now();
                let (mut object_paths, any_native_recompiled) =
                    match compile_native_modules_parallel(
                        &graph,
                        &cache_layout,
                        no_cache,
                        strict_mode,
                        strict_types,
                        enable_optimize,
                        enable_analyze,
                        verbose,
                        &compiler.interner,
                        &mut all_diagnostics,
                    ) {
                        Ok(paths) => paths,
                        Err(err) => {
                            emit_diagnostics(
                                &all_diagnostics,
                                Some(path),
                                Some(source.as_str()),
                                is_multimodule,
                                max_errors,
                                diagnostics_format,
                                all_errors,
                                true,
                            );
                            eprintln!("core_to_llvm module pipeline failed: {err}");
                            std::process::exit(1);
                        }
                    };
                let native_modules_ms = native_modules_start.elapsed().as_secs_f64() * 1000.0;
                let support_start = Instant::now();
                let program_has_main = graph.topo_order().iter().any(|node| {
                    node.program.statements.iter().any(|stmt| {
                        matches!(
                            stmt,
                            flux::syntax::statement::Statement::Function { name, .. }
                                if compiler.interner.resolve(*name) == "main"
                        )
                    })
                });
                match compile_native_support_object(
                    &cache_layout,
                    no_cache,
                    enable_optimize,
                    program_has_main,
                ) {
                    Ok(support_object) => {
                        object_paths.insert(0, support_object);
                    }
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(1);
                    }
                }
                let support_ms = support_start.elapsed().as_secs_f64() * 1000.0;

                // Pre-link Flow.* standard library .o files into libflux_std.a
                // so the linker processes fewer inputs (GHC-style).
                let archive_start = Instant::now();
                // Build a set of object paths that belong to Flow.* library modules
                // by matching source paths from the module graph.
                let flow_object_paths: HashSet<PathBuf> = graph
                    .topo_order()
                    .iter()
                    .filter(|node| is_flow_library_path(&node.path))
                    .filter_map(|node| {
                        // Find the corresponding .o in object_paths by matching the module name
                        // embedded in the object filename (e.g., "Array-a732...o" for Flow.Array).
                        let module_stem = node.path.file_stem()?.to_str()?;
                        object_paths
                            .iter()
                            .find(|obj| {
                                obj.file_name()
                                    .and_then(|f| f.to_str())
                                    .is_some_and(|f| f.starts_with(&format!("{module_stem}-")))
                            })
                            .cloned()
                    })
                    .collect();
                let std_lib_objects: Vec<PathBuf> = object_paths
                    .iter()
                    .filter(|p| flow_object_paths.contains(*p))
                    .cloned()
                    .collect();

                let link_paths = if std_lib_objects.len() >= 2 && !no_cache {
                    let archive_name = if enable_optimize {
                        "libflux_std_O2.a"
                    } else {
                        "libflux_std_O0.a"
                    };
                    let archive_path = cache_layout.native_dir().join(archive_name);
                    let need_rebuild = !flux::core_to_llvm::pipeline::archive_is_up_to_date(
                        &std_lib_objects,
                        &archive_path,
                    );
                    if need_rebuild {
                        if let Err(err) = flux::core_to_llvm::pipeline::create_archive(
                            &std_lib_objects,
                            &archive_path,
                        ) {
                            eprintln!(
                                "warning: failed to create libflux_std.a: {err}, falling back to individual .o files"
                            );
                            object_paths.clone()
                        } else {
                            // Replace individual Flow.* .o files with the archive.
                            let std_set: HashSet<PathBuf> =
                                std_lib_objects.iter().cloned().collect();
                            let mut paths: Vec<PathBuf> = object_paths
                                .iter()
                                .filter(|p| !std_set.contains(*p))
                                .cloned()
                                .collect();
                            paths.push(archive_path);
                            paths
                        }
                    } else {
                        // Archive is up to date, use it directly.
                        let std_set: HashSet<PathBuf> = std_lib_objects.iter().cloned().collect();
                        let mut paths: Vec<PathBuf> = object_paths
                            .iter()
                            .filter(|p| !std_set.contains(*p))
                            .cloned()
                            .collect();
                        paths.push(archive_path);
                        paths
                    }
                } else {
                    object_paths.clone()
                };
                let archive_ms = archive_start.elapsed().as_secs_f64() * 1000.0;

                if !all_diagnostics.is_empty() {
                    let report = DiagnosticsAggregator::new(&all_diagnostics)
                        .with_default_source(path, source.as_str())
                        .with_file_headers(true)
                        .with_max_errors(Some(max_errors))
                        .with_stage_filtering(!all_errors)
                        .report();
                    if report.counts.errors > 0 {
                        emit_diagnostics(
                            &all_diagnostics,
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                    emit_diagnostics(
                        &all_diagnostics,
                        Some(path),
                        Some(source.as_str()),
                        is_multimodule,
                        max_errors,
                        diagnostics_format,
                        all_errors,
                        true,
                    );
                }

                let out = output_path
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        if emit_binary {
                            std::path::PathBuf::from(path.strip_suffix(".flx").unwrap_or(path))
                        } else if !no_cache {
                            // Cache the binary so we can skip relinking on re-runs.
                            let bin_name = std::path::Path::new(path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("program");
                            cache_layout.native_dir().join(format!("{bin_name}.bin"))
                        } else {
                            native_temp_dir().join("program")
                        }
                    });

                // Skip relinking if no module was recompiled and the cached binary exists.
                let link_start = Instant::now();
                let binary_up_to_date =
                    !no_cache && !emit_binary && !any_native_recompiled && out.exists();

                if binary_up_to_date {
                    if verbose {
                        eprintln!("[lir→llvm] binary up-to-date, skipping link");
                    }
                } else if let Err(e) = flux::core_to_llvm::pipeline::link_objects(
                    &link_paths,
                    &out,
                    runtime_lib_dir.as_deref(),
                ) {
                    eprintln!("core_to_llvm linker failed: {e}");
                    std::process::exit(1);
                }
                let link_ms = link_start.elapsed().as_secs_f64() * 1000.0;
                if verbose {
                    eprintln!(
                        "[lir→llvm] frontend: {frontend_ms:.1}ms, modules: {native_modules_ms:.1}ms, support: {support_ms:.1}ms, archive: {archive_ms:.1}ms, link: {link_ms:.1}ms"
                    );
                }

                if emit_binary {
                    println!("Emitted binary: {}", out.display());
                    return;
                }

                let exec_start = Instant::now();
                match std::process::Command::new(&out).status() {
                    Ok(status) => {
                        let exit_code = status.code().unwrap_or(1);
                        let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                        if show_stats {
                            let compile_ms =
                                compile_start.elapsed().as_secs_f64() * 1000.0 - execute_ms;
                            let total_source_lines: usize = graph
                                .topo_order()
                                .iter()
                                .map(|node| {
                                    std::fs::read_to_string(&node.path)
                                        .map(|s| s.lines().count())
                                        .unwrap_or(0)
                                })
                                .sum();
                            print_stats(&RunStats {
                                parse_ms: Some(parse_ms),
                                compile_ms: Some(compile_ms),
                                compile_backend: Some("llvm"),
                                execute_ms,
                                execute_backend: "native",
                                cached: false,
                                module_count: Some(module_count),
                                cached_module_count: None,
                                compiled_module_count: None,
                                source_lines: total_source_lines,
                                globals_count: None,
                                functions_count: None,
                                instruction_bytes: None,
                            });
                        }
                        // Only clean up temp binaries; keep cached ones for relink skip.
                        if no_cache || emit_binary {
                            let _ = std::fs::remove_file(&out);
                        }
                        if exit_code != 0 {
                            std::process::exit(exit_code);
                        }
                    }
                    Err(e) => {
                        eprintln!("core_to_llvm execution failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            let compile_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
            let bytecode = compiler.bytecode();
            let globals_count = compiler.symbol_table.num_definitions;
            let functions_count = count_bytecode_functions(&bytecode.constants);
            let instruction_bytes = bytecode.instructions.len();
            if trace_aether {
                match compiler.render_aether_report(&program, enable_optimize, false) {
                    Ok(report) => print_aether_trace(
                        path,
                        TraceBackend::Vm,
                        "AST -> Core -> CFG -> bytecode -> VM",
                        Some("disabled"),
                        enable_optimize,
                        enable_analyze,
                        strict_mode,
                        Some(module_count),
                        &report,
                    ),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
            }

            eprintln!("[cfg→vm] Running via CFG → bytecode VM backend...");
            let mut vm = VM::new(bytecode);
            vm.set_trace(trace);
            if profiling {
                vm.set_profiling(true, compiler.cost_centre_infos.clone());
            }
            let exec_start = Instant::now();
            if let Err(err) = vm.run() {
                eprintln!("{}", err);
                std::process::exit(1);
            }
            let execute_ns = exec_start.elapsed().as_nanos() as u64;
            let execute_ms = execute_ns as f64 / 1_000_000.0;
            if profiling {
                vm.print_profile_report(execute_ns);
            }
            if leak_detector {
                print_leak_stats();
            }
            if show_stats {
                print_stats(&RunStats {
                    parse_ms: Some(parse_ms),
                    compile_ms: Some(compile_ms),
                    compile_backend: Some("bytecode"),
                    execute_ms,
                    execute_backend: "vm",
                    cached: false,
                    module_count: Some(module_count),
                    cached_module_count: None,
                    compiled_module_count: None,
                    source_lines: source.lines().count(),
                    globals_count: Some(globals_count),
                    functions_count: Some(functions_count),
                    instruction_bytes: Some(instruction_bytes),
                });
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

// ─── Test Runner ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_test_file(
    path: &str,
    roots_only: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    extra_roots: &[std::path::PathBuf],
    _cache_dir: Option<&Path>,
    test_filter: Option<&str>,
    strict_mode: bool,
    strict_types: bool,
    diagnostics_format: DiagnosticOutputFormat,
    all_errors: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] use_core_to_llvm: bool,
) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, roots_only);

    // --- Parse ---
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let mut program = parser.parse_program();

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_warnings = parser.take_warnings();
    tag_diagnostics(&mut parse_warnings, DiagnosticPhase::Parse);
    for diag in &mut parse_warnings {
        if diag.file().is_none() {
            diag.set_file(path.to_string());
        }
    }
    all_diagnostics.append(&mut parse_warnings);

    if !parser.errors.is_empty() {
        tag_diagnostics(&mut parser.errors, DiagnosticPhase::Parse);
        for diag in &mut parser.errors {
            if diag.file().is_none() {
                diag.set_file(path.to_string());
            }
        }
        emit_diagnostics(
            &parser.errors,
            Some(path),
            Some(source.as_str()),
            false,
            max_errors,
            diagnostics_format,
            all_errors,
            true,
        );
        std::process::exit(1);
    }

    // Auto-import Flow library for test mode too.
    inject_flow_prelude(&mut program, &mut parser, use_core_to_llvm);

    let interner = parser.take_interner();

    // --- Build module graph ---
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    let mut graph_diags = graph_result.diagnostics;
    tag_diagnostics(&mut graph_diags, DiagnosticPhase::ModuleGraph);
    all_diagnostics.extend(graph_diags);

    let failed = graph_result.failed_modules;
    let module_count = graph_result.graph.module_count();
    let is_multimodule = module_count > 1;
    let graph = graph_result.graph;

    // --- Compile ---
    let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
    compiler.set_strict_mode(strict_mode);
    compiler.set_strict_types(strict_types);
    let entry_canonical = std::fs::canonicalize(entry_path).ok();

    // Sort topo_order to compile Flow library modules first.
    // This ensures all modules can access Flow functions (map, filter, etc.)
    // without explicit imports — like Haskell's implicit Prelude.
    let mut ordered_nodes = graph.topo_order();
    ordered_nodes.sort_by_key(|node| {
        let is_flow = node.path.to_string_lossy().contains("lib/Flow/")
            || node.path.to_string_lossy().contains("lib\\Flow\\");
        if is_flow { 0 } else { 1 }
    });

    for node in ordered_nodes {
        if node.imports.iter().any(|e| failed.contains(&e.target_path)) {
            continue;
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
        let is_flow_library = node.path.to_string_lossy().contains("lib/Flow/")
            || node.path.to_string_lossy().contains("lib\\Flow\\");
        compiler.set_strict_require_main(is_entry_module);
        if is_flow_library {
            compiler.set_strict_mode(false);
            compiler.set_strict_types(false);
        }
        let compile_result =
            compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
        if is_flow_library {
            compiler.set_strict_mode(strict_mode);
            compiler.set_strict_types(strict_types);
        }
        let mut compiler_warnings = compiler.take_warnings();
        tag_diagnostics(&mut compiler_warnings, DiagnosticPhase::Validation);
        for diag in &mut compiler_warnings {
            if diag.file().is_none() {
                diag.set_file(node.path.to_string_lossy().to_string());
            }
        }
        all_diagnostics.append(&mut compiler_warnings);

        if let Err(mut diags) = compile_result {
            tag_diagnostics(&mut diags, DiagnosticPhase::TypeCheck);
            for diag in &mut diags {
                if diag.file().is_none() {
                    diag.set_file(node.path.to_string_lossy().to_string());
                }
            }
            all_diagnostics.append(&mut diags);
        }
    }

    if !all_diagnostics.is_empty() {
        let report = DiagnosticsAggregator::new(&all_diagnostics)
            .with_default_source(path, source.as_str())
            .with_file_headers(is_multimodule)
            .with_max_errors(Some(max_errors))
            .with_stage_filtering(!all_errors)
            .report();
        if report.counts.errors > 0 {
            emit_diagnostics(
                &all_diagnostics,
                Some(path),
                Some(source.as_str()),
                is_multimodule,
                max_errors,
                diagnostics_format,
                all_errors,
                true,
            );
            std::process::exit(1);
        }
        emit_diagnostics(
            &all_diagnostics,
            Some(path),
            Some(source.as_str()),
            is_multimodule,
            max_errors,
            diagnostics_format,
            all_errors,
            true,
        );
    }

    // --- Collect test functions ---
    let mut tests = collect_test_functions(&compiler.symbol_table, &compiler.interner);
    if let Some(filter) = test_filter {
        tests.retain(|(name, _)| name.contains(filter));
    }

    if tests.is_empty() {
        let file_name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        println!("Running tests in {}\n", file_name);
        if let Some(filter) = test_filter {
            println!("No test functions found matching filter `{}`.", filter);
        } else {
            println!("No test functions found (define functions named `test_*`).");
        }
        return;
    }

    // --- Run tests ---
    let file_name = entry_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    #[cfg(feature = "native")]
    let all_passed = if use_core_to_llvm {
        run_tests_native(
            file_name,
            path,
            &source,
            &roots,
            &tests,
            enable_optimize,
            strict_mode,
        )
    } else {
        let bytecode = compiler.bytecode();
        let mut vm = VM::new(bytecode);
        if let Err(err) = vm.run() {
            eprintln!("Error during test setup: {}", err);
            std::process::exit(1);
        }

        let results = run_tests(&mut vm, tests);
        print_test_report(file_name, &results)
    };

    #[cfg(not(feature = "native"))]
    let all_passed = {
        let bytecode = compiler.bytecode();
        let mut vm = VM::new(bytecode);
        if let Err(err) = vm.run() {
            eprintln!("Error during test setup: {}", err);
            std::process::exit(1);
        }

        let results = run_tests(&mut vm, tests);
        print_test_report(file_name, &results)
    };

    if !all_passed {
        std::process::exit(1);
    }
}

#[cfg(feature = "native")]
fn run_tests_native(
    file_name: &str,
    source_path: &str,
    source: &str,
    roots: &[PathBuf],
    tests: &[(String, usize)],
    enable_optimize: bool,
    strict_mode: bool,
) -> bool {
    use flux::bytecode::vm::test_runner::{TestOutcome, TestResult, print_test_report};
    use std::fs;
    use std::process::Command;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Failed to locate current executable for native test mode: {e}");
        std::process::exit(1);
    });

    let mut results = Vec::new();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    for (idx, (name, _)) in tests.iter().enumerate() {
        let harness_path = std::env::temp_dir().join(format!(
            "flux_native_test_{}_{}_{}.flx",
            std::process::id(),
            unique,
            idx
        ));
        let harness_source = format!("{source}\n\nfn main() {{ {name}(); }}\n");
        if let Err(e) = fs::write(&harness_path, harness_source) {
            eprintln!(
                "Failed to write native test harness {}: {e}",
                harness_path.display()
            );
            std::process::exit(1);
        }

        let start = Instant::now();
        let mut cmd = Command::new(&exe);
        cmd.arg("--native").arg("--no-cache");
        if enable_optimize {
            cmd.arg("--optimize");
        }
        if strict_mode {
            cmd.arg("--strict");
        }
        for root in roots {
            cmd.arg("--root").arg(root);
        }
        cmd.arg(&harness_path);
        cmd.env("NO_COLOR", "1");
        let output = cmd.output();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        let outcome = match output {
            Ok(output) if output.status.success() => TestOutcome::Pass,
            Ok(output) => {
                let mut text = String::new();
                text.push_str(&String::from_utf8_lossy(&output.stdout));
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                TestOutcome::Fail(text.trim().to_string())
            }
            Err(err) => TestOutcome::Fail(format!(
                "failed to run native test harness for {} (from {}): {}",
                name, source_path, err
            )),
        };

        let _ = fs::remove_file(&harness_path);
        results.push(TestResult {
            name: name.clone(),
            elapsed_ms,
            outcome,
        });
    }

    print_test_report(file_name, &results)
}

// ─── Analytics ───────────────────────────────────────────────────────────────

struct RunStats {
    parse_ms: Option<f64>,
    compile_ms: Option<f64>,
    compile_backend: Option<&'static str>,
    execute_ms: f64,
    execute_backend: &'static str,
    cached: bool,
    module_count: Option<usize>,
    cached_module_count: Option<usize>,
    compiled_module_count: Option<usize>,
    source_lines: usize,
    globals_count: Option<usize>,
    functions_count: Option<usize>,
    instruction_bytes: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn print_aether_trace(
    path: &str,
    backend: TraceBackend,
    pipeline: &str,
    cache: Option<&str>,
    optimize: bool,
    analyze: bool,
    strict: bool,
    module_count: Option<usize>,
    report: &str,
) {
    let backend_name = match backend {
        TraceBackend::Vm => "vm",
    };

    eprintln!();
    eprintln!("── Aether Trace ──");
    eprintln!("file: {}", path);
    eprintln!("backend: {}", backend_name);
    eprintln!("pipeline: {}", pipeline);
    if let Some(cache_mode) = cache {
        eprintln!("cache: {}", cache_mode);
    }
    eprintln!("optimize: {}", if optimize { "on" } else { "off" });
    eprintln!("analyze: {}", if analyze { "on" } else { "off" });
    eprintln!("strict: {}", if strict { "on" } else { "off" });
    if let Some(count) = module_count {
        eprintln!("modules: {}", count);
    }
    eprintln!("────────────────────────");
    eprintln!("{report}");
}

fn count_bytecode_functions(constants: &[flux::runtime::value::Value]) -> usize {
    use flux::runtime::value::Value;
    constants
        .iter()
        .filter(|v| matches!(v, Value::Function(_)))
        .count()
}

fn print_stats(stats: &RunStats) {
    let total_ms =
        stats.parse_ms.unwrap_or(0.0) + stats.compile_ms.unwrap_or(0.0) + stats.execute_ms;

    let w = 46usize;
    eprintln!();
    eprintln!("  ── Flux Analytics {}", "─".repeat(w - 19));

    if let Some(ms) = stats.parse_ms {
        eprintln!("  {:<20} {:>8.2} ms", "parse", ms);
    }

    if stats.cached {
        eprintln!("  {:<20} {:>12}", "compile", "(cached)");
    } else if let Some(ms) = stats.compile_ms {
        eprintln!(
            "  {:<20} {:>8.2} ms  [{}]",
            "compile",
            ms,
            stats.compile_backend.unwrap_or("unknown")
        );
    }

    eprintln!(
        "  {:<20} {:>8.2} ms  [{}]",
        "execute", stats.execute_ms, stats.execute_backend
    );
    eprintln!("  {:<20} {:>8.2} ms", "total", total_ms);
    eprintln!();

    if let Some(n) = stats.module_count {
        match (stats.cached_module_count, stats.compiled_module_count) {
            (Some(cached), Some(compiled)) if cached > 0 => {
                eprintln!(
                    "  {:<20} {:>8}  ({} cached, {} compiled)",
                    "modules", n, cached, compiled
                );
            }
            _ => eprintln!("  {:<20} {:>8}", "modules", n),
        }
    }
    eprintln!("  {:<20} {:>8}", "source lines", stats.source_lines);
    if let Some(n) = stats.globals_count {
        eprintln!("  {:<20} {:>8}", "globals", n);
    }
    if let Some(n) = stats.functions_count {
        eprintln!("  {:<20} {:>8}", "functions", n);
    }
    if let Some(n) = stats.instruction_bytes {
        eprintln!("  {:<20} {:>8} bytes", "instructions", n);
    }
    eprintln!("  {}", "─".repeat(w - 2));
}

fn print_leak_stats() {
    let stats = flux::runtime::leak_detector::snapshot();
    println!(
        "\nLeak stats (approx):\n  compiled_functions: {}\n  closures: {}\n  arrays: {}\n  hashes: {}\n  somes: {}",
        stats.compiled_functions, stats.closures, stats.arrays, stats.hashes, stats.somes
    );
}

/// Locate the Flux C runtime library directory (`runtime/c/`).
///
/// Searches relative to the running executable and common development paths.
/// Returns `None` if not found (linker will search system paths).
#[cfg(feature = "native")]
fn locate_runtime_lib_dir() -> Option<std::path::PathBuf> {
    // Find the runtime/c source directory relative to the executable or cwd.
    let candidates = {
        let mut v = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            let mut dir = exe.parent().map(Path::to_path_buf);
            for _ in 0..5 {
                if let Some(ref d) = dir {
                    v.push(d.join("runtime").join("c"));
                    dir = d.parent().map(Path::to_path_buf);
                }
            }
        }
        v.push(std::path::PathBuf::from("runtime/c"));
        v
    };

    for candidate in &candidates {
        // Check if source directory exists (has flux_rt.h).
        if candidate.join("flux_rt.h").exists() {
            // Auto-build libflux_rt.a if missing or stale.
            #[cfg(feature = "native")]
            if let Err(e) = flux::core_to_llvm::pipeline::ensure_runtime_lib(candidate) {
                eprintln!("Warning: failed to build C runtime: {e}");
            }
            let lib_exists = if cfg!(windows) {
                candidate.join("flux_rt.lib").exists()
            } else {
                candidate.join("libflux_rt.a").exists()
            };
            if lib_exists {
                return Some(candidate.clone());
            }
        }
    }
    None
}

fn extract_cache_dir(args: &mut Vec<String>) -> Option<Option<PathBuf>> {
    let mut cache_dir = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cache-dir" {
            args.remove(i);
            if i < args.len() {
                cache_dir = Some(PathBuf::from(args.remove(i)));
                continue;
            }
            eprintln!("Error: --cache-dir requires a directory path.");
            return None;
        } else if let Some(value) = args[i].strip_prefix("--cache-dir=") {
            cache_dir = Some(PathBuf::from(value));
            args.remove(i);
            continue;
        }
        i += 1;
    }
    Some(cache_dir)
}

fn extract_output_path(args: &mut Vec<String>) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            args.remove(i);
            if i < args.len() {
                return Some(args.remove(i));
            }
            eprintln!("Error: -o requires an output path argument.");
            return None;
        }
        i += 1;
    }
    None
}

fn extract_dump_core_mode(args: &mut Vec<String>) -> Option<CoreDumpMode> {
    let mut mode = CoreDumpMode::None;
    let mut i = 0;
    while i < args.len() {
        let next_mode = if args[i] == "--dump-core" {
            args.remove(i);
            CoreDumpMode::Readable
        } else if let Some(value) = args[i].strip_prefix("--dump-core=") {
            let parsed = match value {
                "debug" => CoreDumpMode::Debug,
                "" => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
                _ => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
            };
            args.remove(i);
            parsed
        } else {
            i += 1;
            continue;
        };
        mode = next_mode;
    }
    Some(mode)
}

fn extract_diagnostic_format(args: &mut Vec<String>) -> Option<DiagnosticOutputFormat> {
    let mut format = DiagnosticOutputFormat::Text;
    let mut i = 0;
    while i < args.len() {
        let value = if args[i] == "--format" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --format <text|json|json-compact>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--format=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };

        format = match value.as_str() {
            "text" => DiagnosticOutputFormat::Text,
            "json" => DiagnosticOutputFormat::Json,
            "json-compact" => DiagnosticOutputFormat::JsonCompact,
            _ => {
                eprintln!("Error: --format expects one of: text, json, json-compact.");
                return None;
            }
        };
    }
    Some(format)
}

fn extract_max_errors(args: &mut Vec<String>) -> Option<usize> {
    let mut max_errors = DEFAULT_MAX_ERRORS;
    let mut i = 0;
    while i < args.len() {
        let value_str = if args[i] == "--max-errors" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --max-errors <n>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--max-errors=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };
        match value_str.parse::<usize>() {
            Ok(parsed) => max_errors = parsed,
            Err(_) => {
                eprintln!("Error: --max-errors expects a non-negative integer.");
                return None;
            }
        }
    }
    Some(max_errors)
}

fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for diag in diags {
        if diag.phase().is_none() {
            *diag = diag.clone().with_phase(phase);
        }
    }
}

fn should_show_file_headers(diagnostics: &[Diagnostic], requested: bool) -> bool {
    if requested {
        return true;
    }

    let mut files = std::collections::BTreeSet::new();
    for diag in diagnostics {
        if let Some(file) = diag.file() {
            files.insert(file);
            if files.len() > 1 {
                return true;
            }
        }
    }

    false
}

#[allow(clippy::too_many_arguments)]
fn emit_diagnostics(
    diagnostics: &[Diagnostic],
    default_file: Option<&str>,
    default_source: Option<&str>,
    show_file_headers: bool,
    max_errors: usize,
    format: DiagnosticOutputFormat,
    all_errors: bool,
    text_to_stderr: bool,
) {
    let show_file_headers = should_show_file_headers(diagnostics, show_file_headers);
    let mut agg = DiagnosticsAggregator::new(diagnostics)
        .with_file_headers(show_file_headers)
        .with_max_errors(Some(max_errors))
        .with_stage_filtering(!all_errors);
    if let Some(file) = default_file {
        if let Some(source) = default_source {
            agg = agg.with_default_source(file.to_string(), source.to_string());
        } else {
            agg = agg.with_default_file(file.to_string());
        }
    }

    match format {
        DiagnosticOutputFormat::Text => {
            let rendered = agg.report().rendered;
            if text_to_stderr {
                eprintln!("{}", rendered);
            } else {
                println!("{}", rendered);
            }
        }
        DiagnosticOutputFormat::Json => {
            let rendered = render_diagnostics_json(
                diagnostics,
                default_file,
                Some(max_errors),
                !all_errors,
                true,
            );
            eprintln!("{}", rendered);
        }
        DiagnosticOutputFormat::JsonCompact => {
            let rendered = render_diagnostics_json(
                diagnostics,
                default_file,
                Some(max_errors),
                !all_errors,
                false,
            );
            eprintln!("{}", rendered);
        }
    }
}

/// Inject auto-imports for Flow library modules into the program AST.
///
/// Currently injects: `import Flow.Option exposing (..)`
///
/// Uses a mini-parser to parse the synthetic import so symbols are
/// correctly interned in the same interner used for the rest of compilation.
/// Flow library modules to auto-import.  Each entry is
/// `(module_name, file_name)` — the file is checked for existence before
/// injecting `import <module_name> exposing (..)`.
const FLOW_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Option", "Option.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Assert", "Assert.flx"),
];

fn inject_flow_prelude(
    program: &mut Program,
    parser: &mut flux::syntax::parser::Parser,
    native_mode: bool,
) {
    let flow_dir = Path::new("lib").join("Flow");
    if !flow_dir.exists() {
        return;
    }

    // Both VM and native backends inject all Flow modules.
    // The native/LLVM backend compiles lib/Flow/*.flx through Core IR
    // alongside user code, enabling cross-module inlining.
    let _ = native_mode; // used by both paths now
    let modules: &[(&str, &str)] = FLOW_PRELUDE_MODULES;

    // Collect the set of already-imported Flow modules.
    let interner = parser.interner();
    let existing_imports: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| {
            if let flux::syntax::statement::Statement::Import { name, .. } = stmt {
                interner.try_resolve(*name).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    // Build the synthetic import source for all missing modules.
    let mut imports = Vec::new();
    for &(module_name, file_name) in modules {
        if existing_imports.iter().any(|s| s == module_name) {
            continue;
        }
        if !flow_dir.join(file_name).exists() {
            continue;
        }
        if module_name == "Flow.List" {
            imports.push(format!("import {module_name} except [concat, delete]"));
        } else {
            imports.push(format!("import {module_name} exposing (..)"));
        }
    }

    if imports.is_empty() {
        return;
    }

    let prelude_source = imports.join("\n");
    let main_interner = parser.take_interner();
    let prelude_lexer =
        flux::syntax::lexer::Lexer::new_with_interner(&prelude_source, main_interner);
    let mut prelude_parser = flux::syntax::parser::Parser::new(prelude_lexer);
    let prelude_program = prelude_parser.parse_program();

    let enriched_interner = prelude_parser.take_interner();
    parser.restore_interner(enriched_interner);

    // Prepend the synthetic imports to the program.
    let mut new_statements = prelude_program.statements;
    new_statements.append(&mut program.statements);
    program.statements = new_statements;
}

/// Extract the module name and symbol from a program's top-level `module Name { ... }` statement.
fn extract_module_name_and_sym(
    program: &Program,
    interner: &flux::syntax::interner::Interner,
) -> Option<(String, flux::syntax::Identifier)> {
    for stmt in &program.statements {
        if let flux::syntax::statement::Statement::Module { name, .. } = stmt {
            return Some((interner.resolve(*name).to_string(), *name));
        }
    }
    None
}

fn extract_test_filter(args: &mut Vec<String>) -> Option<Option<String>> {
    let mut test_filter: Option<String> = None;
    let mut i = 1usize;
    while i < args.len() {
        if args[i] == "--test-filter" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --test --test-filter <pattern>");
                return None;
            }
            test_filter = Some(args.remove(i + 1));
            args.remove(i);
        } else if let Some(v) = args[i].strip_prefix("--test-filter=") {
            test_filter = Some(v.to_string());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Some(test_filter)
}

fn extract_roots(args: &mut Vec<String>, roots: &mut Vec<std::path::PathBuf>) -> bool {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" {
            if i + 1 >= args.len() {
                eprintln!(
                    "Usage: flux <file.flx> --root <path> [--root <path> ...]\n       flux run <file.flx> --root <path> [--root <path> ...]"
                );
                return false;
            }
            let path = args.remove(i + 1);
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else if let Some(v) = args[i].strip_prefix("--root=") {
            let path = v.to_string();
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else {
            i += 1;
        }
    }
    true
}

fn collect_roots(entry_path: &Path, extra_roots: &[PathBuf], roots_only: bool) -> Vec<PathBuf> {
    let mut roots = extra_roots.to_vec();
    if !roots_only {
        if let Some(parent) = entry_path.parent() {
            roots.push(parent.to_path_buf());
        }
        let project_src = Path::new("src");
        if project_src.exists() {
            roots.push(project_src.to_path_buf());
        }
        // Add lib/ as a root for Flow library resolution (Proposal 0120).
        // Searches: relative to cwd, then up from the executable.
        let project_lib = Path::new("lib");
        if project_lib.exists() {
            roots.push(project_lib.to_path_buf());
        }
    }
    roots
}

fn is_flx_file(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()) == Some("flx")
}

fn show_tokens(path: &str) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let mut lexer = Lexer::new(&source);
            println!("Tokens from {}:", path);
            println!("{}", "─".repeat(50));
            for tok in lexer.tokenize() {
                println!(
                    "{:>3}:{:<3} {:12} {:?}",
                    tok.position.line,
                    tok.position.column,
                    tok.token_type.to_string(),
                    tok.literal
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn show_bytecode(
    path: &str,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    strict_mode: bool,
    strict_types: bool,
    diagnostics_format: DiagnosticOutputFormat,
) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let mut compiler = Compiler::new_with_interner(path, interner);
            compiler.set_strict_mode(strict_mode);
            compiler.set_strict_types(strict_types);
            let compile_result =
                compiler.compile_with_opts(&program, enable_optimize, enable_analyze);
            let mut compiler_warnings = compiler.take_warnings();
            for diag in &mut compiler_warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }
            if !compiler_warnings.is_empty() {
                emit_diagnostics(
                    &compiler_warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }
            if let Err(diags) = compile_result {
                emit_diagnostics(
                    &diags,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            let bytecode = compiler.bytecode();
            println!("Bytecode from {}:", path);
            println!("{}", "─".repeat(50));
            println!("Constants:");
            for (i, c) in bytecode.constants.iter().enumerate() {
                println!("  {}: {}", i, c);
            }
            println!("\nInstructions:");
            print!("{}", disassemble(&bytecode.instructions));

            // Disassemble function constants
            for (i, c) in bytecode.constants.iter().enumerate() {
                if let Value::Function(f) = c {
                    let name = f
                        .debug_info
                        .as_ref()
                        .and_then(|d| d.name.as_deref())
                        .unwrap_or("<anonymous>");
                    println!("\nFunction <{}> (constant {}):", name, i);
                    print!("{}", disassemble(&f.instructions));
                }
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn lint_file(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let lints = Linter::new(Some(path.to_string()), &interner).lint(&program);
            if !lints.is_empty() {
                emit_diagnostics(
                    &lints,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    false,
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn fmt_file(path: &str, check: bool) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let formatted = format_source(&source);
            if check {
                if source.trim() != formatted.trim() {
                    eprintln!("format: changes needed");
                    std::process::exit(1);
                }
                return;
            }

            if let Err(err) = fs::write(path, formatted) {
                eprintln!("Error writing {}: {}", path, err);
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn analyze_free_vars(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let free_vars = collect_free_vars_in_program(&program);

            if free_vars.is_empty() {
                println!("✓ No free variables found in {}", path);
            } else {
                println!("Free variables in {}:", path);
                println!("{}", "─".repeat(50));
                let mut vars: Vec<_> = free_vars.iter().map(|sym| interner.resolve(*sym)).collect();
                vars.sort();
                for var in vars {
                    println!("  • {}", var);
                }
                println!("\nTotal: {} free variable(s)", free_vars.len());
                println!(
                    "\nℹ️  Free variables are identifiers that are referenced but not defined."
                );
                println!("   This may indicate undefined variables or missing imports.");
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn analyze_tail_calls(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let tail_calls = find_tail_calls(&program);

            if tail_calls.is_empty() {
                println!("✓ No tail calls found in {}", path);
                println!(
                    "\nℹ️  Tail calls are function calls in tail position that can be optimized."
                );
            } else {
                println!("Tail calls in {}:", path);
                println!("{}", "─".repeat(50));

                // Group by line
                let lines: Vec<_> = source.lines().collect();
                for (idx, call) in tail_calls.iter().enumerate() {
                    let line_num = call.span.start.line;
                    let line_text = if line_num > 0 && line_num <= lines.len() {
                        lines[line_num - 1].trim()
                    } else {
                        "<unknown>"
                    };

                    println!("  {}. Line {}: {}", idx + 1, line_num, line_text);
                }

                println!("\nTotal: {} tail call(s)", tail_calls.len());
                println!("\n✓ These calls are eligible for tail call optimization (TCO).");
                println!(
                    "  The Flux compiler automatically optimizes tail calls to avoid stack overflow."
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn show_cache_info(path: &str, extra_roots: &[PathBuf], cache_dir: Option<&Path>) {
    if !Path::new(path).exists() {
        eprintln!("Error: file not found: {}", path);
        return;
    }
    let entry_path = Path::new(path);
    let cache_layout = cache_paths::resolve_cache_layout(entry_path, cache_dir);

    println!("cache root: {}", cache_layout.root().display());
    println!("entry: {}", entry_path.display());

    match load_module_graph_for_cache_info(path, extra_roots) {
        Ok(graph) => {
            println!("modules: {}", graph.topo_order().len());
            for node in graph.topo_order() {
                print_module_cache_summary(&node.path, &cache_layout, false, false);
                #[cfg(feature = "core_to_llvm")]
                print_native_cache_summary(&node.path, &cache_layout, false, false);
            }
            #[cfg(feature = "core_to_llvm")]
            {
                let support_path =
                    flux::core_to_llvm::module_cache::support_object_path(&cache_layout, false);
                println!(
                    "support artifact: {} ({})",
                    support_path.display(),
                    if support_path.exists() {
                        "present"
                    } else {
                        "missing"
                    }
                );
            }
        }
        Err(err) => {
            println!("module graph: unavailable ({err})");
            print_module_cache_summary(entry_path, &cache_layout, false, false);
            #[cfg(feature = "core_to_llvm")]
            print_native_cache_summary(entry_path, &cache_layout, false, false);
        }
    }
}

fn show_module_cache_info(path: &str, extra_roots: &[PathBuf], cache_dir: Option<&Path>) {
    let entry_path = Path::new(path);
    let cache_layout = cache_paths::resolve_cache_layout(entry_path, cache_dir);
    match load_module_graph_for_cache_info(path, extra_roots) {
        Ok(graph) => {
            println!("cache root: {}", cache_layout.root().display());
            for node in graph.topo_order() {
                print_module_cache_summary(&node.path, &cache_layout, true, false);
            }
        }
        Err(err) => {
            println!("cache root: {}", cache_layout.root().display());
            println!("module graph: unavailable ({err})");
            print_module_cache_summary(entry_path, &cache_layout, true, false);
        }
    }
}

fn show_native_cache_info(path: &str, extra_roots: &[PathBuf], cache_dir: Option<&Path>) {
    let entry_path = Path::new(path);
    let cache_layout = cache_paths::resolve_cache_layout(entry_path, cache_dir);
    #[cfg(not(feature = "core_to_llvm"))]
    {
        let _ = extra_roots;
        let _ = cache_layout;
        println!("native cache inspection requires `core_to_llvm` feature");
        return;
    }
    #[cfg(feature = "core_to_llvm")]
    match load_module_graph_for_cache_info(path, extra_roots) {
        Ok(graph) => {
            println!("cache root: {}", cache_layout.root().display());
            for node in graph.topo_order() {
                print_native_cache_summary(&node.path, &cache_layout, true, false);
            }
            let support_path =
                flux::core_to_llvm::module_cache::support_object_path(&cache_layout, false);
            println!(
                "support artifact: {} ({})",
                support_path.display(),
                if support_path.exists() {
                    "present"
                } else {
                    "missing"
                }
            );
        }
        Err(err) => {
            println!("cache root: {}", cache_layout.root().display());
            println!("module graph: unavailable ({err})");
            print_native_cache_summary(entry_path, &cache_layout, true, false);
        }
    }
}

fn show_interface_info_file(path: &str) {
    let Some(interface) =
        flux::bytecode::compiler::module_interface::load_interface(Path::new(path))
    else {
        println!("interface: not found or invalid");
        return;
    };

    println!("interface file: {}", path);
    println!("module: {}", interface.module_name);
    println!("compiler version: {}", interface.compiler_version);
    println!("format version: {}", interface.cache_format_version);
    println!("source hash: {}", interface.source_hash);
    println!("semantic config hash: {}", interface.semantic_config_hash);
    println!("interface fingerprint: {}", interface.interface_fingerprint);
    println!("schemes: {}", interface.schemes.len());
    println!("borrow signatures: {}", interface.borrow_signatures.len());
    println!(
        "dependency fingerprints: {}",
        interface.dependency_fingerprints.len()
    );

    if interface.dependency_fingerprints.is_empty() {
        println!("dependencies: none");
    } else {
        println!("dependencies:");
        for dependency in &interface.dependency_fingerprints {
            println!(
                "  - {} [{}] {}",
                dependency.module_name,
                short_hash(&dependency.interface_fingerprint),
                dependency.source_path
            );
        }
    }

    let mut members: Vec<_> = interface
        .schemes
        .keys()
        .chain(interface.borrow_signatures.keys())
        .cloned()
        .collect();
    members.sort();
    members.dedup();

    if members.is_empty() {
        println!("exports: none");
        return;
    }

    println!("exports:");
    for member in members {
        println!(
            "  - {}: {}",
            member,
            interface
                .schemes
                .get(&member)
                .map(format_scheme_for_cli)
                .unwrap_or_else(|| "<no scheme>".to_string())
        );
        if let Some(signature) = interface.borrow_signatures.get(&member) {
            println!(
                "    borrow: [{}] ({})",
                signature
                    .params
                    .iter()
                    .map(format_borrow_mode)
                    .collect::<Vec<_>>()
                    .join(", "),
                format_borrow_provenance(signature.provenance)
            );
        }
    }
}

fn load_module_graph_for_cache_info(
    path: &str,
    extra_roots: &[PathBuf],
) -> Result<ModuleGraph, String> {
    let source = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, false);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let mut program = parser.parse_program();
    inject_flow_prelude(&mut program, &mut parser, false);
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        return Err("module graph diagnostics present".to_string());
    }
    Ok(graph_result.graph)
}

fn print_module_cache_summary(
    module_path: &Path,
    cache_layout: &CacheLayout,
    verbose: bool,
    strict_mode: bool,
) {
    let source = match fs::read_to_string(module_path) {
        Ok(source) => source,
        Err(err) => {
            println!("module: {}", module_path.display());
            println!("  source: unreadable ({err})");
            return;
        }
    };
    let source_hash = hash_bytes(source.as_bytes());
    let semantic_config_hash =
        flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
            strict_mode,
            false,
        );
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);
    let interface_path = flux::bytecode::compiler::module_interface::interface_path(
        cache_layout.root(),
        module_path,
    );

    println!("module: {}", module_path.display());
    match flux::bytecode::compiler::module_interface::load_valid_interface(
        cache_layout.root(),
        module_path,
        &source,
        &semantic_config_hash,
    ) {
        Ok(interface) => {
            println!(
                "  interface: valid [{}] {}",
                short_hash(&interface.interface_fingerprint),
                interface_path.display()
            );
            if verbose {
                println!("    compiler version: {}", interface.compiler_version);
                println!("    format version: {}", interface.cache_format_version);
                println!(
                    "    semantic config hash: {}",
                    interface.semantic_config_hash
                );
                if interface.dependency_fingerprints.is_empty() {
                    println!("    dependency fingerprints: none");
                } else {
                    println!("    dependency fingerprints:");
                    for dep in &interface.dependency_fingerprints {
                        let status =
                            match flux::bytecode::compiler::module_interface::load_cached_interface(
                                cache_layout.root(),
                                Path::new(&dep.source_path),
                            ) {
                                Ok(current)
                                    if current.interface_fingerprint
                                        == dep.interface_fingerprint =>
                                {
                                    "ok"
                                }
                                Ok(_) => "stale",
                                Err(_) => "missing",
                            };
                        println!(
                            "      - {} [{}] {} ({})",
                            dep.module_name,
                            short_hash(&dep.interface_fingerprint),
                            dep.source_path,
                            status
                        );
                    }
                }
            }
        }
        Err(err) => {
            println!(
                "  interface: invalid ({}) {}",
                err.message(),
                interface_path.display()
            );
        }
    }

    let module_cache = ModuleBytecodeCache::new(cache_layout.vm_dir());
    match module_cache.inspect(
        module_path,
        &cache_key,
        env!("CARGO_PKG_VERSION"),
        cache_layout.root(),
    ) {
        Ok(info) => {
            println!("  vm artifact: valid {}", info.cache_path.display());
            if verbose {
                println!("    compiler version: {}", info.compiler_version);
                println!("    format version: {}", info.format_version);
                println!("    cache key: {}", info.cache_key);
                if info.dependency_statuses.is_empty() {
                    println!("    dependency fingerprints: none");
                } else {
                    println!("    dependency fingerprints:");
                    for dep in info.dependency_statuses {
                        println!(
                            "      - {} [{} -> {}] ({})",
                            dep.source_path,
                            short_hash(&dep.expected_fingerprint),
                            dep.current_fingerprint
                                .as_deref()
                                .map(short_hash)
                                .unwrap_or("missing"),
                            match dep.status {
                                flux::bytecode::bytecode_cache::module_cache::ModuleDependencyStatus::Ok => "ok",
                                flux::bytecode::bytecode_cache::module_cache::ModuleDependencyStatus::Missing => "missing",
                                flux::bytecode::bytecode_cache::module_cache::ModuleDependencyStatus::Stale => "stale",
                            }
                        );
                    }
                }
            }
        }
        Err(err) => println!("  vm artifact: invalid ({})", err.message()),
    }
}

#[cfg(all(feature = "core_to_llvm", feature = "native"))]
#[allow(dead_code)]
fn print_native_cache_summary(
    module_path: &Path,
    cache_layout: &CacheLayout,
    verbose: bool,
    strict_mode: bool,
) {
    let source = match fs::read_to_string(module_path) {
        Ok(source) => source,
        Err(err) => {
            println!("module: {}", module_path.display());
            println!("  native artifact: unreadable source ({err})");
            return;
        }
    };
    let source_hash = hash_bytes(source.as_bytes());
    let semantic_config_hash =
        flux::bytecode::compiler::module_interface::compute_semantic_config_hash(
            strict_mode,
            false,
        );
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);
    let native_cache =
        flux::core_to_llvm::module_cache::NativeModuleCache::new(cache_layout.native_dir());

    println!("module: {}", module_path.display());
    match native_cache.inspect(module_path, &cache_key, cache_layout.root()) {
        Ok(info) => {
            println!("  native artifact: valid {}", info.object_path.display());
            if verbose {
                println!("    metadata: {}", info.metadata_path.display());
                println!("    compiler version: {}", info.metadata.compiler_version);
                println!("    format version: {}", info.metadata.format_version);
                println!("    cache key: {}", info.metadata.cache_key);
                println!("    optimize: {}", info.metadata.optimize);
                if info.dependency_statuses.is_empty() {
                    println!("    dependency fingerprints: none");
                } else {
                    println!("    dependency fingerprints:");
                    for dep in info.dependency_statuses {
                        println!(
                            "      - {} [{} -> {}] ({})",
                            dep.source_path,
                            short_hash(&dep.expected_fingerprint),
                            dep.current_fingerprint
                                .as_deref()
                                .map(short_hash)
                                .unwrap_or("missing"),
                            match dep.status {
                                flux::core_to_llvm::module_cache::DependencyStatus::Ok => "ok",
                                flux::core_to_llvm::module_cache::DependencyStatus::Missing =>
                                    "missing",
                                flux::core_to_llvm::module_cache::DependencyStatus::Stale =>
                                    "stale",
                            }
                        );
                    }
                }
            }
        }
        Err(err) => println!("  native artifact: invalid ({})", err.message()),
    }
}

#[cfg(not(all(feature = "core_to_llvm", feature = "native")))]
#[allow(dead_code)]
fn print_native_cache_summary(
    _module_path: &Path,
    _cache_layout: &CacheLayout,
    _verbose: bool,
    _strict_mode: bool,
) {
}

fn short_hash(hash: &str) -> &str {
    let len = hash.len().min(12);
    &hash[..len]
}

/// Extract a human-readable module name from a file path.
/// Prefers `interface.module_name` when available (e.g. "Flow.Array").
/// Falls back to file stem (e.g. "Day06Solver" from path).
fn module_display_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Format a GHC-style progress line: `[n of m] Action  ModuleName`
fn progress_line(n: usize, total: usize, action: &str, name: &str) -> String {
    let width = total.to_string().len();
    format!("[{:>width$} of {}] {:<10} {}", n, total, action, name)
}

/// Log the diff between an old and new module interface.
fn log_interface_diff(
    old: &flux::types::module_interface::ModuleInterface,
    new: &flux::types::module_interface::ModuleInterface,
) {
    // Added exports.
    for name in new.schemes.keys() {
        if !old.schemes.contains_key(name) {
            eprintln!(
                "  + public {}: {}",
                name,
                format_scheme_for_cli(&new.schemes[name])
            );
        }
    }
    // Removed exports.
    for name in old.schemes.keys() {
        if !new.schemes.contains_key(name) {
            eprintln!("  - public {}", name);
        }
    }
    // Changed signatures.
    for (name, new_scheme) in &new.schemes {
        if let Some(old_scheme) = old.schemes.get(name)
            && old_scheme != new_scheme
        {
            eprintln!(
                "  ~ public {}: {} -> {}",
                name,
                format_scheme_for_cli(old_scheme),
                format_scheme_for_cli(new_scheme)
            );
        }
    }
}

fn format_scheme_for_cli(scheme: &flux::types::scheme::Scheme) -> String {
    if scheme.forall.is_empty() {
        scheme.infer_type.to_string()
    } else {
        format!("forall {:?}. {}", scheme.forall, scheme.infer_type)
    }
}

fn format_borrow_mode(mode: &flux::aether::borrow_infer::BorrowMode) -> &'static str {
    match mode {
        flux::aether::borrow_infer::BorrowMode::Owned => "Owned",
        flux::aether::borrow_infer::BorrowMode::Borrowed => "Borrowed",
    }
}

fn format_borrow_provenance(
    provenance: flux::aether::borrow_infer::BorrowProvenance,
) -> &'static str {
    match provenance {
        flux::aether::borrow_infer::BorrowProvenance::Inferred => "Inferred",
        flux::aether::borrow_infer::BorrowProvenance::BaseRuntime => "BaseRuntime",
        flux::aether::borrow_infer::BorrowProvenance::Imported => "Imported",
        flux::aether::borrow_infer::BorrowProvenance::Unknown => "Unknown",
    }
}
