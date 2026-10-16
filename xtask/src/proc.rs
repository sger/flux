//! Running a command and teeing its output.

use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// Runs `cmd` with stdout and stderr merged, line by line, as `2>&1` would.
/// Each line is appended to `log` (when given) and passed to `on_line`.
/// Returns the exit code, or -1 when the process had none.
pub fn run_tee(
    cmd: &mut Command,
    log: Option<&Path>,
    mut on_line: impl FnMut(&str),
) -> io::Result<i32> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
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
    Ok(cmd.status()?.code().unwrap_or(-1))
}
