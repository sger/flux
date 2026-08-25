//! Resolve a project's package roots by running the Flux manifest resolver.
//!
//! Manifest handling lives in Flux: `Flume.Roots` reads
//! `flux.toml`, walks path dependencies, derives namespaces, and prints one
//! record per resolved package. This module is the whole Rust side of that
//! boundary — it runs the resolver and turns its output into scoped module
//! roots. There is deliberately no TOML parsing here.
//!
//! The resolver is itself a Flux program compiled by this same driver, so the
//! child invocation must not try to resolve a manifest of its own. The
//! `FLUX_SKIP_MANIFEST_ENV` guard breaks that one level of recursion.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::bytecode::bytecode_cache::hash_bytes;
use crate::driver::spinner::Spinner;
use crate::syntax::module_graph::ModuleRoot;

/// Set on the child process that runs the manifest resolver. While it is set,
/// `resolve_project_roots` returns `None` immediately, so compiling the
/// resolver cannot trigger another resolver run.
pub const FLUX_SKIP_MANIFEST_ENV: &str = "FLUX_SKIP_MANIFEST";

/// Set when `--offline` forbids the resolution from reaching the network, and
/// when `--locked` forbids it from changing `flux.lock`.
///
/// Carried in the environment rather than as a parameter because these are
/// properties of the whole invocation rather than of one module's roots, and
/// because the resolver runs as a child process that has to be told either
/// way. `resolve_project_roots` sits several layers below flag parsing, and
/// widening every signature between them would thread a process-wide mode
/// through code that has no other use for it.
pub const FLUX_OFFLINE_ENV: &str = "FLUX_OFFLINE";
pub const FLUX_LOCKED_ENV: &str = "FLUX_LOCKED";

/// Record the resolution mode for the rest of this process.
pub fn set_resolution_mode(offline: bool, locked: bool) {
    if offline {
        unsafe { std::env::set_var(FLUX_OFFLINE_ENV, "1") };
    }
    if locked {
        unsafe { std::env::set_var(FLUX_LOCKED_ENV, "1") };
    }
}

fn mode_flags() -> Vec<&'static str> {
    let mut flags = Vec::new();
    if std::env::var_os(FLUX_OFFLINE_ENV).is_some() {
        flags.push("--offline");
    }
    if std::env::var_os(FLUX_LOCKED_ENV).is_some() {
        flags.push("--locked");
    }
    flags
}

/// Write an entry file that calls `<module>.main`, and return its path.
///
/// A module's `main` cannot be invoked directly, so every Flume entry point is
/// reached through a generated shim. Shims live in the cache directory rather
/// than in `lib/`, so the stdlib stays free of compiler-private files. See
/// KI-019.
pub fn flume_shim(module: &str) -> Result<PathBuf, String> {
    let dir = shim_dir()?;
    let alias = module.rsplit('.').next().unwrap_or(module);
    let source = format!("import {module} as {alias}\n\nfn main() with IO {{ {alias}.main() }}\n");
    let shim = dir.join(format!("{}.flx", alias.to_lowercase()));
    write_if_changed(&shim, &source)?;
    Ok(shim)
}

/// Shims default to the user's cache directory so `flux init` works before a
/// project (and therefore a project cache) exists.
fn shim_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join("flux-flume-shims");
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base)
}

fn write_if_changed(path: &Path, source: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(source) {
        std::fs::write(path, source).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Resolve the package roots declared by the `flux.toml` at `project_dir`.
///
/// Returns `None` when there is no manifest, when the guard is set, or when
/// the resolver could not be run at all — every one of which means "this is
/// not a package build", and the caller falls back to unscoped roots.
///
/// A manifest that exists but is *invalid* is different: that is a real error
/// the user must see, so it is returned as `Err`.
pub(crate) fn resolve_project_roots(
    project_dir: &Path,
    cache_dir: &Path,
) -> Option<Result<Vec<ModuleRoot>, String>> {
    if std::env::var_os(FLUX_SKIP_MANIFEST_ENV).is_some() {
        return None;
    }
    if !project_dir.join("flux.toml").is_file() {
        return None;
    }

    // Resolving spawns a compile of the Flux resolver, which dominates an
    // otherwise fully-cached build. The result only depends on the manifests
    // it read, so it is cached against their contents.
    // `--locked` and `--offline` are checks, and a cached result would skip
    // them: a lockfile that stopped matching its manifest must fail under
    // `--locked` even when the previous run's roots are still on disk.
    let cache_file = roots_cache_path(cache_dir, project_dir);
    if mode_flags().is_empty()
        && let Some(cached) = read_cached_roots(&cache_file, project_dir)
    {
        return Some(Ok(cached));
    }

    let shim = flume_shim("Flume.Roots").ok()?;
    let exe = std::env::current_exe().ok()?;

    let mut child = Command::new(exe)
        .arg(&shim)
        .arg("--quiet")
        .arg("--cache-dir")
        .arg(cache_dir)
        .arg("--")
        .arg(project_dir)
        .args(mode_flags())
        .env(FLUX_SKIP_MANIFEST_ENV, "1")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    // Read stdout as it arrives rather than after the child exits: a git
    // dependency announces itself before cloning, and a download that takes
    // seconds must say so while it is happening. Records accumulate here and
    // are parsed once the stream ends.
    let mut records = String::new();
    let mut spinner: Option<Spinner> = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !report_progress(&line, &mut spinner) {
                records.push_str(&line);
                records.push('\n');
            }
        }
    }
    // A resolver that failed mid-fetch never printed the matching `fetched`
    // line, so the spinner is still running. Dropping it erases the line.
    drop(spinner);

    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return Some(Err(format!(
            "could not run the manifest resolver: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let resolved = parse_records(&records);
    if let Ok(roots) = &resolved {
        write_cached_roots(&cache_file, project_dir, roots);
    }
    Some(resolved)
}

/// Render one streamed progress line, reporting whether it was one.
///
/// Progress is printed as it arrives rather than parsed with the records: it
/// describes work in flight, and a download that takes seconds has to say so
/// while it is happening. A line this does not recognise is a record and is
/// returned to the caller to accumulate.
fn report_progress(line: &str, spinner: &mut Option<Spinner>) -> bool {
    let line = unquote_line(line.trim());
    if let Some(url) = line.strip_prefix("fetching\t") {
        eprintln!("{:>12} {url}", "Updating");
        // Held across the clone, which is the whole silent stretch: the child
        // prints nothing again until the download lands.
        *spinner = Some(Spinner::start("fetching…"));
        return true;
    }
    if let Some(rest) = line.strip_prefix("fetched\t") {
        // Stop before printing, so the erase cannot land on top of the line
        // that reports the result.
        spinner.take();
        let (url, commit) = rest.split_once('\t').unwrap_or((rest, ""));
        let short = &commit[..commit.len().min(7)];
        eprintln!("{:>12} {url} ({short})", "Fetched");
        return true;
    }
    false
}

/// Strip `print`'s wrapping quotes from a single streamed line.
///
/// Each record arrives as its own `print`, so the quotes wrap the line rather
/// than the whole stream, and the tab separators survive unescaped.
fn unquote_line(line: &str) -> &str {
    line.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(line)
}

/// Where the resolved roots for `project_dir` are cached.
///
/// The path is keyed on the project directory so two projects sharing a cache
/// root do not collide.
fn roots_cache_path(cache_dir: &Path, project_dir: &Path) -> PathBuf {
    let key = hash_bytes(project_dir.to_string_lossy().as_bytes());
    cache_dir
        .join("manifest")
        .join(format!("roots-{}.txt", hex(&key[..8])))
}

/// A cache entry: the epoch, then one line per manifest fingerprint, then the
/// records themselves. Reading it back re-checks every fingerprint, so editing
/// *any* manifest in the dependency graph invalidates the entry.
fn read_cached_roots(cache_file: &Path, project_dir: &Path) -> Option<Vec<ModuleRoot>> {
    let lock_path = project_dir.join("flux.lock");
    let text = std::fs::read_to_string(cache_file).ok()?;
    let mut lines = text.lines();
    if lines.next()? != epoch_line() {
        return None;
    }

    let mut records = String::new();
    for line in lines {
        match line.split_once('\t') {
            // `manifest<TAB><path><TAB><hash>`: still current?
            Some(("manifest", rest)) => {
                let (path, hash) = rest.split_once('\t')?;
                if manifest_fingerprint(Path::new(path))? != hash {
                    return None;
                }
            }
            // `lock<TAB><hash|absent>`: has the lockfile appeared, changed, or
            // been deleted since this entry was written?
            Some(("lock", recorded)) => {
                if fingerprint_or_absent(&lock_path) != recorded {
                    return None;
                }
            }
            _ => {
                records.push_str(line);
                records.push('\n');
            }
        }
    }
    parse_records(&records).ok()
}

fn write_cached_roots(cache_file: &Path, project_dir: &Path, roots: &[ModuleRoot]) {
    let Some(parent) = cache_file.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }

    let mut out = String::new();
    out.push_str(&epoch_line());
    out.push('\n');

    // The lockfile is an input, not an output, as far as caching goes: it
    // decides which version of a registry dependency the resolver settles on.
    // Its *absence* is recorded too, so deleting it re-resolves rather than
    // replaying the versions it used to pin.
    out.push_str(&format!(
        "lock	{}
",
        fingerprint_or_absent(&project_dir.join("flux.lock"))
    ));

    // Every manifest that could change the answer: the project's own, and one
    // per resolved package (a path dependency's manifest names its own deps).
    let mut manifests: Vec<PathBuf> = vec![project_dir.join("flux.toml")];
    for root in roots {
        // A package root is `<pkg>/src`, so its manifest sits one level up;
        // a package laid out flat roots at the package directory itself.
        for dir in [root.path.parent(), Some(root.path.as_path())]
            .into_iter()
            .flatten()
        {
            let candidate = dir.join("flux.toml");
            if candidate.is_file() && !manifests.contains(&candidate) {
                manifests.push(candidate);
            }
        }
    }
    for manifest in &manifests {
        let Some(hash) = manifest_fingerprint(manifest) else {
            // A manifest we cannot read is a manifest we cannot invalidate on.
            return;
        };
        out.push_str(&format!("manifest\t{}\t{hash}\n", manifest.display()));
    }

    for root in roots {
        out.push_str(&format!(
            "ok\t{}\t{}\t{}\n",
            root.package.as_deref().unwrap_or_default(),
            root.namespace.as_deref().unwrap_or_default(),
            root.path.display()
        ));
    }
    let _ = std::fs::write(cache_file, out);
}

/// Entries are invalidated wholesale when the cache epoch moves.
fn epoch_line() -> String {
    format!("epoch {}", crate::shared::cache_paths::CACHE_EPOCH)
}

fn manifest_fingerprint(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex(&hash_bytes(&bytes)[..8]))
}

/// A file's fingerprint, or a marker meaning it was not there.
///
/// A file that does not exist is as much a cache input as one that does: a
/// resolution made without a lockfile must not be replayed once one appears,
/// and vice versa.
fn fingerprint_or_absent(path: &Path) -> String {
    manifest_fingerprint(path).unwrap_or_else(|| "absent".to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse the resolver's `ok`/`err` records into scoped roots.
///
/// The program prints a single quoted string, so surrounding quotes and
/// escaped tabs/newlines are unwrapped before the records are read.
fn parse_records(stdout: &str) -> Result<Vec<ModuleRoot>, String> {
    let mut roots = Vec::new();
    for line in unquote(stdout.trim()).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        match fields.next() {
            Some("ok") => {
                let (Some(package), Some(namespace), Some(dir)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    return Err(format!("malformed root record: {line}"));
                };
                roots.push(ModuleRoot::package(PathBuf::from(dir), namespace, package));
            }
            Some("err") => {
                return Err(fields.collect::<Vec<_>>().join("\t"));
            }
            _ => return Err(format!("malformed root record: {line}")),
        }
    }
    Ok(roots)
}

/// Undo `print`'s string rendering: strip the wrapping quotes and turn the
/// escaped separators back into real ones.
fn unquote(text: &str) -> String {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text);
    inner.replace("\\t", "\t").replace("\\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::{parse_records, unquote};

    #[test]
    fn parses_ok_records_into_scoped_roots() {
        let roots =
            parse_records("\"ok\\tapp\\tApp\\t./src\\nok\\tshared\\tShared\\t../shared/src\"")
                .expect("expected records");
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].namespace.as_deref(), Some("App"));
        assert_eq!(roots[1].namespace.as_deref(), Some("Shared"));
        assert!(roots[1].path.ends_with("shared/src"));
    }

    #[test]
    fn surfaces_the_resolvers_error_message() {
        let err = parse_records("\"err\\tregistry dependency `json` is not supported\"")
            .expect_err("expected an error");
        assert!(err.contains("registry dependency"), "{err}");
    }

    #[test]
    fn rejects_a_record_it_cannot_read() {
        assert!(parse_records("\"what\\tis\\tthis\"").is_err());
    }

    #[test]
    fn unquote_leaves_bare_text_alone() {
        assert_eq!(unquote("ok\tA\tb"), "ok\tA\tb");
    }
}
