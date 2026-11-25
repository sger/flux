//! `cargo xtask bench`: times the programs in `benchmarks/` on the Flux VM,
//! as native Flux binaries, and as the Rust, Python, Haskell, OCaml, Erlang,
//! Elixir and Go ports, with hyperfine. It runs the same way on Linux, macOS
//! and Windows.
//!
//! Before timing, every case runs once and must print what the Flux VM
//! prints, so a port that computes something else is never timed. That run
//! also writes the VM's `flux --metrics` counts. Results are compared with
//! the baseline recorded on the same kind of machine,
//! `benchmarks/baselines/<os>-<arch>/`, using the Mann–Whitney U test
//! (`stats.rs`).

mod hyperfine;
mod report;
mod stats;
mod table;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use table::Benchmark;

use crate::proc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProfileWay {
    Vm,
    Native,
}

impl ProfileWay {
    pub fn parse(s: &str) -> Option<ProfileWay> {
        match s {
            "vm" => Some(ProfileWay::Vm),
            "native" => Some(ProfileWay::Native),
            _ => None,
        }
    }
}

pub struct Options {
    /// A benchmark's name, or `core`, `extended` or `all`.
    pub target: String,
    /// An exact number of runs; otherwise hyperfine picks between 3 and 50.
    pub runs: Option<usize>,
    pub warmup: usize,
    pub full: bool,
    pub save_baseline: bool,
    pub check: bool,
    pub readme: bool,
    pub report_file: Option<PathBuf>,
    pub native: bool,
    pub vms: bool,
    pub build: bool,
    pub profile: Option<ProfileWay>,
}

/// A command hyperfine times, as the program and its arguments, with paths
/// relative to the repository root.
struct Case {
    name: String,
    argv: Vec<String>,
}

const BENCH_DIR: &str = "target/bench";

/// Returns whether every benchmark ran and every port agreed.
pub fn run_bench(root: &Path, options: &Options) -> Result<bool, String> {
    let benchmarks = table::select(&options.target)?;
    if options.full && benchmarks.len() == 1 && benchmarks[0].workload.is_none() {
        return Err(format!("{} has no separate full workload", benchmarks[0].name));
    }
    if let Some(way) = options.profile {
        let [benchmark] = benchmarks[..] else {
            return Err("--profile takes one benchmark, not a group".into());
        };
        return profile(root, benchmark, way, options);
    }
    let hyperfine = on_path("hyperfine")
        .ok_or("hyperfine is not installed: cargo install --locked hyperfine")?;
    if options.build {
        build_rust(root, &benchmarks, &["--release", "--features", "llvm"])?;
    }
    let flux = root.join(exe("target/release/flux"));
    if !flux.is_file() {
        return Err(format!(
            "{} is missing: cargo build --release --features llvm --bin flux",
            flux.display()
        ));
    }
    fs::create_dir_all(root.join(BENCH_DIR)).map_err(|e| format!("{BENCH_DIR}: {e}"))?;

    let mut outcomes = Vec::new();
    for benchmark in &benchmarks {
        if benchmarks.len() > 1 {
            println!("== {} ==", benchmark.name);
        }
        match run_one(root, benchmark, options, &hyperfine) {
            Ok(outcome) => outcomes.push(outcome),
            Err(e) => {
                eprintln!("{}: {e}", benchmark.name);
                outcomes.push(report::Outcome {
                    name: benchmark.name.to_string(),
                    passed: false,
                    vm_vs_baseline: None,
                    native_vs_baseline: None,
                    vm_over_native: None,
                });
            }
        }
    }
    if outcomes.len() > 1 {
        println!("\n{}", report::summary(&outcomes));
    }
    Ok(outcomes.iter().all(|o| o.passed))
}

fn run_one(
    root: &Path,
    b: &Benchmark,
    options: &Options,
    hyperfine: &Path,
) -> Result<report::Outcome, String> {
    let full = options.full && b.workload.is_some();
    let suffix = if full { "_full" } else { "" };
    let cases = prepare_cases(root, b, full, options)?;
    let vm = format!("{}/flux", b.name);
    let native = format!("{}/flux-native", b.name);
    let failed = |name: &str| report::Outcome {
        name: name.to_string(),
        passed: false,
        vm_vs_baseline: None,
        native_vs_baseline: None,
        vm_over_native: None,
    };

    let metrics_file = root.join(BENCH_DIR).join(format!("{}{suffix}.metrics.tsv", b.name));
    let _ = fs::remove_file(&metrics_file);
    if options.check && !check_outputs(root, &cases, &vm, &metrics_file)? {
        return Ok(failed(b.name));
    }

    let results_file = format!("{BENCH_DIR}/{}{suffix}.json", b.name);
    let mut cmd = Command::new(hyperfine);
    cmd.current_dir(root).arg("--shell=none").args(["--warmup", &options.warmup.to_string()]);
    match options.runs {
        Some(n) => cmd.args(["--runs", &n.to_string()]),
        // As Go's b.N: as many runs as fit hyperfine's time budget, so a
        // short program gets enough samples and a long one is not run for
        // an hour.
        None => cmd.args(["--min-runs", "3", "--max-runs", "50"]),
    };
    for case in &cases {
        cmd.args(["--command-name", &case.name]);
    }
    for case in &cases {
        cmd.arg(case.argv.iter().map(|a| shell_word(a)).collect::<Vec<_>>().join(" "));
    }
    cmd.args(["--export-json", &results_file]);
    let status = proc::run(&mut cmd).map_err(|e| format!("hyperfine: {e}"))?;
    if status != 0 {
        return Err(format!("hyperfine exited with {status}"));
    }

    let results = hyperfine::parse(&read(&root.join(&results_file))?)?;
    let metrics = fs::read_to_string(&metrics_file).map(|t| parse_metrics(&t)).unwrap_or_default();
    let platform = platform();
    let baseline_dir = format!("benchmarks/baselines/{platform}");
    let baseline_file = format!("{baseline_dir}/{}{suffix}.json", b.name);
    let baseline_metrics_file = format!("{baseline_dir}/{}{suffix}.metrics.tsv", b.name);
    let baseline = match fs::read_to_string(root.join(&baseline_file)) {
        Ok(text) => Some(hyperfine::parse(&text)?),
        Err(_) => None,
    };
    let baseline_metrics = fs::read_to_string(root.join(&baseline_metrics_file))
        .map(|t| parse_metrics(&t))
        .unwrap_or_default();

    let toolchains = toolchains(root, &cases);
    let runs = options.runs.map_or("3 to 50, as hyperfine chose".to_string(), |n| n.to_string());
    let text = report::render(&report::Report {
        title: b.title,
        generated: report::utc_timestamp(
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        ),
        platform: &platform,
        runs,
        warmup: options.warmup,
        full: b.workload.as_ref().map(|_| full),
        toolchains: &toolchains,
        cases: &results,
        baseline: baseline.as_deref().map(|cases| (baseline_file.clone(), cases)),
        metrics: &metrics,
        baseline_metrics: &baseline_metrics,
        vm: &vm,
        native: &native,
    });
    println!("\n{text}");

    let report_file = options
        .report_file
        .clone()
        .unwrap_or_else(|| root.join(BENCH_DIR).join(format!("{}{suffix}.md", b.name)));
    fs::write(&report_file, &text).map_err(|e| format!("{}: {e}", report_file.display()))?;
    println!("Wrote report: {}", report_file.display());

    if options.save_baseline {
        fs::create_dir_all(root.join(&baseline_dir)).map_err(|e| format!("{baseline_dir}: {e}"))?;
        fs::copy(root.join(&results_file), root.join(&baseline_file))
            .map_err(|e| format!("{baseline_file}: {e}"))?;
        if metrics_file.is_file() {
            fs::copy(&metrics_file, root.join(&baseline_metrics_file))
                .map_err(|e| format!("{baseline_metrics_file}: {e}"))?;
        }
        println!("Saved baseline: {baseline_file}");
    }
    if options.readme {
        let readme_path = root.join("benchmarks/README.md");
        let readme = read(&readme_path)?;
        match report::replace_block(&readme, b.tag, &text) {
            Some(updated) => {
                fs::write(&readme_path, updated).map_err(|e| format!("README: {e}"))?;
                println!("Updated benchmarks/README.md");
            }
            None => println!("benchmarks/README.md has no {} report markers", b.tag),
        }
    }

    let vs_baseline = |name: &str| -> Option<f64> {
        let now = report::find(&results, name)?;
        let then = report::find(baseline.as_deref()?, name)?;
        Some(stats::mean(&now.times) / stats::mean(&then.times))
    };
    Ok(report::Outcome {
        name: b.name.to_string(),
        passed: true,
        vm_vs_baseline: vs_baseline(&vm),
        native_vs_baseline: vs_baseline(&native),
        vm_over_native: report::vm_over_native(&results, &vm, &native),
    })
}

/// Builds each port this machine has a toolchain for, and returns the
/// cases to time. A missing toolchain drops its case with a note.
fn prepare_cases(
    root: &Path,
    b: &Benchmark,
    full: bool,
    options: &Options,
) -> Result<Vec<Case>, String> {
    let name = b.name;
    let arg: Vec<String> = b.port_arg(full).into_iter().map(String::from).collect();
    let with_arg = |mut argv: Vec<String>| {
        argv.extend(arg.iter().cloned());
        argv
    };
    let program = b.flux_program(full);
    let mut cases = vec![Case {
        name: format!("{name}/flux"),
        argv: vec![exe("./target/release/flux"), program.clone()],
    }];
    let skip = |lang: &str, why: &str| println!("{name}: no {lang} case: {why}");

    if options.native {
        let out = format!("{BENCH_DIR}/{name}_flux");
        let mut cmd = Command::new(root.join(exe("target/release/flux")));
        cmd.current_dir(root).args([&program, "--native", "-O", "--emit-binary", "-o", &out]);
        if proc::run(&mut cmd).map_err(|e| format!("flux --native: {e}"))? != 0 {
            return Err(format!("could not build the native binary of {program}"));
        }
        cases.push(Case {
            name: format!("{name}/flux-native"),
            argv: vec![exe(&format!("./{out}"))],
        });
    }

    let rust = exe(&format!("./target/release/{name}_rust"));
    if at(root, &rust).is_file() {
        cases.push(Case { name: format!("{name}/rust"), argv: with_arg(vec![rust]) });
    } else {
        skip("Rust", &format!("cargo build --release --bin {name}_rust"));
    }

    match python() {
        Some(py) => cases.push(Case {
            name: format!("{name}/python"),
            argv: with_arg(vec![py, format!("benchmarks/python/{name}.py")]),
        }),
        None => skip("Python", "no python3 or python on PATH"),
    }

    let hs = exe(&format!("./target/release/{name}_hs"));
    let hs_source = b.haskell_source();
    if on_path("ghc").is_some() && stale(root, &hs, &hs_source) {
        let mut cmd = Command::new("ghc");
        cmd.current_dir(root).args([
            "-O2",
            "-outputdir",
            &format!("{BENCH_DIR}/haskell/{name}"),
            "-o",
            &hs,
            &hs_source,
        ]);
        proc::run(&mut cmd).map_err(|e| format!("ghc: {e}"))?;
    }
    if at(root, &hs).is_file() {
        cases.push(Case { name: format!("{name}/haskell"), argv: with_arg(vec![hs]) });
    } else {
        skip("Haskell", "ghc is not installed");
    }

    let ml = exe(&format!("./target/release/{name}_ocaml"));
    let ml_source = format!("benchmarks/ocaml/{name}.ml");
    if on_path("ocamlopt").is_some() && stale(root, &ml, &ml_source) {
        // ocamlopt writes its object files beside the source, so it
        // compiles a copy under target/.
        let dir = root.join(BENCH_DIR).join("ocaml").join(name);
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        fs::copy(root.join(&ml_source), dir.join(format!("{name}.ml")))
            .map_err(|e| format!("{ml_source}: {e}"))?;
        let mut cmd = Command::new("ocamlopt");
        cmd.current_dir(&dir);
        if name == "binarytrees" {
            cmd.args(["-I", "+unix", "unix.cmxa"]);
        }
        cmd.args(["-o", at(root, &ml).to_str().unwrap_or_default(), &format!("{name}.ml")]);
        proc::run(&mut cmd).map_err(|e| format!("ocamlopt: {e}"))?;
    }
    if at(root, &ml).is_file() {
        cases.push(Case { name: format!("{name}/ocaml"), argv: with_arg(vec![ml]) });
    } else {
        skip("OCaml", "ocamlopt is not installed");
    }

    let erlang_dir = format!("{BENCH_DIR}/erlang");
    if let Some(erlc) = on_path("erlc") {
        fs::create_dir_all(root.join(&erlang_dir)).map_err(|e| format!("{erlang_dir}: {e}"))?;
        let mut cmd = Command::new(erlc);
        cmd.current_dir(root).args(["-o", &erlang_dir, &format!("benchmarks/erlang/{name}.erl")]);
        if proc::run(&mut cmd).map_err(|e| format!("erlc: {e}"))? == 0 {
            let mut argv = vec![tool("erl"), "-noshell".into(), "-pa".into(), erlang_dir.clone()];
            argv.extend(["-run".into(), name.into(), "main".into()]);
            argv.extend(arg.iter().cloned());
            argv.extend(["-s".into(), "init".into(), "stop".into()]);
            cases.push(Case { name: format!("{name}/erlang"), argv });
        }
    } else {
        skip("Erlang", "erlc is not installed");
    }

    let elixir_dir = format!("{BENCH_DIR}/elixir");
    if let Some(elixirc) = on_path("elixirc") {
        fs::create_dir_all(root.join(&elixir_dir)).map_err(|e| format!("{elixir_dir}: {e}"))?;
        let mut cmd = Command::new(elixirc);
        cmd.current_dir(root).args(["-o", &elixir_dir, &format!("benchmarks/elixir/{name}.ex")]);
        if proc::run(&mut cmd).map_err(|e| format!("elixirc: {e}"))? == 0 {
            let mut argv = vec![tool("elixir"), "-pa".into(), elixir_dir.clone(), "-e".into()];
            argv.push(format!("Bench.{}.main(System.argv())", b.elixir_module()));
            if !arg.is_empty() {
                argv.push("--".into());
                argv.extend(arg.iter().cloned());
            }
            cases.push(Case { name: format!("{name}/elixir"), argv });
        }
    } else {
        skip("Elixir", "elixirc is not installed");
    }

    let go = exe(&format!("./target/release/{name}_go"));
    if on_path("go").is_some() {
        let mut cmd = Command::new("go");
        cmd.current_dir(root.join("benchmarks/go")).args([
            "build",
            "-o",
            at(root, &go).to_str().unwrap_or_default(),
            &format!("./{name}"),
        ]);
        if proc::run(&mut cmd).map_err(|e| format!("go build: {e}"))? == 0 {
            cases.push(Case { name: format!("{name}/go"), argv: with_arg(vec![go]) });
        }
    } else {
        skip("Go", "go is not installed");
    }

    if options.vms {
        if on_path("ghc").is_some() {
            let main = format!(":main{}", arg.first().map_or(String::new(), |a| format!(" {a}")));
            cases.push(Case {
                name: format!("{name}/haskell-ghci"),
                argv: vec!["ghc".into(), "-e".into(), main, hs_source],
            });
        }
        if on_path("yaegi").is_some() {
            cases.push(Case {
                name: format!("{name}/go-yaegi"),
                argv: with_arg(vec![
                    "yaegi".into(),
                    "run".into(),
                    format!("benchmarks/go/{name}/main.go"),
                ]),
            });
        }
    }
    Ok(cases)
}

/// Runs every case once and compares its stdout with the VM's. The VM's
/// run also writes `flux --metrics` to `metrics_file`. Returns false when
/// a case fails or prints something else.
fn check_outputs(
    root: &Path,
    cases: &[Case],
    vm: &str,
    metrics_file: &Path,
) -> Result<bool, String> {
    println!("Checking that every case prints the same output...");
    let mut reference: Option<(&str, String)> = None;
    let mut agreed = true;
    for case in cases {
        let mut argv = case.argv.clone();
        if case.name == vm {
            argv.push("--metrics".into());
            argv.push(metrics_file.to_string_lossy().into_owned());
        }
        let program = match argv[0].strip_prefix("./") {
            Some(relative) => root.join(relative),
            None => PathBuf::from(&argv[0]),
        };
        let output = Command::new(&program)
            .args(&argv[1..])
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("{}: {e}", case.name))?;
        if !output.status.success() {
            println!(
                "  {} failed ({}):\n{}",
                case.name,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim_end()
            );
            agreed = false;
            continue;
        }
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        match &reference {
            None => reference = Some((&case.name, stdout)),
            Some((first, expected)) => {
                if let Some(diff) = first_difference(expected, &stdout) {
                    println!("  {} and {first} differ at line {}:", case.name, diff.0);
                    println!("    {first}: {}", diff.1);
                    println!("    {}: {}", case.name, diff.2);
                    agreed = false;
                }
            }
        }
    }
    if agreed {
        println!("  all {} cases agree", cases.len());
    }
    Ok(agreed)
}

/// Line endings and trailing spaces differ between platforms and languages
/// without the answer differing, and Flux's `print` shows a String in
/// quotes where the ports print it bare.
fn normalize(text: &str) -> String {
    let unquote = |line: &str| -> String {
        let line = line.trim_end();
        match line.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
            Some(inner) => inner.to_string(),
            None => line.to_string(),
        }
    };
    text.lines().map(unquote).collect::<Vec<_>>().join("\n").trim_end().to_string()
}

/// A path from the repository root, given as the `./`-prefixed form the
/// timed commands use.
fn at(root: &Path, relative: &str) -> PathBuf {
    root.join(relative.trim_start_matches("./"))
}

/// The first line on which two outputs differ: its number and both lines.
fn first_difference(a: &str, b: &str) -> Option<(usize, String, String)> {
    let (mut la, mut lb) = (a.lines(), b.lines());
    for n in 1.. {
        match (la.next(), lb.next()) {
            (None, None) => return None,
            (x, y) if x == y => continue,
            (x, y) => {
                let show = |l: Option<&str>| l.map_or("(no line)".to_string(), str::to_string);
                return Some((n, show(x), show(y)));
            }
        }
    }
    None
}

fn parse_metrics(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('\t')?;
            Some((key.to_string(), value.trim().parse().ok()?))
        })
        .collect()
}

/// Records a profile with samply: of the VM, from a `profiling` build of
/// `flux` that keeps line tables, or of a native binary built with `-g`.
fn profile(root: &Path, b: &Benchmark, way: ProfileWay, options: &Options) -> Result<bool, String> {
    let samply =
        on_path("samply").ok_or("samply is not installed: cargo install --locked samply")?;
    let program = b.flux_program(options.full);
    let way_name = if way == ProfileWay::Vm { "vm" } else { "native" };
    let saved = format!("{BENCH_DIR}/{}-{way_name}.profile.json.gz", b.name);
    fs::create_dir_all(root.join(BENCH_DIR)).map_err(|e| format!("{BENCH_DIR}: {e}"))?;
    if options.build {
        build_rust(root, &[], &["--profile", "profiling", "--features", "llvm"])?;
    }
    let flux = root.join(exe("target/profiling/flux"));
    let target: Vec<String> = match way {
        ProfileWay::Vm => vec![flux.to_string_lossy().into_owned(), program],
        ProfileWay::Native => {
            let out = format!("{BENCH_DIR}/{}_flux_g", b.name);
            let mut cmd = Command::new(&flux);
            cmd.current_dir(root).args([
                &program,
                "-g",
                "--native",
                "-O",
                "--emit-binary",
                "-o",
                &out,
            ]);
            if proc::run(&mut cmd).map_err(|e| format!("flux --native: {e}"))? != 0 {
                return Err(format!("could not build the native binary of {program}"));
            }
            // A Flux program has its input built in; only the ports take one.
            vec![root.join(exe(&out)).to_string_lossy().into_owned()]
        }
    };
    let mut cmd = Command::new(samply);
    cmd.current_dir(root).args(["record", "-o", &saved, "--"]).args(&target);
    let status = proc::run(&mut cmd).map_err(|e| format!("samply: {e}"))?;
    println!("Saved profile: {saved} (open it again with: samply load {saved})");
    Ok(status == 0)
}

/// `flux` and the requested benchmarks' Rust ports, in one cargo run.
fn build_rust(root: &Path, benchmarks: &[&Benchmark], profile_args: &[&str]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root).arg("build").args(profile_args).args(["--bin", "flux"]);
    for b in benchmarks {
        cmd.args(["--bin", &format!("{}_rust", b.name)]);
    }
    match proc::run(&mut cmd).map_err(|e| format!("cargo build: {e}"))? {
        0 => Ok(()),
        code => Err(format!("cargo build exited with {code}")),
    }
}

/// The versions behind each case, for the report.
fn toolchains(root: &Path, cases: &[Case]) -> Vec<(String, String)> {
    let first_line = |program: &str, args: &[&str], dir: &Path| -> Option<String> {
        let out =
            Command::new(program).args(args).current_dir(dir).stdin(Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        text.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
    };
    let has = |lang: &str| cases.iter().any(|c| c.name.ends_with(&format!("/{lang}")));
    let mut found = Vec::new();
    let mut add = |name: &str, line: Option<String>| {
        if let Some(line) = line {
            found.push((name.to_string(), line));
        }
    };
    add("flux", first_line("git", &["rev-parse", "--short", "HEAD"], root));
    add("hyperfine", first_line("hyperfine", &["--version"], root));
    add("rustc", first_line("rustc", &["-V"], root));
    if has("python") {
        add("python", python().and_then(|py| first_line(&py, &["--version"], root)));
    }
    if has("haskell") {
        add("ghc", first_line("ghc", &["--numeric-version"], root));
    }
    if has("ocaml") {
        add("ocamlopt", first_line("ocamlopt", &["-version"], root));
    }
    if has("erlang") {
        let eval = "io:format(\"OTP ~s~n\", [erlang:system_info(otp_release)]), halt().";
        add("erlang", first_line(&tool("erl"), &["-noshell", "-eval", eval], root));
    }
    if has("elixir") {
        add("elixir", first_line(&tool("elixir"), &["--short-version"], root));
    }
    if has("go") {
        add("go", first_line("go", &["version"], root));
    }
    found
}

/// `macos-aarch64`, `linux-x86_64`, `windows-x86_64`: baselines compare
/// only with runs on the same kind of machine.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A program path with the platform's executable suffix.
fn exe(path: &str) -> String {
    format!("{path}{}", std::env::consts::EXE_SUFFIX)
}

/// Searches PATH without running anything: `erl --version` would start a
/// shell.
fn on_path(tool: &str) -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) { &[".exe", ".bat", ".cmd", ""] } else { &[""] };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        suffixes.iter().map(|s| dir.join(format!("{tool}{s}"))).find(|p| p.is_file())
    })
}

/// A tool as a command names it. On Windows `erl` and `elixir` can be batch
/// files, which run only by their full path.
fn tool(name: &str) -> String {
    if cfg!(windows) {
        on_path(name).map_or(name.to_string(), |p| p.to_string_lossy().replace('\\', "/"))
    } else {
        name.to_string()
    }
}

/// The Python interpreter; Windows's `python3` can be a Store alias that
/// only prints an install hint, so each candidate must run.
fn python() -> Option<String> {
    ["python3", "python"].into_iter().map(String::from).find(|py| {
        Command::new(py)
            .args(["-c", ""])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Whether `binary` is missing or older than `source`.
fn stale(root: &Path, binary: &str, source: &str) -> bool {
    let modified = |p: &str| fs::metadata(at(root, p)).and_then(|m| m.modified()).ok();
    match (modified(binary), modified(source)) {
        (Some(built), Some(edited)) => built < edited,
        _ => true,
    }
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// One argument as hyperfine's `--shell=none` splits a command line, which
/// follows POSIX shell quoting on every platform.
fn shell_word(arg: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./:=+,@%".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_for_hyperfine() {
        assert_eq!(shell_word("./target/release/flux"), "./target/release/flux");
        assert_eq!(shell_word("Bench.Tak.main(System.argv())"), "'Bench.Tak.main(System.argv())'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn outputs_compare_without_line_ending_noise() {
        assert_eq!(normalize("1 \r\n2\r\n\r\n"), "1\n2");
        assert_eq!(normalize("\"10426 10426\"\n\"9\t check: 1\"\n"), "10426 10426\n9\t check: 1");
        assert_eq!(first_difference("a\nb", "a\nb"), None);
        assert_eq!(first_difference("a\nb", "a\nc"), Some((2, "b".into(), "c".into())));
        assert_eq!(first_difference("a", "a\nb"), Some((2, "(no line)".into(), "b".into())));
    }

    #[test]
    fn metrics_are_read_as_key_and_count() {
        let parsed = parse_metrics("vm_allocs\t12\nbad line\naether_dups\t3\n");
        assert_eq!(parsed, vec![("vm_allocs".into(), 12), ("aether_dups".into(), 3)]);
    }
}
