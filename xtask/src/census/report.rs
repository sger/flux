//! The census's summary, as console text and as a self-contained HTML page.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use super::{Census, Class, Diff, Entry, Filed, KnownIssues};
use crate::outcome::format_duration;
use crate::text::Paint;

/// Everything the summary shows.
pub struct Summary<'a> {
    pub tag: &'a str,
    pub took: Duration,
    pub out: &'a str,
    pub census: &'a Census,
    pub known: &'a KnownIssues,
    pub diff: Option<&'a Diff>,
    pub base: Option<&'a str>,
}

/// How many weakest features the console lists.
const WEAKEST: usize = 10;

/// A feature needs this many runs before its share counts as weak.
const MIN_RUNS: usize = 5;

/// How many newly failing rows the console lists.
const SHOWN_DIFFS: usize = 10;

/// Ok and failed runs, the parts of a share.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Share {
    pub ok: usize,
    pub failed: usize,
}

impl Share {
    fn add(&mut self, class: Class) {
        if class.is_ok() {
            self.ok += 1;
        } else if class.is_failure() {
            self.failed += 1;
        }
    }

    pub fn runs(self) -> usize {
        self.ok + self.failed
    }

    /// The ok share in percent, or `None` with no runs.
    #[allow(clippy::cast_precision_loss)] // run counts stay far below 2^52
    pub fn percent(self) -> Option<f64> {
        (self.runs() > 0).then(|| self.ok as f64 * 100.0 / self.runs() as f64)
    }

    /// The ok share in tenths, 0 to 10, for the matrix's colour.
    fn tenths(self) -> Option<usize> {
        (self.runs() > 0).then(|| self.ok * 10 / self.runs())
    }
}

impl Summary<'_> {
    fn way_share(&self, way: &str) -> Share {
        let mut share = Share::default();
        for e in self.census.entries.iter().filter(|e| e.row.way == way) {
            share.add(e.row.class);
        }
        share
    }

    /// Each feature's share over every way, weakest first; directories
    /// (`dir:` tags) apart from features.
    fn weakest(&self, directories: bool) -> Vec<(String, Share)> {
        let mut shares: BTreeMap<&str, Share> = BTreeMap::new();
        for e in &self.census.entries {
            for feature in &e.features {
                if feature.starts_with("dir:") == directories {
                    shares.entry(feature).or_default().add(e.row.class);
                }
            }
        }
        let mut weak: Vec<(String, Share)> = shares
            .into_iter()
            .filter(|(_, share)| share.failed > 0 && share.runs() >= MIN_RUNS)
            .map(|(feature, share)| (feature.to_string(), share))
            .collect();
        weak.sort_by(|a, b| {
            a.1.percent()
                .partial_cmp(&b.1.percent())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        weak
    }

    pub fn text(&self, paint: Paint) -> String {
        let mut text = String::new();
        let rule = "─".repeat(100);
        let _ = writeln!(text, "{}", paint.banner(&rule));
        let _ = writeln!(
            text,
            "{}",
            paint.banner(&format!(
                " census {} · {} · {}",
                self.tag,
                format_duration(self.took),
                self.out
            ))
        );
        let _ = writeln!(text, "{}", paint.banner(&rule));
        self.way_table(paint, &mut text);
        self.weakest_lines(paint, &mut text);
        self.totals(paint, &mut text);
        text
    }

    /// One line per way: its count of each class and its supported share.
    fn way_table(&self, paint: Paint, text: &mut String) {
        let classes = Class::ALL;
        let mut header = format!("{:<10}", "way");
        for class in classes {
            let _ = write!(header, "{:>15}", class.name());
        }
        // Pad before painting: colour codes would count towards the width.
        let _ = writeln!(
            text,
            "{}",
            paint.bold(&format!("{header}{:>11}", "supported"))
        );
        for way in &self.census.ways {
            let mut line = format!("{way:<10}");
            for class in classes {
                let n = self.census.count(way, class);
                let cell = format!("{n:>15}");
                let cell = if n == 0 {
                    paint.dim(&cell)
                } else if class.is_failure() {
                    paint.red(&cell)
                } else if class.is_ok() {
                    paint.green(&cell)
                } else {
                    paint.yellow(&cell)
                };
                line.push_str(&cell);
            }
            let share = self.way_share(way);
            let percent = share
                .percent()
                .map_or("-".to_string(), |p| format!("{p:.1}%"));
            let _ = writeln!(text, "{line}{}", paint.bold(&format!("{percent:>11}")));
        }
    }

    fn weakest_lines(&self, paint: Paint, text: &mut String) {
        for (title, directories) in [("weakest features", false), ("weakest directories", true)] {
            let weak = self.weakest(directories);
            if weak.is_empty() {
                continue;
            }
            let _ = writeln!(
                text,
                "\n{}",
                paint.bold(&format!("{title} (ok share over every way):"))
            );
            for (feature, share) in weak.iter().take(WEAKEST) {
                let _ = writeln!(
                    text,
                    "  {:<32} {:>6}  ({} of {} runs ok)",
                    feature.trim_start_matches("dir:"),
                    paint.red(&format!("{:.1}%", share.percent().unwrap_or(0.0))),
                    share.ok,
                    share.runs()
                );
            }
        }
    }

    /// Failures, how many are filed, backend gaps, the change since the base,
    /// and where the files are.
    fn totals(&self, paint: Paint, text: &mut String) {
        let failures = self
            .census
            .entries
            .iter()
            .filter(|e| e.row.class.is_failure())
            .count();
        let unfiled = self.census.unfiled().count();
        let resolved = self
            .census
            .unfiled()
            .filter(|e| matches!(e.filed, Filed::Resolved(_)))
            .count();
        let _ = writeln!(text);
        let _ = writeln!(
            text,
            "failures: {failures} · with an open KI: {} · unfiled: {} (cited only by a resolved KI: {resolved})",
            failures - unfiled,
            if unfiled == 0 {
                paint.green("0")
            } else {
                paint.red(&unfiled.to_string())
            }
        );
        let _ = writeln!(
            text,
            "backend gaps: {} file(s) one way handles and another fails",
            self.census.backend_gaps().len()
        );
        if let (Some(diff), Some(base)) = (self.diff, self.base) {
            let _ = writeln!(
                text,
                "since {base}: {} newly failing, {} newly working, {} other change(s), {} new row(s)",
                paint.red(&diff.newly_failing.len().to_string()),
                paint.green(&diff.newly_working.len().to_string()),
                diff.changed,
                diff.added
            );
            for line in diff.newly_failing.iter().take(SHOWN_DIFFS) {
                let _ = writeln!(text, "  {}", paint.red(line));
            }
        }
        let _ = writeln!(
            text,
            "files: census.tsv · matrix.tsv · unfiled.txt · census.html in {}",
            self.out
        );
    }
}

/// The census as one HTML page with no external assets.
pub fn html(summary: &Summary<'_>) -> String {
    let census = summary.census;
    let mut page = String::new();
    let _ = write!(
        page,
        "<!doctype html><html lang=en><head><meta charset=utf-8>\
         <meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>Flux census {}</title><style>{STYLE}</style></head><body><main>",
        escape(summary.tag)
    );
    let _ = write!(
        page,
        "<header><p class=eyebrow>Flux · support census</p><h1>Census {}</h1>\
         <p class=meta>{} · {} rows · {}</p></header>",
        escape(summary.tag),
        format_duration(summary.took),
        census.entries.len(),
        escape(summary.out)
    );

    html_cards(&mut page, summary);
    html_diff(&mut page, summary);
    let matrix = census.matrix();
    html_matrix(&mut page, census, &matrix, "Features", false);
    html_matrix(&mut page, census, &matrix, "Directories", true);
    html_unfiled(&mut page, census);
    html_gaps(&mut page, census);
    html_by_directory(&mut page, census);
    let open = summary.known.issues.iter().filter(|i| i.open).count();
    let _ = write!(
        page,
        "<footer class=small>{open} open known issues in docs/known_issues.md.</footer></main></body></html>"
    );
    page
}

fn html_cards(page: &mut String, summary: &Summary<'_>) {
    let census = summary.census;
    page.push_str("<section class=cards>");
    for way in &census.ways {
        let share = summary.way_share(way);
        let percent = share
            .percent()
            .map_or("–".to_string(), |p| format!("{p:.1}%"));
        let _ = write!(
            page,
            "<div class=card><div class=way>{}</div><div class=big>{percent}</div>\
             <div class=small>{} ok of {} run · {} not run</div></div>",
            escape(way),
            share.ok,
            share.runs(),
            census.count(way, Class::NotRun)
        );
    }
    page.push_str("</section>");
}

fn html_diff(page: &mut String, summary: &Summary<'_>) {
    let (Some(diff), Some(base)) = (summary.diff, summary.base) else {
        return;
    };
    let _ = write!(
        page,
        "<section><h2>Since {}</h2><p>{} newly failing · {} newly working · {} other changes · {} new rows</p>",
        escape(base),
        diff.newly_failing.len(),
        diff.newly_working.len(),
        diff.changed,
        diff.added
    );
    list(page, "Newly failing", &diff.newly_failing);
    list(page, "Newly working", &diff.newly_working);
    page.push_str("</section>");
}

/// The feature (or, with `directories`, the directory) × way table.
fn html_matrix(
    page: &mut String,
    census: &Census,
    matrix: &BTreeMap<&str, BTreeMap<(&str, Class), usize>>,
    title: &str,
    directories: bool,
) {
    let _ = write!(
        page,
        "<section><h2>{title} × way</h2><p class=small>Each cell: the share of runs that \
         did what the file asks (works, expected error, compile-only), and how many ran. \
         Hover for every class.</p><div class=scroll><table class=matrix><thead><tr><th></th>"
    );
    for way in &census.ways {
        let _ = write!(page, "<th>{}</th>", escape(way));
    }
    page.push_str("</tr></thead><tbody>");
    for (feature, cells) in matrix
        .iter()
        .filter(|(feature, _)| feature.starts_with("dir:") == directories)
    {
        let _ = write!(
            page,
            "<tr><th>{}</th>",
            escape(feature.trim_start_matches("dir:"))
        );
        for way in &census.ways {
            let mut share = Share::default();
            let mut tip = Vec::new();
            for class in Class::ALL {
                let n = cells.get(&(way.as_str(), class)).copied().unwrap_or(0);
                for _ in 0..n {
                    share.add(class);
                }
                if n > 0 {
                    tip.push(format!("{} {n}", class.name()));
                }
            }
            let (bucket, label) = match (share.tenths(), share.percent()) {
                (Some(tenths), Some(p)) => (format!("r{tenths}"), format!("{p:.0}%")),
                _ => ("none".to_string(), "–".to_string()),
            };
            let _ = write!(
                page,
                "<td class={bucket} title=\"{}\">{label}<span>{}</span></td>",
                escape(&tip.join(", ")),
                share.runs()
            );
        }
        page.push_str("</tr>");
    }
    page.push_str("</tbody></table></div></section>");
}

fn html_unfiled(page: &mut String, census: &Census) {
    let unfiled: Vec<&Entry> = census.unfiled().collect();
    let _ = write!(
        page,
        "<section><h2>Unfiled failures ({})</h2><p class=small>Failures no open KI covers: \
         fix the fixture's header, or file a KI.</p>",
        unfiled.len()
    );
    entry_table(page, &unfiled);
    page.push_str("</section>");
}

fn html_gaps(page: &mut String, census: &Census) {
    let gaps = census.backend_gaps();
    let _ = write!(
        page,
        "<section><h2>Backend gaps ({})</h2><div class=scroll><table><thead><tr>\
         <th>file</th><th>ok on</th><th>fails on</th></tr></thead><tbody>",
        gaps.len()
    );
    for (file, ok, failed) in &gaps {
        let _ = write!(
            page,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(file),
            escape(&ok.join(", ")),
            escape(&failed.join(", "))
        );
    }
    page.push_str("</tbody></table></div></section>");
}

fn html_by_directory(page: &mut String, census: &Census) {
    page.push_str("<section><h2>Failures by directory</h2>");
    let mut by_dir: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in census.entries.iter().filter(|e| e.row.class.is_failure()) {
        let dir = e
            .features
            .iter()
            .find_map(|f| f.strip_prefix("dir:"))
            .unwrap_or("?");
        by_dir.entry(dir).or_default().push(e);
    }
    for (dir, entries) in by_dir {
        let _ = write!(
            page,
            "<details><summary>{} <span class=small>({})</span></summary>",
            escape(dir),
            entries.len()
        );
        entry_table(page, &entries);
        page.push_str("</details>");
    }
    page.push_str("</section>");
}

fn list(page: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = write!(
        page,
        "<details><summary>{title} ({})</summary><ul>",
        items.len()
    );
    for item in items {
        let _ = write!(page, "<li>{}</li>", escape(item));
    }
    page.push_str("</ul></details>");
}

fn entry_table(page: &mut String, entries: &[&Entry]) {
    page.push_str(
        "<div class=scroll><table><thead><tr><th>file</th><th>way</th><th>class</th>\
         <th>KI</th><th>note</th></tr></thead><tbody>",
    );
    for e in entries {
        let ki = match &e.filed {
            Filed::Open(id) => id.clone(),
            Filed::Resolved(id) => format!("{id} (resolved)"),
            Filed::Unfiled => String::new(),
        };
        let _ = write!(
            page,
            "<tr><td>{}</td><td>{}</td><td class=c-{}>{}</td><td>{}</td><td class=note>{}</td></tr>",
            escape(&e.row.file),
            escape(&e.row.way),
            e.row.class.name(),
            e.row.class.name(),
            escape(&ki),
            escape(&e.row.note)
        );
    }
    page.push_str("</tbody></table></div>");
}

/// `text` safe inside HTML text and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "
:root{--bg:#f4f6f8;--surface:#fff;--ink:#17202a;--muted:#5b6876;--rule:#d8dee4;--bad:#a23b32;--ok:#2c7a4b;
--r0:#f3c7c2;--r1:#f3c7c2;--r2:#f3cfc0;--r3:#f3d6bf;--r4:#f2debf;--r5:#f0e5c0;--r6:#e8e8c0;--r7:#dde8c1;--r8:#cfe6c4;--r9:#c2e2c7;--r10:#b5dec9}
@media (prefers-color-scheme:dark){:root{--bg:#0f151b;--surface:#161e26;--ink:#e3e9ef;--muted:#93a1ae;--rule:#26333f;--bad:#e88a80;--ok:#6cc28f;
--r0:#5a2420;--r1:#5a2420;--r2:#5a2d1f;--r3:#58361f;--r4:#55401f;--r5:#4f4920;--r6:#464e22;--r7:#3b5024;--r8:#305028;--r9:#284f2d;--r10:#214d33}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif}
main{max-width:1100px;margin:0 auto;padding:32px 16px 64px;display:grid;gap:28px}
h1{margin:0;font-size:32px}h2{margin:0 0 8px;font-size:19px}.eyebrow{margin:0;font:12px ui-monospace,Menlo,monospace;text-transform:uppercase;letter-spacing:.06em;color:var(--muted)}
.meta,.small{color:var(--muted);font-size:13px}.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px}
.card{background:var(--surface);border:1px solid var(--rule);border-radius:8px;padding:14px}.way{font:600 13px ui-monospace,Menlo,monospace;color:var(--muted)}
.big{font-size:30px;font-weight:700;font-variant-numeric:tabular-nums}.scroll{overflow-x:auto}
table{border-collapse:collapse;width:100%;background:var(--surface);font-size:13px}th,td{border:1px solid var(--rule);padding:4px 8px;text-align:left;vertical-align:top}
.matrix td{text-align:center;font-variant-numeric:tabular-nums;white-space:nowrap}.matrix td span{display:block;font-size:11px;color:var(--muted)}
.r0{background:var(--r0)}.r1{background:var(--r1)}.r2{background:var(--r2)}.r3{background:var(--r3)}.r4{background:var(--r4)}.r5{background:var(--r5)}
.r6{background:var(--r6)}.r7{background:var(--r7)}.r8{background:var(--r8)}.r9{background:var(--r9)}.r10{background:var(--r10)}
.c-wrong,.c-disagree,.c-rejected,.c-crashed{color:var(--bad);font-weight:600}.note{font-family:ui-monospace,Menlo,monospace;font-size:12px;word-break:break-word}
details{background:var(--surface);border:1px solid var(--rule);border-radius:6px;padding:6px 10px;margin:6px 0}summary{cursor:pointer;font-weight:600}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_text_is_escaped() {
        assert_eq!(
            escape("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn a_share_counts_ok_and_failed_runs_only() {
        let mut share = Share::default();
        for class in [
            Class::Works,
            Class::ExpectedError,
            Class::Crashed,
            Class::NotRun,
        ] {
            share.add(class);
        }
        assert_eq!(share, Share { ok: 2, failed: 1 });
        assert_eq!(share.percent().map(|p| p.round()), Some(67.0));
        assert_eq!(Share::default().percent(), None);
    }
}
