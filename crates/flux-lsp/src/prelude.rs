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
    /// to a member's declaration site in the module file.
    pub module_programs: HashMap<String, (Program, Arc<str>, PathBuf)>,
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
        }
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
        let mut module_programs: HashMap<String, (Program, Arc<str>, PathBuf)> = HashMap::new();
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
                module_programs.insert(short.to_string(), (program, source_arc, source_path));
            }
        }

        Self {
            compiler,
            flow_module_symbol,
            loaded_count: primed.loaded_count,
            flow_dir: primed.flow_dir,
            loaded_modules,
            module_programs,
        }
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
            self.module_programs
                .insert(short.to_string(), (program, source_arc, source_path));
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

/// Resolve a `file://` URI to the parent directory of the file it refers to.
///
/// VS Code emits Windows paths as `file:///C:/...`; we normalize that so
/// `find_project_root` can walk up looking for `Cargo.toml`.
pub fn parent_dir_of_uri(uri: &lsp_types::Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let stripped = s.strip_prefix("file://").unwrap_or(s);
    // Decode FIRST, then strip the leading slash on Windows. VS Code emits
    // `file:///e%3A/...`, so without decoding first the drive-letter check
    // (`chars().nth(2) == ':'`) sees `%` and the leading slash survives —
    // producing a path that no Windows API can resolve.
    let decoded = percent_decode(stripped);
    let path_str = if decoded.starts_with('/')
        && decoded
            .chars()
            .nth(2)
            .is_some_and(|c| c == ':' || cfg!(not(windows)))
    {
        if cfg!(windows) {
            decoded.trim_start_matches('/').to_string()
        } else {
            decoded
        }
    } else {
        decoded
    };
    let path = PathBuf::from(path_str);
    path.parent().map(|p| p.to_path_buf())
}

/// Minimal `%XX` percent-decode for `file://` URIs. VS Code encodes colons,
/// spaces, and a few other characters; without decoding the path won't exist
/// on disk and prelude loading silently fails.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
