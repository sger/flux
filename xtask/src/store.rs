//! Where verify keeps its outputs: one folder per run tag, under
//! `.flux-cache/verify/` at the repository root. `.gitignore` lists
//! `.flux-cache/`, so runs are never committed, and `cargo clean` leaves them
//! alone. `FLUX_VERIFY_OUT` puts them elsewhere. `cargo xtask prune` deletes
//! old runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The folder holding every run of the checkout at `root`.
pub fn store_root(root: &Path) -> PathBuf {
    std::env::var_os("FLUX_VERIFY_OUT")
        .map_or_else(|| root.join(".flux-cache").join("verify"), PathBuf::from)
}

/// The folder for the run tagged `tag`, which must be one plain name.
pub fn run_dir(root: &Path, tag: &str) -> Result<PathBuf, String> {
    let plain = !tag.is_empty()
        && tag != "."
        && tag != ".."
        && !tag.contains(['/', '\\', ':'])
        && !tag.starts_with('-');
    if plain {
        Ok(store_root(root).join(tag))
    } else {
        Err(format!(
            "`{tag}` cannot be a tag: use a plain name such as s4 or main"
        ))
    }
}

/// `path` with the home folder shown as `~`.
pub fn display(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// One stored run.
#[derive(Debug)]
pub struct Run {
    pub tag: String,
    pub path: PathBuf,
    pub modified: SystemTime,
    pub bytes: u64,
}

/// Every stored run, newest first.
fn runs(store: &Path) -> Vec<Run> {
    let Ok(entries) = fs::read_dir(store) else {
        return Vec::new();
    };
    let mut runs: Vec<Run> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let path = entry.path();
            Run {
                tag: entry.file_name().to_string_lossy().into_owned(),
                modified: newest_change(&path),
                bytes: size_of(&path),
                path,
            }
        })
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.modified));
    runs
}

/// The latest modification time of anything under `path`: a run's folder is
/// older than the files a later rerun of the same tag rewrites.
fn newest_change(path: &Path) -> SystemTime {
    let own = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(entries) = fs::read_dir(path) else {
        return own;
    };
    entries
        .flatten()
        .map(|entry| {
            let child = entry.path();
            if child.is_dir() {
                newest_change(&child)
            } else {
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH)
            }
        })
        .fold(own, SystemTime::max)
}

fn size_of(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return fs::metadata(path).map_or(0, |m| m.len());
    };
    entries
        .flatten()
        .map(|entry| {
            let child = entry.path();
            if child.is_dir() {
                size_of(&child)
            } else {
                entry.metadata().map_or(0, |m| m.len())
            }
        })
        .sum()
}

/// The runs to delete: all but the newest `keep`, never one tagged in
/// `keep_tags`. `runs` is newest first.
fn to_prune<'a>(runs: &'a [Run], keep: usize, keep_tags: &[String]) -> Vec<&'a Run> {
    runs.iter()
        .filter(|run| !keep_tags.contains(&run.tag))
        .skip(keep)
        .collect()
}

/// Lists the stored runs and what pruning would delete; deletes them with
/// `yes`. Returns whether every deletion succeeded.
pub fn prune(root: &Path, keep: usize, keep_tags: &[String], yes: bool) -> bool {
    let store = store_root(root);
    let runs = runs(&store);
    println!("runs in {}: {}", display(&store), runs.len());
    let doomed = to_prune(&runs, keep, keep_tags);
    for run in &runs {
        let fate = if doomed.iter().any(|d| d.tag == run.tag) {
            "delete"
        } else {
            "keep  "
        };
        println!("  {fate} {:<24} {:>8}", run.tag, megabytes(run.bytes));
    }
    let freed: u64 = doomed.iter().map(|run| run.bytes).sum();
    if doomed.is_empty() {
        println!("nothing to prune");
        return true;
    }
    if !yes {
        println!(
            "would free {} from {} run(s); run again with --yes to delete",
            megabytes(freed),
            doomed.len()
        );
        return true;
    }
    let mut ok = true;
    for run in doomed {
        if let Err(e) = fs::remove_dir_all(&run.path) {
            println!("could not delete {}: {e}", display(&run.path));
            ok = false;
        }
    }
    println!("freed {}", megabytes(freed));
    ok
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn run(tag: &str, age: u64) -> Run {
        Run {
            tag: tag.to_string(),
            path: PathBuf::from(tag),
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 - age),
            bytes: 1,
        }
    }

    #[test]
    fn prunes_all_but_the_newest_and_the_protected() {
        // Newest first, as `runs` returns them.
        let runs = [
            run("s4", 1),
            run("s3", 2),
            run("main", 3),
            run("s2", 4),
            run("s1", 5),
        ];
        let tags = |list: Vec<&Run>| list.iter().map(|r| r.tag.clone()).collect::<Vec<_>>();
        assert_eq!(tags(to_prune(&runs, 2, &[])), ["main", "s2", "s1"]);
        assert_eq!(tags(to_prune(&runs, 2, &["main".into()])), ["s2", "s1"]);
        assert!(to_prune(&runs, 9, &[]).is_empty());
    }

    #[test]
    fn a_tag_is_one_plain_name() {
        let root = Path::new("/repo");
        assert!(run_dir(root, "s4").is_ok());
        assert!(run_dir(root, "debugger-s10a").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "--base"] {
            assert!(run_dir(root, bad).is_err(), "{bad:?}");
        }
    }
}
