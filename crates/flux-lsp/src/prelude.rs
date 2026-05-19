use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flux::compiler::Compiler;
use flux::lsp_support;
use flux::syntax::Identifier;
use flux::syntax::program::Program;

/// LSP-side wrapper around the long-lived `flux::compiler::Compiler` we use for
/// every buffer in the session. Holding the same Compiler across buffers lets
/// each buffer's inference reuse the Flow prelude schemes preloaded during
/// `try_load_from`.
pub struct Prelude {
    pub compiler: Compiler,
    pub flow_module_symbol: Identifier,
    /// How many Flow prelude modules landed during load. Zero is valid and
    /// causes the LSP to fall back to single-file inference for prelude names.
    pub loaded_count: usize,
    /// Resolved `lib/Flow/` directory. `None` when running outside a Flux
    /// workspace; in that case dynamic-import loading is a no-op.
    pub flow_dir: Option<PathBuf>,
    /// Qualified names already loaded into `compiler.cached_member_schemes`.
    /// Seeded from the auto-prelude list during `try_load_from`; extended as
    /// buffer-driven `import Flow.*` statements arrive.
    pub loaded_modules: HashSet<String>,
    /// Short module name (e.g. `"Math"`) → (parsed program, source text, file
    /// path). Populated alongside `loaded_modules` so goto-definition can jump
    /// to a member's declaration site in the module file. The `Program` is
    /// `Arc`-wrapped so a `Snapshot` can share it without a deep AST copy.
    pub module_programs: HashMap<String, (Arc<Program>, Arc<str>, PathBuf)>,
    /// Short module name → exported member names, for completion. Derived
    /// from the loaded Flow modules; rebuilt only when a module loads, then
    /// shared by every `Snapshot` via `Arc` rather than recomputed per edit.
    pub module_members: Arc<HashMap<String, Vec<String>>>,
    /// Final dotted segment of every loaded module (e.g. `"String"` for
    /// `Flow.String`). Shared by `Arc`, like `module_members`.
    pub module_short_names: Arc<HashSet<String>>,
}

impl Prelude {
    /// Construct an empty prelude — used when no workspace is available (e.g.
    /// hermetic integration tests).
    pub fn empty() -> Self {
        let mut compiler = Compiler::new();
        let flow_module_symbol = compiler.interner.intern("Flow");
        Self {
            compiler,
            flow_module_symbol,
            loaded_count: 0,
            flow_dir: None,
            loaded_modules: HashSet::new(),
            module_programs: HashMap::new(),
            module_members: Arc::new(HashMap::new()),
            module_short_names: Arc::new(HashSet::new()),
        }
    }

    /// Recompute the `Arc`-shared `module_members` / `module_short_names`
    /// indexes from the currently loaded Flow modules. Called once at load
    /// and again whenever a module is dynamically preloaded — never per edit.
    fn rebuild_member_index(&mut self) {
        // Final dotted segment of every module the prelude knows so hover
        // can recognize `String.join` as referencing `Flow.String`. Includes
        // modules that were only parsed for completion/goto-def
        // (`module_programs`), not just the inferred `loaded_modules`.
        let mut module_short_names: HashSet<String> = self
            .loaded_modules
            .iter()
            .map(|qual| qual.rsplit('.').next().unwrap_or(qual.as_str()).to_string())
            .collect();
        module_short_names.extend(self.module_programs.keys().cloned());

        // Index module members by short name for completion. Walks the
        // compiler's `cached_member_schemes`; one entry per `(module, member)`.
        let mut module_members: HashMap<String, Vec<String>> = HashMap::new();
        for (module_sym, member_sym) in self.compiler.cached_member_schemes().keys() {
            let Some(qualified) = self.compiler.interner.try_resolve(*module_sym) else {
                continue;
            };
            let Some(member) = self.compiler.interner.try_resolve(*member_sym) else {
                continue;
            };
            if !self.loaded_modules.contains(qualified) {
                // Only surface members from prelude modules we actually
                // recognize — avoids leaking arbitrary compiler-internal
                // keys (e.g. effect-op intrinsics) into completion.
                continue;
            }
            let short = qualified.rsplit('.').next().unwrap_or(qualified);
            module_members
                .entry(short.to_string())
                .or_default()
                .push(member.to_string());
        }
        for v in module_members.values_mut() {
            v.sort();
            v.dedup();
        }

        self.module_members = Arc::new(module_members);
        self.module_short_names = Arc::new(module_short_names);
    }

    /// Parse + infer every `lib/Flow/*.flx` reachable from `start`, threading
    /// their schemes through one shared `Compiler`. On any failure (workspace
    /// not found, missing files, IO error) falls back to `empty`.
    pub fn try_load_from(start: &Path) -> Self {
        let primed = lsp_support::try_load_prelude_compiler(start);
        if !primed.flow_dir_found {
            tracing::warn!(
                "flux-lsp: lib/Flow/ not found near {}; prelude inference disabled",
                start.display()
            );
        } else if primed.loaded_count == 0 {
            tracing::warn!(
                "flux-lsp: lib/Flow/ found near {} but no modules loaded",
                start.display()
            );
        } else {
            tracing::info!(
                "flux-lsp: loaded {} Flow prelude module(s) from {}",
                primed.loaded_count,
                start.display()
            );
        }

        let mut compiler = primed.compiler;
        let flow_module_symbol = compiler.interner.intern("Flow");
        let loaded_modules: HashSet<String> = primed.loaded_module_names.iter().cloned().collect();

        // Build the goto-def module cache by re-parsing each prelude module.
        // Inference already ran (schemes are in the compiler); we just need the
        // AST. Re-parsing is cheap (~1ms per file) and only happens once at
        // startup.
        let mut module_programs: HashMap<String, (Arc<Program>, Arc<str>, PathBuf)> =
            HashMap::new();
        if let Some(ref flow_dir) = primed.flow_dir {
            for qualified in &loaded_modules {
                let Some(source_path) = lsp_support::flow_module_file_for(flow_dir, qualified)
                else {
                    continue;
                };
                let Ok(source) = std::fs::read_to_string(&source_path) else {
                    continue;
                };
                let source_arc: Arc<str> = source.into();
                let program =
                    lsp_support::parse_module_for_goto_def(&mut compiler, source_arc.as_ref());
                let short = qualified.rsplit('.').next().unwrap_or(qualified.as_str());
                module_programs.insert(
                    short.to_string(),
                    (Arc::new(program), source_arc, source_path),
                );
            }

            // Index every *other* `lib/Flow/*.flx` module — parse-only, no
            // inference — so completion and goto-definition cover the whole
            // stdlib, not just the auto-prelude + buffer-imported subset.
            // These carry no cached schemes (hover/type-detail on their
            // members is limited), but member listing and goto-definition are
            // AST-driven and work from the parsed program alone. If the
            // buffer later imports such a module, `preload_module_if_needed`
            // does the full parse+infer and replaces this entry.
            index_remaining_flow_modules(&mut compiler, flow_dir, &mut module_programs);
        }

        let mut prelude = Self {
            compiler,
            flow_module_symbol,
            loaded_count: primed.loaded_count,
            flow_dir: primed.flow_dir,
            loaded_modules,
            module_programs,
            module_members: Arc::new(HashMap::new()),
            module_short_names: Arc::new(HashSet::new()),
        };
        prelude.rebuild_member_index();
        prelude
    }

    /// Load a single `Flow.*` module on demand, if it isn't already in
    /// `cached_member_schemes`. Returns `true` if the module was just loaded
    /// (or was already loaded); `false` if the module could not be resolved
    /// or inference failed.
    ///
    /// Called by `Snapshot::build` for every `Statement::Import { name, .. }`
    /// in the buffer whose name starts with `"Flow."`.
    pub fn preload_module_if_needed(&mut self, module_name: &str) -> bool {
        if self.loaded_modules.contains(module_name) {
            return true;
        }
        let Some(flow_dir) = self.flow_dir.as_ref() else {
            return false;
        };
        let Some(source_path) = lsp_support::flow_module_file_for(flow_dir, module_name) else {
            return false;
        };
        let Ok(source) = std::fs::read_to_string(&source_path) else {
            return false;
        };
        let source_arc: Arc<str> = source.into();
        let path_str = source_path.to_string_lossy().into_owned();
        if let Some(program) = lsp_support::preload_module_into_compiler_with_program(
            &mut self.compiler,
            module_name,
            &path_str,
            source_arc.to_string(),
        ) {
            tracing::info!(
                "flux-lsp: dynamically preloaded {} from {}",
                module_name,
                path_str
            );
            self.loaded_modules.insert(module_name.to_string());
            let short = module_name.rsplit('.').next().unwrap_or(module_name);
            self.module_programs.insert(
                short.to_string(),
                (Arc::new(program), source_arc, source_path),
            );
            // A new module changes the shared member indexes — refresh them
            // now so the snapshot built from this buffer sees the new names.
            self.rebuild_member_index();
            true
        } else {
            tracing::warn!(
                "flux-lsp: failed to preload {} from {}",
                module_name,
                path_str
            );
            false
        }
    }
}

/// Parse every `*.flx` under `flow_dir` not already in `module_programs`,
/// inserting a parse-only entry keyed by the file stem (the module's short
/// name). Recurses into subdirectories so a future nested Flow namespace is
/// still picked up. Parse failures and unreadable files are skipped.
fn index_remaining_flow_modules(
    compiler: &mut Compiler,
    flow_dir: &Path,
    module_programs: &mut HashMap<String, (Arc<Program>, Arc<str>, PathBuf)>,
) {
    let Ok(entries) = std::fs::read_dir(flow_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            index_remaining_flow_modules(compiler, &path, module_programs);
            continue;
        }
        if path.extension().is_none_or(|e| e != "flx") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if module_programs.contains_key(stem) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let source_arc: Arc<str> = source.into();
        let program = lsp_support::parse_module_for_goto_def(compiler, source_arc.as_ref());
        module_programs.insert(stem.to_string(), (Arc::new(program), source_arc, path));
    }
}

// URI ↔ path helpers (`parent_dir_of_uri`, percent-decoding) now live in
// `crate::vfs`, the single owner of path/URI conversion.
