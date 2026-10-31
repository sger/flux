//! `cargo xtask scale` (proposal 0192 S5): compile inputs of growing size and
//! fail when a compiler count grows faster than allowed, as Swift's
//! `utils/scale-test` does. A count from `--metrics`, not a time, so the
//! answer is the same on a laptop and on CI.
//!
//! Each test generates a program for a size `n`. The count for `n = 1` is
//! taken off every size, so the library and the empty program do not hide
//! the growth, and the growth exponent is the slope of log count against
//! log `n`. A linear pass gives about 1.0, a quadratic one about 2.0.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::proc::run;
use crate::store;
use crate::sweep::run_with_timeout;
use crate::text::Paint;
use crate::verify::{cargo, flux_binary};

pub struct Options {
    /// A `flux` to use instead of building one.
    pub flux: Option<PathBuf>,
    /// The tests to run; all of them when empty.
    pub names: Vec<String>,
}

struct ScaleTest {
    name: &'static str,
    metric: &'static str,
    /// The largest growth exponent that passes.
    limit: f64,
    /// The known issue that makes this test fail today. Such a test is
    /// reported but does not fail the run; once it passes, the marker has to
    /// go, so a fix cannot slip back unnoticed.
    known: Option<&'static str>,
    generate: fn(usize) -> String,
}

const SIZES: [usize; 4] = [250, 500, 1000, 2000];

/// Allowed growth: a little above linear, since a pass that is linear in
/// theory still grows its tables and maps in steps.
const LIMIT: f64 = 1.2;

const TESTS: [ScaleTest; 2] = [
    ScaleTest {
        name: "let_chain",
        metric: "compiler_allocs",
        limit: LIMIT,
        known: Some("KI-151"),
        generate: let_chain,
    },
    ScaleTest {
        name: "many_fns",
        metric: "compiler_allocs",
        limit: LIMIT,
        known: Some("KI-154"),
        generate: many_fns,
    },
];

/// One function made of `n` `let`s, each using the one before.
fn let_chain(n: usize) -> String {
    let mut src = String::from("fn main() with IO {\n    let x0 = 1\n");
    for i in 1..n {
        src.push_str(&format!("    let x{i} = x{} + {i}\n", i - 1));
    }
    src.push_str(&format!("    print(x{})\n}}\n", n - 1));
    src
}

/// `n` small functions, of which `main` calls one. Not a call chain: the VM
/// limits call depth, and the test is about compiling them.
fn many_fns(n: usize) -> String {
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!("fn f{i}(x) {{\n    x + {i}\n}}\n\n"));
    }
    src.push_str(&format!("fn main() with IO {{\n    print(f{}(1))\n}}\n", n - 1));
    src
}

/// Returns `Ok(true)` when every test passed or failed as known.
pub fn run_scale(root: &Path, options: &Options) -> Result<bool, String> {
    let tests: Vec<&ScaleTest> = if options.names.is_empty() {
        TESTS.iter().collect()
    } else {
        options
            .names
            .iter()
            .map(|name| {
                TESTS.iter().find(|test| test.name == name).ok_or_else(|| {
                    let names: Vec<_> = TESTS.iter().map(|test| test.name).collect();
                    format!("no scale test {name}; there are {}", names.join(", "))
                })
            })
            .collect::<Result<_, _>>()?
    };
    let flux = match &options.flux {
        Some(flux) => flux.clone(),
        None => {
            let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
                .map_err(|e| e.to_string())?;
            if build != 0 {
                return Err("cargo build failed".to_string());
            }
            flux_binary(root)
        }
    };

    let out = store::run_dir(root, "scale")?;
    // A cache left by an earlier run would already hold these programs, and
    // a cached module costs almost nothing to load.
    let cache = out.join("cache");
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    }
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // Fill the cache with the library first, from a program none of the
    // tests generate, so the n = 1 count does not include compiling it.
    measure(root, &flux, &out, "warm_up", "fn main() with IO {\n    print(0)\n}\n", None)?;

    let paint = Paint::detect();
    let mut passed = true;
    for test in tests {
        let base = measure(
            root,
            &flux,
            &out,
            &format!("{}_1", test.name),
            &(test.generate)(1),
            Some(test.metric),
        )?;
        let mut counts = Vec::new();
        for n in SIZES {
            let source = (test.generate)(n);
            let count = measure(
                root,
                &flux,
                &out,
                &format!("{}_{n}", test.name),
                &source,
                Some(test.metric),
            )?;
            counts.push((n, count.saturating_sub(base)));
        }
        let exponent = growth_exponent(&counts)
            .ok_or_else(|| format!("{}: {} did not grow with n", test.name, test.metric))?;

        println!("scale test: {}", test.name);
        let sizes: Vec<_> = counts.iter().map(|(n, _)| format!("{n:>9}")).collect();
        let values: Vec<_> = counts.iter().map(|(_, c)| format!("{:>9}", millions(*c))).collect();
        println!("  {:<16}{}", "n", sizes.join(""));
        println!("  {:<16}{}", test.metric, values.join(""));
        let verdict = match (exponent <= test.limit, test.known) {
            (true, None) => paint.green("ok"),
            (false, Some(ki)) => paint.yellow(&format!("FAIL, known ({ki})")),
            (false, None) => {
                passed = false;
                paint.red("FAIL")
            }
            (true, Some(ki)) => {
                passed = false;
                paint.red(&format!("passes now: remove the {ki} marker"))
            }
        };
        println!("  growth exponent {exponent:.2}, limit {:.2}: {verdict}", test.limit);
    }
    Ok(passed)
}

/// Writes `source` to `<out>/<name>.flx` and compiles and runs it with
/// `--metrics`; returns `metric`'s count.
fn measure(
    root: &Path,
    flux: &Path,
    out: &Path,
    name: &str,
    source: &str,
    metric: Option<&str>,
) -> Result<u64, String> {
    let file = out.join(format!("{name}.flx"));
    let metrics = out.join(format!("{name}.tsv"));
    fs::write(&file, source).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut command = Command::new(flux);
    command.arg(&file).arg("--cache-dir").arg(out.join("cache")).arg("--metrics").arg(&metrics);
    let ran = run_with_timeout(command, root, Duration::from_secs(300));
    if !ran.finished || ran.exit != 0 {
        let how = if ran.finished { format!("exit {}", ran.exit) } else { "timed out".to_string() };
        return Err(format!("{}: {how}\n{}", store::display(&file), ran.stderr.trim_end()));
    }
    let Some(metric) = metric else {
        return Ok(0);
    };
    let text = fs::read_to_string(&metrics).map_err(|e| format!("{}: {e}", metrics.display()))?;
    text.lines()
        .find_map(|line| line.strip_prefix(metric)?.strip_prefix('\t')?.trim().parse().ok())
        .ok_or_else(|| format!("{}: no {metric} count", store::display(&metrics)))
}

/// The least-squares slope of ln(count) against ln(n), or `None` when a
/// count is zero and has no logarithm.
fn growth_exponent(counts: &[(usize, u64)]) -> Option<f64> {
    if counts.len() < 2 || counts.iter().any(|&(_, c)| c == 0) {
        return None;
    }
    let points: Vec<(f64, f64)> =
        counts.iter().map(|&(n, c)| ((n as f64).ln(), (c as f64).ln())).collect();
    let len = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / len;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / len;
    let covariance: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let variance: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    Some(covariance / variance)
}

fn millions(count: u64) -> String {
    format!("{:.2}M", count as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_exponent_reads_the_power_of_n() {
        let linear: Vec<_> = SIZES.iter().map(|&n| (n, 300 * n as u64)).collect();
        let quadratic: Vec<_> = SIZES.iter().map(|&n| (n, 7 * (n * n) as u64)).collect();
        assert!((growth_exponent(&linear).unwrap() - 1.0).abs() < 1e-9);
        assert!((growth_exponent(&quadratic).unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(growth_exponent(&[(250, 0), (500, 10)]), None);
    }

    #[test]
    fn generated_chains_end_in_the_last_binding() {
        assert!(let_chain(3).ends_with("    let x2 = x1 + 2\n    print(x2)\n}\n"));
        assert!(many_fns(2).contains("fn f1(x) {\n    x + 1\n}\n"));
        assert!(many_fns(2).ends_with("print(f1(1))\n}\n"));
    }
}
