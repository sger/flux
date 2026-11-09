//! `cargo xtask fuzz reduce` (proposal 0190 §7): shrink a case to a small
//! program that still shows the same bug.
//!
//! A mismatch is reduced on the cheapest pair of ways that disagree on the
//! original, so a bug the VM shows never builds native code. A candidate is
//! kept when the pair still differs on the same stream (exit, stdout or
//! stderr) and each way still compiles it exactly when it compiled the
//! original. The two ways run directly rather than through `parity-check`,
//! which would run every way per candidate. A case every way agrees on but
//! the compiler fails on itself is reduced while it fails the same way.
//!
//! Passes that keep a program well formed go first, as Perses and llvm-reduce
//! do: delete a top-level item, a statement or a match arm, replace a `let`'s
//! value with a literal of its declared type, inline a `let` used once, and
//! replace an expression with a literal or one of its parts. Line ddmin runs
//! after them; a deletion that splits a brace pair is widened to the lines the
//! pair encloses. Rounds repeat until one changes nothing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::{Compile, judge};
use crate::outcome::format_duration;
use crate::proc::run;
use crate::rewrite::{compile_errors, parallel};
use crate::store;
use crate::sweep::{Ran, run_flux, run_with_timeout};
use crate::verify::{cargo, flux_binary};

pub struct ReduceOptions {
    pub case: String,
    pub ways: String,
    /// Candidates tested at once.
    pub jobs: usize,
    pub timeout: Duration,
}

pub fn run_reduce(root: &Path, options: &ReduceOptions) -> Result<bool, String> {
    let case = root.join(&options.case);
    let text = fs::read_to_string(&case).map_err(|e| format!("{}: {e}", case.display()))?;
    let stem = case.file_stem().and_then(|s| s.to_str()).ok_or("the case has no file name")?;
    let out = store::store_root(root).join(format!("reduce_{stem}"));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let started = Instant::now();

    let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
        .map_err(|e| e.to_string())?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }
    let flux = flux_binary(root);
    let ways = options.ways.split(',').map(Way::parse).collect::<Result<Vec<_>, _>>()?;
    let original = out.join("original.flx");
    fs::write(&original, &text).map_err(|e| e.to_string())?;
    let file = original.to_string_lossy().into_owned();
    let runs =
        parallel(&ways, options.jobs, |way| way.run(root, &flux, &file, options.timeout, None));

    let target = match pick_pair(&ways, &runs) {
        Some((a, b, verdict)) => {
            println!("reducing on {} vs {}: they differ on {}", a.name, b.name, verdict.stream);
            Target::Mismatch { a, b, verdict }
        }
        None => {
            let ran = run_flux(root, &flux, &["--dump-core"], &file, options.timeout);
            match judge(ran.finished, ran.exit, &ran.stderr) {
                Compile::Internal(why) => {
                    println!("reducing an internal error: {why}");
                    Target::Internal(why)
                }
                _ => return Err(format!("{}: every way agrees and it compiles", options.case)),
            }
        }
    };

    let test = |text: &str, slot: usize| {
        let candidate = out.join(format!("candidate_{slot}.flx"));
        if fs::write(&candidate, text).is_err() {
            return false;
        }
        let cache = out.join(format!("cache_{slot}"));
        target.holds(root, &flux, &candidate.to_string_lossy(), options.timeout, Some(&cache))
    };
    let mut tester = Tester::new(&test, options.jobs);
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let before = lines.len();
    let mut last = Instant::now();
    let reduced = reduce(lines, &mut tester, &mut |step, n, runs| {
        println!("  {step}: {n} lines ({} runs, {})", runs, format_duration(last.elapsed()));
        last = Instant::now();
    });
    // The candidates' native runs share a cache; the result must show the
    // bug without one.
    let min_text = join(&reduced);
    let check = out.join("check.flx");
    fs::write(&check, &min_text).map_err(|e| e.to_string())?;
    let confirmed = target.holds(root, &flux, &check.to_string_lossy(), options.timeout, None);

    let min = case.with_file_name(format!("{stem}.min.flx"));
    fs::write(&min, &min_text).map_err(|e| format!("{}: {e}", min.display()))?;
    println!(
        "{} -> {} lines in {} runs ({}): {}",
        before,
        reduced.len(),
        tester.runs,
        format_duration(started.elapsed()),
        store::display(&min)
    );
    if !confirmed {
        println!("but it doesn't show the bug with --no-cache: a native cache entry was stale");
    }
    Ok(confirmed)
}

/// A way `reduce` can run: parity's plain and optimizer ways.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Way {
    name: &'static str,
    flags: &'static [&'static str],
}

impl Way {
    const ALL: [Way; 4] = [
        Way { name: "vm", flags: &[] },
        Way { name: "vm_opt", flags: &["--optimize"] },
        Way { name: "llvm", flags: &["--native"] },
        Way { name: "llvm_opt", flags: &["--native", "--optimize"] },
    ];

    fn parse(name: &str) -> Result<Way, String> {
        let name = name.trim();
        Way::ALL
            .into_iter()
            .find(|way| way.name == name)
            .ok_or_else(|| format!("fuzz reduce: unknown way {name} (vm, vm_opt, llvm, llvm_opt)"))
    }

    fn native(&self) -> bool {
        self.flags.contains(&"--native")
    }

    /// Runs `file` this way. A native way given a `cache` builds the
    /// standard library into it once instead of on every run, which is most
    /// of a small program's native build.
    fn run(
        &self,
        root: &Path,
        flux: &Path,
        file: &str,
        timeout: Duration,
        cache: Option<&Path>,
    ) -> Ran {
        match cache {
            Some(cache) if self.native() => {
                let mut command = Command::new(flux);
                command.arg("--quiet").arg("--cache-dir").arg(cache).args(self.flags).arg(file);
                run_with_timeout(command, root, timeout)
            }
            _ => run_flux(root, flux, self.flags, file, timeout),
        }
    }
}

/// Where two runs of one program first differ.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stream {
    Exit,
    Stdout,
    Stderr,
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stream::Exit => "exit",
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        })
    }
}

/// How two ways disagree on a program.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Verdict {
    stream: Stream,
    /// Whether each way compiled it.
    compiled: (bool, bool),
}

fn verdict(a: &Ran, b: &Ran) -> Option<Verdict> {
    let stream = if (a.finished, a.exit) != (b.finished, b.exit) {
        Stream::Exit
    } else if a.stdout != b.stdout {
        Stream::Stdout
    } else if a.stderr != b.stderr {
        Stream::Stderr
    } else {
        return None;
    };
    Some(Verdict { stream, compiled: (!compile_errors(&a.stderr), !compile_errors(&b.stderr)) })
}

/// The pair to reduce on: the one with the fewest native ways among those
/// that disagree, then the earliest in `ways`.
fn pick_pair(ways: &[Way], runs: &[Ran]) -> Option<(Way, Way, Verdict)> {
    let mut pairs = Vec::new();
    for i in 0..ways.len() {
        for j in i + 1..ways.len() {
            if let Some(v) = verdict(&runs[i], &runs[j]) {
                let cost = usize::from(ways[i].native()) + usize::from(ways[j].native());
                pairs.push((cost, ways[i], ways[j], v));
            }
        }
    }
    pairs.sort_by_key(|pair| pair.0);
    pairs.into_iter().next().map(|(_, a, b, v)| (a, b, v))
}

/// What a candidate must still show.
enum Target {
    Mismatch {
        a: Way,
        b: Way,
        verdict: Verdict,
    },
    /// The compiler fails on its own, with this first error line.
    Internal(String),
}

impl Target {
    fn holds(
        &self,
        root: &Path,
        flux: &Path,
        file: &str,
        timeout: Duration,
        cache: Option<&Path>,
    ) -> bool {
        match self {
            Target::Mismatch { a, b, verdict: want } => {
                // A native run costs seconds; a candidate that doesn't compile
                // is turned away by the cheap front-end run first.
                if (a.native() || b.native()) && want.compiled == (true, true) {
                    let core = run_flux(root, flux, &["--dump-core"], file, timeout);
                    if compile_errors(&core.stderr) {
                        return false;
                    }
                }
                let runs = parallel(&[*a, *b], 2, |way| way.run(root, flux, file, timeout, cache));
                verdict(&runs[0], &runs[1]) == Some(*want)
            }
            Target::Internal(why) => {
                let ran = run_flux(root, flux, &["--dump-core"], file, timeout);
                judge(ran.finished, ran.exit, &ran.stderr) == Compile::Internal(why.clone())
            }
        }
    }
}

/// Asks whether candidates still show the bug, `jobs` at a time, and each
/// distinct text once.
pub(crate) struct Tester<'a> {
    /// The test for a candidate's text; the slot (below `jobs`) names the
    /// file it may write.
    test: &'a (dyn Fn(&str, usize) -> bool + Sync),
    jobs: usize,
    seen: HashMap<String, bool>,
    pub runs: usize,
}

impl<'a> Tester<'a> {
    pub(crate) fn new(test: &'a (dyn Fn(&str, usize) -> bool + Sync), jobs: usize) -> Self {
        Tester { test, jobs: jobs.max(1), seen: HashMap::new(), runs: 0 }
    }

    /// The first of `candidates` that still shows the bug.
    fn first(&mut self, candidates: &[Vec<String>]) -> Option<usize> {
        let texts: Vec<String> = candidates.iter().map(|c| join(c)).collect();
        let mut start = 0;
        while start < texts.len() {
            let end = (start + self.jobs).min(texts.len());
            let mut fresh = Vec::new();
            let mut batch = HashSet::new();
            for text in &texts[start..end] {
                if !self.seen.contains_key(text) && batch.insert(text) {
                    fresh.push((fresh.len(), text));
                }
            }
            let test = self.test;
            let results = parallel(&fresh, self.jobs, |(slot, text)| test(text, *slot));
            self.runs += fresh.len();
            for ((_, text), result) in fresh.iter().zip(results) {
                self.seen.insert((*text).clone(), result);
            }
            if let Some(at) = (start..end).find(|&i| self.seen[&texts[i]]) {
                return Some(at);
            }
            start = end;
        }
        None
    }
}

fn join(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

type Pass = fn(&[String]) -> Vec<Vec<String>>;

/// A round's steps, in order. Line ddmin goes before the expressions pass,
/// which offers several replacements for every bracket group and so costs
/// the most runs per line: it should see the smallest program.
const STEPS: &[(&str, Option<Pass>)] = &[
    ("items", Some(delete_items)),
    ("statements", Some(delete_statements)),
    ("arms", Some(delete_arms)),
    ("branches", Some(pick_branches)),
    ("let values", Some(literal_lets)),
    ("inline", Some(inline_lets)),
    ("lines", None),
    ("expressions", Some(simplify_expressions)),
];

/// `lines` reduced until a whole round changes nothing. `log` hears each
/// step that changed the program, with its line count and the runs so far.
pub(crate) fn reduce(
    mut lines: Vec<String>,
    tester: &mut Tester<'_>,
    log: &mut dyn FnMut(&str, usize, usize),
) -> Vec<String> {
    loop {
        let round = lines.clone();
        for (name, pass) in STEPS {
            let before = lines.clone();
            lines = match pass {
                Some(pass) => greedy(lines, *pass, tester),
                None => ddmin(lines, tester),
            };
            if lines != before {
                log(name, lines.len(), tester.runs);
            }
        }
        if lines == round {
            return lines;
        }
    }
}

/// Applies `pass`'s candidates that keep the bug, in order. After each one
/// it goes on from the same position rather than starting over, which would
/// retest every candidate before it: each is a new text once the program
/// changed. What it passed over is tried again in the next round.
fn greedy(mut lines: Vec<String>, pass: Pass, tester: &mut Tester<'_>) -> Vec<String> {
    let mut from = 0;
    loop {
        let candidates: Vec<Vec<String>> =
            pass(&lines).into_iter().filter(|c| *c != lines).collect();
        let Some(rest) = candidates.get(from..) else {
            return lines;
        };
        match tester.first(rest) {
            Some(at) => {
                from += at;
                lines = candidates[from].clone();
            }
            None => return lines,
        }
    }
}

/// Delta debugging over lines, deleting chunks. Comments go too: a
/// promoted case gets its header written then.
fn ddmin(mut lines: Vec<String>, tester: &mut Tester<'_>) -> Vec<String> {
    let mut n = 2;
    loop {
        let units: Vec<usize> = (0..lines.len()).collect();
        if units.is_empty() {
            return lines;
        }
        let n_now = n.min(units.len());
        let size = units.len().div_ceil(n_now);
        let candidates: Vec<Vec<String>> = units
            .chunks(size)
            .map(|chunk| {
                let mut gone: BTreeSet<usize> = chunk.iter().copied().collect();
                widen(&lines, &mut gone);
                without(&lines, &gone)
            })
            .filter(|c| c.len() < lines.len())
            .collect();
        match tester.first(&candidates) {
            Some(at) => {
                lines = candidates[at].clone();
                n = n_now.saturating_sub(1).max(2);
            }
            None if n_now >= units.len() => return lines,
            None => n = (n_now * 2).min(units.len()),
        }
    }
}

fn without(lines: &[String], gone: &BTreeSet<usize>) -> Vec<String> {
    lines.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, l)| l.clone()).collect()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// Grows `gone` so that no brace pair loses one end: a pair with either end
/// deleted goes whole.
fn widen(lines: &[String], gone: &mut BTreeSet<usize>) {
    let pairs = brace_pairs(lines);
    loop {
        let mut grew = false;
        for &(open, close) in &pairs {
            let cut = gone.contains(&open) || gone.contains(&close);
            if cut && !(open..=close).all(|i| gone.contains(&i)) {
                gone.extend(open..=close);
                grew = true;
            }
        }
        if !grew {
            return;
        }
    }
}

/// The `{` and `}` of `line` outside string literals and comments.
fn braces(line: &str) -> Vec<char> {
    let mut found = Vec::new();
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '/' if chars.peek() == Some(&'/') => break,
            '{' | '}' => found.push(c),
            _ => {}
        }
    }
    found
}

/// Each brace pair that spans lines, as (opening line, closing line).
fn brace_pairs(lines: &[String]) -> Vec<(usize, usize)> {
    let mut open = Vec::new();
    let mut pairs = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for brace in braces(line) {
            if brace == '{' {
                open.push(i);
            } else if let Some(start) = open.pop()
                && start != i
            {
                pairs.push((start, i));
            }
        }
    }
    pairs
}

/// How many braces are open before each line.
fn depths(lines: &[String]) -> Vec<usize> {
    let mut depth = 0usize;
    lines
        .iter()
        .map(|line| {
            let before = depth;
            for brace in braces(line) {
                depth = if brace == '{' { depth + 1 } else { depth.saturating_sub(1) };
            }
            before
        })
        .collect()
}

/// The last line of what starts at `i`: its brace pair's closing line, or `i`.
fn span_end(pairs: &[(usize, usize)], i: usize) -> usize {
    pairs.iter().filter(|p| p.0 == i).map(|p| p.1).max().unwrap_or(i)
}

/// `lines` without `start..=end`, and without a blank line left behind.
fn cut(lines: &[String], start: usize, end: usize) -> Vec<String> {
    let mut end = end;
    if lines.get(end + 1).is_some_and(|l| l.trim().is_empty()) {
        end += 1;
    }
    let gone = (start..=end).collect();
    without(lines, &gone)
}

/// Each top-level `fn` (but `main`) or `data` deleted.
fn delete_items(lines: &[String]) -> Vec<Vec<String>> {
    let depth = depths(lines);
    let pairs = brace_pairs(lines);
    (0..lines.len())
        .filter(|&i| depth[i] == 0)
        .filter(|&i| {
            let l = &lines[i];
            (l.starts_with("fn ") && !l.starts_with("fn main")) || l.starts_with("data ")
        })
        .map(|i| cut(lines, i, span_end(&pairs, i)))
        .collect()
}

/// Each statement inside a function deleted: a line that follows a `{` or a
/// `;`, through the end of what it opens, ending in `;`.
fn delete_statements(lines: &[String]) -> Vec<Vec<String>> {
    let depth = depths(lines);
    let pairs = brace_pairs(lines);
    let mut candidates = Vec::new();
    for i in 1..lines.len() {
        if depth[i] == 0 || is_comment(&lines[i]) || lines[i].trim().is_empty() {
            continue;
        }
        let previous = lines[..i].iter().rev().map(|l| l.trim_end()).find(|l| !l.is_empty());
        if !previous.is_some_and(|p| p.ends_with('{') || p.ends_with(';')) {
            continue;
        }
        let end = span_end(&pairs, i);
        if lines[end].trim_end().ends_with(';') {
            candidates.push(cut(lines, i, end));
        }
    }
    candidates
}

/// Each one-line `match` arm deleted when a later arm of the same `match` is
/// a wildcard, so the match stays exhaustive.
fn delete_arms(lines: &[String]) -> Vec<Vec<String>> {
    let depth = depths(lines);
    let pairs = brace_pairs(lines);
    let mut candidates = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let arm = line.trim();
        if !arm.contains(" -> ") || !arm.ends_with(',') || arm.starts_with("_ ->") {
            continue;
        }
        let Some(&(_, close)) =
            pairs.iter().filter(|p| p.0 < i && i < p.1).min_by_key(|p| p.1 - p.0)
        else {
            continue;
        };
        let wildcard_later = (i + 1..close)
            .any(|j| depth[j] == depth[i] && lines[j].trim_start().starts_with("_ ->"));
        if wildcard_later && span_end(&pairs, i) == i {
            candidates.push(without(lines, &BTreeSet::from([i])));
        }
    }
    candidates
}

/// Where `keyword` starts in `line` as a word.
fn keyword_at(line: &str, keyword: &str) -> Option<usize> {
    line.match_indices(keyword)
        .map(|(at, _)| at)
        .find(|&at| !line[..at].chars().next_back().is_some_and(is_ident))
}

/// What follows the `}` that starts `line`, as `;` in `};`.
fn after_close(line: &str) -> &str {
    line.trim_start().strip_prefix('}').unwrap_or("")
}

/// Each `if` / `else` replaced by one of its branches, and each `match` by
/// one arm's value, where that branch or arm is one line. The passes that
/// delete lines can't do it: the `if` needs both its branches.
fn pick_branches(lines: &[String]) -> Vec<Vec<String>> {
    let pairs = brace_pairs(lines);
    let depth = depths(lines);
    let replace = |open: usize, end: usize, value: &str, at: usize| {
        let mut candidate = lines[..open].to_vec();
        candidate.push(format!(
            "{}{}{}",
            &lines[open][..at],
            value.trim(),
            after_close(&lines[end])
        ));
        candidate.extend_from_slice(&lines[end + 1..]);
        candidate
    };
    let mut candidates = Vec::new();
    for &(open, close) in &pairs {
        if !lines[open].trim_end().ends_with('{') {
            continue;
        }
        if let Some(at) = keyword_at(&lines[open], "if ") {
            if lines[close].trim() != "} else {" {
                continue;
            }
            let Some(&(_, end)) = pairs.iter().find(|p| p.0 == close) else {
                continue;
            };
            for body in [&lines[open + 1..close], &lines[close + 1..end]] {
                if let [value] = body {
                    candidates.push(replace(open, end, value, at));
                }
            }
        } else if let Some(at) = keyword_at(&lines[open], "match ") {
            if let Some(value) = only_arm_value(lines, open, close, at) {
                candidates.push(replace(open, close, &value, at));
            }
            for arm in open + 1..close {
                if depth[arm] != depth[open] + 1 || span_end(&pairs, arm) != arm {
                    continue;
                }
                if let Some((_, value)) = lines[arm].split_once(" -> ") {
                    candidates.push(replace(
                        open,
                        close,
                        value.trim_end().trim_end_matches(','),
                        at,
                    ));
                }
            }
        }
    }
    candidates
}

/// A `match` with one arm whose pattern only names the scrutinee or its
/// tuple's parts, as that arm's value with the parts put in for the names.
/// Picking the arm alone would leave the names unbound.
fn only_arm_value(lines: &[String], open: usize, close: usize, at: usize) -> Option<String> {
    let [arm] = &lines[open + 1..close] else {
        return None;
    };
    let scrutinee = lines[open][at + "match ".len()..].trim_end().strip_suffix('{')?.trim();
    let (pattern, value) = arm.trim().split_once(" -> ")?;
    let mut value = value.trim_end_matches(',').trim().to_string();
    let tuple = |text: &str| -> Option<Vec<String>> {
        let inner = text.strip_prefix('(')?.strip_suffix(')')?;
        Some(elements(inner))
    };
    let pairs: Vec<(String, String)> = match (tuple(pattern), tuple(scrutinee)) {
        (Some(names), Some(parts)) if names.len() == parts.len() => {
            names.into_iter().zip(parts).collect()
        }
        (None, _) => vec![(pattern.to_string(), scrutinee.to_string())],
        _ => return None,
    };
    for (name, part) in pairs {
        if !name.chars().all(is_ident) {
            return None;
        }
        let part = if is_atom(&part) { part } else { format!("({part})") };
        for at in word_at(&value, &name).into_iter().rev() {
            value.replace_range(at..at + name.len(), &part);
        }
    }
    Some(value)
}

/// `let <name>: <type> = <value>` split at its top level.
fn split_let(line: &str) -> Option<(&str, &str, &str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let rest = rest.strip_prefix("let ")?;
    let (name, rest) = rest.split_once(": ")?;
    let (ty, value) = rest.split_once(" = ")?;
    Some((indent, name, ty, value))
}

/// A literal of the written type `ty`, where there is a short one.
fn literal_of(ty: &str) -> Option<String> {
    let ty = ty.trim();
    Some(match ty {
        "Int" => "0".to_string(),
        "Bool" => "false".to_string(),
        "String" => "\"\"".to_string(),
        _ if ty.starts_with("List<") => "[]".to_string(),
        _ if ty.starts_with("Option<") => "None".to_string(),
        _ if ty.starts_with('(') && ty.ends_with(')') => {
            let parts = split_top_level(&ty[1..ty.len() - 1], ',', true);
            let literals = parts.iter().map(|p| literal_of(p)).collect::<Option<Vec<_>>>()?;
            format!("({})", literals.join(", "))
        }
        _ => return None,
    })
}

/// `text` split at each `sep` outside brackets and strings; `<` and `>`
/// count as brackets in a type, not in an expression.
fn split_top_level(text: &str, sep: char, in_type: bool) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_string = !in_string,
            _ if in_string => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if in_type => depth += 1,
            '>' if in_type => depth -= 1,
            _ if c == sep && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Each `let`'s value, over however many lines, replaced by a literal of its
/// declared type.
fn literal_lets(lines: &[String]) -> Vec<Vec<String>> {
    let pairs = brace_pairs(lines);
    let mut candidates = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((indent, name, ty, value)) = split_let(line) else {
            continue;
        };
        let Some(literal) = literal_of(ty) else {
            continue;
        };
        if value.trim_end_matches(';') == literal {
            continue;
        }
        let end = span_end(&pairs, i);
        let mut candidate = lines[..i].to_vec();
        candidate.push(format!("{indent}let {name}: {ty} = {literal};"));
        candidate.extend_from_slice(&lines[end + 1..]);
        candidates.push(candidate);
    }
    candidates
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The byte offsets where `name` occurs in `line` as a whole word.
fn word_at(line: &str, name: &str) -> Vec<usize> {
    line.match_indices(name)
        .filter(|(at, _)| {
            let before = line[..*at].chars().next_back();
            let after = line[at + name.len()..].chars().next();
            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        })
        .map(|(at, _)| at)
        .collect()
}

/// Each one-line `let` read exactly once, inlined at that read.
fn inline_lets(lines: &[String]) -> Vec<Vec<String>> {
    let pairs = brace_pairs(lines);
    let mut candidates = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((_, name, _, value)) = split_let(line) else {
            continue;
        };
        let Some(value) = value.trim_end().strip_suffix(';') else {
            continue;
        };
        if span_end(&pairs, i) != i {
            continue;
        }
        let uses: Vec<(usize, usize)> = lines
            .iter()
            .enumerate()
            .filter(|(j, l)| *j != i && !is_comment(l))
            .flat_map(|(j, l)| word_at(l, name).into_iter().map(move |at| (j, at)))
            .collect();
        let [(j, at)] = uses[..] else {
            continue;
        };
        let value = if is_atom(value) { value.to_string() } else { format!("({value})") };
        let mut candidate = lines.to_vec();
        candidate[j] = format!("{}{value}{}", &lines[j][..at], &lines[j][at + name.len()..]);
        candidate.remove(i);
        candidates.push(candidate);
    }
    candidates
}

/// A literal, a name, a call, or one parenthesized or bracketed group.
fn is_atom(text: &str) -> bool {
    let text = text.trim();
    text.chars().all(|c| is_ident(c) || c == '.')
        || (text.starts_with('"')
            && text.ends_with('"')
            && text[1..text.len() - 1].find('"').is_none())
        || group_end(text, 0) == Some(text.len() - 1)
        || text.find('(').is_some_and(|open| {
            text[..open].chars().all(is_ident) && group_end(text, open) == Some(text.len() - 1)
        })
}

/// Where the group opened at byte `start` of `text` closes.
fn group_end(text: &str, start: usize) -> Option<usize> {
    let open = text[start..].chars().next()?;
    let close = match open {
        '(' => ')',
        '[' => ']',
        _ => return None,
    };
    let mut depth = 0;
    let mut in_string = false;
    for (i, c) in text[start..].char_indices() {
        match c {
            '"' => in_string = !in_string,
            _ if in_string => {}
            _ if c == open => depth += 1,
            _ if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + i);
                }
            }
            _ => {}
        }
    }
    None
}

const OPERATORS: [&str; 14] = [
    " |> ", " || ", " && ", " == ", " != ", " <= ", " >= ", " < ", " > ", " + ", " - ", " * ",
    " / ", " % ",
];

/// What an expression could be replaced by: literals, then its operands, or
/// a call's arguments.
fn replacements(group: &str) -> Vec<String> {
    let mut found: Vec<String> =
        ["0", "1", "false", "true", "\"\"", "[]", "None"].iter().map(|s| s.to_string()).collect();
    if group.starts_with('"') {
        return vec!["\"\"".to_string()];
    }
    if let Some(inner) = group.strip_prefix('[').and_then(|g| g.strip_suffix(']')) {
        let mut found = vec!["[]".to_string()];
        if !inner.contains(" | ") {
            found.extend(elements(inner));
        }
        return found;
    }
    if let Some(inner) = group.strip_prefix('(').and_then(|g| g.strip_suffix(')')) {
        let parts = elements(inner);
        if parts.len() > 1 {
            found.extend(parts);
        }
        for op in OPERATORS {
            let parts = split_top_level_str(inner, op);
            if parts.len() == 2 {
                found.extend(parts.iter().map(|p| p.trim().to_string()));
                break;
            }
        }
    } else if let Some(open) = group.find('(') {
        let args = &group[open + 1..group.len() - 1];
        found.extend(split_top_level(args, ',', false).iter().map(|a| a.trim().to_string()));
    }
    found.retain(|r| !r.is_empty() && r != group);
    found
}

/// The comma-separated elements of a tuple's or a list's inside.
fn elements(inner: &str) -> Vec<String> {
    split_top_level(inner, ',', false).iter().map(|e| e.trim().to_string()).collect()
}

/// `text` split at the first `op` outside brackets and strings.
fn split_top_level_str<'t>(text: &'t str, op: &str) -> Vec<&'t str> {
    let mut depth = 0i32;
    let mut in_string = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_string = !in_string,
            _ if in_string => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth == 0 && text[i..].starts_with(op) => {
                return vec![&text[..i], &text[i + op.len()..]];
            }
            _ => {}
        }
    }
    vec![text]
}

/// The expressions on `line`: groups in brackets, calls and strings, as
/// (start, end) byte ranges, end exclusive.
fn expressions(line: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut in_string = None;
    for (i, c) in line.char_indices() {
        if let Some(start) = in_string {
            if c == '"' {
                found.push((start, i + 1));
                in_string = None;
            }
            continue;
        }
        match c {
            '"' => in_string = Some(i),
            '(' | '[' => {
                if let Some(end) = group_end(line, i) {
                    let callee = line[..i].trim_end_matches(is_ident);
                    let start = if c == '(' { callee.len() } else { i };
                    found.push((start, end + 1));
                }
            }
            _ => {}
        }
    }
    found
}

/// Each expression replaced by a literal or one of its parts, the largest
/// expressions first.
fn simplify_expressions(lines: &[String]) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let head = line.trim_start();
        if is_comment(line) || head.starts_with("fn ") || head.starts_with("data ") {
            continue;
        }
        for (start, end) in expressions(line) {
            for replacement in replacements(&line[start..end]) {
                let text = format!("{}{replacement}{}", &line[..start], &line[end..]);
                found.push((end - start, i, text));
            }
        }
    }
    found.sort_by_key(|(size, i, _)| (std::cmp::Reverse(*size), *i));
    found
        .into_iter()
        .map(|(_, i, text)| {
            let mut candidate = lines.to_vec();
            candidate[i] = text;
            candidate
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn reduce_with(text: &str, test: &(dyn Fn(&str, usize) -> bool + Sync)) -> String {
        let mut tester = Tester::new(test, 3);
        join(&reduce(lines(text), &mut tester, &mut |_, _, _| {}))
    }

    /// A fake front end: braces and brackets balance, and every name read is
    /// bound by a `let` or a parameter first.
    fn well_formed(text: &str) -> bool {
        let opens = text.matches('{').count() == text.matches('}').count();
        let reads = ["v1", "v2", "v3"].iter().all(|v| {
            let first_use = text.find(&format!("({v}")).or_else(|| text.find(&format!("{v})")));
            let bound = text.find(&format!("let {v}:"));
            match (first_use, bound) {
                (Some(u), Some(b)) => b < u,
                (Some(_), None) => false,
                _ => true,
            }
        });
        opens && reads
    }

    const PROGRAM: &str = "// expect: any
fn helper(n: Int) -> Int {
    n
}

fn main() with IO {
    let v1: Int = (40 + 2);
    print(v1);
    let v2: Int = helper((v1 * 3));
    print(v2);
    let v3: String = match v2 {
        1 -> \"one\",
        2 -> \"two\",
        _ -> \"many\",
    };
    print(v3);
}
";

    #[test]
    fn ddmin_finds_the_two_lines_that_matter() {
        let text = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let test = |t: &str, _: usize| t.contains("c\n") && t.contains("f\n");
        assert_eq!(reduce_with(text, &test), "c\nf\n");
    }

    #[test]
    fn a_deletion_that_splits_a_brace_pair_takes_the_whole_pair() {
        let program = lines("a {\n  b\n} else {\n  c\n}\nd");
        let mut gone = BTreeSet::from([2]);
        widen(&program, &mut gone);
        assert_eq!(gone, (0..=4).collect());
        let mut inner = BTreeSet::from([1]);
        widen(&program, &mut inner);
        assert_eq!(inner, BTreeSet::from([1]));
    }

    #[test]
    fn braces_in_strings_and_comments_dont_count() {
        assert_eq!(braces(r#"let s = "x#{v}y" + "\"{"; // }"#), Vec::<char>::new());
        assert_eq!(braces("match x { // {"), vec!['{']);
    }

    #[test]
    fn a_bug_in_one_sum_reduces_to_that_sum() {
        // The planted fold: wrong whenever `40 + 2` is still in the program
        // and its value still printed.
        let test =
            |t: &str, _: usize| well_formed(t) && t.contains("(40 + 2)") && t.contains("print(");
        let reduced = reduce_with(PROGRAM, &test);
        assert!(reduced.lines().count() <= 6, "{reduced}");
        assert!(reduced.contains("(40 + 2)"), "{reduced}");
    }

    #[test]
    fn statements_items_and_arms_are_deleted_whole() {
        let program = lines(PROGRAM);
        let items = delete_items(&program);
        assert_eq!(items.len(), 1);
        assert!(!join(&items[0]).contains("helper(n"));
        assert!(!join(&items[0]).contains("\n\n\n"));

        let statements: Vec<String> = delete_statements(&program).iter().map(|c| join(c)).collect();
        assert!(statements.iter().any(|c| !c.contains("let v3") && !c.contains("\"many\"")));
        assert!(statements.iter().all(|c| c.contains("fn main")));

        let arms: Vec<String> = delete_arms(&program).iter().map(|c| join(c)).collect();
        assert_eq!(arms.len(), 2);
        assert!(arms.iter().all(|c| c.contains("_ -> \"many\"")));
    }

    #[test]
    fn a_let_takes_a_literal_of_its_type_over_all_its_lines() {
        let program = lines(PROGRAM);
        let candidates: Vec<String> = literal_lets(&program).iter().map(|c| join(c)).collect();
        assert!(candidates.iter().any(|c| c.contains("let v1: Int = 0;")));
        let v3 = candidates.iter().find(|c| c.contains("let v3: String = \"\";")).unwrap();
        assert!(!v3.contains("\"two\""));
        assert_eq!(literal_of("(Int, List<Bool>)"), Some("(0, [])".to_string()));
        assert_eq!(literal_of("D3"), None);
    }

    #[test]
    fn an_if_or_a_match_becomes_one_of_its_branches() {
        let program = lines(
            "fn main() with IO {\n    let v: Int = if (1 > 0) {\n        7\n    } else {\n        8\n    };\n    print(v);\n}",
        );
        let candidates: Vec<String> = pick_branches(&program).iter().map(|c| join(c)).collect();
        assert!(
            candidates.contains(
                &"fn main() with IO {\n    let v: Int = 7;\n    print(v);\n}\n".to_string()
            )
        );
        assert!(candidates.iter().any(|c| c.contains("let v: Int = 8;")));

        let program = lines(PROGRAM);
        let candidates: Vec<String> = pick_branches(&program).iter().map(|c| join(c)).collect();
        assert_eq!(candidates.len(), 3);
        assert!(
            candidates.iter().any(|c| c.contains("let v3: String = \"many\";\n    print(v3);"))
        );
    }

    #[test]
    fn a_match_with_one_arm_puts_the_scrutinee_in_for_its_names() {
        let program = lines(
            "fn main() with IO {\n    let v: List<Int> = match (\"\", f(20)) {\n        (p, q) -> [q, q],\n    };\n    print(v);\n}",
        );
        let candidates: Vec<String> = pick_branches(&program).iter().map(|c| join(c)).collect();
        assert!(
            candidates.contains(
                &"fn main() with IO {\n    let v: List<Int> = [f(20), f(20)];\n    print(v);\n}\n"
                    .to_string()
            ),
            "{candidates:?}"
        );
        let program = lines("let v: Int = match x + 1 {\n    n -> n * 2,\n};");
        assert_eq!(join(&pick_branches(&program)[0]), "let v: Int = (x + 1) * 2;\n");
    }

    #[test]
    fn a_let_read_once_is_inlined() {
        let program = lines("fn main() with IO {\n    let v: Int = 1 + 2;\n    print(v);\n}");
        let candidates = inline_lets(&program);
        assert_eq!(join(&candidates[0]), "fn main() with IO {\n    print((1 + 2));\n}\n");
        assert_eq!(word_at("v1 + v + xv", "v"), vec![5]);
    }

    #[test]
    fn an_expression_becomes_a_literal_or_one_of_its_parts() {
        assert_eq!(expressions(r#"f(a, (b + 1)) + "s""#), vec![(0, 13), (5, 12), (16, 19)]);
        let r = replacements("(b + 1)");
        assert!(r.contains(&"b".to_string()) && r.contains(&"1".to_string()));
        assert!(replacements("f(a, (b + 1))").contains(&"(b + 1)".to_string()));
        assert_eq!(replacements("\"abc\""), vec!["\"\"".to_string()]);
        assert!(replacements("(v25, 0)").contains(&"v25".to_string()));
        assert_eq!(replacements("[f(20)]"), vec!["[]".to_string(), "f(20)".to_string()]);
        assert_eq!(replacements("[p | t]"), vec!["[]".to_string()]);
    }

    #[test]
    fn each_text_is_tested_once() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let test = |_: &str, _: usize| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        };
        let mut tester = Tester::new(&test, 2);
        let a = lines("x");
        assert_eq!(tester.first(&[a.clone(), a.clone(), lines("y")]), None);
        assert_eq!(tester.first(&[a]), None);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(tester.runs, 2);
    }

    #[test]
    fn the_cheapest_disagreeing_pair_is_chosen() {
        let ran = |stdout: &str| Ran {
            finished: true,
            exit: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        };
        let ways = Way::ALL;
        let runs = [ran("1"), ran("2"), ran("1"), ran("2")];
        let (a, b, v) = pick_pair(&ways, &runs).unwrap();
        assert_eq!((a.name, b.name, v.stream), ("vm", "vm_opt", Stream::Stdout));
        let runs = [ran("1"), ran("1"), ran("2"), ran("2")];
        let (a, b, _) = pick_pair(&ways, &runs).unwrap();
        assert_eq!((a.name, b.name), ("vm", "llvm"));
        assert!(pick_pair(&ways, &[ran("1"), ran("1"), ran("1"), ran("1")]).is_none());
    }
}
