//! Running a command and teeing its output.

use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::text::Paint;

/// `cmd` as a shell line: its environment overrides, then the program and
/// arguments, with paths under its working directory shown relative to it.
fn describe(cmd: &Command) -> String {
    let dir = cmd.get_current_dir();
    let show = |s: &std::ffi::OsStr| {
        let path = Path::new(s);
        let shown = dir
            .and_then(|d| path.strip_prefix(d).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        if shown.is_empty() || shown.contains(' ') { format!("'{shown}'") } else { shown }
    };
    let mut parts: Vec<String> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some(format!("{}={}", k.to_string_lossy(), v?.to_string_lossy())))
        .collect();
    // `cargo` arrives as the absolute path in `$CARGO`; its name says enough.
    let program = Path::new(cmd.get_program());
    parts.push(match program.file_name() {
        Some(name) if dir.is_none_or(|d| !program.starts_with(d)) => {
            name.to_string_lossy().into_owned()
        }
        _ => show(cmd.get_program()),
    });
    parts.extend(cmd.get_args().map(show));
    parts.join(" ")
}

/// Prints `$ <cmd>`, so the console says what is about to run.
fn echo(cmd: &Command) {
    println!("{}", Paint::detect().dim(&format!("$ {}", describe(cmd))));
}

/// Runs `cmd` with stdout and stderr merged, line by line, as `2>&1` would.
/// Each line is appended to `log` (when given) and passed to `on_line`.
/// Returns the exit code, or -1 when the process had none.
pub fn run_tee(
    cmd: &mut Command,
    log: Option<&Path>,
    mut on_line: impl FnMut(&str),
) -> io::Result<i32> {
    echo(cmd);
    if let Some(log) = log {
        println!("{}", Paint::detect().dim(&format!("  log: {}", log.display())));
    }
    let mut child =
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (tx, rx) = mpsc::channel::<String>();
    let readers = [
        forward_lines(child.stdout.take().expect("piped stdout"), tx.clone()),
        forward_lines(child.stderr.take().expect("piped stderr"), tx),
    ];
    let mut log = match log {
        Some(path) => Some(OpenOptions::new().create(true).append(true).open(path)?),
        None => None,
    };
    for line in rx {
        if let Some(log) = log.as_mut() {
            writeln!(log, "{line}")?;
        }
        on_line(&line);
    }
    for reader in readers {
        let _ = reader.join();
    }
    Ok(child.wait()?.code().unwrap_or(-1))
}

fn forward_lines(
    stream: impl Read + Send + 'static,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while let Ok(n) = reader.read_until(b'\n', &mut buf) {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            buf.clear();
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// Runs `cmd` with inherited stdio and returns its exit code.
pub fn run(cmd: &mut Command) -> io::Result<i32> {
    echo(cmd);
    Ok(cmd.status()?.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_a_command_as_a_shell_line() {
        let mut cmd = Command::new("/opt/rust/bin/cargo");
        cmd.current_dir("/repo").env("INSTA_FORCE_PASS", "1").args(["test", "--all"]);
        assert_eq!(describe(&cmd), "INSTA_FORCE_PASS=1 cargo test --all");

        let mut cmd = Command::new("/repo/target/debug/flux");
        cmd.current_dir("/repo").args(["parity-check", "/repo/tests/parity", "--ways", "vm,llvm"]);
        assert_eq!(describe(&cmd), "target/debug/flux parity-check tests/parity --ways vm,llvm");
    }
}
