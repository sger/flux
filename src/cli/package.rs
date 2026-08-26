//! The `flux init` / `new` / `build` / `run` / `test` / `check` commands.
//!
//! The package manager is written in Flux. These functions therefore make no
//! packaging decisions: they invoke `Flume.Cli`
//! for scaffolding and for target selection, then hand the resolved entry file
//! to the ordinary compile path. Manifest parsing, namespace derivation, and
//! layout conventions all live in Flux.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::cli::cmdline::PackageAction;
use crate::cli::render::text::profile_native_without_llvm;
use crate::driver::manifest_roots::{FLUX_SKIP_MANIFEST_ENV, flume_shim};
use crate::driver::{
    RunMode,
    backend::Backend,
    backend_policy::validate_flags,
    flags::{DriverFlags, Profile},
    pipeline::RunTarget,
};

const PACKAGE_FORMAT_VERSION: u64 = 1;

/// What the package manager reported: its message, and whether it failed.
struct Reply {
    failed: bool,
    message: String,
}

/// Run `Flume.Cli` with `args` and read its single `ok`/`err` record.
fn call_flume(args: &[&str]) -> Result<Reply, String> {
    call_module("Flume.Cli", args)
}

/// Run `module`'s entry point with `args` and read its `ok`/`err` record.
fn call_module(module: &str, args: &[&str]) -> Result<Reply, String> {
    let shim = flume_shim(module)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    let mut command = Command::new(exe);
    command.arg(&shim);
    if let Some(cache_dir) = module_cache_dir(args) {
        command.arg("--cache-dir").arg(cache_dir);
    }
    let output = command
        .arg("--")
        .args(args)
        .env(FLUX_SKIP_MANIFEST_ENV, "1")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_reply(&stdout)
}

/// Return the normal project cache for a package-manager shim. Flume shims
/// live outside the project tree, so their default cache would otherwise be
/// derived from the shim location and shared by concurrent test processes.
fn module_cache_dir(args: &[&str]) -> Option<PathBuf> {
    args.iter()
        .find(|arg| !arg.starts_with('-') && Path::new(arg).is_dir())
        .map(|dir| Path::new(dir).join("target").join("flux"))
}

/// Read the `ok<TAB>message` / `err<TAB>message` record the command printed.
fn parse_reply(stdout: &str) -> Result<Reply, String> {
    let text = stdout.trim();
    // A fetch may print progress before the final record. Each `print` is
    // rendered as one quoted Flux string, so decode line-by-line; decoding
    // only the whole stdout value would leave the quotes around the final
    // record and make `last_record` miss it.
    let inner = text
        .lines()
        .map(|line| {
            // A single-line print has both quotes, while a multiline Flux
            // string has the opening quote on its first line and the closing
            // quote on its last. Strip either boundary independently.
            line.strip_prefix('"')
                .unwrap_or(line)
                .strip_suffix('"')
                .unwrap_or(line.strip_prefix('"').unwrap_or(line))
                .replace("\\t", "\t")
                .replace("\\n", "\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    // The record is the last line that opens one, and its message runs to the
    // end of the output: a reply may be multi-line — `tree` renders a whole
    // graph — so the message cannot be assumed to stop at a newline. Anything
    // printed before the record is the resolver's own progress and is skipped.
    let Some(record) = last_record(&inner) else {
        let last = inner.lines().next_back().unwrap_or("").trim();
        return Err(format!("unexpected reply from the package manager: {last}"));
    };
    match record.split_once('\t') {
        Some(("ok", message)) => Ok(Reply {
            failed: false,
            message: message.to_string(),
        }),
        Some(("err", message)) => Ok(Reply {
            failed: true,
            message: message.to_string(),
        }),
        _ => Err(format!(
            "unexpected reply from the package manager: {record}"
        )),
    }
}

/// The tail of `text` starting at the last line that opens an `ok`/`err`
/// record.
fn last_record(text: &str) -> Option<&str> {
    let mut offset = None;
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("ok\t") || trimmed.starts_with("err\t") {
            offset = Some(at + (line.len() - trimmed.len()));
        }
        at += line.len();
    }
    offset.map(|start| text[start..].trim_end())
}

fn report(result: Result<Reply, String>) -> ExitCode {
    match result {
        Ok(reply) if reply.failed => {
            eprintln!("error: {}", reply.message);
            ExitCode::FAILURE
        }
        Ok(reply) => {
            println!("{}", reply.message);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `flux init [name] [--lib]` — scaffold a package in the working directory.
pub fn init(name: Option<&str>, is_lib: bool) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Without an explicit name a package is named for its directory, the same
    // convention `cargo init` uses.
    let derived = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".to_string());
    let name = name.unwrap_or(&derived);
    let dir = cwd.to_string_lossy().into_owned();
    report(call_flume(&[
        "init",
        &dir,
        name,
        if is_lib { "--lib" } else { "" },
    ]))
}

/// `flux new <name> [--lib]` — scaffold a package into a new directory.
pub fn new(name: &str, is_lib: bool) -> ExitCode {
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(name);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("error: could not create {}: {err}", dir.display());
        return ExitCode::FAILURE;
    }
    let dir = dir.to_string_lossy().into_owned();
    report(call_flume(&[
        "new",
        &dir,
        name,
        if is_lib { "--lib" } else { "" },
    ]))
}

/// Ask the package manager which file this project builds.
///
/// `bin` selects a named `[[bin]]` target; without one the package's
/// conventional entry point is used.
pub fn entry_file(project_dir: &Path, bin: Option<&str>) -> Result<PathBuf, String> {
    let dir = project_dir.to_string_lossy().into_owned();
    let reply = match bin {
        Some(name) => call_flume(&["bin", &dir, name])?,
        None => call_flume(&["entry", &dir])?,
    };
    if reply.failed {
        return Err(reply.message);
    }
    Ok(PathBuf::from(reply.message))
}

/// The `Flume.Cli` command a manifest-editing action invokes.
///
/// `None` for the actions that compile, and for `update`, which runs in the
/// `Flume.Build.Graph` process instead — see `package_command`.
fn editing_verb(action: PackageAction) -> Option<&'static str> {
    match action {
        PackageAction::Add => Some("add"),
        PackageAction::Remove => Some("remove"),
        _ => None,
    }
}

/// Run `build` / `run` / `test` / `check` against the current package.
///
/// The entry file comes from `Flume.Cli`, which honours `[[bin]]` and `[lib]`
/// targets and the conventional layout. Everything after that is the ordinary
/// compile path, so a package build behaves exactly like running its entry
/// file directly.
pub fn package_command(
    action: PackageAction,
    mut flags: DriverFlags,
    bin: Option<&str>,
    program_args: Vec<String>,
) -> ExitCode {
    if action == PackageAction::Build && program_args.iter().any(|arg| arg == "--explain-rebuild") {
        // The compiler already records detailed interface/cache miss reasons
        // behind verbose reporting; expose that diagnostic surface under the
        // package-manager spelling as well.
        flags.runtime.verbose = true;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(project) = crate::shared::cache_paths::find_project_root(&cwd) else {
        eprintln!("error: no `flux.toml` found in this directory or any parent");
        return ExitCode::FAILURE;
    };
    let package_project =
        crate::shared::cache_paths::find_package_root(&cwd).unwrap_or_else(|| project.clone());

    let is_plan = action == PackageAction::Build && program_args.iter().any(|arg| arg == "--plan");
    if let Err(message) = apply_package_profile(&project, &mut flags, action, !is_plan) {
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    }

    // `tree` reads manifests and prints; it compiles nothing, so it takes none
    // of the entry-point resolution below — a package with no entry point
    // still has a dependency graph worth showing.
    if action == PackageAction::Tree {
        return report(call_flume(&["tree", &project.to_string_lossy()]));
    }

    if action == PackageAction::Metadata {
        return phase3_metadata(&project, flags.diagnostics.diagnostics_format, &flags);
    }

    if action == PackageAction::Publish {
        return publish_package(
            &package_project,
            program_args.iter().any(|arg| arg == "--dry-run"),
        );
    }

    if action == PackageAction::Build && program_args.iter().any(|arg| arg == "--plan") {
        return phase3_build_plan(&project, &flags);
    }

    // `update` re-resolves and rewrites `flux.lock` through the graph module,
    // which owns the resolver's progress and final package-manager record.
    if action == PackageAction::Update {
        let dir = project.to_string_lossy().into_owned();
        let mut call = vec![dir.as_str(), "--update"];
        call.extend(program_args.iter().map(String::as_str));
        return report(call_module("Flume.Build.Graph", &call));
    }

    // `add` and `remove` edit the manifest; like `tree` they compile nothing,
    // and the arguments after the subcommand are the dependency and its
    // source, forwarded verbatim to the package manager.
    if let Some(verb) = editing_verb(action) {
        let dir = project.to_string_lossy().into_owned();
        let mut call = vec![verb, &dir];
        call.extend(program_args.iter().map(String::as_str));
        return report(call_flume(&call));
    }

    let entry = match entry_file(&package_project, bin) {
        Ok(entry) => entry,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    let mode = match action {
        PackageAction::Test => RunMode::Tests,
        _ => RunMode::Program,
    };
    // `build` and `check` compile but do not execute. In Phase 1 they do the
    // same work: the frontend and compilation surface every error, and neither
    // yet caches artifacts differently from a run.
    if matches!(action, PackageAction::Build | PackageAction::Check) {
        flags.runtime.check_only = true;
    }

    crate::driver::pipeline::run_pipeline(
        flags,
        RunTarget {
            path: entry.to_string_lossy().into_owned(),
            mode,
            program_args,
        },
    );
    ExitCode::SUCCESS
}

/// Resolve the package profile through Flume, then layer explicit CLI
/// overrides over the resolved settings.
fn apply_package_profile(
    project: &Path,
    flags: &mut DriverFlags,
    action: PackageAction,
    validate_backend: bool,
) -> Result<(), String> {
    let name = flags
        .profile
        .name
        .clone()
        .unwrap_or_else(|| "dev".to_string());
    let dir = project.to_string_lossy().into_owned();
    let reply = call_flume(&["profile", &dir, &name])?;
    if reply.failed {
        return Err(reply.message);
    }
    let mut fields = reply.message.split('\t');
    let backend = match fields.next() {
        Some("vm") => Backend::Vm,
        Some("native") => Backend::Native,
        Some(found) => return Err(format!("invalid backend in profile reply: {found}")),
        None => return Err("profile resolver returned no backend".to_string()),
    };
    let optimize = match fields.next() {
        Some("true") => true,
        Some("false") => false,
        Some(found) => return Err(format!("invalid optimize value in profile reply: {found}")),
        None => return Err("profile resolver returned no optimize value".to_string()),
    };
    if fields.next().is_some() {
        return Err("profile resolver returned too many fields".to_string());
    }

    let profile = Profile { backend, optimize };
    flags.profile.resolved = Some(profile);
    flags.backend.use_llvm = flags
        .profile
        .cli_use_llvm
        .unwrap_or(profile.backend == Backend::Native);
    flags.language.enable_optimize = flags.profile.cli_optimize.unwrap_or(profile.optimize);
    *flags = flags.clone().finalize_backend();

    if validate_backend
        && matches!(
            action,
            PackageAction::Build | PackageAction::Run | PackageAction::Test | PackageAction::Check
        )
        && let Err(error) = validate_flags(flags, action == PackageAction::Test)
    {
        if profile.backend == Backend::Native
            && flags.profile.cli_use_llvm != Some(false)
            && !cfg!(feature = "llvm")
        {
            return Err(profile_native_without_llvm(&name));
        }
        return Err(error.to_string());
    }
    Ok(())
}

/// Emit the stable, versioned graph consumed by external tooling.
fn phase3_metadata(
    project: &Path,
    format: crate::driver::DiagnosticOutputFormat,
    flags: &DriverFlags,
) -> ExitCode {
    let dir = project.to_string_lossy().into_owned();
    let reply = match resolve_graph(&dir, flags.cache.cache_dir.as_deref()) {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let packages = graph_records(&reply);
    if packages.is_empty() {
        let message = reply
            .lines()
            .find_map(|line| line.strip_prefix("err\t"))
            .unwrap_or("resolver returned no package roots");
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    }
    let members = workspace_members(project);
    let profile_name = flags.profile.name.as_deref().unwrap_or("dev");
    let backend = if flags.is_native_backend() {
        "native"
    } else {
        "vm"
    };
    let value = serde_json::json!({
        "format_version": PACKAGE_FORMAT_VERSION,
        "workspace": { "root": project, "members": members },
        "packages": packages,
        "targets": {
            "backend": backend,
            "profile": profile_name,
            "optimize": flags.language.enable_optimize,
            "cache_root": crate::shared::cache_paths::resolve_cache_root(project, None),
            "store_root": crate::driver::artifact_store::store_root(),
        }
    });
    let output = if matches!(format, crate::driver::DiagnosticOutputFormat::Json) {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    };
    match output {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: could not encode metadata: {error}");
            ExitCode::FAILURE
        }
    }
}

fn phase3_build_plan(project: &Path, flags: &DriverFlags) -> ExitCode {
    let dir = project.to_string_lossy().into_owned();
    let reply = match resolve_graph(&dir, flags.cache.cache_dir.as_deref()) {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let backend = if flags.is_native_backend() {
        "native"
    } else {
        "vm"
    };
    let records = graph_records(&reply);
    if records.is_empty() {
        eprintln!(
            "error: {}",
            reply
                .lines()
                .find_map(|line| line.strip_prefix("err\t"))
                .unwrap_or("resolver returned no build units")
        );
        return ExitCode::FAILURE;
    }
    let units = records
        .into_iter()
        .map(|package| {
            let root = package
                .get("root")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| project.join("src"));
            let namespace = package
                .get("namespace")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("main");
            let source = [root.join("main.flx"), root.join(format!("{namespace}.flx"))]
                .into_iter()
                .find(|candidate| candidate.is_file())
                .unwrap_or(root);
            let source_text = std::fs::read_to_string(&source).unwrap_or_default();
            let semantic = crate::compiler::module_interface::compute_semantic_config_hash(
                flags.language.strict_mode,
                flags.language.enable_optimize,
            );
            let hash = crate::driver::artifact_store::unit_hash(
                &source,
                &source_text,
                &semantic,
                if backend == "native" {
                    crate::driver::backend::Backend::Native
                } else {
                    crate::driver::backend::Backend::Vm
                },
                &[],
            );
            serde_json::json!({
                "package": package.get("name").cloned().unwrap_or(serde_json::Value::Null),
                "target": "lib",
                "mode": "program",
                "backend": backend,
                "profile": flags.profile.name.as_deref().unwrap_or("dev"),
                "unit_hash": hash,
                "source": source,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "format_version": PACKAGE_FORMAT_VERSION,
            "workspace_root": project,
            "units": units,
        }))
        .unwrap_or_else(|_| {
            format!(
                "{{\"format_version\":{},\"units\":[]}}",
                PACKAGE_FORMAT_VERSION
            )
        })
    );
    ExitCode::SUCCESS
}

/// Run the graph entry module while retaining every root record. `call_module`
/// intentionally returns one final reply for command-style modules, whereas
/// the graph protocol is a stream of `ok<TAB>package<TAB>namespace<TAB>root`
/// records.
fn resolve_graph(dir: &str, configured_cache_dir: Option<&Path>) -> Result<String, String> {
    let shim = flume_shim("Flume.Build.Graph")?;
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let cache_dir = configured_cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(dir).join("target").join("flux"));
    let output = Command::new(exe)
        .arg(shim)
        .arg("--cache-dir")
        .arg(cache_dir)
        .arg("--")
        .arg(dir)
        .arg("--quiet")
        .env(FLUX_SKIP_MANIFEST_ENV, "1")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            line.trim()
                .strip_prefix('"')
                .unwrap_or(line.trim())
                .strip_suffix('"')
                .unwrap_or(line.trim())
                .replace("\\t", "\t")
                .replace("\\n", "\n")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn graph_records(reply: &str) -> Vec<serde_json::Map<String, serde_json::Value>> {
    reply
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            if fields.next()? != "ok" {
                return None;
            }
            let name = fields.next()?.to_string();
            let namespace = fields.next()?.to_string();
            let root = fields.next()?.to_string();
            Some(serde_json::Map::from_iter([
                ("name".into(), serde_json::Value::String(name)),
                ("namespace".into(), serde_json::Value::String(namespace)),
                ("root".into(), serde_json::Value::String(root)),
            ]))
        })
        .collect()
}

/// Create an archive, build it from a fresh extraction, and optionally stop
/// before the future registry upload.
fn publish_package(project: &Path, dry_run: bool) -> ExitCode {
    let (name, version) = manifest_identity(project);
    let output_dir = project.join("target").join("flux").join("publish");
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        eprintln!("error: cannot create publish directory: {error}");
        return ExitCode::FAILURE;
    }
    let archive = output_dir.join(format!("{name}-{version}.tar"));
    let tar = Command::new("tar")
        .args([
            "-cf",
            &archive.to_string_lossy(),
            "--exclude=target",
            "--exclude=.git",
            "--exclude=.flux",
            "--exclude=.DS_Store",
            "--exclude=*.swp",
            "--exclude=*~",
            "-C",
            &project.to_string_lossy(),
            ".",
        ])
        .output();
    match tar {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            eprintln!(
                "error: could not create package archive: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("error: could not run tar: {error}");
            return ExitCode::FAILURE;
        }
    }
    let verify = std::env::temp_dir().join(format!(
        "flux-publish-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    if let Err(error) = std::fs::create_dir_all(&verify) {
        eprintln!("error: could not create verification directory: {error}");
        return ExitCode::FAILURE;
    }
    let unpack = Command::new("tar")
        .args([
            "-xf",
            &archive.to_string_lossy(),
            "-C",
            &verify.to_string_lossy(),
        ])
        .output();
    if !matches!(unpack, Ok(ref output) if output.status.success()) {
        eprintln!("error: could not unpack package for verification");
        let _ = std::fs::remove_dir_all(&verify);
        return ExitCode::FAILURE;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("error: could not locate flux for verification: {error}");
            let _ = std::fs::remove_dir_all(&verify);
            return ExitCode::FAILURE;
        }
    };
    let build = Command::new(exe)
        .current_dir(&verify)
        .args(["build", "--no-cache", "--quiet"])
        .env("NO_COLOR", "1")
        .output();
    let verified = matches!(build, Ok(ref output) if output.status.success());
    if !verified {
        if let Ok(output) = build {
            eprintln!(
                "error: clean-checkout verification failed:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(&verify);
        return ExitCode::FAILURE;
    }
    let checksum = match file_sha256(&archive) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("error: cannot hash package archive: {error}");
            let _ = std::fs::remove_dir_all(&verify);
            return ExitCode::FAILURE;
        }
    };
    let _ = std::fs::remove_dir_all(&verify);
    println!("created {}", archive.display());
    println!("sha256:{checksum}");
    println!("verified clean checkout");
    if !dry_run {
        eprintln!("error: upload is unavailable (KI-035: HTTPS registry upload is not supported)");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn file_sha256(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn manifest_identity(project: &Path) -> (String, String) {
    let text = std::fs::read_to_string(project.join("flux.toml")).unwrap_or_default();
    let mut name = "package".to_string();
    let mut version = "0.0.0".to_string();
    let mut package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            package = line == "[package]";
        }
        if package && let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"').to_string();
            if key.trim() == "name" {
                name = value.clone();
            }
            if key.trim() == "version" {
                version = value;
            }
        }
    }
    (name, version)
}

fn workspace_members(project: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(project.join("flux.toml")).unwrap_or_default();
    let mut workspace = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            workspace = line == "[workspace]";
        }
        if workspace
            && line.starts_with("members")
            && let Some((_, value)) = line.split_once('=')
        {
            return value
                .trim()
                .trim_matches(|ch| ch == '[' || ch == ']')
                .split(',')
                .filter_map(|member| {
                    let member = member.trim().trim_matches('"');
                    (!member.is_empty()).then_some(member.to_string())
                })
                .collect();
        }
    }
    vec![".".into()]
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{module_cache_dir, parse_reply};

    #[test]
    fn finds_the_project_directory_in_graph_style_arguments() {
        assert_eq!(
            module_cache_dir(&[".", "--update"]),
            Some(PathBuf::from("./target/flux"))
        );
    }

    #[test]
    fn finds_the_project_directory_in_command_style_arguments() {
        assert_eq!(
            module_cache_dir(&["profile", ".", "dev"]),
            Some(PathBuf::from("./target/flux"))
        );
    }

    #[test]
    fn reads_an_ok_record() {
        let reply = parse_reply("\"ok\\tcreated package `demo`\"").expect("ok record");
        assert!(!reply.failed);
        assert_eq!(reply.message, "created package `demo`");
    }

    #[test]
    fn reads_an_err_record() {
        let reply = parse_reply("\"err\\tno entry point\"").expect("err record");
        assert!(reply.failed);
        assert_eq!(reply.message, "no entry point");
    }

    #[test]
    fn rejects_a_record_it_cannot_read() {
        assert!(parse_reply("\"something else\"").is_err());
    }

    #[test]
    fn reads_a_reply_after_quoted_fetch_progress() {
        let reply =
            parse_reply("\"fetching\\turl\"\n\"ok\\tupdated dep\"").expect("reply after progress");
        assert!(!reply.failed);
        assert_eq!(reply.message, "updated dep");
    }

    #[test]
    fn reads_a_multiline_reply() {
        let reply = parse_reply("\"ok\\tapp v0.1.0\n└── shared (git: local#abc123)\"")
            .expect("multiline reply");
        assert!(!reply.failed);
        assert_eq!(reply.message, "app v0.1.0\n└── shared (git: local#abc123)");
    }
}
