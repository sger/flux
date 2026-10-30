//! Compare two runs' `metrics.tsv`, a base and a head, count by count
//! (proposal 0190 §12). Each row is a fixture, a way, a metric and its value,
//! as `flux parity-check --metrics` writes them.
//!
//! A count that moves in its bad direction by more than its metric's
//! tolerance is worse; one that moves the other way is better. A count only
//! one side has, from a new fixture or a key a run left out, is counted but
//! not judged.

use std::collections::BTreeMap;

/// A row's key: the fixture, the way and the metric.
pub type Key = (String, String, String);

pub type Counts = BTreeMap<Key, u64>;

/// The file's first line, as parity-check writes it.
pub const HEADER: &str = "file\tway\tmetric\tvalue";

pub fn parse(text: &str) -> Counts {
    text.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_end_matches('\r').split('\t');
            let (file, way, metric, value) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let value = value.parse().ok()?;
            Some(((file.to_string(), way.to_string(), metric.to_string()), value))
        })
        .collect()
}

/// Which way a metric moves when the program got worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worse {
    Grows,
    Drops,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub worse: Worse,
    /// The share of the base value a count may move by and still count as
    /// unchanged.
    pub tolerance: f64,
}

/// The rule for `metric`. The allocator's counts move a little from run to
/// run, since it counts every thread; every other count is exact. A metric
/// this table does not know is judged as one that should not grow.
pub fn rule(metric: &str) -> Rule {
    match metric {
        "aether_reuses" => Rule { worse: Worse::Drops, tolerance: 0.0 },
        "compiler_allocs" | "vm_allocs" => Rule { worse: Worse::Grows, tolerance: 0.01 },
        _ => Rule { worse: Worse::Grows, tolerance: 0.0 },
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// `<file> <way> <metric> <base> -> <head> (<change>)`, in key order.
    pub worse: Vec<String>,
    pub better: Vec<String>,
    /// Counts only one side has.
    pub unmatched: usize,
}

pub fn compare(base: &Counts, head: &Counts) -> Report {
    let mut report = Report::default();
    for (key, &after) in head {
        let Some(&before) = base.get(key) else {
            report.unmatched += 1;
            continue;
        };
        let (file, way, metric) = key;
        let rule = rule(metric);
        if (after as f64 - before as f64).abs() <= before as f64 * rule.tolerance {
            continue;
        }
        let grew = after > before;
        let line = format!("{file} {way} {metric} {before} -> {after} ({})", change(before, after));
        if grew == (rule.worse == Worse::Grows) {
            report.worse.push(line);
        } else {
            report.better.push(line);
        }
    }
    report.unmatched += base.keys().filter(|key| !head.contains_key(*key)).count();
    report
}

/// `+21.7%`, or `new` when the base was 0.
fn change(before: u64, after: u64) -> String {
    if before == 0 {
        return "new".to_string();
    }
    format!("{:+.1}%", (after as f64 - before as f64) / before as f64 * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(rows: &[(&str, &str, u64)]) -> Counts {
        rows.iter()
            .map(|&(way, metric, value)| {
                (("t/a.flx".to_string(), way.to_string(), metric.to_string()), value)
            })
            .collect()
    }

    #[test]
    fn the_header_and_broken_lines_are_skipped() {
        let text = "\u{feff}file\tway\tmetric\tvalue\r\nt/a.flx\tvm\tcore_size\t40\r\nnot a row\nt/a.flx\tvm\tvm_allocs\tmany\n";
        assert_eq!(parse(text), counts(&[("vm", "core_size", 40)]));
    }

    #[test]
    fn an_exact_count_is_worse_when_it_grows_at_all() {
        let report = compare(
            &counts(&[("vm", "vm_instructions", 1980)]),
            &counts(&[("vm", "vm_instructions", 1981)]),
        );
        assert_eq!(report.worse, ["t/a.flx vm vm_instructions 1980 -> 1981 (+0.1%)"]);
        assert!(report.better.is_empty());
    }

    #[test]
    fn fewer_reuses_are_worse_and_more_are_better() {
        let base = counts(&[("vm", "aether_reuses", 4), ("vm_opt", "aether_reuses", 4)]);
        let head = counts(&[("vm", "aether_reuses", 3), ("vm_opt", "aether_reuses", 5)]);
        let report = compare(&base, &head);
        assert_eq!(report.worse, ["t/a.flx vm aether_reuses 4 -> 3 (-25.0%)"]);
        assert_eq!(report.better, ["t/a.flx vm_opt aether_reuses 4 -> 5 (+25.0%)"]);
    }

    #[test]
    fn allocations_may_move_by_one_percent() {
        let base = counts(&[("vm", "vm_allocs", 1000), ("vm_opt", "vm_allocs", 1000)]);
        let head = counts(&[("vm", "vm_allocs", 1010), ("vm_opt", "vm_allocs", 1011)]);
        assert_eq!(compare(&base, &head).worse, ["t/a.flx vm_opt vm_allocs 1000 -> 1011 (+1.1%)"]);
    }

    #[test]
    fn a_count_from_zero_is_new() {
        let report =
            compare(&counts(&[("vm", "aether_dups", 0)]), &counts(&[("vm", "aether_dups", 2)]));
        assert_eq!(report.worse, ["t/a.flx vm aether_dups 0 -> 2 (new)"]);
    }

    #[test]
    fn counts_only_one_side_has_are_not_judged() {
        let base = counts(&[("vm", "core_size", 40), ("vm", "vm_instructions", 9)]);
        let head = counts(&[("vm", "core_size", 40), ("vm_opt", "core_size", 31)]);
        let report = compare(&base, &head);
        assert_eq!(report, Report { worse: vec![], better: vec![], unmatched: 2 });
    }

    #[test]
    fn a_metric_the_rules_do_not_know_should_not_grow() {
        assert_eq!(rule("frobs"), Rule { worse: Worse::Grows, tolerance: 0.0 });
    }
}
