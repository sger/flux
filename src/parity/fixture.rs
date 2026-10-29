//! Fixture metadata parsing for parity corpus files.
//!
//! Reads inline metadata from comment headers in `.flx` fixture files:
//!
//! ```text
//! // parity: vm, llvm
//! // expect: success
//! // expect-error: E300
//! // parity-expected-stderr-begin
//! // error[E300]: type mismatch
//! // parity-expected-stderr-end
//! // skip: reason to skip this fixture
//! // skip-way: llvm, llvm_strict: KI-091 a known divergence on these ways
//! // requires: unix        (or `windows`) — skip elsewhere, with a reason
//! // requires: network     — skip unless the run allows it (`--allow network`)
//! // census-skip: reason    — leave the file out of `--census` runs only
//! // stdout: varies         — output depends on the clock or the host; compare exits only
//! // root: examples/x       — an extra module root, from the repo root
//! // flags: --strict        — compile in strict mode on every way
//! ```
//!
//! Instead of `expect-error:`, a fixture can mark each expected error on the
//! line it is reported on, so the line is checked too (proposal 0191):
//!
//! ```text
//! add("x", 2) //~ E300       — this line
//! //~^ E034                  — the line above; `^^` is two above
//! //~v E034                  — the line below
//! ```
//!
//! A `parity.dir` file gives the same directives, one per line and without the
//! `//`, to every fixture in its folder and below; a fixture's own header
//! wins. A line `name.flx: <directive>` applies to that one file, for fixtures
//! whose line numbers cannot move (snapshots, debugger scripts):
//!
//! ```text
//! # examples/diagnostics/parity.dir
//! expect: any
//! hint_demos/math_module/Math.flx: expect: success
//! // bug: description of the bug shape
//! ```

use std::path::Path;

use super::Way;

/// Expected outcome for a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Success,
    CompileError,
    RuntimeError,
    /// A diagnostic demo: it may compile, or fail at compile or run time,
    /// but every way must do the same. Its messages are pinned elsewhere, by
    /// snapshots, so only the ways' agreement is checked here.
    Any,
}

impl Expect {
    /// The header spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::CompileError => "compile_error",
            Self::RuntimeError => "runtime_error",
            Self::Any => "any",
        }
    }
}

/// A way a fixture is known to diverge on, and why. The reason names the
/// open known issue (`KI-nnn`) that tracks it, so no skip is anonymous and
/// each one is removed with the bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipWay {
    pub way: Way,
    pub reason: String,
}

/// Parsed metadata from a parity fixture file.
#[derive(Debug)]
pub struct FixtureMeta {
    /// Which ways to compare (defaults to `[Vm, Llvm]`).
    pub ways: Vec<Way>,
    /// Whether a `// parity:` line chose `ways`, rather than the default.
    pub declared_ways: bool,
    /// Expected outcome (defaults to `Success`).
    pub expect: Expect,
    /// Expected diagnostic error codes for error fixtures.
    pub expected_errors: Vec<String>,
    /// Expected errors as (line, code), from `//~` annotations. A fixture uses
    /// these or `expected_errors`, never both.
    pub expected_locations: Vec<(usize, String)>,
    /// Expected errors located in a module the fixture imports, as (file,
    /// line, code), from `//~ File.flx:5 E400` annotations.
    pub expected_in_modules: Vec<ModuleError>,
    /// Extra CLI args forwarded only for this fixture.
    pub extra_args: Vec<String>,
    /// One-line description of the bug shape.
    pub bug: Option<String>,
    /// Optional reason to skip this fixture while still reporting it.
    pub skip: Option<String>,
    /// Ways removed from this fixture's run, from `// skip-way:`. Unlike
    /// `// parity:`, they hold when `--ways` is given.
    pub skip_ways: Vec<SkipWay>,
    /// Why `--census` leaves this fixture out (`// census-skip:`): it is run
    /// by its own harness, such as a debugger or replay test.
    pub census_skip: Option<String>,
    /// `// stdout: varies`: the program prints the time, timings or the host's
    /// core count, so only its ways' exits are compared.
    pub stdout_varies: bool,
    /// Header lines that could not be understood. A fixture with any fails
    /// instead of running, so a typo cannot silently widen or narrow a run.
    pub meta_errors: Vec<String>,
    /// Platform this fixture requires, verbatim from `// requires:`.
    ///
    /// A fixture that spawns `/bin/echo`, or asserts that reading a directory
    /// gives `IsADirectory`, is making a claim about the host OS rather than
    /// about Flux. Declaring that here keeps the coverage where it is real and
    /// turns the other platforms into a reported `SKIP` rather than a failure
    /// nobody can act on. An unmet requirement is folded into [`Self::skip`],
    /// so the reporter needs no special case.
    pub requires: Option<String>,
    /// Optional expected normalized stdout block.
    pub expected_stdout: Option<String>,
    /// Optional expected normalized stderr block.
    pub expected_stderr: Option<String>,
    /// What Aether must achieve, from `// expect-reuse:` and
    /// `// expect-allocs:`, checked against `flux --metrics` (0190 §15).
    pub expected_counts: Vec<ExpectedCount>,
}

/// A count a fixture states, such as `// expect-reuse: >= 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedCount {
    /// The directive, as written before the colon (`expect-reuse`).
    pub directive: &'static str,
    /// The `--metrics` key it reads (`aether_reuses`).
    pub key: &'static str,
    pub op: CountOp,
    pub value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountOp {
    Exactly,
    AtLeast,
    AtMost,
}

impl ExpectedCount {
    pub fn holds(&self, seen: u64) -> bool {
        match self.op {
            CountOp::Exactly => seen == self.value,
            CountOp::AtLeast => seen >= self.value,
            CountOp::AtMost => seen <= self.value,
        }
    }
}

impl std::fmt::Display for ExpectedCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let op = match self.op {
            CountOp::Exactly => "",
            CountOp::AtLeast => ">= ",
            CountOp::AtMost => "<= ",
        };
        write!(f, "// {}: {op}{}", self.directive, self.value)
    }
}

/// The count directives and the `--metrics` key each reads.
const COUNT_DIRECTIVES: [(&str, &str); 2] =
    [("expect-reuse", "aether_reuses"), ("expect-allocs", "aether_allocs")];

/// `>= 1`, `<= 3`, `= 3` or `3`.
fn parse_count(
    directive: &'static str,
    key: &'static str,
    value: &str,
) -> Result<ExpectedCount, String> {
    let value = value.trim();
    let (op, number) = if let Some(rest) = value.strip_prefix(">=") {
        (CountOp::AtLeast, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (CountOp::AtMost, rest)
    } else if let Some(rest) = value.strip_prefix('=') {
        (CountOp::Exactly, rest)
    } else {
        (CountOp::Exactly, value)
    };
    let value = number
        .trim()
        .parse()
        .map_err(|_| "expected a count: `3`, `= 3`, `>= 1` or `<= 3`".to_string())?;
    Ok(ExpectedCount { directive, key, op, value })
}

/// An expected error in another file: (file, line, code).
pub type ModuleError = (String, usize, String);

impl FixtureMeta {
    /// Whether the fixture lists its errors with `//~` annotations.
    pub fn is_annotated(&self) -> bool {
        !self.expected_locations.is_empty() || !self.expected_in_modules.is_empty()
    }

    /// The expected error codes, whichever form the fixture states them in.
    pub fn expected_codes(&self) -> Vec<String> {
        if !self.is_annotated() {
            return self.expected_errors.clone();
        }
        let in_fixture = self.expected_locations.iter().map(|(_, code)| code.clone());
        in_fixture.chain(self.expected_in_modules.iter().map(|(_, _, code)| code.clone())).collect()
    }
}

impl Default for FixtureMeta {
    fn default() -> Self {
        Self {
            ways: vec![Way::Vm, Way::Llvm],
            declared_ways: false,
            expect: Expect::Success,
            expected_errors: Vec::new(),
            expected_locations: Vec::new(),
            expected_in_modules: Vec::new(),
            extra_args: Vec::new(),
            bug: None,
            skip: None,
            skip_ways: Vec::new(),
            census_skip: None,
            stdout_varies: false,
            meta_errors: Vec::new(),
            requires: None,
            expected_stdout: None,
            expected_stderr: None,
            expected_counts: Vec::new(),
        }
    }
}

/// Parse fixture metadata from inline comments at the top of a `.flx` file.
///
/// Returns `FixtureMeta::default()` if no metadata comments are found.
pub fn parse_fixture_meta(path: &Path) -> FixtureMeta {
    parse_fixture_meta_allowing(path, &[])
}

/// [`parse_fixture_meta`], with the `// requires:` resources this run may use
/// (`network`) in `allowed`.
pub fn parse_fixture_meta_allowing(path: &Path, allowed: &[String]) -> FixtureMeta {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return FixtureMeta::default(),
    };

    let mut meta = directory_defaults(path);
    let mut collecting_block: Option<ExpectedBlockKind> = None;
    let mut expected_block_lines: Vec<String> = Vec::new();

    let mut in_header = true;

    for line in content.lines() {
        let trimmed = line.trim();

        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            in_header = false;
        }

        let Some(comment) = trimmed.strip_prefix("//") else {
            continue;
        };
        let comment_trimmed = comment.trim();

        if let Some(kind) = collecting_block {
            if kind.ends_at(comment_trimmed) {
                let value = expected_block_lines.join("\n").trim().to_string();
                match kind {
                    ExpectedBlockKind::Stdout => meta.expected_stdout = Some(value),
                    ExpectedBlockKind::Stderr => meta.expected_stderr = Some(value),
                }
                collecting_block = None;
                expected_block_lines.clear();
                continue;
            }
            // Preserve internal whitespace for multi-line strings in expected
            // output. We only strip the `// ` prefix (one leading space), not
            // the rest of the content.
            let content_line = comment.strip_prefix(' ').unwrap_or(comment).to_string();
            expected_block_lines.push(content_line);
            continue;
        }
        let comment = comment_trimmed;

        if comment == "parity-expected-stdout-begin" || comment == "parity-oracle-stdout-begin" {
            collecting_block = Some(ExpectedBlockKind::Stdout);
            continue;
        }

        if comment == "parity-expected-stderr-begin" {
            collecting_block = Some(ExpectedBlockKind::Stderr);
            continue;
        }

        if !in_header {
            continue;
        }

        apply_directive(&mut meta, comment);
    }

    if let Some(kind) = collecting_block {
        let value = expected_block_lines.join("\n").trim().to_string();
        match kind {
            ExpectedBlockKind::Stdout => meta.expected_stdout = Some(value),
            ExpectedBlockKind::Stderr => meta.expected_stderr = Some(value),
        }
    }

    let (locations, in_modules, errors) = parse_annotations(&content);
    meta.expected_locations = locations;
    meta.expected_in_modules = in_modules;
    meta.meta_errors.extend(errors);
    if meta.is_annotated() {
        for line in annotations_inside_tokens(&content) {
            meta.meta_errors.push(format!(
                "the `//~` on line {line} is read as part of the program, not as a comment"
            ));
        }
    }
    if meta.is_annotated() && !meta.expected_errors.is_empty() {
        meta.meta_errors.push(
            "`// expect-error:` and `//~` annotations both list expected errors; keep one"
                .to_string(),
        );
    }

    apply_platform_requirement(&mut meta, allowed);
    meta
}

/// Reads the `//~` annotations of a fixture: each expected error in it as
/// (line, code), lines counted from 1, each one in another file as (file,
/// line, code), and one message per malformed annotation.
///
/// A line's comment starts at its first `//` outside a string literal, and is
/// an annotation only when it starts `//~`. Lines inside the expected-output
/// blocks are skipped: they quote the source, annotations included.
pub fn parse_annotations(content: &str) -> (Vec<(usize, String)>, Vec<ModuleError>, Vec<String>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut locations = Vec::new();
    let mut in_modules = Vec::new();
    let mut errors = Vec::new();
    let mut in_block = false;
    for (index, line) in lines.iter().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix("//").map(str::trim)
            && (comment.starts_with("parity-expected-") || comment.starts_with("parity-oracle-"))
        {
            in_block = comment.ends_with("-begin");
            continue;
        }
        if in_block {
            continue;
        }
        let Some(annotation) = comment_of(line).and_then(|c| c.strip_prefix("//~")) else {
            continue;
        };
        let parsed = match module_location(annotation) {
            Some((file, line, codes)) => parse_codes(codes).map(|codes| {
                in_modules.extend(codes.into_iter().map(|code| (file.to_string(), line, code)))
            }),
            None => parse_annotation(annotation, number, lines.len())
                .map(|found| locations.extend(found)),
        };
        if let Err(reason) = parsed {
            errors.push(format!("`//~{annotation}` on line {number}: {reason}"));
        }
    }
    (locations, in_modules, errors)
}

/// The `File.flx:5` an annotation starts with, when it expects an error in
/// another file, and the codes after it.
fn module_location(annotation: &str) -> Option<(&str, usize, &str)> {
    let text = annotation.trim_start();
    let (location, codes) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let (file, line) = location.rsplit_once(':')?;
    if !file.ends_with(".flx") {
        return None;
    }
    Some((file, line.parse().ok()?, codes))
}

/// `source` with its `//~` annotations rewritten to list `expected`, as
/// (line, code), and `in_modules`, as (file, line, code), plus a message for
/// each error it could not place.
///
/// No line is added or removed for an error in the fixture, since that would
/// move every error below it: an annotation that stood on its own line leaves
/// the line empty. Each line's codes go at its end; if it already has a
/// comment or ends inside a token, on the next line that can take one, with
/// one `^` per line up; failing that, on the nearest line above, with one `v`
/// per line down.
///
/// An error in another file takes a line of its own. Lines that held one are
/// reused, and any more go above the first line of code, moving the
/// fixture's own errors below them down.
pub fn with_annotations(
    source: &str,
    expected: &[(usize, String)],
    in_modules: &[ModuleError],
) -> (String, Vec<String>) {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let mut in_block = false;
    let mut free = vec![false; lines.len()];
    let mut slots = Vec::new();
    let mut first_code = None;
    for (index, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix("//").map(str::trim)
            && (comment.starts_with("parity-expected-") || comment.starts_with("parity-oracle-"))
        {
            in_block = comment.ends_with("-begin");
            continue;
        }
        if in_block {
            continue;
        }
        if first_code.is_none() && !trimmed.is_empty() && !trimmed.starts_with("//") {
            first_code = Some(index);
        }
        if let Some(comment) = comment_of(line).filter(|c| c.starts_with("//~")) {
            let elsewhere = module_location(&comment[3..]).is_some();
            let keep = line.len() - comment.len();
            line.truncate(keep);
            let trimmed = line.trim_end().len();
            line.truncate(trimmed);
            if elsewhere && line.is_empty() {
                slots.push(index);
            }
        }
        free[index] = comment_of(line).is_none();
    }

    let mut by_location: std::collections::BTreeMap<(&str, usize), Vec<&str>> = Default::default();
    for (file, line, code) in in_modules {
        by_location.entry((file, *line)).or_default().push(code);
    }
    let mut expected = expected.to_vec();
    if by_location.len() > slots.len() {
        let extra = by_location.len() - slots.len();
        let at = slots.last().map(|&i| i + 1).or(first_code).unwrap_or(lines.len());
        lines.splice(at..at, std::iter::repeat_n(String::new(), extra));
        free.splice(at..at, std::iter::repeat_n(false, extra));
        slots.extend(at..at + extra);
        for (line, _) in &mut expected {
            if *line > at {
                *line += extra;
            }
        }
    }
    for (slot, ((file, line), codes)) in slots.iter().zip(&by_location) {
        lines[*slot] = format!("//~ {file}:{line} {}", codes.join(" "));
        free[*slot] = false;
    }

    let mut by_line: std::collections::BTreeMap<usize, Vec<&str>> = Default::default();
    for (line, code) in &expected {
        by_line.entry(*line).or_default().push(code);
    }
    let targets: Vec<(usize, Vec<&str>)> = by_line.into_iter().collect();
    // Whether a comment stays a comment depends on the comment itself: one
    // after a string left open becomes part of the string. So place, ask the
    // lexer, and place again without the lines it read into a token.
    loop {
        let mut placed = lines.clone();
        let unplaced = place_all(&mut placed, &mut free.clone(), &targets);
        let mut text = placed.join("\n");
        if source.ends_with('\n') {
            text.push('\n');
        }
        let mut retry = false;
        for line in annotations_inside_tokens(&text) {
            retry |= std::mem::replace(&mut free[line - 1], false);
        }
        if !retry {
            return (text, unplaced);
        }
    }
}

/// Places each of `targets` (line, codes) on its own line if free, else the
/// nearest free line below, with one `^` per line up, else above, with one
/// `v` per line down. Returns a message for each it could not place.
fn place_all(
    lines: &mut [String],
    free: &mut [bool],
    targets: &[(usize, Vec<&str>)],
) -> Vec<String> {
    let mut unplaced = Vec::new();
    let mut pending = Vec::new();
    for (line, codes) in targets {
        match line.checked_sub(1).filter(|&i| i < lines.len() && free[i]) {
            Some(index) => place(lines, free, index, "", codes),
            None => pending.push((*line, codes)),
        }
    }
    for (line, codes) in pending {
        let below = (line..lines.len()).find(|&i| free[i]);
        let above = (0..(line - 1).min(lines.len())).rev().find(|&i| free[i]);
        if let Some(index) = below {
            place(lines, free, index, &"^".repeat(index + 1 - line), codes);
        } else if let Some(index) = above {
            place(lines, free, index, &"v".repeat(line - 1 - index), codes);
        } else {
            unplaced.push(format!(
                "cannot place {} for line {line}; annotate it by hand",
                codes.join(" ")
            ));
        }
    }
    unplaced
}

/// `source` moved from `// expect-error:` lines to `//~` annotations: the
/// lines go, and with `drop_stderr_block` so does the expected-stderr block,
/// then each of `reported` (line, code, numbered as in `source`) is annotated
/// where it lands, and each of `in_modules` above the code. An error on a
/// removed line, or one that can't be placed, is an `Err` naming it.
pub fn converted_source(
    source: &str,
    reported: &[(usize, String)],
    in_modules: &[ModuleError],
    drop_stderr_block: bool,
) -> Result<String, String> {
    let mut kept: Vec<&str> = Vec::new();
    // `new_line[i]` is where line `i + 1` of `source` ends up, if it stays.
    let mut new_line: Vec<Option<usize>> = Vec::new();
    let mut in_stderr_block = false;
    for line in source.lines() {
        let comment = line.trim().strip_prefix("//").map(str::trim);
        let starts_block = comment == Some("parity-expected-stderr-begin");
        let ends_block = comment == Some("parity-expected-stderr-end");
        let drop = comment.is_some_and(|c| c.starts_with("expect-error:"))
            || (drop_stderr_block && (in_stderr_block || starts_block));
        if starts_block {
            in_stderr_block = true;
        }
        if ends_block {
            in_stderr_block = false;
        }
        if drop {
            new_line.push(None);
        } else {
            kept.push(line);
            new_line.push(Some(kept.len()));
        }
    }
    // The line past the end, where an error at the end of the file is
    // reported, stays past the end.
    new_line.push(Some(kept.len() + 1));
    let moved = reported
        .iter()
        .map(|(line, code)| match new_line.get(line.wrapping_sub(1)).copied().flatten() {
            Some(at) => Ok((at, code.clone())),
            None => Err(format!("{code} is reported on line {line}, which the move removes")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut text = kept.join("\n");
    if source.ends_with('\n') {
        text.push('\n');
    }
    let (text, unplaced) = with_annotations(&text, &moved, in_modules);
    if unplaced.is_empty() { Ok(text) } else { Err(unplaced.join("; ")) }
}

/// Lines whose `//~` annotation the lexer does not read as a comment, because
/// it falls inside a token such as a string left open on that line. Placing
/// one there changes the program the fixture tests.
pub fn annotations_inside_tokens(source: &str) -> Vec<usize> {
    let tokens = crate::syntax::lexer::Lexer::new(source).tokenize();
    let mut lines = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some(comment) = comment_of(line).filter(|c| c.starts_with("//~")) else {
            // After a `"` that never closes, `comment_of` sees no comment at
            // all, yet a matcher-blind `//~` may sit in the open string.
            let in_open_string = line.match_indices("//~").any(|(at, _)| {
                inside_unterminated_string(&tokens, index + 1, line[..at].chars().count())
            });
            if in_open_string {
                lines.push(index + 1);
            }
            continue;
        };
        let column = line[..line.len() - comment.len()].chars().count();
        if inside_token(&tokens, index + 1, column) {
            lines.push(index + 1);
        }
    }
    lines
}

/// Whether `line`:`column` falls inside a string the lexer found no end for.
fn inside_unterminated_string(
    tokens: &[crate::syntax::token::Token],
    line: usize,
    column: usize,
) -> bool {
    tokens.iter().any(|token| {
        let start = (token.position.line, token.position.column);
        let end = (token.end_position.line, token.end_position.column);
        token.token_type == crate::syntax::token_type::TokenType::UnterminatedString
            && start <= (line, column)
            && (line, column) < end
    })
}

/// Whether `line`:`column` falls inside one of `tokens`. Lines count from 1
/// and columns from 0, in characters.
fn inside_token(tokens: &[crate::syntax::token::Token], line: usize, column: usize) -> bool {
    tokens.iter().any(|token| {
        let start = (token.position.line, token.position.column);
        let end = (token.end_position.line, token.end_position.column);
        start <= (line, column) && (line, column) < end
    })
}

/// Appends an annotation to `lines[at]`, which then takes no other.
fn place(lines: &mut [String], free: &mut [bool], at: usize, marker: &str, codes: &[&str]) {
    let line = &mut lines[at];
    let gap = if line.trim().is_empty() { "" } else { " " };
    line.push_str(&format!("{gap}//~{marker} {}", codes.join(" ")));
    free[at] = false;
}

/// The line's comment, from its first `//` outside a string literal.
fn comment_of(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => return Some(&line[i..]),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The text after `//~`: `^`s or `v`s, then one or more codes.
fn parse_annotation(
    text: &str,
    line: usize,
    line_count: usize,
) -> Result<Vec<(usize, String)>, String> {
    let ups = text.chars().take_while(|&c| c == '^').count();
    let downs = text.chars().take_while(|&c| c == 'v').count();
    let (target, codes) = if ups > 0 {
        (line.checked_sub(ups), &text[ups..])
    } else {
        (Some(line + downs), &text[downs..])
    };
    // One past the last line is where an error at the end of the file is
    // reported; `//~v` on the last line points there.
    let target = target
        .filter(|&t| t >= 1 && t <= line_count + 1)
        .ok_or_else(|| "points outside the file".to_string())?;
    Ok(parse_codes(codes)?.into_iter().map(|code| (target, code)).collect())
}

/// One or more error codes, separated by spaces.
fn parse_codes(text: &str) -> Result<Vec<String>, String> {
    let codes: Vec<&str> = text.split_whitespace().collect();
    if codes.is_empty() {
        return Err("expected an error code such as `E300`".to_string());
    }
    codes
        .into_iter()
        .map(|code| {
            let digits = code.strip_prefix('E').unwrap_or("");
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                Err(format!("`{code}` is not an error code such as `E300`"))
            } else {
                Ok(code.to_string())
            }
        })
        .collect()
}

/// Applies one header directive (the text after `//`) to `meta`. Text that is
/// not a directive is ignored: headers are also prose.
fn apply_directive(meta: &mut FixtureMeta, comment: &str) {
    if let Some(value) = comment.strip_prefix("parity:") {
        let value = value.trim();
        let ways: Vec<Way> = value.split(',').filter_map(|s| Way::parse(s.trim())).collect();
        if !ways.is_empty() {
            meta.ways = ways;
            meta.declared_ways = true;
        }
    } else if let Some(value) = comment.strip_prefix("expect:") {
        let value = value.trim();
        meta.expect = match value {
            "success" => Expect::Success,
            "compile_error" => Expect::CompileError,
            "runtime_error" => Expect::RuntimeError,
            "any" => Expect::Any,
            other => {
                meta.meta_errors.push(format!(
                    "`// expect: {other}`: expected success, compile_error, runtime_error or any"
                ));
                Expect::Success
            }
        };
    } else if let Some((directive, key, value)) =
        COUNT_DIRECTIVES.iter().find_map(|&(directive, key)| {
            comment
                .strip_prefix(directive)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|value| (directive, key, value))
        })
    {
        match parse_count(directive, key, value) {
            Ok(count) => meta.expected_counts.push(count),
            Err(error) => meta.meta_errors.push(format!("`// {directive}:{value}`: {error}")),
        }
    } else if let Some(code) = comment.strip_prefix("expect-error:") {
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        meta.expected_errors.push(code.to_string());
    } else if let Some(value) = comment.strip_prefix("root:") {
        let value = value.trim();
        if !value.is_empty() {
            meta.extra_args.push("--root".to_string());
            meta.extra_args.push(value.to_string());
        }
    } else if let Some(value) = comment.strip_prefix("flags:") {
        // The examples snapshot harness compiles in-process and honours only
        // `--strict`; any other flag would pass here and be ignored there.
        for flag in value.split_whitespace() {
            if flag == "--strict" {
                meta.extra_args.push(flag.to_string());
            } else {
                meta.meta_errors.push(format!("`// flags: {flag}`: the only flag is `--strict`"));
            }
        }
    } else if let Some(value) = comment.strip_prefix("bug:") {
        meta.bug = Some(value.trim().to_string());
    } else if let Some(value) = comment.strip_prefix("requires:") {
        let value = value.trim();
        if !value.is_empty() {
            meta.requires = Some(value.to_string());
        }
    } else if let Some(value) = comment.strip_prefix("skip-way:") {
        match parse_skip_way(value) {
            Ok(skips) => meta.skip_ways.extend(skips),
            Err(error) => meta.meta_errors.push(format!("`// skip-way:{value}`: {error}")),
        }
    } else if let Some(value) = comment.strip_prefix("stdout:") {
        match value.trim() {
            "varies" => meta.stdout_varies = true,
            other => {
                meta.meta_errors.push(format!("`// stdout: {other}`: the only value is `varies`"))
            }
        }
    } else if let Some(value) = comment.strip_prefix("census-skip:") {
        meta.census_skip = Some(value.trim().to_string());
    } else if let Some(value) = comment.strip_prefix("skip:") {
        let value = value.trim();
        meta.skip = Some(if value.is_empty() {
            "fixture metadata requested skip".to_string()
        } else {
            value.to_string()
        });
    }
}

/// The name of the folder-wide defaults file.
pub const DIRECTORY_FILE: &str = "parity.dir";

/// What the `parity.dir` files around `path` declare for it, outermost first
/// so a nearer folder wins; [`FixtureMeta::default`] where none says anything.
fn directory_defaults(path: &Path) -> FixtureMeta {
    let mut meta = FixtureMeta::default();
    let mut folders: Vec<&Path> = path.ancestors().skip(1).collect();
    // Stop at the repository root, the folder with `Cargo.toml`.
    if let Some(root) = folders.iter().position(|dir| dir.join("Cargo.toml").is_file()) {
        folders.truncate(root + 1);
    }
    for folder in folders.iter().rev() {
        let Ok(text) = std::fs::read_to_string(folder.join(DIRECTORY_FILE)) else {
            continue;
        };
        let relative = path.strip_prefix(folder).ok();
        apply_directory_file(&mut meta, &text, relative);
    }
    meta
}

/// Applies a `parity.dir` file's lines to `meta`, for the fixture at
/// `relative` (its path from the file's folder).
fn apply_directory_file(meta: &mut FixtureMeta, text: &str, relative: Option<&Path>) {
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let scoped = line
            .split_once(".flx:")
            .map(|(name, directive)| (format!("{name}.flx"), directive.trim()));
        match scoped {
            Some((name, directive)) => {
                if relative.is_some_and(|rel| rel == Path::new(&name)) {
                    apply_directive(meta, directive);
                }
            }
            None => apply_directive(meta, line),
        }
    }
}

/// Parses the value of `// skip-way: <way>[, <way>…]: <reason>`.
///
/// The reason must name a known issue (`KI-nnn`): a skip is a recorded,
/// tracked divergence, not a way to make a run quiet.
fn parse_skip_way(value: &str) -> Result<Vec<SkipWay>, String> {
    let Some((ways, reason)) = value.split_once(':') else {
        return Err("expected `<way>[, <way>…]: <reason>`".to_string());
    };
    let reason = reason.trim();
    if !reason.contains("KI-") {
        return Err("the reason must name a known issue, as `KI-nnn`".to_string());
    }
    ways.split(',')
        .map(str::trim)
        .map(|name| {
            Way::parse(name)
                .map(|way| SkipWay { way, reason: reason.to_string() })
                .ok_or_else(|| format!("unknown way `{name}`"))
        })
        .collect()
}

/// Turn an unmet `// requires:` into a skip, leaving an explicit `// skip:` alone.
///
/// An unrecognised platform skips too, and says so. Silently ignoring a typo
/// would run the fixture on the platform it declared it cannot run on, which is
/// the failure this directive exists to prevent; skipping loudly costs coverage
/// on one platform and is visible in the `SKIP` line.
fn apply_platform_requirement(meta: &mut FixtureMeta, allowed: &[String]) {
    let Some(requires) = meta.requires.as_deref() else {
        return;
    };
    if meta.skip.is_some() {
        return;
    }
    meta.skip = match requires {
        "unix" => (!cfg!(unix)).then(|| "fixture requires a unix host".to_string()),
        "windows" => (!cfg!(windows)).then(|| "fixture requires a windows host".to_string()),
        "network" => (!allowed.iter().any(|a| a == "network"))
            .then(|| "fixture requires network access (--allow network)".to_string()),
        other => Some(format!(
            "unrecognised `requires: {other}` (expected `unix`, `windows` or `network`)"
        )),
    };
}

#[derive(Debug, Clone, Copy)]
enum ExpectedBlockKind {
    Stdout,
    Stderr,
}

impl ExpectedBlockKind {
    fn ends_at(self, comment: &str) -> bool {
        match self {
            Self::Stdout => {
                comment == "parity-expected-stdout-end" || comment == "parity-oracle-stdout-end"
            }
            Self::Stderr => comment == "parity-expected-stderr-end",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_full_metadata() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_fixture.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// parity: vm, llvm").unwrap();
        writeln!(f, "// expect: runtime_error").unwrap();
        writeln!(f, "// expect-error: E1004").unwrap();
        writeln!(f, "// bug: division by zero differs across backends").unwrap();
        writeln!(f, "// skip: temporary backend gap").unwrap();
        writeln!(f, "// parity-expected-stdout-begin").unwrap();
        writeln!(f, "// line 1").unwrap();
        writeln!(f, "// line 2").unwrap();
        writeln!(f, "// parity-expected-stdout-end").unwrap();
        writeln!(f, "// parity-expected-stderr-begin").unwrap();
        writeln!(f, "// error[E1004]: boundary violation").unwrap();
        writeln!(f, "//   label").unwrap();
        writeln!(f, "// parity-expected-stderr-end").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert!(meta.declared_ways);
        assert_eq!(meta.expect, Expect::RuntimeError);
        assert_eq!(meta.expected_errors, vec!["E1004"]);
        assert_eq!(meta.extra_args, Vec::<String>::new());
        assert_eq!(meta.bug.as_deref(), Some("division by zero differs across backends"));
        assert_eq!(meta.skip.as_deref(), Some("temporary backend gap"));
        assert_eq!(meta.expected_stdout.as_deref(), Some("line 1\nline 2"));
        assert_eq!(
            meta.expected_stderr.as_deref(),
            Some("error[E1004]: boundary violation\n  label")
        );

        let _ = std::fs::remove_file(&path);
    }

    fn annotations(source: &str) -> (Vec<(usize, String)>, Vec<String>) {
        let (found, in_modules, errors) = parse_annotations(source);
        assert!(in_modules.is_empty(), "{in_modules:?}");
        (found, errors)
    }

    fn at(pairs: &[(usize, &str)]) -> Vec<(usize, String)> {
        pairs.iter().map(|(line, code)| (*line, code.to_string())).collect()
    }

    #[test]
    fn an_annotation_names_its_own_line_or_a_neighbour() {
        let source =
            "let a = 1 //~ E300\nlet b = 2\n//~^ E004\n//~^^^ E001\n//~v E034\nlet c = 3\n";
        let (found, errors) = annotations(source);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found, at(&[(1, "E300"), (2, "E004"), (1, "E001"), (6, "E034")]));
    }

    #[test]
    fn one_annotation_can_list_several_codes() {
        let (found, errors) = annotations("f(1, 2, 3) //~ E056 E300\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found, at(&[(1, "E056"), (1, "E300")]));
    }

    #[test]
    fn a_malformed_annotation_is_reported_with_its_line() {
        let (found, errors) = annotations("a //~ E300 oops\nb //~\nc //~^^^^ E1\nd //~ e300\n");
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors[0].contains("line 1") && errors[0].contains("`oops`"), "{}", errors[0]);
        assert!(errors[1].contains("line 2") && errors[1].contains("expected an error code"));
        assert!(errors[2].contains("line 3") && errors[2].contains("outside the file"));
        assert!(errors[3].contains("line 4") && errors[3].contains("`e300`"));
    }

    #[test]
    fn only_a_comment_that_starts_with_the_marker_is_an_annotation() {
        let source = concat!(
            "print(\"//~ E300\")\n",
            "print(\"a \\\" //~ E301\")\n",
            "x // see //~ E302\n",
            "// parity-expected-stderr-begin\n",
            "// 9 | f() //~ E303\n",
            "// parity-expected-stderr-end\n",
            "y //~ E304\n",
        );
        let (found, errors) = annotations(source);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found, at(&[(7, "E304")]));
    }

    #[test]
    fn annotations_and_expect_error_lines_cannot_both_list_errors() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("both_forms.flx");
        std::fs::write(
            &path,
            "// expect: compile_error\n// expect-error: E300\n\nadd(\"x\") //~ E300\n",
        )
        .unwrap();
        let meta = parse_fixture_meta(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(meta.expected_locations, at(&[(4, "E300")]));
        assert!(meta.meta_errors.iter().any(|e| e.contains("keep one")), "{:?}", meta.meta_errors);
    }

    #[test]
    fn blessing_rewrites_annotations_and_leaves_the_rest_of_each_line() {
        let source = "let a = f(1) //~ E004\nlet b = g(\"//\", 2)\n";
        let (text, unplaced) =
            with_annotations(source, &at(&[(1, "E300"), (2, "E300"), (2, "E056")]), &[]);
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(text, "let a = f(1) //~ E300\nlet b = g(\"//\", 2) //~ E300 E056\n");
        assert_eq!(parse_annotations(&text).0, at(&[(1, "E300"), (2, "E300"), (2, "E056")]));
    }

    #[test]
    fn a_line_with_a_comment_is_annotated_from_a_neighbour() {
        let source = "a() // why\nb() // why\nc()\nd()\n";
        let (text, unplaced) = with_annotations(source, &at(&[(1, "E300"), (4, "E004")]), &[]);
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(text, "a() // why\nb() // why\nc() //~^^ E300\nd() //~ E004\n");
        assert_eq!(parse_annotations(&text).0, at(&[(1, "E300"), (4, "E004")]));

        // Nothing free below: the line above takes a `//~v`.
        let (text, _) = with_annotations("a()\nb() // why\n", &at(&[(2, "E1")]), &[]);
        assert_eq!(text, "a() //~v E1\nb() // why\n");
    }

    #[test]
    fn a_standalone_annotation_line_is_emptied_not_removed() {
        let source = "a()\n//~^ E004\nb()\n";
        let (text, unplaced) = with_annotations(source, &at(&[(3, "E001")]), &[]);
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(text, "a()\n\nb() //~ E001\n");
    }

    #[test]
    fn expected_output_blocks_are_left_alone_and_unplaceable_errors_reported() {
        let source = "// parity-expected-stderr-begin\n// 3 | x //~ E300\n// parity-expected-stderr-end\nx() // a\ny() // b\n";
        let (text, unplaced) = with_annotations(source, &at(&[(4, "E300")]), &[]);
        assert_eq!(text, source);
        assert_eq!(unplaced, ["cannot place E300 for line 4; annotate it by hand"]);
    }

    #[test]
    fn converting_drops_the_header_lines_and_annotates_the_shifted_lines() {
        let source = concat!(
            "// parity-expected-stderr-begin\n",
            "// error[E300]: x\n",
            "// parity-expected-stderr-end\n",
            "// expect: compile_error\n",
            "// expect-error: E300\n",
            "// expect-error: E004\n",
            "\n",
            "let a = f(1)\n",
            "let b = g // why\n",
            "let c = 3\n",
        );
        let reported = at(&[(8, "E300"), (9, "E004")]);
        let text = converted_source(source, &reported, &[], true).unwrap();
        assert_eq!(
            text,
            "// expect: compile_error\n\nlet a = f(1) //~ E300\nlet b = g // why\nlet c = 3 //~^ E004\n"
        );
        assert_eq!(parse_annotations(&text).0, at(&[(3, "E300"), (4, "E004")]));

        // Keeping the block keeps its lines, and the shift is smaller.
        let kept = converted_source(source, &reported, &[], false).unwrap();
        assert!(kept.starts_with("// parity-expected-stderr-begin\n"), "{kept}");
        assert_eq!(parse_annotations(&kept).0, at(&[(6, "E300"), (7, "E004")]));
    }

    #[test]
    fn an_error_at_the_end_of_the_file_is_annotated_from_the_last_line() {
        let source = "// expect-error: E034\n\nmodule Demo\n";
        let text = converted_source(source, &at(&[(4, "E034")]), &[], true).unwrap();
        assert_eq!(text, "\nmodule Demo //~v E034\n");
        assert_eq!(annotations(&text), (at(&[(3, "E034")]), vec![]));
    }

    #[test]
    fn converting_refuses_an_error_on_a_removed_or_crowded_line() {
        let source = "// expect-error: E300\nx() // a\n";
        let removed = converted_source(source, &at(&[(1, "E300")]), &[], true).unwrap_err();
        assert!(removed.contains("which the move removes"), "{removed}");
        let crowded = converted_source(source, &at(&[(2, "E300")]), &[], true).unwrap_err();
        assert!(crowded.contains("cannot place E300"), "{crowded}");
    }

    fn elsewhere(errors: &[(&str, usize, &str)]) -> Vec<ModuleError> {
        errors
            .iter()
            .map(|(file, line, code)| (file.to_string(), *line, code.to_string()))
            .collect()
    }

    #[test]
    fn an_annotation_can_name_another_file() {
        let source = "//~ Broken.flx:5 E400 E300
//~ Lib/Mod.flx:12 E1000
import Broken
f() //~ E004
//~ Mod.flx:x E1
";
        let (found, in_modules, errors) = parse_annotations(source);
        assert_eq!(found, at(&[(4, "E004")]));
        assert_eq!(
            in_modules,
            elsewhere(&[
                ("Broken.flx", 5, "E400"),
                ("Broken.flx", 5, "E300"),
                ("Lib/Mod.flx", 12, "E1000")
            ])
        );
        // Without a line number it is not a location, so `Mod.flx:x` reads as a bad code.
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("line 5"), "{}", errors[0]);
    }

    #[test]
    fn errors_in_other_files_reuse_their_lines_then_go_above_the_code() {
        let source = "// header
//~ Old.flx:1 E1
import A
f()
";
        // One location reuses the old line; nothing moves.
        let (text, unplaced) =
            with_annotations(source, &at(&[(4, "E004")]), &elsewhere(&[("A.flx", 3, "E400")]));
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(
            text,
            "// header
//~ A.flx:3 E400
import A
f() //~ E004
"
        );

        // A second one is inserted after it, and the fixture's own error moves down.
        let two = elsewhere(&[("A.flx", 3, "E400"), ("B.flx", 9, "E300")]);
        let (text, unplaced) = with_annotations(source, &at(&[(4, "E004")]), &two);
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(
            text,
            "// header
//~ A.flx:3 E400
//~ B.flx:9 E300
import A
f() //~ E004
"
        );
        let (found, in_modules, _) = parse_annotations(&text);
        assert_eq!((found, in_modules), (at(&[(5, "E004")]), two.clone()));

        // With none to reuse, they go above the first line of code.
        let (text, _) = with_annotations(
            "// header
import A
f()
",
            &at(&[(3, "E004")]),
            &two,
        );
        assert_eq!(
            text,
            "// header
//~ A.flx:3 E400
//~ B.flx:9 E300
import A
f() //~ E004
"
        );
    }

    #[test]
    fn converting_moves_errors_in_other_files_above_the_code() {
        let source = "// expect: compile_error
// expect-error: E400
// expect-error: E004
import A
f()
";
        let text = converted_source(
            source,
            &at(&[(5, "E004")]),
            &elsewhere(&[("A.flx", 3, "E400")]),
            true,
        )
        .unwrap();
        assert_eq!(
            text,
            "// expect: compile_error
//~ A.flx:3 E400
import A
f() //~ E004
"
        );
    }

    #[test]
    fn several_vs_point_that_many_lines_down() {
        let (found, errors) = annotations("a() //~vv E071\nb()\nc()\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found, at(&[(3, "E071")]));
    }

    #[test]
    fn no_annotation_goes_where_an_open_string_would_take_it() {
        // From the `"` inside `#{` on, the lexer reads the rest of the file as
        // one unterminated string, so only the lines above can be used.
        let source = "fn main() {\n    let name = 1\n    print(\"Hi #{name\")\n}\n";
        let (text, unplaced) =
            with_annotations(source, &at(&[(3, "E034"), (3, "E071"), (4, "E071")]), &[]);
        assert!(unplaced.is_empty(), "{unplaced:?}");
        assert_eq!(
            text,
            "fn main() { //~vvv E071\n    let name = 1 //~v E034 E071\n    print(\"Hi #{name\")\n}\n"
        );
        assert!(annotations_inside_tokens(&text).is_empty());
        assert_eq!(annotations(&text).0, at(&[(4, "E071"), (3, "E034"), (3, "E071")]));
    }

    #[test]
    fn an_annotation_the_lexer_reads_into_a_token_is_found() {
        assert!(annotations_inside_tokens("let a = f(1) //~ E300\nlet b = \"//~\"\n").is_empty());
        // The `"` inside the open interpolation does not close the string, so
        // the comment becomes part of it.
        let open = "print(\"Hello #{name\") //~ E034\n}\n";
        assert_eq!(annotations_inside_tokens(open), [1]);
    }

    #[test]
    fn an_annotation_after_a_string_left_open_on_its_line_is_found() {
        assert_eq!(annotations_inside_tokens("let x = \"open //~ E071\n}\n"), [1]);
    }

    #[test]
    fn expected_codes_come_from_either_form() {
        let listed = FixtureMeta { expected_errors: vec!["E300".into()], ..Default::default() };
        assert_eq!(listed.expected_codes(), ["E300"]);
        let annotated = FixtureMeta {
            expected_locations: at(&[(3, "E004")]),
            expected_in_modules: elsewhere(&[("A.flx", 1, "E400")]),
            ..Default::default()
        };
        assert_eq!(annotated.expected_codes(), ["E004", "E400"]);
    }

    fn counts_of(lines: &[&str]) -> FixtureMeta {
        let mut meta = FixtureMeta::default();
        for line in lines {
            apply_directive(&mut meta, line);
        }
        meta
    }

    #[test]
    fn expected_counts_parse_each_comparison() {
        let meta = counts_of(&[
            "expect-reuse: >= 1",
            "expect-allocs: <= 3",
            "expect-reuse: = 2",
            "expect-allocs: 0",
        ]);
        assert!(meta.meta_errors.is_empty(), "{:?}", meta.meta_errors);
        let shown: Vec<String> = meta.expected_counts.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "// expect-reuse: >= 1",
                "// expect-allocs: <= 3",
                "// expect-reuse: 2",
                "// expect-allocs: 0",
            ]
        );
        assert_eq!(meta.expected_counts[0].key, "aether_reuses");
        assert_eq!(meta.expected_counts[1].key, "aether_allocs");
    }

    #[test]
    fn expected_counts_hold_by_their_comparison() {
        let meta = counts_of(&["expect-reuse: >= 1", "expect-allocs: <= 3", "expect-reuse: 2"]);
        let [at_least, at_most, exactly] = &meta.expected_counts[..] else {
            panic!("three counts");
        };
        assert!(at_least.holds(1) && at_least.holds(5) && !at_least.holds(0));
        assert!(at_most.holds(3) && at_most.holds(0) && !at_most.holds(4));
        assert!(exactly.holds(2) && !exactly.holds(1) && !exactly.holds(3));
    }

    #[test]
    fn a_bad_expected_count_is_a_meta_error() {
        let meta = counts_of(&["expect-reuse: lots", "expect-allocs: >= -1", "expect-reuse:"]);
        assert!(meta.expected_counts.is_empty());
        assert_eq!(meta.meta_errors.len(), 3, "{:?}", meta.meta_errors);
        assert!(meta.meta_errors[0].starts_with("`// expect-reuse: lots`"));
    }

    #[test]
    fn requires_matching_platform_does_not_skip() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "windows" } else { "unix" }.to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert_eq!(meta.skip, None);
    }

    #[test]
    fn requires_other_platform_skips_with_a_reason() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "unix" } else { "windows" }.to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert!(
            meta.skip.as_deref().is_some_and(|r| r.contains("requires a")),
            "expected a platform skip reason, got {:?}",
            meta.skip
        );
    }

    #[test]
    fn unrecognised_requires_skips_rather_than_running() {
        // A typo must not run the fixture on the platform it declared it
        // cannot run on. Skipping is visible in the SKIP line; ignoring is not.
        let mut meta = FixtureMeta { requires: Some("unixx".to_string()), ..Default::default() };
        apply_platform_requirement(&mut meta, &[]);
        assert!(
            meta.skip.as_deref().is_some_and(|r| r.contains("unixx")),
            "expected the typo to be named, got {:?}",
            meta.skip
        );
    }

    #[test]
    fn a_directory_file_gives_defaults_and_names_single_files() {
        let mut meta = FixtureMeta::default();
        let text = "# demos\nexpect: any\n\nsub/b.flx: expect: runtime_error\na.flx: census-skip: run by its harness\n";
        apply_directory_file(&mut meta, text, Some(Path::new("a.flx")));
        assert_eq!(meta.expect, Expect::Any);
        assert_eq!(meta.census_skip.as_deref(), Some("run by its harness"));
        let mut meta = FixtureMeta::default();
        apply_directory_file(&mut meta, text, Some(Path::new("sub/b.flx")));
        assert_eq!(meta.expect, Expect::RuntimeError);
        assert_eq!(meta.census_skip, None);
        let mut meta = FixtureMeta::default();
        apply_directory_file(
            &mut meta,
            "x.flx: stdout: varies\ny.flx: stdout: maybe\n",
            Some(Path::new("x.flx")),
        );
        assert!(meta.stdout_varies && meta.meta_errors.is_empty());
    }

    #[test]
    fn flags_add_strict_and_reject_other_flags() {
        let mut meta = FixtureMeta::default();
        apply_directive(&mut meta, "flags: --strict");
        assert_eq!(meta.extra_args, ["--strict"]);
        assert!(meta.meta_errors.is_empty(), "{:?}", meta.meta_errors);
        apply_directive(&mut meta, "flags: --optimize");
        assert_eq!(meta.extra_args, ["--strict"]);
        assert_eq!(meta.meta_errors.len(), 1);
    }

    #[test]
    fn an_unknown_expect_value_is_a_meta_error() {
        let mut meta = FixtureMeta::default();
        apply_directive(&mut meta, "expect: error");
        assert_eq!(meta.meta_errors.len(), 1, "{:?}", meta.meta_errors);
    }

    #[test]
    fn a_fixture_header_wins_over_its_folder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_dir_test");
        let _ = std::fs::create_dir_all(dir.join("inner"));
        std::fs::write(dir.join(DIRECTORY_FILE), "expect: any\nskip: outer\n").unwrap();
        std::fs::write(dir.join("inner").join(DIRECTORY_FILE), "skip: inner\n").unwrap();
        let fixture = dir.join("inner").join("x.flx");
        std::fs::write(&fixture, "// expect: compile_error\nfn main() { 1 }\n").unwrap();
        let meta = parse_fixture_meta(&fixture);
        assert_eq!(meta.expect, Expect::CompileError);
        assert_eq!(meta.skip.as_deref(), Some("inner"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_is_skipped_unless_allowed() {
        let network =
            || FixtureMeta { requires: Some("network".to_string()), ..Default::default() };
        let mut meta = network();
        apply_platform_requirement(&mut meta, &[]);
        assert!(meta.skip.as_deref().is_some_and(|r| r.contains("--allow network")));
        let mut meta = network();
        apply_platform_requirement(&mut meta, &["network".to_string()]);
        assert_eq!(meta.skip, None);
    }

    #[test]
    fn an_explicit_skip_survives_a_met_requirement() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "windows" } else { "unix" }.to_string()),
            skip: Some("a real backend gap".to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert_eq!(meta.skip.as_deref(), Some("a real backend gap"));
    }

    #[test]
    fn defaults_without_metadata() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_no_meta.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert!(!meta.declared_ways);
        assert_eq!(meta.expect, Expect::Success);
        assert_eq!(meta.extra_args, Vec::<String>::new());
        assert!(meta.bug.is_none());
        assert!(meta.expected_stdout.is_none());
        assert!(meta.expected_stderr.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_expected_stdout_block_outside_header() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_expected_stdout_after_code.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    print(1)").unwrap();
        writeln!(f, "}}").unwrap();
        writeln!(f, "// parity-expected-stdout-begin").unwrap();
        writeln!(f, "// 1").unwrap();
        writeln!(f, "// parity-expected-stdout-end").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.expected_stdout.as_deref(), Some("1"));
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert_eq!(meta.expect, Expect::Success);
        assert_eq!(meta.extra_args, Vec::<String>::new());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_expected_stderr_block_outside_header() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_expected_stderr_after_code.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    print(1)").unwrap();
        writeln!(f, "}}").unwrap();
        writeln!(f, "// parity-expected-stderr-begin").unwrap();
        writeln!(f, "// error[E300]: type mismatch").unwrap();
        writeln!(f, "//   at line 1").unwrap();
        writeln!(f, "// parity-expected-stderr-end").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(
            meta.expected_stderr.as_deref(),
            Some("error[E300]: type mismatch\n  at line 1")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skip_way_names_one_or_more_ways_and_a_ki() {
        assert_eq!(
            parse_skip_way(" llvm: KI-091 float formatting of -0.0"),
            Ok(vec![SkipWay {
                way: Way::Llvm,
                reason: "KI-091 float formatting of -0.0".to_string(),
            }])
        );
        let ways: Vec<Way> = parse_skip_way(" vm, vm_strict : KI-102 x")
            .unwrap()
            .into_iter()
            .map(|skip| skip.way)
            .collect();
        assert_eq!(ways, [Way::Vm, Way::VmStrict]);
    }

    #[test]
    fn skip_way_rejects_an_anonymous_reason_an_unknown_way_and_no_reason() {
        assert!(parse_skip_way(" llvm: flaky").unwrap_err().contains("KI-"));
        assert!(parse_skip_way(" llvmm: KI-091 x").unwrap_err().contains("llvmm"));
        assert!(parse_skip_way(" llvm").is_err());
    }

    #[test]
    fn skip_way_lines_accumulate_and_bad_ones_are_kept_as_errors() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_skip_way.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// skip-way: llvm: KI-091 a").unwrap();
        writeln!(f, "// skip-way: vm_strict: KI-076 b").unwrap();
        writeln!(f, "// skip-way: vm: no issue named").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        let ways: Vec<Way> = meta.skip_ways.iter().map(|skip| skip.way).collect();
        assert_eq!(ways, [Way::Llvm, Way::VmStrict]);
        assert_eq!(meta.skip, None);
        assert_eq!(meta.meta_errors.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_fixture_root_into_extra_args() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_root_meta.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// root: examples").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.extra_args, vec!["--root", "examples"]);

        let _ = std::fs::remove_file(&path);
    }
}
