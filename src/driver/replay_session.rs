//! `--record` and `--replay` for one run (proposal 0188 S1).
//!
//! Before the program runs, the driver identifies the program by hashing its
//! modules, then either writes a new tape's header or loads a tape and checks
//! that it belongs to this program. The VM records or replays in this
//! process; a native program does it in its own process, told where the tape
//! is through `FLUX_TAPE_RECORD` / `FLUX_TAPE_REPLAY`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::diagnostics::render_runtime_diagnostic;
use crate::driver::flags::{DriverFlags, TapeRequest};
use crate::runtime::replay::{self, LoadedTape, TapeError, TapeHeader};
use crate::syntax::module_graph::{ModuleGraph, ModuleKind};

/// The environment variable a native program records into.
pub(crate) const RECORD_ENV: &str = "FLUX_TAPE_RECORD";
/// The environment variable a native program replays from.
pub(crate) const REPLAY_ENV: &str = "FLUX_TAPE_REPLAY";

enum Session {
    Record { path: PathBuf, header: TapeHeader },
    Replay { tape: LoadedTape },
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Sets up the tape the flags ask for, before the program runs. A tape that
/// cannot be written or replayed for this program stops the run here.
pub(crate) fn prepare(graph: &ModuleGraph, entry_path: &Path, flags: &DriverFlags, native: bool) {
    let Some(request) = &flags.runtime.tape else {
        return;
    };
    if flags.runtime.check_only {
        eprintln!("Error: --record and --replay apply only to running a program.");
        std::process::exit(1);
    }
    let program_hash = program_hash(graph, entry_path);
    let session = match request {
        TapeRequest::Record(path) => {
            let argv = crate::vm::program_args_snapshot().to_vec();
            let header = TapeHeader::new(program_hash, argv, if native { "native" } else { "vm" });
            if native {
                // The native program appends its events after this header.
                replay::create_tape(path, &header).unwrap_or_else(|err| fail(&err, path));
            }
            Session::Record {
                path: path.clone(),
                header,
            }
        }
        TapeRequest::Replay(path) => {
            let tape = LoadedTape::load(path).unwrap_or_else(|err| fail(&err, path));
            if tape.header.program_hash != program_hash {
                fail(
                    &TapeError::BadTape {
                        path: path.display().to_string(),
                        reason: "it was recorded from a different program, or from this one \
                                 before it was changed"
                            .to_string(),
                    },
                    path,
                );
            }
            let backend = if native { "native" } else { "vm" };
            if tape.has_async() && tape.header.backend != backend {
                fail(
                    &TapeError::BadTape {
                        path: path.display().to_string(),
                        reason: format!(
                            "it records async code run on {recorded}, which replays only on \
                             {recorded}; replay it with {flag}",
                            recorded = backend_name(&tape.header.backend),
                            flag = if tape.header.backend == "native" {
                                "--native"
                            } else {
                                "the VM (without --native)"
                            },
                        ),
                    },
                    path,
                );
            }
            if tape.header.flux_version != env!("CARGO_PKG_VERSION") {
                eprintln!(
                    "warning: tape `{}` was recorded by flux {}; this is flux {}.",
                    path.display(),
                    tape.header.flux_version,
                    env!("CARGO_PKG_VERSION")
                );
            }
            if tape.is_truncated() {
                eprintln!(
                    "warning: tape `{}` is incomplete: the recorded run did not finish.",
                    path.display()
                );
            }
            Session::Replay { tape }
        }
    };
    let _ = SESSION.set(session);
}

/// Starts recording or replaying in this process, for a run on the VM.
pub(crate) fn activate_in_process(program_path: &str) {
    if SESSION.get().is_some() {
        // A tape error inside the async scheduler cannot be returned to the
        // program; report it like any other tape error and stop.
        let program_path = program_path.to_string();
        replay::async_tape::set_fatal_handler(Box::new(move |err| {
            let _ = replay::finish(true, Some(err.message()));
            let _ = std::io::Write::flush(&mut std::io::stdout());
            report(err, &program_path);
        }));
    }
    match SESSION.get() {
        None => {}
        Some(Session::Record { path, header }) => {
            replay::start_record(path, header).unwrap_or_else(|err| fail(&err, path));
        }
        Some(Session::Replay { tape }) => replay::start_replay(tape.clone()),
    }
}

/// The environment variable that hands the tape to a native program.
pub(crate) fn native_child_env() -> Option<(&'static str, PathBuf)> {
    match SESSION.get()? {
        Session::Record { path, .. } => Some((RECORD_ENV, absolute(path))),
        Session::Replay { tape } => Some((REPLAY_ENV, absolute(Path::new(&tape.path)))),
    }
}

/// Warns when a native program's recording did not reach its end, for
/// example because the process was killed: replaying it stops early.
pub(crate) fn check_native_recording() {
    let Some(Session::Record { path, .. }) = SESSION.get() else {
        return;
    };
    let finished = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .last()
                .map(|line| line.contains("\"kind\":\"exit\""))
        })
        .unwrap_or(false);
    if !finished {
        eprintln!(
            "warning: tape `{}` is incomplete: the native program did not finish recording.",
            path.display()
        );
    }
}

/// Ends the tape after a VM run: records how the run ended, or checks it. A
/// replay that does not end the way the recording did is reported here.
pub(crate) fn finish_vm_run(error: Option<&str>, program_path: &str) {
    if !replay::is_active() {
        return;
    }
    let message = error.map(|err| first_line(&strip_ansi(err)));
    if let Err(err) = replay::finish(error.is_some(), message) {
        if let Some(error) = error {
            eprintln!("{error}");
        }
        report(&err, program_path);
        std::process::exit(1);
    }
}

/// Prints a tape error that has no source position.
pub(crate) fn report(err: &TapeError, program_path: &str) {
    let diagnostic = err.to_diagnostic(program_path, None);
    eprintln!(
        "{}",
        render_runtime_diagnostic(&diagnostic, program_path, None, &[])
    );
}

fn backend_name(backend: &str) -> &str {
    match backend {
        "native" => "the native backend",
        "vm" => "the VM",
        other => other,
    }
}

fn fail(err: &TapeError, path: &Path) -> ! {
    report(err, &path.display().to_string());
    std::process::exit(1);
}

/// Identifies the program a tape belongs to: SHA-256 over each of the
/// program's own modules (Flow's standard library excluded; the flux version
/// covers it), as its path relative to the entry file and its source text.
fn program_hash(graph: &ModuleGraph, entry_path: &Path) -> String {
    let base = entry_path.parent().unwrap_or(Path::new(""));
    let mut modules: Vec<(String, Vec<u8>)> = graph
        .topo_order()
        .into_iter()
        .filter(|node| node.kind == ModuleKind::User)
        .map(|node| {
            let relative = node.path.strip_prefix(base).unwrap_or(&node.path);
            let name = relative.to_string_lossy().replace('\\', "/");
            (name, std::fs::read(&node.path).unwrap_or_default())
        })
        .collect();
    modules.sort();
    let mut hasher = Sha256::new();
    for (name, source) in modules {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(&source);
        hasher.update([0]);
    }
    format!("sha256:{}", crate::shared::hex::encode(&hasher.finalize()))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The line of a rendered runtime error that names it (`error[E1008]: …`),
/// or its first line when none does.
fn first_line(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.clone().next().unwrap_or("");
    lines
        .find(|line| line.starts_with("error"))
        .unwrap_or(first)
        .to_string()
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
