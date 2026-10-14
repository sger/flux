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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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

/// Compares the sweeps in two directories, prints the report and returns
/// whether it failed.
pub fn run(base: &Path, head: &Path) -> Result<bool, String> {
    let report = compare(&read_sweep(base)?, &read_sweep(head)?);
    println!("base: {}", base.display());
    println!("head: {}", head.display());
    report.print();
    Ok(report.failed())
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

    #[test]
    fn identical_sweeps_pass() {
        let base = sweep(&[("a.flx", "ok", ""), ("b.flx", "error", "E300")]);
        let report = compare(&base, &base.clone());
        assert!(!report.failed());
        assert!(report.newly_accepted.is_empty() && report.codes_changed.is_empty());
    }
}
