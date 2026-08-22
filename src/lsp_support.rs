//! Stable API surface for LSP-style consumers that need a primed `Compiler`
//! without going through `cli::run`.
//!
//! Unlike the CLI driver, which reaches Aether/Core/bytecode for every module,
//! the LSP only needs HM-level schemes. This module exposes a frontend-only
//! prelude loader that mirrors `rust-analyzer`'s approach: parse every Flow
//! prelude module in topological order, run HM inference, and stash the
//! resolved schemes back into the shared `Compiler` so the buffer's inference
//! can resolve `print`, `map`, `Console`, `Unit`, and friends.

use std::path::Path;

use crate::ast::type_infer::{InferProgramConfig, InferProgramResult, infer_program};
use crate::compiler::Compiler;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;
use crate::syntax::program::Program;
use crate::types::module_interface::ModuleInterface;

/// Flow prelude modules, in topological order. Mirrors `FLOW_PRELUDE_MODULES`
/// in [src/driver/frontend.rs]. Order matters: each entry may depend only on
/// names exported by earlier entries.
const FLOW_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Option", "Option.flx"),
    ("Flow.Either", "Either.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.Math", "Math.flx"),
    ("Flow.Primops", "Primops.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Debug", "Debug.flx"),
    ("Flow.Assert", "Assert.flx"),
];

/// Result of an LSP prelude-load attempt.
pub struct PreludeCompiler {
    /// Primed compiler. Reuse it for every buffer's inference.
    pub compiler: Compiler,
    /// How many Flow modules were successfully parsed and inferred. Zero is
    /// valid and falls back to single-file inference (matches pre-Phase-2
    /// LSP behavior).
    pub loaded_count: usize,
    /// True if `lib/Flow/` was found at all. When false, the LSP is running
    /// outside a Flux workspace and prelude loading is a no-op by design.
    pub flow_dir_found: bool,
    /// Resolved `lib/Flow/` directory, if any. The LSP keeps this so it can
    /// load additional `Flow.*` modules on demand when the buffer imports
    /// them.
    pub flow_dir: Option<std::path::PathBuf>,
    /// Qualified names of the modules successfully preloaded. The LSP seeds
    /// its dynamic-import cache from this list so it does not redundantly
    /// reload prelude modules when a buffer imports them explicitly.
    pub loaded_module_names: Vec<String>,
}

/// Walk up from `workspace_hint` looking for a directory containing a
/// `lib/Flow/` subdirectory, then parse and infer every Flow prelude module
/// in topological order, threading schemes through a single shared `Compiler`.
///
/// On any per-module failure (parse error, IO error, inference panic) we skip
/// that module and continue — partial preloads are still useful. The caller
/// can inspect `loaded_count` to know how many of the expected 10 modules
/// landed.
pub fn try_load_prelude_compiler(workspace_hint: &Path) -> PreludeCompiler {
    let Some(flow_dir) = find_flow_dir(workspace_hint) else {
        return PreludeCompiler {
            compiler: Compiler::new(),
            loaded_count: 0,
            flow_dir_found: false,
            flow_dir: None,
            loaded_module_names: Vec::new(),
        };
    };

    let mut compiler = Compiler::new();
    let mut loaded = 0usize;
    let mut loaded_module_names = Vec::new();

    for (module_name, file_name) in FLOW_PRELUDE_MODULES {
        let source_path = flow_dir.join(file_name);
        if !source_path.exists() {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&source_path) else {
            continue;
        };
        let path_str = source_path.to_string_lossy().into_owned();
        if preload_module_into_compiler(&mut compiler, module_name, &path_str, source).is_some() {
            loaded += 1;
            loaded_module_names.push((*module_name).to_string());
        }
    }

    PreludeCompiler {
        compiler,
        loaded_count: loaded,
        flow_dir_found: true,
        flow_dir: Some(flow_dir),
        loaded_module_names,
    }
}

/// Parse one Flow module, run inference against the schemes already loaded
/// into `compiler`, and stash the resolved schemes back into the compiler so
/// the next module sees them.
///
/// Public so the LSP can call this for buffer-driven `import Flow.*`
/// statements that fall outside the auto-prelude list.
pub fn preload_module_into_compiler(
    compiler: &mut Compiler,
    module_name: &str,
    source_path: &str,
    source: String,
) -> Option<()> {
    // Swap the compiler's interner out, parse with it, swap the enriched
    // version back. This is the same pattern `inject_flow_prelude` uses in
    // `src/driver/frontend.rs`.
    let main_interner = std::mem::take(&mut compiler.interner);
    let lexer = Lexer::new_with_interner(&source, main_interner);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    compiler.interner = parser.take_interner();

    compiler.phase_reset_for_lsp();
    compiler.set_file_path(source_path.to_string());

    // Inference is resilient to missing schemes (returns diagnostics, never
    // panics) but we still guard against unforeseen panics here.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = compiler.build_infer_config(&program);
        infer_program(&program, &compiler.interner, config)
    }))
    .ok()?;

    // Extract this module's member schemes from `module_member_schemes`,
    // which holds `(module_id, member_id) -> Scheme` for every function/let
    // declared inside a `module Flow.X { ... }` block. `resolved_binding_schemes`
    // only carries *top-level* bindings (i.e. items outside any `module {}`
    // block), so it misses everything Flow.Async / Flow.Primops actually
    // declares. Filter by the module name we're loading.
    let module_id = compiler.interner.intern(module_name);
    let mut interface = ModuleInterface::new(module_name, "lsp-prelude", "lsp-prelude");
    for ((mid, member_id), scheme) in &result.module_member_schemes {
        if *mid != module_id {
            continue;
        }
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface.schemes.insert(name.to_string(), scheme.clone());
            interface.member_is_value.insert(name.to_string(), false);
        }
    }
    // Some prelude declarations (e.g. effect aliases, top-level lets in
    // documentation modules) still live in `resolved_binding_schemes`; merge
    // those in too so we don't regress whatever M2c already worked for.
    for (member_id, scheme) in &result.resolved_binding_schemes {
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface
                .schemes
                .entry(name.to_string())
                .or_insert_with(|| scheme.clone());
            interface
                .member_is_value
                .entry(name.to_string())
                .or_insert(false);
        }
    }
    compiler.preload_module_interface(&interface);
    Some(())
}

/// Like [`preload_module_into_compiler`] but also returns the parsed `Program`.
/// Used by the LSP to cache module ASTs for cross-file goto-definition without
/// parsing the same file twice.
pub fn preload_module_into_compiler_with_program(
    compiler: &mut Compiler,
    module_name: &str,
    source_path: &str,
    source: String,
) -> Option<Program> {
    let main_interner = std::mem::take(&mut compiler.interner);
    let lexer = Lexer::new_with_interner(&source, main_interner);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    compiler.interner = parser.take_interner();

    compiler.phase_reset_for_lsp();
    compiler.set_file_path(source_path.to_string());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = compiler.build_infer_config(&program);
        infer_program(&program, &compiler.interner, config)
    }))
    .ok()?;

    let module_id = compiler.interner.intern(module_name);
    let mut interface = ModuleInterface::new(module_name, "lsp-prelude", "lsp-prelude");
    for ((mid, member_id), scheme) in &result.module_member_schemes {
        if *mid != module_id {
            continue;
        }
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface.schemes.insert(name.to_string(), scheme.clone());
            interface.member_is_value.insert(name.to_string(), false);
        }
    }
    for (member_id, scheme) in &result.resolved_binding_schemes {
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface
                .schemes
                .entry(name.to_string())
                .or_insert_with(|| scheme.clone());
            interface
                .member_is_value
                .entry(name.to_string())
                .or_insert(false);
        }
    }
    compiler.preload_module_interface(&interface);
    Some(program)
}

/// Stash an already-inferred user module's schemes into `compiler` so later
/// modules in topological order — and the entry buffer — resolve
/// `import <module_name>` and qualified `Module.member` references.
///
/// This is the cross-file analogue of [`preload_module_into_compiler`]: it
/// performs only the "extract member schemes → `ModuleInterface` → preload"
/// tail, fed by an `InferProgramResult` the caller already computed (e.g.
/// while building an LSP `Snapshot`), so the module is not inferred twice.
pub fn stash_module_schemes(
    compiler: &mut Compiler,
    module_name: &str,
    result: &InferProgramResult,
) {
    let module_id = compiler.interner.intern(module_name);
    let mut interface = ModuleInterface::new(module_name, "lsp-user", "lsp-user");
    for ((mid, member_id), scheme) in &result.module_member_schemes {
        if *mid != module_id {
            continue;
        }
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface.schemes.insert(name.to_string(), scheme.clone());
            interface.member_is_value.insert(name.to_string(), false);
        }
    }
    for (member_id, scheme) in &result.resolved_binding_schemes {
        if let Some(name) = compiler.interner.try_resolve(*member_id) {
            interface
                .schemes
                .entry(name.to_string())
                .or_insert_with(|| scheme.clone());
            interface
                .member_is_value
                .entry(name.to_string())
                .or_insert(false);
        }
    }
    compiler.preload_module_interface(&interface);
}

/// Parse a module source file with the compiler's shared interner (so
/// identifier IDs match those already in the snapshot) and return the `Program`
/// without running inference. Used by the LSP to populate the goto-definition
/// module cache for auto-prelude modules that were loaded before the LSP
/// started storing ASTs.
pub fn parse_module_for_goto_def(compiler: &mut Compiler, source: &str) -> Program {
    let main_interner = std::mem::take(&mut compiler.interner);
    let lexer = Lexer::new_with_interner(source, main_interner);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    compiler.interner = parser.take_interner();
    program
}

/// Wrapper around `Compiler::build_infer_config` so callers don't need to
/// reach into `Compiler` directly. Kept as a function (not a method) so the
/// LSP only depends on this module, not on the broader `compiler` surface.
///
/// Callers should ensure `reset_per_file_state` has been invoked since the
/// previous buffer to clear per-file scratch.
pub fn build_infer_config_for_program(
    compiler: &mut Compiler,
    program: &Program,
) -> InferProgramConfig {
    compiler.build_infer_config(program)
}

/// LSP-facing wrapper that runs class-declaration collection for a
/// program. Call before `build_infer_config_for_program` so the
/// compiler's `class_env` includes the buffer's classes — required for
/// class-method dispatch resolution (`lookup_class_method` in
/// `src/ast/type_infer/mod.rs`) which the LSP consumes via
/// `InferProgramResult::class_method_dispatch` for goto-definition on
/// class-method calls.
///
/// Returns the class/instance validation diagnostics so the caller can publish
/// the single-buffer-sound subset as squiggles.
pub fn collect_classes_for_program(
    compiler: &mut Compiler,
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    compiler.collect_classes_for_lsp(program)
}

/// Wrapper around `Compiler::phase_reset_for_lsp` so the LSP can clear
/// per-file scratch state between buffers without holding stale errors or
/// scope info. Does **not** clear `cached_member_schemes`.
pub fn reset_per_file_state(compiler: &mut Compiler) {
    compiler.phase_reset_for_lsp();
}

/// Resolve a qualified module name like `Flow.Async` to its on-disk
/// `<flow_dir>/Async.flx` path. Returns `None` if the name does not start
/// with `Flow.` or the resolved file does not exist.
///
/// Dotted-path tails are split on `.` and joined as subdirectories
/// (`Flow.Net.Tcp` → `<flow_dir>/Net/Tcp.flx`) so the helper stays
/// forward-compatible if Flow modules ever nest. All current Flow modules
/// are flat under `lib/Flow/`.
pub fn flow_module_file_for(flow_dir: &Path, module_name: &str) -> Option<std::path::PathBuf> {
    let tail = module_name.strip_prefix("Flow.")?;
    if tail.is_empty() {
        return None;
    }
    let parts: Vec<&str> = tail.split('.').collect();
    let mut path = flow_dir.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            path.push(format!("{part}.flx"));
        } else {
            path.push(part);
        }
    }
    if path.is_file() { Some(path) } else { None }
}

/// Walk up from `start` looking for the first ancestor that contains a
/// `lib/Flow/` directory. This is more permissive than `find_project_root`
/// (which stops at the first `Cargo.toml`) because in a Cargo workspace the
/// prelude lives at the workspace root, not the inner crate root.
fn find_flow_dir(start: &Path) -> Option<std::path::PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    for _ in 0..16 {
        let candidate = current.join("lib").join("Flow");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locate `lib/Flow/` from the crate manifest directory.
    fn flow_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("lib")
            .join("Flow")
    }

    /// The LSP mirrors `FLOW_PRELUDE_MODULES` from `driver::frontend`. Both
    /// lists are private, so nothing but a test can catch them drifting apart
    /// — and drift is silent: the LSP would simply stop resolving whatever the
    /// driver injects, producing phantom "unknown name" diagnostics.
    #[test]
    fn prelude_module_files_all_exist() {
        let dir = flow_dir();
        for (module_name, file_name) in FLOW_PRELUDE_MODULES {
            let path = dir.join(file_name);
            assert!(
                path.is_file(),
                "prelude module {module_name} maps to {file_name}, which does not exist \
                 under lib/Flow/"
            );
        }
    }

    /// Auto-injecting a module makes every program pay to compile it, so the
    /// prelude is deliberately small. `Flow.Path` (proposal 0178) is an
    /// explicit-import module and must stay out of this list; the same goes
    /// for the other opt-in modules.
    #[test]
    fn explicit_import_modules_are_not_in_the_prelude() {
        let injected: Vec<&str> = FLOW_PRELUDE_MODULES.iter().map(|(name, _)| *name).collect();
        for opt_in in [
            "Flow.Path",
            "Flow.Result",
            "Flow.Fs",
            "Flow.Crypto",
            "Flow.IoError",
            "Flow.Array",
            "Flow.Map",
            "Flow.Async",
            "Flow.Http",
            "Flow.Tcp",
            "Flow.Json",
        ] {
            assert!(
                !injected.contains(&opt_in),
                "{opt_in} is an explicit-import module but appears in FLOW_PRELUDE_MODULES"
            );
        }
    }

    /// Entries are ordered so each may depend only on names exported by
    /// earlier ones. `Flow.Option` underpins the rest and must stay first.
    #[test]
    fn prelude_order_starts_with_option() {
        assert_eq!(
            FLOW_PRELUDE_MODULES.first().map(|(name, _)| *name),
            Some("Flow.Option"),
            "Flow.Option must be injected first; later modules depend on it"
        );
    }

    /// `Flow.Result` stays *out* of the prelude. It is the error model for
    /// fallible stdlib operations (proposal 0178), which makes auto-injection
    /// tempting — but injecting it is the hard-to-reverse direction: removing
    /// it later breaks every program that relied on a bare `Result`, whereas
    /// adding it later breaks nothing. `import Flow.Result` already brings the
    /// type and both constructors into scope unqualified, so nothing is lost.
    #[test]
    fn result_module_is_not_auto_injected() {
        assert!(
            !FLOW_PRELUDE_MODULES
                .iter()
                .any(|(name, _)| *name == "Flow.Result"),
            "Flow.Result must stay an explicit-import module; auto-injection is \
             the one choice that cannot be walked back"
        );
    }

    /// The module file still has to exist — it is referenced by proposal 0178
    /// and imported by the stdlib test fixtures.
    #[test]
    fn result_module_file_exists() {
        assert!(
            flow_dir().join("Result.flx").is_file(),
            "lib/Flow/Result.flx must exist even though it is not auto-injected"
        );
    }

    /// `find_flow_dir` walks up to a `lib/Flow/` ancestor. This is how an LSP
    /// session started in a nested directory still finds the stdlib.
    #[test]
    fn find_flow_dir_walks_up_from_a_nested_path() {
        let nested = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("driver");
        assert_eq!(
            find_flow_dir(&nested).as_deref(),
            Some(flow_dir().as_path()),
            "find_flow_dir should locate lib/Flow from a nested source directory"
        );
    }

    /// `Flow.Path` ships as a real stdlib file even though it is not injected.
    #[test]
    fn flow_path_module_file_exists() {
        assert!(
            flow_dir().join("Path.flx").is_file(),
            "lib/Flow/Path.flx should exist (proposal 0178 stage 0)"
        );
    }
}
