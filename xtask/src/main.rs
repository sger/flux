//! Repository tasks, run with `cargo xtask <command>` (an alias in
//! `.cargo/config.toml`).
//!
//! ```text
//! cargo xtask verify <tag> [--base <tag> [--cmp]] [--skip-suite] [--skip-sweeps]
//!     lint, the suite, the three corpus sweeps (compared with --base's, by
//!     status and by dump; --cmp fails on any changed dump) and parity;
//!     outputs in .git/parked/out/
//! cargo xtask sweep --out <dir> [--mode cfg|core|core-opt] [--files <list>]
//!                   [--jobs <n>] [--timeout <seconds>] [--flux <path>] [--determinism]
//!     compile every .flx file under examples/, tests/ and lib/; with
//!     --determinism, twice into <dir>_a and <dir>_b, which must agree
//! cargo xtask compare <base sweep dir> <head sweep dir> [--content]
//!     exits 1 when a file is newly rejected or newly internal, or with
//!     --content when a file's dump changed
//! ```
//!
//! Relative paths are taken from the repository root.

mod compare;
mod outcome;
mod proc;
mod sweep;
mod text;
mod verify;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use sweep::Mode;

const USAGE: &str = "usage:
  cargo xtask verify <tag> [--base <tag> [--cmp]] [--skip-suite] [--skip-sweeps]
  cargo xtask sweep --out <dir> [--mode cfg|core|core-opt] [--files <list>] [--jobs <n>] [--timeout <seconds>] [--flux <path>] [--determinism]
  cargo xtask compare <base sweep dir> <head sweep dir> [--content]";

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root")
        .to_path_buf();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&root, &args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::from(2)
        }
    }
}

/// Runs the command; `Ok(false)` means it ran and found a failure.
fn dispatch(root: &Path, args: &[String]) -> Result<bool, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let mut args = Args::new(rest);
    match command.as_str() {
        "verify" => {
            let options = verify::Options {
                base: args.value("--base")?,
                cmp: args.flag("--cmp"),
                skip_suite: args.flag("--skip-suite"),
                skip_sweeps: args.flag("--skip-sweeps"),
                tag: args.positional()?.ok_or(USAGE)?,
            };
            args.finish()?;
            if options.cmp && (options.base.is_none() || options.skip_sweeps) {
                return Err(format!(
                    "--cmp compares the sweeps with --base's, so it needs --base and the sweeps\n{USAGE}"
                ));
            }
            verify::run_verify(root, &options)
        }
        "sweep" => {
            let mode = match args.value("--mode")? {
                Some(name) => Mode::parse(&name).ok_or(format!("unknown mode {name}"))?,
                None => Mode::Cfg,
            };
            let options = sweep::Options {
                out_dir: resolve(root, &args.value("--out")?.ok_or(USAGE)?),
                mode,
                files: args.value("--files")?.map(|f| resolve(root, &f)),
                jobs: args.number("--jobs")?.unwrap_or_else(sweep::default_jobs),
                timeout: Duration::from_secs(args.number("--timeout")?.unwrap_or(120) as u64),
                flux: args
                    .value("--flux")?
                    .map(|f| resolve(root, &f))
                    .unwrap_or_else(|| verify::flux_binary(root)),
            };
            let determinism = args.flag("--determinism");
            args.finish()?;
            if determinism {
                return sweep::determinism(root, &options);
            }
            sweep::run(root, &options)?;
            Ok(true)
        }
        "compare" => {
            let content = args.flag("--content");
            let base = args.positional()?.ok_or(USAGE)?;
            let head = args.positional()?.ok_or(USAGE)?;
            args.finish()?;
            let comparison =
                compare::run(root, &resolve(root, &base), &resolve(root, &head), content)?;
            Ok(!comparison.failed && comparison.changed.is_empty())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(true)
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn resolve(root: &Path, path: &str) -> PathBuf {
    root.join(path)
}

/// The arguments after the command; each accessor removes what it reads, so
/// `finish` can reject anything left over.
struct Args(Vec<String>);

impl Args {
    fn new(args: &[String]) -> Args {
        Args(args.to_vec())
    }

    fn flag(&mut self, name: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|a| a != name);
        self.0.len() != before
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(at) = self.0.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if at + 1 >= self.0.len() {
            return Err(format!("{name} needs a value"));
        }
        let value = self.0.remove(at + 1);
        self.0.remove(at);
        Ok(Some(value))
    }

    fn number(&mut self, name: &str) -> Result<Option<usize>, String> {
        self.value(name)?
            .map(|v| v.parse().map_err(|_| format!("{name}: not a number: {v}")))
            .transpose()
    }

    /// The first argument that is not an option.
    fn positional(&mut self) -> Result<Option<String>, String> {
        match self.0.iter().position(|a| !a.starts_with("--")) {
            Some(at) => Ok(Some(self.0.remove(at))),
            None => Ok(None),
        }
    }

    fn finish(&self) -> Result<(), String> {
        match self.0.first() {
            Some(extra) => Err(format!("unexpected argument {extra}\n{USAGE}")),
            None => Ok(()),
        }
    }
}
