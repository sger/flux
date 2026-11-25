//! The Markdown report of one benchmark, the summary of a group, and the
//! README block each report is copied into.

use std::fmt::Write as _;

use super::hyperfine::Case;
use super::stats::{geomean, mann_whitney_p, mean, stddev};

pub struct Report<'a> {
    pub title: &'a str,
    pub generated: String,
    pub platform: &'a str,
    pub runs: String,
    pub warmup: usize,
    /// The size every program ran with, and whether it is the smoke or
    /// the full one.
    pub workload: String,
    pub toolchains: &'a [(String, String)],
    pub cases: &'a [Case],
    /// The baseline's path, as the report shows it, and its cases.
    pub baseline: Option<(String, &'a [Case])>,
    pub metrics: &'a [(String, u64)],
    pub baseline_metrics: &'a [(String, u64)],
    pub vm: &'a str,
    pub native: &'a str,
    /// The direct-style twin's cases, when the benchmark has one.
    pub vm_direct: &'a str,
    pub native_direct: &'a str,
}

/// One benchmark's line in a group's summary.
pub struct Outcome {
    pub name: String,
    pub passed: bool,
    pub vm_vs_baseline: Option<f64>,
    pub native_vs_baseline: Option<f64>,
    pub vm_over_native: Option<f64>,
    /// The effectful program's time over its direct-style twin's.
    pub vm_overhead: Option<f64>,
    pub native_overhead: Option<f64>,
}

impl Outcome {
    pub fn failed(name: &str) -> Outcome {
        Outcome {
            name: name.to_string(),
            passed: false,
            vm_vs_baseline: None,
            native_vs_baseline: None,
            vm_over_native: None,
            vm_overhead: None,
            native_overhead: None,
        }
    }
}

pub fn render(r: &Report<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {} Benchmark Report\n", r.title);
    let _ = writeln!(out, "- Generated: {}", r.generated);
    let _ = writeln!(out, "- Machine: {}", r.platform);
    let _ = writeln!(out, "- Runs: {}, warmup: {}", r.runs, r.warmup);
    let _ = writeln!(out, "- Workload: {}", r.workload);
    if !r.toolchains.is_empty() {
        // `rustc -V` and `go version` already start with the tool's name.
        let label = |(name, version): &(String, String)| {
            if version.to_ascii_lowercase().starts_with(name.as_str()) {
                version.replacen("go version ", "", 1)
            } else {
                format!("{name} {version}")
            }
        };
        let list: Vec<String> = r.toolchains.iter().map(label).collect();
        let _ = writeln!(out, "- Toolchains: {}", list.join("; "));
    }

    let reference = find(r.cases, r.native).or_else(|| r.cases.first()).map(|c| mean(&c.times));
    let reference_name = if find(r.cases, r.native).is_some() { "native" } else { "first" };
    let _ = writeln!(
        out,
        "\n| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × {reference_name} |"
    );
    let _ = writeln!(out, "|:---|---:|---:|---:|---:|---:|---:|---:|");
    for case in r.cases {
        let m = mean(&case.times);
        let min = case.times.iter().copied().fold(f64::INFINITY, f64::min);
        let max = case.times.iter().copied().fold(0.0, f64::max);
        let factor = reference.map_or(String::new(), |base| format!("{:.2}×", m / base));
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} |",
            case.name,
            seconds(m),
            seconds(stddev(&case.times)),
            seconds(min),
            seconds(max),
            case.peak_rss.map_or("–".into(), bytes),
            case.instructions.map_or("–".into(), count),
            factor,
        );
    }
    if let Some(ratio) = vm_over_native(r.cases, r.vm, r.native) {
        let _ = writeln!(out, "\nVM / native: {ratio:.2}×");
    }
    let vm_overhead = over(r.cases, r.vm, r.vm_direct);
    let native_overhead = over(r.cases, r.native, r.native_direct);
    if vm_overhead.is_some() || native_overhead.is_some() {
        let shown = |r: Option<f64>| r.map_or("–".to_string(), |r| format!("{r:.2}×"));
        let _ = writeln!(
            out,
            "\nEffect overhead (with handlers / direct style): VM {}, native {}",
            shown(vm_overhead),
            shown(native_overhead),
        );
    }

    if let Some((path, baseline)) = &r.baseline {
        render_baseline(&mut out, path, r.cases, baseline);
    } else {
        let _ =
            writeln!(out, "\nNo baseline for {} yet: record one with --save-baseline.", r.platform);
    }
    render_metrics(&mut out, r.metrics, r.baseline_metrics);
    out
}

fn render_baseline(out: &mut String, path: &str, cases: &[Case], baseline: &[Case]) {
    let _ = writeln!(out, "\n### Against the baseline\n\nBaseline: `{path}`\n");
    let _ = writeln!(
        out,
        "| Case | Mean now | Baseline | Now / baseline | p | Verdict | Instructions now / baseline |"
    );
    let _ = writeln!(out, "|:---|---:|---:|---:|---:|:---|---:|");
    let mut ratios = Vec::new();
    for case in cases {
        let Some(base) = find(baseline, &case.name) else { continue };
        let (now_mean, base_mean) = (mean(&case.times), mean(&base.times));
        let ratio = now_mean / base_mean;
        ratios.push(ratio);
        let p = mann_whitney_p(&case.times, &base.times);
        let verdict = match (p < 0.05, ratio > 1.0) {
            (false, _) => "within noise",
            (true, true) => "slower",
            (true, false) => "faster",
        };
        let instructions = match (case.instructions, base.instructions) {
            (Some(now), Some(then)) => format!("{:.4}×", now / then),
            _ => "–".into(),
        };
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {ratio:.3}× | {p:.3} | {verdict} | {instructions} |",
            case.name,
            seconds(now_mean),
            seconds(base_mean),
        );
    }
    if let Some(g) = geomean(&ratios) {
        let _ = writeln!(out, "\nGeomean of now / baseline: {g:.3}×");
    }
    let _ = writeln!(
        out,
        "\np is the Mann–Whitney U test on every run's time. Below 0.05 the change is unlikely to \
         be noise; with fewer than 4 runs a side it cannot get there. Instructions vary far less \
         than time, so a change there is a better sign than one in time."
    );
}

fn render_metrics(out: &mut String, metrics: &[(String, u64)], baseline: &[(String, u64)]) {
    if metrics.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n### VM counts\n\nFrom `flux --metrics` on the VM case. They are the same on every run \
         of the same compiler.\n"
    );
    let _ = writeln!(out, "| Count | Now | Baseline | Change |");
    let _ = writeln!(out, "|:---|---:|---:|---:|");
    for (key, now) in metrics {
        let then = baseline.iter().find(|(k, _)| k == key).map(|(_, v)| *v);
        let change = match then {
            Some(0) if *now == 0 => "0".to_string(),
            Some(0) => "new".to_string(),
            Some(then) => format!("{:+.2}%", (*now as f64 / then as f64 - 1.0) * 100.0),
            None => "–".to_string(),
        };
        let then = then.map_or("–".to_string(), |v| v.to_string());
        let _ = writeln!(out, "| `{key}` | {now} | {then} | {change} |");
    }
}

/// The table that ends a group run, with a geomean row. The effect
/// overhead columns appear when some benchmark has a direct-style twin.
pub fn summary(outcomes: &[Outcome]) -> String {
    let overheads = outcomes.iter().any(|o| o.vm_overhead.is_some() || o.native_overhead.is_some());
    let mut out = String::from(
        "## Summary\n\n| Benchmark | VM now / baseline | Native now / baseline | VM / native |",
    );
    let mut rule = String::from("|:---|---:|---:|---:|");
    if overheads {
        out.push_str(" VM effect overhead | Native effect overhead |");
        rule.push_str("---:|---:|");
    }
    let _ = writeln!(out, "\n{rule}");
    let ratio = |r: Option<f64>| r.map_or("–".to_string(), |r| format!("{r:.3}×"));
    let factor = |r: Option<f64>| r.map_or("–".to_string(), |r| format!("{r:.1}×"));
    let overhead = |r: Option<f64>| r.map_or("–".to_string(), |r| format!("{r:.2}×"));
    let row = |out: &mut String, name: &str, cells: [Option<f64>; 5]| {
        let _ = write!(
            out,
            "| {name} | {} | {} | {} |",
            ratio(cells[0]),
            ratio(cells[1]),
            factor(cells[2])
        );
        if overheads {
            let _ = write!(out, " {} | {} |", overhead(cells[3]), overhead(cells[4]));
        }
        out.push('\n');
    };
    for o in outcomes {
        let name = if o.passed { o.name.clone() } else { format!("{} (failed)", o.name) };
        row(
            &mut out,
            &name,
            [
                o.vm_vs_baseline,
                o.native_vs_baseline,
                o.vm_over_native,
                o.vm_overhead,
                o.native_overhead,
            ],
        );
    }
    let column = |pick: fn(&Outcome) -> Option<f64>| {
        geomean(&outcomes.iter().filter_map(pick).collect::<Vec<_>>())
    };
    row(
        &mut out,
        "**geomean**",
        [
            column(|o| o.vm_vs_baseline),
            column(|o| o.native_vs_baseline),
            column(|o| o.vm_over_native),
            column(|o| o.vm_overhead),
            column(|o| o.native_overhead),
        ],
    );
    out
}

pub fn find<'a>(cases: &'a [Case], name: &str) -> Option<&'a Case> {
    cases.iter().find(|c| c.name == name)
}

pub fn vm_over_native(cases: &[Case], vm: &str, native: &str) -> Option<f64> {
    over(cases, vm, native)
}

/// The mean time of the case `a` over that of `b`, when both ran.
pub fn over(cases: &[Case], a: &str, b: &str) -> Option<f64> {
    Some(mean(&find(cases, a)?.times) / mean(&find(cases, b)?.times))
}

/// Replaces what lies between `<!-- tag-report:start -->` and its end
/// marker; `None` when the markers are missing.
pub fn replace_block(readme: &str, tag: &str, block: &str) -> Option<String> {
    let start = format!("<!-- {tag}-report:start -->");
    let end = format!("<!-- {tag}-report:end -->");
    let from = readme.find(&start)? + start.len();
    let to = from + readme[from..].find(&end)?;
    Some(format!("{}\n{}{}", &readme[..from], block, &readme[to..]))
}

fn seconds(s: f64) -> String {
    if s >= 1.0 {
        format!("{s:.3} s")
    } else if s >= 0.01 {
        format!("{:.1} ms", s * 1e3)
    } else {
        format!("{:.2} ms", s * 1e3)
    }
}

fn bytes(b: f64) -> String {
    format!("{:.1} MiB", b / (1024.0 * 1024.0))
}

fn count(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.2} G", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} M", n / 1e6)
    } else {
        format!("{n:.0}")
    }
}

/// `YYYY-MM-DD HH:MM UTC` for seconds since the Unix epoch.
pub fn utc_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, times: &[f64]) -> Case {
        Case { name: name.into(), times: times.to_vec(), peak_rss: None, instructions: None }
    }

    #[test]
    fn timestamps_are_utc_calendar_dates() {
        assert_eq!(utc_timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_timestamp(1_795_595_488), "2026-11-25 08:31 UTC");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn a_readme_block_is_replaced_between_its_markers() {
        let readme = "a\n<!-- x-report:start -->\nold\n<!-- x-report:end -->\nb\n";
        assert_eq!(
            replace_block(readme, "x", "new\n").unwrap(),
            "a\n<!-- x-report:start -->\nnew\n<!-- x-report:end -->\nb\n"
        );
        assert_eq!(replace_block(readme, "y", "new\n"), None);
    }

    #[test]
    fn a_report_compares_with_its_baseline() {
        let cases = [case("b/flux", &[2.0, 2.1, 2.0, 2.1]), case("b/flux-native", &[0.1; 4])];
        let base = [case("b/flux", &[1.0, 1.1, 1.0, 1.1]), case("b/flux-native", &[0.1; 4])];
        let text = render(&Report {
            title: "B",
            generated: "now".into(),
            platform: "macos-aarch64",
            runs: "4".into(),
            warmup: 1,
            workload: "10 (smoke)".into(),
            toolchains: &[],
            cases: &cases,
            baseline: Some(("base.json".into(), &base)),
            metrics: &[("vm_allocs".into(), 110)],
            baseline_metrics: &[("vm_allocs".into(), 100)],
            vm: "b/flux",
            native: "b/flux-native",
            vm_direct: "b/flux-direct",
            native_direct: "b/flux-native-direct",
        });
        assert!(text.contains("VM / native: 20.50×"), "{text}");
        assert!(!text.contains("Effect overhead"), "{text}");
        assert!(text.contains("| slower |"), "{text}");
        assert!(text.contains("| within noise |"), "{text}");
        assert!(text.contains("| `vm_allocs` | 110 | 100 | +10.00% |"), "{text}");
    }

    #[test]
    fn a_report_with_a_twin_shows_the_effect_overhead() {
        let cases = [
            case("e/flux", &[3.0; 4]),
            case("e/flux-direct", &[2.0; 4]),
            case("e/flux-native", &[0.5; 4]),
            case("e/flux-native-direct", &[0.1; 4]),
        ];
        let text = render(&Report {
            title: "E",
            generated: "now".into(),
            platform: "macos-aarch64",
            runs: "4".into(),
            warmup: 1,
            workload: "10 (smoke)".into(),
            toolchains: &[],
            cases: &cases,
            baseline: None,
            metrics: &[],
            baseline_metrics: &[],
            vm: "e/flux",
            native: "e/flux-native",
            vm_direct: "e/flux-direct",
            native_direct: "e/flux-native-direct",
        });
        assert!(
            text.contains("Effect overhead (with handlers / direct style): VM 1.50×, native 5.00×"),
            "{text}"
        );
    }

    #[test]
    fn a_summary_gets_overhead_columns_only_when_a_benchmark_has_a_twin() {
        let plain = Outcome { vm_over_native: Some(100.0), ..Outcome::failed("tak") };
        assert!(!summary(&[plain]).contains("effect overhead"));

        let twin = Outcome {
            passed: true,
            vm_overhead: Some(2.0),
            native_overhead: Some(8.0),
            ..Outcome::failed("countdown")
        };
        let text = summary(&[Outcome { passed: true, ..Outcome::failed("fib") }, twin]);
        assert!(text.contains("| VM effect overhead | Native effect overhead |"), "{text}");
        assert!(text.contains("| countdown | – | – | – | 2.00× | 8.00× |"), "{text}");
        assert!(text.contains("| fib | – | – | – | – | – |"), "{text}");
        assert!(text.contains("| **geomean** | – | – | – | 2.00× | 8.00× |"), "{text}");
    }
}
