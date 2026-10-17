//! `cargo xtask census`: what Flux supports (proposal 0190 §16).
//!
//! Runs every `.flx` file through `flux parity-check --census`, tags each file
//! with the features it uses (`flux features`), joins every failure to a known
//! issue, and writes the support matrix: `census.tsv`, `matrix.tsv`,
//! `unfiled.txt`, `summary.txt` and `census.html`, in
//! `.verify/census_<tag>/`.

mod known_issues;
mod report;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::outcome::{Steps, format_duration};
use crate::proc::{run, run_tee};
use crate::store;
use crate::text::{Paint, StatusLine, strip_ansi};
use crate::verify::{Verdict, cargo, flux_binary, is_cargo_progress, progress, tail};

pub use known_issues::KnownIssues;

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    pub jobs: usize,
    pub ways: Option<String>,
    pub allow: Vec<String>,
    pub dirs: Vec<String>,
}

/// The ways a census runs unless `--ways` says otherwise.
const DEFAULT_WAYS: &str = "vm,llvm,vm_opt,llvm_opt";

/// The directories a census covers unless others are named.
const DEFAULT_DIRS: [&str; 3] = ["examples", "tests", "lib"];

/// Returns `Ok(true)` once the census is written: it records, never fails on
/// what it finds.
pub fn run_census(root: &Path, options: &Options) -> Result<bool, String> {
    let out = store::run_dir(root, &format!("census_{}", options.tag))?;
    let base_out = options
        .base
        .as_deref()
        .map(|base| store::run_dir(root, &format!("census_{base}")))
        .transpose()?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("outputs: {}", store::display(&out));
    let started = Instant::now();
    let mut steps = Steps::new(4);
    let io = |e: std::io::Error| e.to_string();

    steps.start("build");
    let build = run(cargo()
        .current_dir(root)
        .args(["build", "--all-features", "--quiet"]))
    .map_err(io)?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }
    let flux = flux_binary(root);

    let ways = options.ways.as_deref().unwrap_or(DEFAULT_WAYS);
    let dirs: Vec<&str> = if options.dirs.is_empty() {
        DEFAULT_DIRS.to_vec()
    } else {
        options.dirs.iter().map(String::as_str).collect()
    };
    let run_started = steps.start(&format!("run {} on {ways}", dirs.join(" ")));
    let rows_file = out.join("rows.tsv");
    let exit = run_parity(
        root,
        &flux,
        &dirs,
        ways,
        options,
        &rows_file,
        &out.join("census.txt"),
    )
    .map_err(io)?;
    println!("took {}", format_duration(run_started.elapsed()));
    if exit != 0 {
        return Err(format!(
            "parity-check exited {exit}; see {}",
            store::display(&out.join("census.txt"))
        ));
    }

    steps.start("feature tags");
    let rows = read_rows(&fs::read_to_string(&rows_file).map_err(io)?);
    let files: BTreeSet<&str> = rows.iter().map(|row| row.file.as_str()).collect();
    let features = tag_files(root, &flux, &files)?;
    println!("{} file(s) tagged", features.len());

    steps.start("report");
    let known = KnownIssues::parse(
        &fs::read_to_string(root.join("docs/known_issues.md")).unwrap_or_default(),
    );
    let census = Census::build(rows, &features, &known);
    let base = base_out.as_deref().map(|dir| {
        fs::read_to_string(dir.join("census.tsv"))
            .map(|text| base_classes(&text))
            .unwrap_or_default()
    });
    let diff = base.as_ref().map(|base| census.diff(base));
    let summary = report::Summary {
        tag: &options.tag,
        took: started.elapsed(),
        out: &store::display(&out),
        census: &census,
        known: &known,
        diff: diff.as_ref(),
        base: options.base.as_deref(),
    };
    write(&out.join("census.tsv"), &census.to_tsv())?;
    write(&out.join("matrix.tsv"), &census.matrix_tsv())?;
    write(&out.join("unfiled.txt"), &census.unfiled_text())?;
    write(&out.join("summary.txt"), &summary.text(Paint::plain()))?;
    write(&out.join("census.html"), &report::html(&summary))?;
    print!("{}", summary.text(Paint::detect()));
    Ok(true)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs `parity-check --census`, logging its output and showing one status
/// line that counts files as their verdicts arrive.
fn run_parity(
    root: &Path,
    flux: &Path,
    dirs: &[&str],
    ways: &str,
    options: &Options,
    rows_file: &Path,
    log: &Path,
) -> std::io::Result<i32> {
    let started = Instant::now();
    let paint = Paint::detect();
    let mut status = StatusLine::new(paint.is_on());
    let mut stdout = std::io::stdout();
    let mut log = fs::File::create(log)?;
    let mut total: Option<usize> = None;
    let mut done = 0;
    let mut not_passing = 0;
    let mut command = Command::new(flux);
    command
        .current_dir(root)
        .arg("parity-check")
        .args(dirs)
        .args(["--ways", ways, "--jobs", &options.jobs.to_string()])
        .arg("--census")
        .arg(rows_file);
    for resource in &options.allow {
        command.args(["--allow", resource]);
    }
    let exit = run_tee(&mut command, None, |line| {
        if is_cargo_progress(line) {
            return;
        }
        let plain = strip_ansi(line);
        let _ = writeln!(log, "{plain}");
        if let Some(n) = plain
            .strip_prefix("[parity] fixtures: ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
        {
            total = Some(n);
        }
        if let Some(verdict) = Verdict::parse(&plain) {
            done += 1;
            if verdict != Verdict::Pass {
                not_passing += 1;
            }
            let file = plain.split_once(' ').map_or("", |(_, rest)| rest);
            status.update(
                &mut stdout,
                &paint.dim(&format!(
                    "{} · {not_passing} not passing · {}",
                    progress(done, total, started),
                    tail(file, 48)
                )),
            );
            if !paint.is_on() && done % 100 == 0 {
                println!("{}", progress(done, total, started));
            }
        } else if plain.starts_with("[parity]") {
            status.clear(&mut stdout);
            println!("{}", paint.dim(&plain));
        }
    })?;
    status.clear(&mut stdout);
    Ok(exit)
}

/// `flux features` over `files`, a few hundred at a time.
fn tag_files(
    root: &Path,
    flux: &Path,
    files: &BTreeSet<&str>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let files: Vec<&str> = files.iter().copied().collect();
    let mut features = BTreeMap::new();
    for chunk in files.chunks(300) {
        let output = Command::new(flux)
            .current_dir(root)
            .arg("features")
            .args(chunk)
            .output()
            .map_err(|e| format!("flux features: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "flux features failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        features.extend(parse_features(&String::from_utf8_lossy(&output.stdout)));
    }
    Ok(features)
}

fn parse_features(text: &str) -> BTreeMap<String, Vec<String>> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(file, tags)| {
            let tags = tags
                .split(',')
                .filter(|tag| !tag.is_empty() && !tag.starts_with("unreadable"))
                .map(str::to_string)
                .collect();
            (file.to_string(), tags)
        })
        .collect()
}

/// One row of `parity-check --census` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub way: String,
    pub exit: String,
    pub class: Class,
    pub codes: String,
    pub stdout: String,
    pub ms: String,
    pub note: String,
}

/// A row's class, as `parity-check --census` writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Works,
    ExpectedError,
    CompileOnly,
    Wrong,
    Disagree,
    Rejected,
    Crashed,
    NotRun,
}

impl Class {
    pub const ALL: [Self; 8] = [
        Self::Works,
        Self::ExpectedError,
        Self::CompileOnly,
        Self::Wrong,
        Self::Disagree,
        Self::Rejected,
        Self::Crashed,
        Self::NotRun,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Works => "works",
            Self::ExpectedError => "expected-error",
            Self::CompileOnly => "compile-only",
            Self::Wrong => "wrong",
            Self::Disagree => "disagree",
            Self::Rejected => "rejected",
            Self::Crashed => "crashed",
            Self::NotRun => "not-run",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.name() == name)
    }

    /// The way did what the file asks of it.
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Works | Self::ExpectedError | Self::CompileOnly)
    }

    /// The way failed the file: a bug, or a missing header.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Wrong | Self::Disagree | Self::Rejected | Self::Crashed
        )
    }
}

fn read_rows(text: &str) -> Vec<Row> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.splitn(8, '\t');
            let mut next = || fields.next().map(str::to_string);
            let file = next()?;
            let way = next()?;
            let exit = next()?;
            let class = Class::parse(&next()?)?;
            Some(Row {
                file,
                way,
                exit,
                class,
                codes: next()?,
                stdout: next()?,
                ms: next()?,
                note: next().unwrap_or_default(),
            })
        })
        .collect()
}

/// Each `(file, way)`'s class in an earlier census's `census.tsv`.
fn base_classes(text: &str) -> BTreeMap<(String, String), Class> {
    read_rows(text)
        .into_iter()
        .map(|row| ((row.file, row.way), row.class))
        .collect()
}

/// A failure's link to a known issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// An open KI, from the row's skip reason or an entry citing the file.
    Open(String),
    /// Only resolved KIs cite the file: perhaps a regression.
    Resolved(String),
    Unfiled,
}

/// A census row with what the census added to it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub row: Row,
    pub features: Vec<String>,
    pub filed: Filed,
}

/// The whole census.
pub struct Census {
    pub ways: Vec<String>,
    pub entries: Vec<Entry>,
}

/// What changed since a base census.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub newly_failing: Vec<String>,
    pub newly_working: Vec<String>,
    pub changed: usize,
    pub added: usize,
}

impl Census {
    pub fn build(
        rows: Vec<Row>,
        features: &BTreeMap<String, Vec<String>>,
        known: &KnownIssues,
    ) -> Self {
        let mut ways: Vec<String> = Vec::new();
        for row in &rows {
            if !ways.contains(&row.way) {
                ways.push(row.way.clone());
            }
        }
        let entries = rows
            .into_iter()
            .map(|row| {
                let mut tags = features.get(&row.file).cloned().unwrap_or_default();
                tags.push(format!("dir:{}", directory(&row.file)));
                let filed = if row.class.is_failure() {
                    filed(&row, known)
                } else {
                    Filed::Unfiled
                };
                Entry {
                    row,
                    features: tags,
                    filed,
                }
            })
            .collect();
        Self { ways, entries }
    }

    /// Rows of `way` with class `class`.
    pub fn count(&self, way: &str, class: Class) -> usize {
        self.entries
            .iter()
            .filter(|e| e.row.way == way && e.row.class == class)
            .count()
    }

    /// Failures with no open KI.
    pub fn unfiled(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.row.class.is_failure() && !matches!(e.filed, Filed::Open(_)))
    }

    /// Files one way handles and another fails.
    pub fn backend_gaps(&self) -> Vec<(String, Vec<String>, Vec<String>)> {
        let mut by_file: BTreeMap<&str, (Vec<String>, Vec<String>)> = BTreeMap::new();
        for e in &self.entries {
            let (ok, failed) = by_file.entry(&e.row.file).or_default();
            if e.row.class.is_ok() {
                ok.push(e.row.way.clone());
            } else if e.row.class.is_failure() {
                failed.push(format!("{} {}", e.row.way, e.row.class.name()));
            }
        }
        by_file
            .into_iter()
            .filter(|(_, (ok, failed))| !ok.is_empty() && !failed.is_empty())
            .map(|(file, (ok, failed))| (file.to_string(), ok, failed))
            .collect()
    }

    /// Every feature, with its counts per way and class.
    pub fn matrix(&self) -> BTreeMap<&str, BTreeMap<(&str, Class), usize>> {
        let mut matrix: BTreeMap<&str, BTreeMap<(&str, Class), usize>> = BTreeMap::new();
        for e in &self.entries {
            for feature in &e.features {
                *matrix
                    .entry(feature)
                    .or_default()
                    .entry((&e.row.way, e.row.class))
                    .or_default() += 1;
            }
        }
        matrix
    }

    pub fn to_tsv(&self) -> String {
        let mut text =
            String::from("file\tway\texit\tclass\tcodes\tstdout\tms\tnote\tfeatures\tki\n");
        for e in &self.entries {
            let r = &e.row;
            let ki = match &e.filed {
                Filed::Open(id) => id.clone(),
                Filed::Resolved(id) => format!("{id} (resolved)"),
                Filed::Unfiled => String::new(),
            };
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{ki}",
                r.file,
                r.way,
                r.exit,
                r.class.name(),
                r.codes,
                r.stdout,
                r.ms,
                r.note,
                e.features.join(","),
            );
        }
        text
    }

    pub fn matrix_tsv(&self) -> String {
        let mut text = String::from("feature\tway\tclass\tcount\n");
        for (feature, cells) in self.matrix() {
            for ((way, class), count) in cells {
                let _ = writeln!(text, "{feature}\t{way}\t{}\t{count}", class.name());
            }
        }
        text
    }

    pub fn unfiled_text(&self) -> String {
        let mut text = String::new();
        for e in self.unfiled() {
            let resolved = match &e.filed {
                Filed::Resolved(id) => format!("\tcited by resolved {id}"),
                _ => String::new(),
            };
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}{resolved}",
                e.row.file,
                e.row.way,
                e.row.class.name(),
                e.row.note
            );
        }
        text
    }

    pub fn diff(&self, base: &BTreeMap<(String, String), Class>) -> Diff {
        let mut diff = Diff::default();
        for e in &self.entries {
            let key = (e.row.file.clone(), e.row.way.clone());
            let label = format!("{} {}", e.row.file, e.row.way);
            match base.get(&key) {
                None => diff.added += 1,
                Some(&before) if before == e.row.class => {}
                Some(&before) => {
                    if !before.is_failure() && e.row.class.is_failure() {
                        diff.newly_failing.push(format!(
                            "{label}: {} → {}",
                            before.name(),
                            e.row.class.name()
                        ));
                    } else if before.is_failure() && e.row.class.is_ok() {
                        diff.newly_working.push(format!(
                            "{label}: {} → {}",
                            before.name(),
                            e.row.class.name()
                        ));
                    } else {
                        diff.changed += 1;
                    }
                }
            }
        }
        diff
    }
}

/// `examples/type_system` for `examples/type_system/a/b.flx`: the top folder
/// and the one below it.
fn directory(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').collect();
    match parts.as_slice() {
        [top, sub, _, ..] => format!("{top}/{sub}"),
        [top, _] => (*top).to_string(),
        _ => file.to_string(),
    }
}

fn filed(row: &Row, known: &KnownIssues) -> Filed {
    if let Some(id) = known_issues::ki_in(&row.note)
        && known.get(&id).is_none_or(|issue| issue.open)
    {
        return Filed::Open(id);
    }
    let mut resolved = None;
    for issue in known.citing(&row.file) {
        if issue.open {
            return Filed::Open(issue.id.clone());
        }
        resolved.get_or_insert_with(|| issue.id.clone());
    }
    resolved.map_or(Filed::Unfiled, Filed::Resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: &str = "\
file\tway\texit\tclass\tcodes\tstdout\tms\tnote
examples/a/x.flx\tvm\tSuccess\tworks\t\tab\t10\t
examples/a/x.flx\tllvm\tCrash\tcrashed\t\t\t20\tthread 'main' panicked
examples/a/y.flx\tvm\tCompileError\trejected\tE300\t\t5\terror[E300]
examples/a/y.flx\tllvm\t\tnot-run\t\t\t\tskip-way: KI-103 alias
tests/z.flx\tvm\tSuccess\twrong\t\tcd\t7\tstdout differs from llvm
";

    fn census() -> Census {
        let known = KnownIssues::parse("## Open\n### KI-200 — y\nexamples/a/y.flx\n");
        let mut features = BTreeMap::new();
        features.insert("examples/a/x.flx".to_string(), vec!["closures".to_string()]);
        Census::build(read_rows(ROWS), &features, &known)
    }

    #[test]
    fn rows_read_back_with_their_classes() {
        let rows = read_rows(ROWS);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1].class, Class::Crashed);
        assert_eq!(rows[3].note, "skip-way: KI-103 alias");
    }

    #[test]
    fn failures_are_filed_or_listed_as_unfiled() {
        let census = census();
        let filed: Vec<&Filed> = census.entries.iter().map(|e| &e.filed).collect();
        assert_eq!(filed[2], &Filed::Open("KI-200".to_string()));
        let unfiled: Vec<String> = census
            .unfiled()
            .map(|e| format!("{} {}", e.row.file, e.row.way))
            .collect();
        assert_eq!(unfiled, ["examples/a/x.flx llvm", "tests/z.flx vm"]);
    }

    #[test]
    fn the_matrix_counts_each_feature_and_directory() {
        let census = census();
        let matrix = census.matrix();
        assert_eq!(matrix["closures"][&("llvm", Class::Crashed)], 1);
        assert_eq!(matrix["dir:examples/a"][&("vm", Class::Works)], 1);
        assert_eq!(matrix["dir:tests"][&("vm", Class::Wrong)], 1);
        assert_eq!(census.ways, ["vm", "llvm"]);
        assert_eq!(census.count("vm", Class::Rejected), 1);
    }

    #[test]
    fn a_backend_gap_is_a_file_one_way_handles_and_another_fails() {
        let gaps = census().backend_gaps();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].0, "examples/a/x.flx");
        assert_eq!(gaps[0].2, ["llvm crashed"]);
    }

    #[test]
    fn a_diff_names_what_newly_fails_and_newly_works() {
        let census = census();
        let mut base = base_classes(&census.to_tsv());
        base.insert(
            ("examples/a/x.flx".to_string(), "llvm".to_string()),
            Class::Works,
        );
        base.insert(
            ("tests/z.flx".to_string(), "vm".to_string()),
            Class::Crashed,
        );
        base.remove(&("examples/a/y.flx".to_string(), "llvm".to_string()));
        let diff = census.diff(&base);
        assert_eq!(
            diff.newly_failing,
            ["examples/a/x.flx llvm: works → crashed"]
        );
        assert_eq!(diff.changed, 1);
        assert_eq!(diff.added, 1);
        assert!(diff.newly_working.is_empty());
    }

    #[test]
    fn features_parse_from_flux_output() {
        let parsed = parse_features("a.flx\tclosures,lists\nb.flx\t\nc.flx\tunreadable: gone\n");
        assert_eq!(parsed["a.flx"], ["closures", "lists"]);
        assert!(parsed["b.flx"].is_empty());
        assert!(parsed["c.flx"].is_empty());
    }
}
