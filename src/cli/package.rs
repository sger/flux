//! The `flux init` / `new` / `build` / `run` / `test` / `check` commands.
//!
//! The package manager is written in Flux. These functions therefore make no
//! packaging decisions: they invoke `Flume.Cli`
//! for scaffolding and for target selection, then hand the resolved entry file
//! to the ordinary compile path. Manifest parsing, namespace derivation, and
//! layout conventions all live in Flux.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::cli::cmdline::PackageAction;
use crate::driver::manifest_roots::{FLUX_SKIP_MANIFEST_ENV, flume_shim};
use crate::driver::{RunMode, flags::DriverFlags, pipeline::RunTarget};

/// What the package manager reported: its message, and whether it failed.
struct Reply {
    failed: bool,
    message: String,
}

/// Run `Flume.Cli` with `args` and read its single `ok`/`err` record.
fn call_flume(args: &[&str]) -> Result<Reply, String> {
    let shim = flume_shim("Flume.Cli")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    let output = Command::new(exe)
        .arg(&shim)
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

/// Read the `ok<TAB>message` / `err<TAB>message` record the command printed.
fn parse_reply(stdout: &str) -> Result<Reply, String> {
    let text = stdout.trim();
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text)
        .replace("\\t", "\t")
        .replace("\\n", "\n");
    let line = inner.lines().next_back().unwrap_or("").trim();
    match line.split_once('\t') {
        Some(("ok", message)) => Ok(Reply {
            failed: false,
            message: message.to_string(),
        }),
        Some(("err", message)) => Ok(Reply {
            failed: true,
            message: message.to_string(),
        }),
        _ => Err(format!("unexpected reply from the package manager: {line}")),
    }
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(project) = crate::shared::cache_paths::find_project_root(&cwd) else {
        eprintln!("error: no `flux.toml` found in this directory or any parent");
        return ExitCode::FAILURE;
    };

    let entry = match entry_file(&project, bin) {
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

#[cfg(test)]
mod tests {
    use super::parse_reply;

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
}
