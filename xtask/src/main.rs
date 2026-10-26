//! Repository tasks, run with `cargo xtask <command>` (an alias in
//! `.cargo/config.toml`).
//!
//! ```text
//! cargo xtask verify <tag> [--base <tag> [--cmp | --cmp=structure]] [--skip-suite]
//!                    [--skip-sweeps] [--det-sample <n>]
//!     lint, the suite, the five corpus sweeps (each with a rerun of n files,
//!     default 50, that must give the same dumps; compared with --base's, by
//!     status and by dump; --cmp fails on any changed dump, --cmp=structure
//!     only on one that is more than renumbered) and parity; outputs in
//!     .verify/<tag>/ (git-ignored; `store.rs`)
//! cargo xtask sweep --out <dir> [--mode cfg|core|core-opt|aether|lir|cache] [--files <list>]
//!                   [--jobs <n>] [--timeout <seconds>] [--flux <path>] [--determinism]
//!     compile every .flx file under examples/, tests/ and lib/; with
//!     --determinism, twice into <dir>_a and <dir>_b, which must agree;
//!     --mode cache --determinism runs each tests/parity program twice with
//!     an empty cache and compares the caches written
//! cargo xtask prune [--keep <n>] [--keep-tag <tag>]... [--yes]
//!     delete all but the newest n (default 5) verify runs, and never a
//!     --keep-tag one; only lists them without --yes
//! cargo xtask compare <base sweep dir> <head sweep dir> [--content]
//!     exits 1 when a file is newly rejected or newly internal, or with
//!     --content when a file's dump changed
//! cargo xtask census [--tag <tag>] [--base <tag>] [--jobs <n>] [--ways <w,…>]
//!                    [--allow network] [<dir>…]
//!     run every .flx file on each way, classify each result, tag features,
//!     join failures to known issues and write the support matrix; outputs
//!     in .verify/census_<tag>/
//! cargo xtask rewrite [--kind reorder|rename|let-wrap|all] [--seed <n>]
//!                     [--ways <w,…>] [--jobs <n>] [<dir|file>…]
//!     rewrite each fixture (default tests/parity and examples/guide) in
//!     ways that cannot change its output, and check the copies against the
//!     original's output with parity-check; outputs in .verify/rewrite_<seed>/
//! cargo xtask bisect-opt <fixture> [--way vm_opt|llvm_opt] [--timeout <seconds>]
//!                        [--flux <path>]
//!     find the optimizing rewrite that makes the opt way differ from the
//!     plain way, with --opt-bisect-limit; report in .verify/bisect_<name>/
//! cargo xtask sanitize [--ways <w,…>] [<dir>…]
//!     the native ways (default llvm,llvm_opt) over tests/parity with the C
//!     runtime built with ASan and UBSan; report in .verify/sanitize/
//! cargo xtask affected [<base ref>] [--path <path>]... [--dry-run] [-- <nextest args>]
//!     run the tests the changed files are likely to break: staged, unstaged
//!     and untracked files, plus with a base ref everything committed since
//!     it, or the --path ones instead; --dry-run prints the command
//! cargo xtask phase <phase>... [--dry-run] [-- <nextest args>]
//! cargo xtask phase --list
//!     run the tests for compiler phases (`affected.rs`)
//! ```
//!
//! Relative paths are taken from the repository root.

mod affected;
mod bisect;
mod census;
mod compare;
mod outcome;
mod proc;
mod rewrite;
mod sanitize;
mod store;
mod sweep;
mod text;
mod verify;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use sweep::Mode;

const USAGE: &str = "usage:
  cargo xtask verify <tag> [--base <tag> [--cmp | --cmp=structure]] [--skip-suite] [--skip-sweeps] [--det-sample <n>]
  cargo xtask sweep --out <dir> [--mode cfg|core|core-opt|aether|lir|cache] [--files <list>] [--jobs <n>] [--timeout <seconds>] [--flux <path>] [--determinism]
  cargo xtask compare <base sweep dir> <head sweep dir> [--content]
  cargo xtask prune [--keep <n>] [--keep-tag <tag>]... [--yes]
  cargo xtask census [--tag <tag>] [--base <tag>] [--jobs <n>] [--ways <w,...>] [--allow network] [<dir>...]
  cargo xtask rewrite [--kind reorder|rename|let-wrap|all] [--seed <n>] [--ways <w,...>] [--jobs <n>] [<dir|file>...]
  cargo xtask bisect-opt <fixture> [--way vm_opt|llvm_opt] [--timeout <seconds>] [--flux <path>]
  cargo xtask sanitize [--ways <w,...>] [<dir>...]
  cargo xtask affected [<base ref>] [--path <path>]... [--dry-run] [-- <nextest args>]
  cargo xtask phase <phase>... [--dry-run] [-- <nextest args>] | --list";

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
    // Everything after `--` goes to nextest, for the commands that run it.
    let (rest, passthrough) = match rest.iter().position(|a| a == "--") {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, &[][..]),
    };
    if !passthrough.is_empty() && !matches!(command.as_str(), "affected" | "phase") {
        return Err(format!("{command} takes no arguments after --\n{USAGE}"));
    }
    let mut args = Args::new(rest);
    match command.as_str() {
        "verify" => {
            let cmp = match (args.flag("--cmp"), args.flag("--cmp=structure")) {
                (false, false) => verify::Cmp::Off,
                (true, false) => verify::Cmp::Any,
                (false, true) => verify::Cmp::Structure,
                (true, true) => return Err(format!("--cmp or --cmp=structure, not both\n{USAGE}")),
            };
            let options = verify::Options {
                base: args.value("--base")?,
                cmp,
                skip_suite: args.flag("--skip-suite"),
                skip_sweeps: args.flag("--skip-sweeps"),
                det_sample: args.number("--det-sample")?.unwrap_or(50),
                tag: args.positional()?.ok_or(USAGE)?,
            };
            args.finish()?;
            if options.cmp != verify::Cmp::Off && (options.base.is_none() || options.skip_sweeps) {
                return Err(format!(
                    "--cmp compares the sweeps with --base's, so it needs --base and the sweeps\n{USAGE}"
                ));
            }
            verify::run_verify(root, &options)
        }
        "sweep" => {
            let mode_name = args.value("--mode")?;
            let cache = mode_name.as_deref() == Some("cache");
            let mode = match mode_name.filter(|_| !cache) {
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
            if cache {
                if !determinism {
                    return Err(format!(
                        "--mode cache only checks determinism: add --determinism\n{USAGE}"
                    ));
                }
                return sweep::cache_determinism(root, &options);
            }
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
        "census" => {
            let options = census::Options {
                tag: args.value("--tag")?.unwrap_or_else(|| "latest".to_string()),
                base: args.value("--base")?,
                jobs: args.number("--jobs")?.unwrap_or(4).max(1),
                ways: args.value("--ways")?,
                allow: args.values("--allow")?,
                dirs: std::iter::from_fn(|| args.positional().transpose())
                    .collect::<Result<_, _>>()?,
            };
            args.finish()?;
            census::run_census(root, &options)
        }
        "rewrite" => {
            let options = rewrite_options(&mut args)?;
            args.finish()?;
            rewrite::run_rewrite(root, &options)
        }
        "bisect-opt" => {
            let way = args.value("--way")?.unwrap_or_else(|| "vm_opt".to_string());
            let options = bisect::Options {
                way: bisect::Way::parse(&way)
                    .ok_or(format!("--way is vm_opt or llvm_opt, not {way}\n{USAGE}"))?,
                timeout: Duration::from_secs(args.number("--timeout")?.unwrap_or(60) as u64),
                flux: args
                    .value("--flux")?
                    .map_or_else(|| verify::flux_binary(root), |f| resolve(root, &f)),
                fixture: args.positional()?.ok_or(USAGE)?,
            };
            args.finish()?;
            bisect::run_bisect(root, &options)
        }
        "sanitize" => {
            let ways = args
                .value("--ways")?
                .unwrap_or_else(|| "llvm,llvm_opt".to_string());
            let mut dirs = Vec::new();
            while let Some(dir) = args.positional()? {
                dirs.push(dir);
            }
            if dirs.is_empty() {
                dirs.push("tests/parity".to_string());
            }
            args.finish()?;
            sanitize::run_sanitize(root, &sanitize::Options { ways, dirs })
        }
        "prune" => {
            let keep = args.number("--keep")?.unwrap_or(5);
            let keep_tags = args.values("--keep-tag")?;
            let yes = args.flag("--yes");
            args.finish()?;
            Ok(store::prune(root, keep, &keep_tags, yes))
        }
        "affected" => {
            let dry_run = args.flag("--dry-run");
            let paths = args.values("--path")?;
            let base = args.positional()?;
            args.finish()?;
            if base.is_some() && !paths.is_empty() {
                return Err(format!("a base ref or --path, not both\n{USAGE}"));
            }
            let repo = affected::Repo::load(root)?;
            let paths = if paths.is_empty() {
                affected::changed_paths(root, base.as_deref())?
            } else {
                paths
            };
            let selection = affected::select_for_paths(&repo, &paths);
            affected::run_selection(root, &selection, passthrough, dry_run)
        }
        "phase" => {
            if args.flag("--list") {
                args.finish()?;
                println!("{}", affected::phase_names().join("\n"));
                return Ok(true);
            }
            let dry_run = args.flag("--dry-run");
            let phases: Vec<String> =
                std::iter::from_fn(|| args.positional().transpose()).collect::<Result<_, _>>()?;
            args.finish()?;
            if phases.is_empty() {
                return Err(format!(
                    "name a phase: {}\n{USAGE}",
                    affected::phase_names().join(" ")
                ));
            }
            let repo = affected::Repo::load(root)?;
            let selection = affected::select_for_phases(&repo, &phases)?;
            affected::run_selection(root, &selection, passthrough, dry_run)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(true)
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn rewrite_options(args: &mut Args) -> Result<rewrite::Options, String> {
    let kind = args.value("--kind")?.unwrap_or_else(|| "all".to_string());
    let mut options = rewrite::Options {
        kinds: rewrite::Kind::parse(&kind).ok_or(format!("unknown kind {kind}\n{USAGE}"))?,
        seed: args.number("--seed")?.unwrap_or(1) as u64,
        ways: args
            .value("--ways")?
            .unwrap_or_else(|| rewrite::DEFAULT_WAYS.to_string()),
        jobs: args.number("--jobs")?.unwrap_or_else(sweep::default_jobs),
        // After the options, so their values are not taken as paths.
        paths: std::iter::from_fn(|| args.positional().transpose()).collect::<Result<_, _>>()?,
    };
    if options.paths.is_empty() {
        options.paths = rewrite::DEFAULT_PATHS.map(String::from).to_vec();
    }
    Ok(options)
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

    /// Every value of an option that may repeat, in order.
    fn values(&mut self, name: &str) -> Result<Vec<String>, String> {
        let mut values = Vec::new();
        while let Some(value) = self.value(name)? {
            values.push(value);
        }
        Ok(values)
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
