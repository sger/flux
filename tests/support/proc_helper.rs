//! A deterministic child process for the `Flow.Process` fixture.
//!
//! The fixture used to spawn `/bin/echo`, `sh`, `true` and `false`. Windows
//! has none of them: `echo` there is a shell builtin, not a program, and
//! reaching it through `cmd` would re-parse the very arguments the fixture
//! exists to prove are *never* re-parsed — an empty argument disappears and a
//! quoted one loses its spaces. Nothing on a stock Windows install passes its
//! argument vector through unaltered.
//!
//! So the child is ours. Every expectation in the fixture then holds
//! unchanged on both platforms, and the fixture stops assuming a POSIX shell
//! is installed at all.
//!
//! Modes, each reading its arguments positionally:
//!
//! ```text
//! echo [arg...]           the arguments, space-separated, and a newline
//! exit <code>             exits with <code>, printing nothing
//! streams <out> <err>     one line to stdout, one to stderr
//! bulk <n> <text>         <text> and a newline, <n> times, on stdout
//! both <n> <out> <err>    <n> lines to each stream, interleaved
//! nul                     the three bytes `a`, NUL, `b`
//! ```

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);

    match mode {
        // Joined, never inspected: a metacharacter is returned as itself.
        "echo" => write_out(format!("{}\n", rest.join(" ")).as_bytes()),
        "exit" => {
            let code = rest.first().and_then(|c| c.parse::<u8>().ok()).unwrap_or(0);
            return ExitCode::from(code);
        }
        "streams" => {
            write_out(format!("{}\n", arg(rest, 0)).as_bytes());
            write_err(format!("{}\n", arg(rest, 1)).as_bytes());
        }
        "bulk" => {
            let line = format!("{}\n", arg(rest, 1));
            let mut bytes = Vec::new();
            for _ in 0..count(rest, 0) {
                bytes.extend_from_slice(line.as_bytes());
            }
            write_out(&bytes);
        }
        // Both streams busy at once: the case a sequential drain deadlocks on,
        // so the writes interleave rather than finishing one stream first.
        "both" => {
            let out_line = format!("{}\n", arg(rest, 1));
            let err_line = format!("{}\n", arg(rest, 2));
            for _ in 0..count(rest, 0) {
                write_out(out_line.as_bytes());
                write_err(err_line.as_bytes());
            }
        }
        // A NUL is data, not a terminator, for a length-carrying string.
        "nul" => write_out(b"a\0b"),
        _ => {
            write_err(format!("flux_proc_helper: unknown mode `{mode}`\n").as_bytes());
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

fn arg(rest: &[String], at: usize) -> &str {
    rest.get(at).map(String::as_str).unwrap_or("")
}

fn count(rest: &[String], at: usize) -> u32 {
    rest.get(at).and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Written as bytes rather than through `print!`, so the payload reaches the
/// pipe exactly as given — no line-ending translation, and a NUL survives.
fn write_out(bytes: &[u8]) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(bytes);
    let _ = handle.flush();
}

fn write_err(bytes: &[u8]) {
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = handle.write_all(bytes);
    let _ = handle.flush();
}
