//! A native program's own panic trace (proposal 0188 S10a).
//!
//! Every native build carries a DWARF line table, so when the program dies
//! of an error it walks its stack and names the Flux frames it is in, from
//! that table: `at name (file:line:col)`, as the VM prints them. Runtime and
//! Rust frames have no `.flx` file and are left out. The driver renders the
//! lines into its diagnostic; a program run on its own prints them as they
//! are.

use std::io::Write;

/// Frames shown from each end of a long trace, as the VM shows them
/// (`render_stack_trace`): where the error was raised, and how a deep
/// recursion was entered.
const HEAD: usize = 20;
const TAIL: usize = 10;

/// Prints the Flux frames the program is in to stderr, innermost first.
#[unsafe(no_mangle)]
pub extern "C" fn flux_print_backtrace() {
    let mut frames = flux_frames();
    if frames.is_empty() {
        return;
    }
    frames.push("at <main>".to_string());
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "\nStack trace:");
    for line in shown(&frames) {
        let _ = writeln!(err, "  {line}");
    }
}

/// Each Flux frame on the stack, inlined ones included, innermost first.
fn flux_frames() -> Vec<String> {
    let mut frames = Vec::new();
    backtrace::trace(|frame| {
        backtrace::resolve_frame(frame, |symbol| {
            let (Some(file), Some(line)) = (symbol.filename(), symbol.lineno()) else {
                return;
            };
            if file.extension().is_none_or(|ext| ext != "flx") {
                return;
            }
            let name = symbol
                .name()
                .and_then(|name| name.as_str().map(str::to_string))
                .unwrap_or_else(|| "<unknown>".to_string());
            let col = symbol.colno().unwrap_or(1);
            frames.push(format!("at {name} ({}:{line}:{col})", file.display()));
        });
        true
    });
    frames
}

/// The frames of a long trace kept, as the VM keeps them: the first and
/// last ones, and how many are left out between.
fn shown(frames: &[String]) -> Vec<String> {
    if frames.len() <= HEAD + TAIL {
        return frames.to_vec();
    }
    let mut shown = frames[..HEAD].to_vec();
    shown.push(format!(
        "... {} more frames ...",
        frames.len() - HEAD - TAIL
    ));
    shown.extend_from_slice(&frames[frames.len() - TAIL..]);
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_trace_keeps_both_ends() {
        let frames: Vec<String> = (0..50).map(|i| format!("at f{i}")).collect();
        let kept = shown(&frames);
        assert_eq!(kept.len(), HEAD + TAIL + 1);
        assert_eq!(kept[0], "at f0");
        assert_eq!(kept[HEAD], "... 20 more frames ...");
        assert_eq!(kept.last().unwrap(), "at f49");
        assert_eq!(shown(&frames[..5]), frames[..5].to_vec());
    }
}
