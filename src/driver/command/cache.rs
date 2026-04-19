//! Cache inspection and cleanup commands.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(feature = "llvm")]
use crate::llvm::module_cache::{
    DependencyStatus, NativeModuleCache, compute_native_cache_key, support_object_path,
};
use crate::{
    bytecode::bytecode_cache::module_cache::ModuleDependencyStatus,
    bytecode::bytecode_cache::{hash_bytes, hash_cache_key, module_cache::ModuleBytecodeCache},
    compiler::module_interface::{
        compute_semantic_config_hash, interface_path, load_cached_interface, load_interface,
        load_valid_interface,
    },
    driver::{
        backend::Backend,
        backend_policy::{
            cache_artifact_prefix, native_cache_available, native_cache_unavailable_message,
        },
        command::{
            cache_support::{
                CacheCommandInput, CacheDisplaySelection, load_cache_graph,
                print_native_cache_unavailable_if_needed, resolve_cache_layout_for_input,
            },
            shared::require_input_path,
        },
        flags::DriverFlags,
        support::shared::{
            format_borrow_mode, format_borrow_provenance, format_scheme_for_cli, short_hash,
        },
    },
    shared::cache_paths::{self, CacheLayout},
};

/// Shows cache availability for the selected backend around the input program.
pub fn show_cache_info(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux cache-info <file.flx>");
    print_native_cache_unavailable_if_needed(flags);
    let selection = CacheDisplaySelection::from_flags(flags);
    show_cache_info_for_path(
        path,
        &flags.input.roots,
        flags.cache.cache_dir.as_deref(),
        selection,
    );
}

/// Shows detailed VM module cache status for each module in the input graph.
pub fn show_module_cache_info(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux module-cache-info <file.flx>");
    show_module_cache_info_for_path(path, &flags.input.roots, flags.cache.cache_dir.as_deref());
}

/// Shows detailed native module cache status for each module in the input graph.
pub fn show_native_cache_info(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux native-cache-info <file.flx>");
    show_native_cache_info_for_path(path, &flags.input.roots, flags.cache.cache_dir.as_deref());
}

/// Removes the resolved driver cache directory.
pub fn clean(flags: &DriverFlags) {
    let entry = flags
        .input
        .input_path
        .as_deref()
        .map_or(Path::new("."), Path::new);
    let layout = cache_paths::resolve_cache_layout(entry, flags.cache.cache_dir.as_deref());
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

/// Prints the serialized module interface metadata for one `.flxi` file.
pub fn show_interface_info(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux interface-info <file.flxi>");
    show_interface_info_file(path);
}

pub(crate) fn show_cache_info_for_path(
    path: &str,
    extra_roots: &[PathBuf],
    cache_dir: Option<&Path>,
    selection: CacheDisplaySelection,
) {
    if !Path::new(path).exists() {
        eprintln!("Error: file not found: {}", path);
        return;
    }
    let input = CacheCommandInput {
        path,
        extra_roots,
        cache_dir,
    };
    let (entry_path, cache_layout) = resolve_cache_layout_for_input(input);

    println!("cache root: {}", cache_layout.root().display());
    println!("entry: {}", entry_path.display());

    match load_cache_graph(input) {
        Ok(graph) => {
            println!("modules: {}", graph.topo_order().len());
            for node in graph.topo_order() {
                if selection.show_vm {
                    print_module_cache_summary(&node.path, &cache_layout, false, false);
                }
                if selection.show_native {
                    print_native_cache_summary(&node.path, &cache_layout, false, false);
                }
            }
            #[cfg(feature = "llvm")]
            if selection.show_native {
                let support_path = support_object_path(&cache_layout, false);
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
            if selection.show_vm {
                print_module_cache_summary(entry_path, &cache_layout, false, false);
            }
            if selection.show_native {
                print_native_cache_summary(entry_path, &cache_layout, false, false);
            }
        }
    }
}

pub(crate) fn show_module_cache_info_for_path(
    path: &str,
    extra_roots: &[PathBuf],
    cache_dir: Option<&Path>,
) {
    let input = CacheCommandInput {
        path,
        extra_roots,
        cache_dir,
    };
    let (entry_path, cache_layout) = resolve_cache_layout_for_input(input);
    match load_cache_graph(input) {
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

pub(crate) fn show_native_cache_info_for_path(
    path: &str,
    extra_roots: &[PathBuf],
    cache_dir: Option<&Path>,
) {
    let input = CacheCommandInput {
        path,
        extra_roots,
        cache_dir,
    };
    let (entry_path, cache_layout) = resolve_cache_layout_for_input(input);
    if !native_cache_available() {
        println!("{}", native_cache_unavailable_message());
        return;
    }
    match load_cache_graph(input) {
        Ok(graph) => {
            println!("cache root: {}", cache_layout.root().display());
            for node in graph.topo_order() {
                print_native_cache_summary(&node.path, &cache_layout, true, false);
            }
            #[cfg(feature = "llvm")]
            {
                let support_path = support_object_path(&cache_layout, false);
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
            println!("cache root: {}", cache_layout.root().display());
            println!("module graph: unavailable ({err})");
            print_native_cache_summary(entry_path, &cache_layout, true, false);
        }
    }
}

pub(crate) fn show_interface_info_file(path: &str) {
    let Some(interface) = load_interface(Path::new(path)) else {
        println!("interface: not found or invalid");
        return;
    };
    let mut interner = crate::syntax::interner::Interner::new();
    let remap = interface.build_symbol_remap(&mut interner);

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
                .map(|scheme| format_scheme_for_cli(&interner, &scheme.remap_symbols(&remap)))
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

/// Prints VM cache summary information for a single module path.
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
    let semantic_config_hash = compute_semantic_config_hash(strict_mode, false);
    let cache_key = hash_cache_key(&source_hash, &semantic_config_hash);
    let interface_path = interface_path(cache_layout.root(), module_path);

    println!("module: {}", module_path.display());
    match load_valid_interface(
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
                        let status = match load_cached_interface(
                            cache_layout.root(),
                            Path::new(&dep.source_path),
                        ) {
                            Ok(current)
                                if current.interface_fingerprint == dep.interface_fingerprint =>
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
            println!(
                "  {}: valid {}",
                cache_artifact_prefix(Backend::Vm),
                info.cache_path.display()
            );
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
                                ModuleDependencyStatus::Ok => "ok",
                                ModuleDependencyStatus::Missing => "missing",
                                ModuleDependencyStatus::Stale => "stale",
                            }
                        );
                    }
                }
            }
        }
        Err(err) => println!(
            "  {}: invalid ({})",
            cache_artifact_prefix(Backend::Vm),
            err.message()
        ),
    }
}

#[cfg(all(feature = "llvm", feature = "native"))]
/// Prints native cache summary information for a single module path.
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
            println!(
                "  {}: unreadable source ({err})",
                cache_artifact_prefix(Backend::Native)
            );
            return;
        }
    };
    let source_hash = hash_bytes(source.as_bytes());
    let semantic_config_hash = compute_semantic_config_hash(strict_mode, false);
    let cache_key = compute_native_cache_key(&source_hash, &semantic_config_hash);
    let native_cache = NativeModuleCache::new(cache_layout.native_dir());

    println!("module: {}", module_path.display());
    match native_cache.inspect(module_path, &cache_key, cache_layout.root()) {
        Ok(info) => {
            println!(
                "  {}: valid {}",
                cache_artifact_prefix(Backend::Native),
                info.object_path.display()
            );
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
                                DependencyStatus::Ok => "ok",
                                DependencyStatus::Missing => "missing",
                                DependencyStatus::Stale => "stale",
                            }
                        );
                    }
                }
            }
        }
        Err(err) => println!(
            "  {}: invalid ({})",
            cache_artifact_prefix(Backend::Native),
            err.message()
        ),
    }
}

#[cfg(not(all(feature = "llvm", feature = "native")))]
#[allow(dead_code)]
fn print_native_cache_summary(
    _module_path: &Path,
    _cache_layout: &CacheLayout,
    _verbose: bool,
    _strict_mode: bool,
) {
}

#[cfg(test)]
mod tests {
    use super::show_cache_info_for_path;
    use crate::driver::command::cache_support::CacheDisplaySelection;

    #[test]
    fn cache_info_for_missing_file_returns_without_panic() {
        show_cache_info_for_path(
            "definitely-missing.flx",
            &[],
            None,
            CacheDisplaySelection {
                show_vm: true,
                show_native: false,
            },
        );
    }
}
