//! A verify run's bookkeeping: numbered steps, how long each took, the
//! closing summary, and the summary saved for the next run to compare with.

use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

use crate::text::Paint;

/// How a summary entry counts towards the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Fail,
    /// Informational: never fails the run.
    Note,
}

impl Mark {
    fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Fail => "FAIL",
            Self::Note => "note",
        }
    }
}

/// One line of the closing summary.
#[derive(Debug)]
pub struct Entry {
    /// A name that does not depend on the tag, such as `parity` or
    /// `sweep core`, so a step can be matched with the base run's.
    pub step: String,
    pub mark: Mark,
    pub line: String,
    pub took: Option<Duration>,
    /// Shown under a failed entry: what failed, and where to look.
    pub details: Vec<String>,
}

/// The closing summary, one entry per step, and when the run started.
pub struct Outcome {
    entries: Vec<Entry>,
    started: Instant,
}

impl Outcome {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            started: Instant::now(),
        }
    }

    pub fn record(&mut self, step: &str, ok: bool, line: String, took: Duration) -> &mut Entry {
        self.push(Entry {
            step: step.to_string(),
            mark: if ok { Mark::Ok } else { Mark::Fail },
            line,
            took: Some(took),
            details: Vec::new(),
        })
    }

    /// An entry that never fails the run, timed when it is a step of its
    /// own (a sweep) and untimed when it only reports something.
    pub fn note(&mut self, step: &str, line: String, took: Option<Duration>) {
        self.push(Entry {
            step: step.to_string(),
            mark: Mark::Note,
            line,
            took,
            details: Vec::new(),
        });
    }

    fn push(&mut self, entry: Entry) -> &mut Entry {
        self.entries.push(entry);
        self.entries.last_mut().expect("just pushed")
    }

    pub fn failed(&self) -> bool {
        self.entries.iter().any(|entry| entry.mark == Mark::Fail)
    }

    pub fn total(&self) -> Duration {
        self.started.elapsed()
    }

    /// The summary: a banner, then each entry with its time and, when a base
    /// run's times are known, the change from them.
    pub fn print(&self, title: &str, out_dir: &str, base: Option<&HashMap<String, Duration>>) {
        let paint = Paint::detect();
        let total = self.total();
        let delta = |step: &str, took: Duration| {
            base.and_then(|times| times.get(step))
                .map(|&before| format!("  {}", format_delta(took, before)))
                .unwrap_or_default()
        };
        let times: Vec<String> = self
            .entries
            .iter()
            .map(|entry| {
                entry
                    .took
                    .map(|took| format!("{}{}", format_duration(took), delta(&entry.step, took)))
                    .unwrap_or_default()
            })
            .collect();
        let total_time = format!("{}{}", format_duration(total), delta(TOTAL, total));
        // Wide enough that every time lines up, however long a line is.
        let width = self
            .entries
            .iter()
            .zip(&times)
            .map(|(entry, time)| entry.line.chars().count() + 2 + time.chars().count())
            .chain([MIN_WIDTH])
            .max()
            .unwrap_or(MIN_WIDTH);
        let rule = "─".repeat(width + 5);
        println!("{}", paint.banner(&rule));
        println!(
            "{}",
            paint.banner(&format!(
                " summary {title} · {} · logs in {out_dir}",
                format_duration(total)
            ))
        );
        println!("{}", paint.banner(&rule));
        for (entry, time) in self.entries.iter().zip(&times) {
            let text = pad(&entry.line, time, width);
            match entry.mark {
                Mark::Ok => println!("{} {text}", paint.green("ok  ")),
                Mark::Fail => println!("{} {text}", paint.red("FAIL")),
                Mark::Note => println!("     {}", paint.yellow(&text)),
            }
            for detail in &entry.details {
                println!("       {}", paint.red(detail));
            }
        }
        println!("     {}", paint.bold(&pad("total", &total_time, width)));
    }

    /// The summary as a file: one tab-separated `step, mark, seconds, line`
    /// per entry (seconds empty for a note), then the total.
    pub fn to_file(&self) -> String {
        let mut text = String::new();
        for entry in &self.entries {
            let seconds = entry
                .took
                .map(|took| took.as_secs().to_string())
                .unwrap_or_default();
            let _ = writeln!(
                text,
                "{}\t{}\t{seconds}\t{}",
                entry.step,
                entry.mark.word(),
                entry.line
            );
        }
        let _ = writeln!(text, "{TOTAL}\t\t{}\t", self.total().as_secs());
        text
    }
}

/// The step name under which the whole run's time is saved.
const TOTAL: &str = "total";

/// The narrowest the summary gets, after its mark column.
const MIN_WIDTH: usize = 67;

/// `line` followed by `time`, right-aligned to `width`.
fn pad(line: &str, time: &str, width: usize) -> String {
    if time.is_empty() {
        return line.to_string();
    }
    let used = line.chars().count() + time.chars().count();
    let gap = width.saturating_sub(used).max(2);
    format!("{line}{}{time}", " ".repeat(gap))
}

/// Each timed step's duration in a summary file that [`Outcome::to_file`]
/// wrote. Lines it cannot read are ignored.
pub fn parse_summary_file(text: &str) -> HashMap<String, Duration> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let step = fields.next()?;
            let seconds = fields.nth(1)?.parse().ok()?;
            Some((step.to_string(), Duration::from_secs(seconds)))
        })
        .collect()
}

/// `m:ss`, or `h:mm:ss` from an hour on.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// How `now` compares with `before`: `+3:12`, `−0:20` or `±0:00`.
pub fn format_delta(now: Duration, before: Duration) -> String {
    // Whole seconds on both sides, as shown, so the sign always agrees with
    // the difference: 15.9 s against 16.1 s is −0:01, not −0:00.
    let (now, before) = (now.as_secs(), before.as_secs());
    let sign = match now.cmp(&before) {
        std::cmp::Ordering::Greater => "+",
        std::cmp::Ordering::Less => "−",
        std::cmp::Ordering::Equal => "±",
    };
    format!(
        "{sign}{}",
        format_duration(Duration::from_secs(now.abs_diff(before)))
    )
}

/// The run's steps, numbered as they start: `== [4/9] debug build parity`.
pub struct Steps {
    done: usize,
    total: usize,
}

impl Steps {
    pub fn new(total: usize) -> Self {
        Self { done: 0, total }
    }

    /// Prints the next step's heading and returns when it started.
    pub fn start(&mut self, title: &str) -> Instant {
        self.done += 1;
        let heading = format!("== [{}/{}] {title}", self.done, self.total);
        println!("{}", Paint::detect().heading(&heading));
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn durations_read_as_minutes_until_an_hour() {
        assert_eq!(format_duration(secs(0)), "0:00");
        assert_eq!(format_duration(secs(892)), "14:52");
        assert_eq!(format_duration(secs(3600 + 61)), "1:01:01");
    }

    #[test]
    fn a_delta_has_a_sign_either_way() {
        assert_eq!(format_delta(secs(300), secs(108)), "+3:12");
        assert_eq!(format_delta(secs(100), secs(120)), "−0:20");
        assert_eq!(format_delta(secs(5), secs(5)), "±0:00");
        let ms = Duration::from_millis;
        assert_eq!(format_delta(ms(15_900), ms(16_100)), "−0:01");
        assert_eq!(format_delta(ms(16_100), ms(16_900)), "±0:00");
    }

    #[test]
    fn a_saved_summary_reads_back_its_timed_steps() {
        let mut outcome = Outcome::new();
        outcome.record("parity", true, "parity (0 MISMATCH)".into(), secs(892));
        outcome.record("fmt", false, "fmt".into(), secs(3));
        outcome.note("internal cfg", "sweep_s3: 0 internal".into(), None);
        outcome.note("sweep cfg", "sweep_s3 (cfg)".into(), Some(secs(40)));
        let times = parse_summary_file(&outcome.to_file());
        assert_eq!(times.get("parity"), Some(&secs(892)));
        assert_eq!(times.get("fmt"), Some(&secs(3)));
        assert!(times.contains_key(TOTAL));
        assert_eq!(times.get("sweep cfg"), Some(&secs(40)));
        assert!(!times.contains_key("internal cfg"));
        assert!(parse_summary_file("garbage\nno\ttabs here").is_empty());
    }

    #[test]
    fn only_a_failed_step_fails_the_run() {
        let mut outcome = Outcome::new();
        outcome.record("fmt", true, "fmt".into(), secs(1));
        outcome.note("parity", "parity: 9 MISMATCH".into(), None);
        assert!(!outcome.failed());
        outcome.record("debug parity", false, "debug parity".into(), secs(1));
        assert!(outcome.failed());
    }

    #[test]
    fn a_time_is_right_aligned_and_never_touches_its_line() {
        let padded = pad("fmt", "0:03", 20);
        assert_eq!(padded.chars().count(), 20);
        assert!(padded.ends_with(" 0:03"));
        let long = "x".repeat(20);
        assert!(pad(&long, "0:03", 20).ends_with("x  0:03"));
        assert_eq!(pad("note", "", 20), "note");
    }
}
