//! Shared frontend helpers used by driver entrypoints before backend dispatch.

use std::path::{Path, PathBuf};

use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, types::ErrorType,
};
use crate::syntax::module_graph::ModuleRoot;
use crate::syntax::{
    Identifier, interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser,
    program::Program, statement::Statement,
};

/// Environment variable naming the directory that holds `Flow/`.
///
/// Checked first so a user can point at a stdlib anywhere.
pub const FLUX_LIB_DIR_ENV: &str = "FLUX_LIB_DIR";

/// Locate the stdlib's `Flow/` directory.
///
/// Tried in order:
/// 1. `$FLUX_LIB_DIR/Flow` — explicit override.
/// 2. `lib/Flow` walking up from the entry file — a project checkout, and the
///    workspace case where the prelude sits above the inner crate.
/// 3. `../lib/Flow` and `lib/Flow` beside the executable — an installed binary.
///
/// Returns `None` when no stdlib can be found; callers report that rather than
/// silently continuing with no module roots.
pub(crate) fn find_flow_dir(entry_path: &Path) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(FLUX_LIB_DIR_ENV) {
        let candidate = Path::new(&dir).join("Flow");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    let start = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.to_path_buf());
    let mut current = if start.is_file() {
        start.parent().map(Path::to_path_buf)
    } else {
        Some(start)
    };
    // Bounded so a path with many components cannot spin.
    for _ in 0..32 {
        let Some(dir) = current else { break };
        let candidate = dir.join("lib").join("Flow");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    // Beside the executable: the installed layout is <prefix>/bin/flux with
    // <prefix>/lib/Flow. Walking up also covers a dev binary run from
    // target/debug, whose stdlib sits at the repo root.
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        let mut current = exe.parent().map(Path::to_path_buf);
        for _ in 0..32 {
            let Some(dir) = current else { break };
            let candidate = dir.join("lib").join("Flow");
            if candidate.is_dir() {
                return Some(candidate);
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    }

    None
}

const FLOW_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Eq", "Eq.flx"),
    ("Flow.Ord", "Ord.flx"),
    ("Flow.Add", "Add.flx"),
    ("Flow.Num", "Num.flx"),
    ("Flow.Show", "Show.flx"),
    ("Flow.Option", "Option.flx"),
    ("Flow.Either", "Either.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Semigroup", "Semigroup.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.Math", "Math.flx"),
    ("Flow.Primops", "Primops.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Debug", "Debug.flx"),
    ("Flow.Assert", "Assert.flx"),
];

/// User-facing names exported from `Flow.Primops`. The module also declares
/// compiler-internal `__primop_*` intrinsics used by synthesized default
/// handlers; those names are deliberately omitted here so they never enter
/// user scope through the auto-injected prelude. `Flow.Primops` is also
/// rejected as a direct user import (E083) — see `validate_no_primops_import`.
const FLOW_PRIMOPS_USER_FACING: &[&str] = &[
    "print",
    "println",
    "read_file",
    "read_lines",
    "write_file",
    "read_stdin",
    "clock_now",
    "now_ms",
    "idiv",
    "imod",
    "index",
    "array_get",
    "panic",
];

/// Rejects user-written `import Flow.Primops` statements. `Flow.Primops` is
/// the intrinsic-backed implementation layer for effectful prelude operations
/// (`print`, `println`, `read_file`, ...). Those operations are exposed via
/// other stdlib modules and the auto-injected prelude; users should not
/// import `Flow.Primops` directly or name it in qualified calls.
///
/// Must be called on the parsed program *before* `inject_flow_prelude`, so
/// the synthesized prelude import is not itself flagged.
pub(crate) fn validate_no_primops_import(
    program: &Program,
    interner: &Interner,
    file: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for stmt in &program.statements {
        if let Statement::Import { name, span, .. } = stmt
            && interner.try_resolve(*name) == Some("Flow.Primops")
        {
            let diag = Diagnostic::make_error_dynamic(
                "E083",
                "RESERVED PRIMOP MODULE",
                ErrorType::Compiler,
                "`Flow.Primops` is reserved for the compiler's intrinsic implementation layer \
                 and is not user-importable."
                    .to_string(),
                Some(
                    "Remove this import. Effectful prelude operations like `print`, \
                     `println`, `read_file`, and `now_ms` are available without an explicit \
                     import."
                        .to_string(),
                ),
                file.to_string(),
                *span,
            )
            .with_category(DiagnosticCategory::NameResolution)
            .with_phase(DiagnosticPhase::Parse)
            .with_primary_label(*span, "reserved internal module import");
            out.push(diag);
        }
    }
    out
}

/// Injects Flow prelude imports for standard modules that are present but not explicitly imported.
pub(crate) fn inject_flow_prelude(
    program: &mut Program,
    parser: &mut Parser,
    native_mode: bool,
    entry_path: &Path,
) {
    let Some(flow_dir) = find_flow_dir(entry_path) else {
        return;
    };

    let _ = native_mode;
    let interner = parser.interner();
    let existing_imports: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| {
            if let Statement::Import { name, .. } = stmt {
                interner.try_resolve(*name).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    let mut imports = Vec::new();
    for &(module_name, file_name) in FLOW_PRELUDE_MODULES {
        if existing_imports.iter().any(|s| s == module_name) {
            continue;
        }
        if !flow_dir.join(file_name).exists() {
            continue;
        }
        let exposing = if module_name == "Flow.Primops" {
            format!("({})", FLOW_PRIMOPS_USER_FACING.join(", "))
        } else {
            "(..)".to_string()
        };
        imports.push(format!("import {module_name} exposing {exposing}"));
    }

    if imports.is_empty() {
        return;
    }

    let prelude_source = imports.join("\n");
    let main_interner = parser.take_interner();
    let prelude_lexer = Lexer::new_with_interner(&prelude_source, main_interner);
    let mut prelude_parser = Parser::new(prelude_lexer);
    let prelude_program = prelude_parser.parse_program();

    let enriched_interner = prelude_parser.take_interner();
    parser.restore_interner(enriched_interner);

    let mut new_statements = prelude_program.statements;
    new_statements.append(&mut program.statements);
    program.statements = new_statements;
}

/// Collects module search roots for the given entry file.
///
/// Roots are computed from the entry file and its project root, never from the
/// process working directory, so `flux run foo/bar.flx` and
/// `cd foo && flux run bar.flx` resolve the same set.
pub(crate) fn collect_roots(
    entry_path: &Path,
    extra_roots: &[PathBuf],
    roots_only: bool,
) -> Vec<PathBuf> {
    let mut roots = extra_roots.to_vec();
    if !roots_only {
        if let Some(parent) = entry_path.parent() {
            roots.push(parent.to_path_buf());
        }
        // `src`/`lib` beside the entry file, then beside the project root.
        // The entry-relative pair is what lets a program run from anywhere;
        // the project-root pair replaces an earlier CWD-relative lookup, which
        // made the resolved roots depend on the directory flux was invoked
        // from.
        let entry_dir = entry_path.parent().map(Path::to_path_buf);
        let project_dir = crate::shared::cache_paths::find_project_root(entry_path);
        for base in [entry_dir.as_deref(), project_dir.as_deref()]
            .into_iter()
            .flatten()
        {
            for name in ["src", "lib"] {
                let candidate = base.join(name);
                if candidate.is_dir() && !roots.contains(&candidate) {
                    roots.push(candidate);
                }
            }
        }
        // The stdlib's parent, so `Flow.X` resolves to `<lib>/Flow/X.flx`.
        if let Some(flow_dir) = find_flow_dir(entry_path)
            && let Some(lib_dir) = flow_dir.parent()
        {
            let lib_dir = lib_dir.to_path_buf();
            if !roots.contains(&lib_dir) {
                roots.push(lib_dir);
            }
        }
    }
    roots
}

/// Collect module search roots, scoping them to package namespaces when the
/// entry file belongs to a project with a `flux.toml`.
///
/// Falls back to the unscoped roots `collect_roots` produces whenever there is
/// no manifest, so script mode is unaffected. A manifest that exists but does
/// not resolve is returned as an error rather than silently ignored.
pub(crate) fn collect_module_roots(
    entry_path: &Path,
    extra_roots: &[PathBuf],
    roots_only: bool,
    cache_dir: &Path,
) -> Result<Vec<ModuleRoot>, String> {
    let base: Vec<ModuleRoot> = collect_roots(entry_path, extra_roots, roots_only)
        .into_iter()
        .map(ModuleRoot::unscoped)
        .collect();

    if roots_only {
        return Ok(base);
    }
    let Some(project_dir) = crate::shared::cache_paths::find_project_root(entry_path) else {
        return Ok(base);
    };
    let Some(resolved) =
        crate::driver::manifest_roots::resolve_project_roots(&project_dir, cache_dir)
    else {
        return Ok(base);
    };

    // Package roots come first so a namespaced import resolves through its
    // own package before the entry-relative and stdlib fallbacks.
    match resolved {
        Ok(mut roots) => {
            roots.extend(base);
            Ok(roots)
        }
        // The manifest error is the real diagnosis, but the caller still needs
        // usable roots: reporting it alongside a cascade of "cannot find
        // Flow.Option" would bury it.
        Err(message) => Err(message),
    }
}

/// Extracts the declared module name from a parsed program.
pub(crate) fn extract_module_name_and_sym(
    program: &Program,
    interner: &Interner,
) -> Option<(String, Identifier)> {
    for stmt in &program.statements {
        if let Statement::Module { name, .. } = stmt {
            return Some((interner.resolve(*name).to_string(), *name));
        }
    }
    None
}

/// Loads a module graph for cache inspection commands and rejects graphs with diagnostics.
pub(crate) fn load_module_graph_for_cache_info(
    path: &str,
    extra_roots: &[PathBuf],
) -> Result<ModuleGraph, String> {
    let source = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, false);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let mut program = parser.parse_program();
    inject_flow_prelude(&mut program, &mut parser, false, Path::new(path));
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        return Err("module graph diagnostics present".to_string());
    }
    Ok(graph_result.graph)
}
