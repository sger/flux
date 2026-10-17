//! Compare two sweeps, a base (say `main`) and a head (the branch), file by
//! file. Reports:
//!
//! - newly rejected: ok in base, anything else in head
//! - new internal: ice, panic, timeout or crash in head, not in base
//! - newly accepted: anything else in base, ok in head
//! - codes changed: rejected in both, with different diagnostic codes
//! - only in base / only in head
//!
//! A file newly rejected or newly internal fails the comparison.
//!
//! With `content`, a file `ok` on both sides is also compared by its dump's
//! hash (`hashes.tsv`). For the first few that changed, both sweeps' own
//! compilers are rerun on the file to show the first line that differs. The
//! full list goes to `<head>/content_diff.txt`. What a changed dump means is
//! the caller's to decide: verify notes it, or fails on it with `--cmp`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::sweep::{self, DumpHash, Mode};

pub const INTERNAL: [&str; 4] = ["ice", "panic", "timeout", "crash"];

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub status: String,
    pub codes: String,
}

pub type Sweep = BTreeMap<String, Run>;

/// Reads a sweep's `results.tsv`: `file\tstatus\texit\tcodes\tms` per line.
pub fn read_sweep(dir: &Path) -> Result<Sweep, String> {
    let tsv = dir.join("results.tsv");
    let text = fs::read_to_string(&tsv)
        .map_err(|e| format!("no results.tsv in {}; is it a sweep? ({e})", dir.display()))?;
    Ok(parse_results(&text))
}

pub fn parse_results(text: &str) -> Sweep {
    text.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(parse_line)
        .collect()
}

fn parse_line(line: &str) -> Option<(String, Run)> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() {
        return None;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    let field = |i: usize| fields.get(i).copied().unwrap_or("").to_string();
    Some((
        field(0),
        Run {
            status: field(1),
            codes: field(3),
        },
    ))
}

#[derive(Debug, Default)]
pub struct Report {
    pub newly_rejected: Vec<String>,
    pub new_internal: Vec<String>,
    pub newly_accepted: Vec<String>,
    pub codes_changed: Vec<String>,
    pub only_base: Vec<String>,
    pub only_head: Vec<String>,
}

impl Report {
    pub fn failed(&self) -> bool {
        !self.newly_rejected.is_empty() || !self.new_internal.is_empty()
    }

    pub fn print(&self) {
        let sections = [
            ("newly rejected", &self.newly_rejected),
            ("new internal", &self.new_internal),
            ("newly accepted", &self.newly_accepted),
            ("codes changed", &self.codes_changed),
            ("only in base", &self.only_base),
            ("only in head", &self.only_head),
        ];
        for (title, lines) in sections {
            println!("{title}: {}", lines.len());
            for line in lines {
                println!("  {line}");
            }
        }
    }
}

fn is_internal(status: &str) -> bool {
    INTERNAL.contains(&status)
}

pub fn compare(base: &Sweep, head: &Sweep) -> Report {
    let mut report = Report::default();
    for (file, h) in head {
        let Some(b) = base.get(file) else {
            report.only_head.push(file.clone());
            continue;
        };
        if is_internal(&h.status) && !is_internal(&b.status) {
            report
                .new_internal
                .push(format!("{} -> {}  {file}", b.status, h.status));
        } else if b.status == "ok" && h.status != "ok" {
            report
                .newly_rejected
                .push(format!("ok -> {} [{}]  {file}", h.status, h.codes));
        } else if b.status != "ok" && h.status == "ok" {
            report
                .newly_accepted
                .push(format!("{} [{}] -> ok  {file}", b.status, b.codes));
        } else if b.status != "ok" && h.codes != b.codes {
            report
                .codes_changed
                .push(format!("[{}] -> [{}]  {file}", b.codes, h.codes));
        }
    }
    report.only_base = base
        .keys()
        .filter(|file| !head.contains_key(*file))
        .cloned()
        .collect();
    report
}

/// What comparing two sweeps found.
#[derive(Debug, Default)]
pub struct Comparison {
    /// A file was newly rejected or newly internal.
    pub failed: bool,
    /// Files `ok` on both sides whose dumps differ; empty without `content`.
    pub changed: Vec<String>,
}

impl Comparison {
    /// The file listing every changed dump, in the head sweep.
    pub fn diff_file(head: &Path) -> PathBuf {
        head.join("content_diff.txt")
    }
}

/// How many changed files get their first differing line shown.
const SHOWN_DIFFS: usize = 20;

/// Compares the sweeps in two directories, by status and, with `content`,
/// by dump, and prints the report.
pub fn run(root: &Path, base: &Path, head: &Path, content: bool) -> Result<Comparison, String> {
    let (base_sweep, head_sweep) = (read_sweep(base)?, read_sweep(head)?);
    let report = compare(&base_sweep, &head_sweep);
    println!("base: {}", base.display());
    println!("head: {}", head.display());
    report.print();
    let mut comparison = Comparison {
        failed: report.failed(),
        changed: Vec::new(),
    };
    if !content {
        return Ok(comparison);
    }
    let (mode, base_hashes) = read_hashes(base)?;
    let (_, head_hashes) = read_hashes(head)?;
    comparison.changed = changed_dumps(&base_sweep, &head_sweep, &base_hashes, &head_hashes);
    println!("dumps changed: {}", comparison.changed.len());
    for file in comparison.changed.iter().take(SHOWN_DIFFS) {
        println!("  {file}");
        match show_difference(root, base, head, mode, file) {
            Ok(lines) => lines.iter().for_each(|line| println!("    {line}")),
            Err(e) => println!("    {e}"),
        }
    }
    if comparison.changed.len() > SHOWN_DIFFS {
        println!("  … and {} more", comparison.changed.len() - SHOWN_DIFFS);
    }
    let mut list = comparison.changed.join("\n");
    if !list.is_empty() {
        list.push('\n');
    }
    let diff_file = Comparison::diff_file(head);
    fs::write(&diff_file, list).map_err(|e| format!("{}: {e}", diff_file.display()))?;
    Ok(comparison)
}

pub type Hashes = BTreeMap<String, DumpHash>;

/// Reads a sweep's `hashes.tsv`, with the mode its header names.
fn read_hashes(dir: &Path) -> Result<(Mode, Hashes), String> {
    let path = dir.join("hashes.tsv");
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "no hashes.tsv in {}; sweep it again with this xtask to compare content ({e})",
            dir.display()
        )
    })?;
    let (mode, hashes) = parse_hashes(&text);
    let mode = mode
        .and_then(|name| Mode::parse(&name))
        .ok_or_else(|| format!("{} names no known mode", path.display()))?;
    Ok((mode, hashes))
}

/// Parses `hashes.tsv`: the `# mode <name>` header, then
/// `file, fnv (hex), bytes, lines` per line. Unreadable lines are skipped.
pub fn parse_hashes(text: &str) -> (Option<String>, Hashes) {
    let mut mode = None;
    let mut hashes = Hashes::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("# mode ") {
            mode = Some(name.trim().to_string());
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [file, fnv, bytes, lines] = fields[..] else {
            continue;
        };
        let (Ok(fnv), Ok(bytes), Ok(lines)) =
            (u64::from_str_radix(fnv, 16), bytes.parse(), lines.parse())
        else {
            continue;
        };
        hashes.insert(file.to_string(), DumpHash { fnv, bytes, lines });
    }
    (mode, hashes)
}

/// The files `ok` in both sweeps whose dumps differ. A file rejected on
/// either side is the status comparison's business, not this one's.
pub fn changed_dumps(
    base: &Sweep,
    head: &Sweep,
    base_hashes: &Hashes,
    head_hashes: &Hashes,
) -> Vec<String> {
    head_hashes
        .iter()
        .filter(|(file, after)| {
            let ok = |sweep: &Sweep| sweep.get(*file).is_some_and(|run| run.status == "ok");
            ok(base)
                && ok(head)
                && base_hashes
                    .get(*file)
                    .is_some_and(|before| before != *after)
        })
        .map(|(file, _)| file.clone())
        .collect()
}

/// Reruns both sweeps' compilers on `file` and describes where the dumps
/// first differ.
fn show_difference(
    root: &Path,
    base: &Path,
    head: &Path,
    mode: Mode,
    file: &str,
) -> Result<Vec<String>, String> {
    let dump = |dir: &Path| {
        let flux = sweep::compiler_in(dir);
        if !flux.is_file() {
            return Err(format!("no compiler kept in {}", dir.display()));
        }
        Ok(sweep::run_flux(root, &flux, mode.flags(), file, Duration::from_secs(120)).stdout)
    };
    let (before, after) = (dump(base)?, dump(head)?);
    let Some(diff) = first_difference(&before, &after) else {
        return Ok(vec![
            "rerun gives equal dumps: the hash differed, so the output is not deterministic"
                .to_string(),
        ]);
    };
    let mut lines = vec![format!("first difference at line {}", diff.line)];
    lines.extend(diff.base.iter().map(|l| format!("- {l}")));
    lines.extend(diff.head.iter().map(|l| format!("+ {l}")));
    Ok(lines)
}

/// Where two texts first differ: the 1-based line, and up to
/// [`CONTEXT`] lines from each side starting there.
#[derive(Debug, PartialEq, Eq)]
pub struct Difference {
    pub line: usize,
    pub base: Vec<String>,
    pub head: Vec<String>,
}

const CONTEXT: usize = 3;

pub fn first_difference(base: &str, head: &str) -> Option<Difference> {
    let (b, h): (Vec<&str>, Vec<&str>) = (base.lines().collect(), head.lines().collect());
    let at = (0..b.len().max(h.len())).find(|&i| b.get(i) != h.get(i))?;
    let from = |lines: &[&str]| -> Vec<String> {
        lines
            .iter()
            .skip(at)
            .take(CONTEXT)
            .map(|l| (*l).to_string())
            .collect()
    };
    Some(Difference {
        line: at + 1,
        base: from(&b),
        head: from(&h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sweep(rows: &[(&str, &str, &str)]) -> Sweep {
        rows.iter()
            .map(|(file, status, codes)| {
                (
                    file.to_string(),
                    Run {
                        status: status.to_string(),
                        codes: codes.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn parses_results_with_empty_codes_bom_and_crlf() {
        let parsed =
            parse_results("\u{feff}a.flx\tok\t0\t\t12\r\nb.flx\terror\t1\tE300,E301\t9\r\n\r\n");
        assert_eq!(
            parsed,
            sweep(&[("a.flx", "ok", ""), ("b.flx", "error", "E300,E301")])
        );
    }

    #[test]
    fn sorts_each_file_into_one_category() {
        let base = sweep(&[
            ("same.flx", "ok", ""),
            ("rejected.flx", "ok", ""),
            ("internal.flx", "error", "E300"),
            ("still_internal.flx", "panic", ""),
            ("accepted.flx", "error", "E300"),
            ("codes.flx", "error", "E300"),
            ("gone.flx", "ok", ""),
        ]);
        let head = sweep(&[
            ("same.flx", "ok", ""),
            ("rejected.flx", "error", "E301"),
            ("internal.flx", "ice", ""),
            ("still_internal.flx", "ice", ""),
            ("accepted.flx", "ok", ""),
            ("codes.flx", "error", "E301"),
            ("new.flx", "ok", ""),
        ]);
        let report = compare(&base, &head);
        assert_eq!(report.newly_rejected, ["ok -> error [E301]  rejected.flx"]);
        assert_eq!(report.new_internal, ["error -> ice  internal.flx"]);
        assert_eq!(report.newly_accepted, ["error [E300] -> ok  accepted.flx"]);
        // Internal in both: its codes are empty in both, so nothing to report.
        assert_eq!(report.codes_changed, ["[E300] -> [E301]  codes.flx"]);
        assert_eq!(report.only_base, ["gone.flx"]);
        assert_eq!(report.only_head, ["new.flx"]);
        assert!(report.failed());
    }

    fn hash(n: u64) -> DumpHash {
        DumpHash {
            fnv: n,
            bytes: 1,
            lines: 1,
        }
    }

    #[test]
    fn finds_the_first_differing_line_with_context() {
        assert_eq!(first_difference("a\nb\nc", "a\nb\nc"), None);
        assert_eq!(
            first_difference("a\nb\nc\nd\ne", "a\nB\nc\nd\ne"),
            Some(Difference {
                line: 2,
                base: vec!["b".into(), "c".into(), "d".into()],
                head: vec!["B".into(), "c".into(), "d".into()],
            })
        );
        // A dump that stops early differs where the longer one goes on.
        assert_eq!(
            first_difference("a\nb", "a\nb\nc"),
            Some(Difference {
                line: 3,
                base: vec![],
                head: vec!["c".into()],
            })
        );
    }

    #[test]
    fn reads_back_the_hashes_a_sweep_writes() {
        let (mode, hashes) = parse_hashes(
            "# mode core-opt\na.flx\t00000000000000ff\t10\t2\nbroken line\nb.flx\tzz\t1\t1\n",
        );
        assert_eq!(mode.as_deref(), Some("core-opt"));
        assert_eq!(hashes.len(), 1);
        assert_eq!(
            hashes["a.flx"],
            DumpHash {
                fnv: 0xff,
                bytes: 10,
                lines: 2
            }
        );
    }

    #[test]
    fn only_files_ok_on_both_sides_count_as_changed_dumps() {
        let base = sweep(&[
            ("same.flx", "ok", ""),
            ("changed.flx", "ok", ""),
            ("rejected.flx", "error", "E300"),
        ]);
        let head = sweep(&[
            ("same.flx", "ok", ""),
            ("changed.flx", "ok", ""),
            ("rejected.flx", "ok", ""),
            ("new.flx", "ok", ""),
        ]);
        let before: Hashes = [("same.flx", 1), ("changed.flx", 2), ("rejected.flx", 3)]
            .into_iter()
            .map(|(f, n)| (f.to_string(), hash(n)))
            .collect();
        let after: Hashes = [
            ("same.flx", 1),
            ("changed.flx", 9),
            ("rejected.flx", 9),
            ("new.flx", 9),
        ]
        .into_iter()
        .map(|(f, n)| (f.to_string(), hash(n)))
        .collect();
        assert_eq!(
            changed_dumps(&base, &head, &before, &after),
            ["changed.flx"]
        );
    }

    #[test]
    fn identical_sweeps_pass() {
        let base = sweep(&[("a.flx", "ok", ""), ("b.flx", "error", "E300")]);
        let report = compare(&base, &base.clone());
        assert!(!report.failed());
        assert!(report.newly_accepted.is_empty() && report.codes_changed.is_empty());
    }
}
