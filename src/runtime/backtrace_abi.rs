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
            if let Some(place) = flux_place(symbol) {
                frames.push(format!("at {place}"));
            }
        });
        true
    });
    frames
}

/// `name (file:line:col)` for a symbol in a `.flx` file, or `None` for a
/// runtime or Rust one.
fn flux_place(symbol: &backtrace::Symbol) -> Option<String> {
    let (Some(file), Some(line)) = (symbol.filename(), symbol.lineno()) else {
        return None;
    };
    if file.extension().is_none_or(|ext| ext != "flx") {
        return None;
    }
    let name = symbol
        .name()
        .and_then(|name| name.as_str().map(str::to_string))
        .unwrap_or_else(|| "<unknown>".to_string());
    let col = symbol.colno().unwrap_or(1);
    Some(format!("{name} ({}:{line}:{col})", file.display()))
}

/// Records the return addresses on the stack, innermost first, into
/// `out[..len]`, padding with zeros: where the debug heap allocated or freed
/// a block (0188 S10c). Nothing is resolved here, so it stays cheap enough
/// to run on every allocation.
///
/// # Safety
/// `out` points to `len` writable words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_capture_site(out: *mut usize, len: usize) {
    // SAFETY: the caller hands over `len` writable words.
    let out = unsafe { std::slice::from_raw_parts_mut(out, len) };
    out.fill(0);
    let mut next = 0;
    backtrace::trace(|frame| {
        if next == out.len() {
            return false;
        }
        out[next] = frame.ip() as usize;
        next += 1;
        true
    });
}

/// Writes the innermost Flux frame among the addresses `flux_capture_site`
/// recorded, as `name (file:line:col)`, into `buf` (NUL-terminated, cut to
/// fit), or `the runtime` when none is a Flux frame. Returns the length
/// written.
///
/// # Safety
/// `site` points to `len` words; `buf` to `cap` writable bytes, `cap > 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_describe_site(
    site: *const usize,
    len: usize,
    buf: *mut u8,
    cap: usize,
) -> usize {
    // SAFETY: the caller hands over `len` readable words.
    let site = unsafe { std::slice::from_raw_parts(site, len) };
    let text = describe_site(site);
    let bytes = &text.as_bytes()[..text.len().min(cap.saturating_sub(1))];
    // SAFETY: `buf` has `cap` bytes, and `bytes.len() < cap`.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }
    bytes.len()
}

fn describe_site(site: &[usize]) -> String {
    for &ip in site.iter().take_while(|&&ip| ip != 0) {
        let mut place = None;
        // A return address points after its call: step back into it.
        backtrace::resolve((ip - 1) as *mut std::ffi::c_void, |symbol| {
            if place.is_none() {
                place = flux_place(symbol);
            }
        });
        if let Some(place) = place {
            return relative_to_cwd(place);
        }
    }
    "the runtime".to_string()
}

/// A place under the working directory, named from it, as `flux` names
/// the files of a trace.
fn relative_to_cwd(place: String) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return place;
    };
    let mut prefix = cwd.display().to_string();
    prefix.push(std::path::MAIN_SEPARATOR);
    place.replace(&format!("({prefix}"), "(")
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
    fn a_site_outside_flux_code_is_the_runtime() {
        let mut site = [0usize; 8];
        // SAFETY: `site` has 8 words.
        unsafe { flux_capture_site(site.as_mut_ptr(), site.len()) };
        assert_ne!(site[0], 0);
        let mut buf = [0u8; 8];
        // SAFETY: `site` has 8 words and `buf` 8 bytes.
        let len = unsafe { flux_describe_site(site.as_ptr(), site.len(), buf.as_mut_ptr(), 8) };
        // "the runtime", cut to 7 bytes and a NUL.
        assert_eq!(&buf[..len], b"the run");
        assert_eq!(buf[len], 0);
    }

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
