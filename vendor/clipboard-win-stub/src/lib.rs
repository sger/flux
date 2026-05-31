//! Local no-op replacement for the `clipboard-win` crate, wired in via
//! `[patch.crates-io]` in the workspace root `Cargo.toml`.
//!
//! `rustyline` (the `flux repl` line editor) depends on `clipboard-win` on Windows
//! solely to paste from the *system* clipboard. That drags Win32 clipboard / bitmap
//! imports (`SetClipboardData`, `GetClipboardData`, `GetDIBits`, …) into every flux
//! binary, which makes small unsigned executables look like clipboard-hijacking
//! malware to Windows Defender's ML heuristic (`Trojan:Win32/Wacatac.B!ml`). GHC's
//! GHCi avoids this by using `haskeline`, whose kill-ring is internal and never
//! touches the OS clipboard.
//!
//! This stub provides the *only* two items `rustyline` references —
//! [`ErrorCode`] and [`get_clipboard_string`] — with no Win32 clipboard calls, so
//! `rustyline` keeps its full editing, history, and internal kill-ring while
//! paste-from-OS-clipboard becomes an inert no-op. `error-code` (re-exported for
//! `ErrorCode`) references only `kernel32`, never the clipboard APIs.

pub use error_code::ErrorCode;

/// Mirror of `clipboard_win::SysResult`.
pub type SysResult<T> = Result<T, ErrorCode>;

/// No-op replacement for `clipboard_win::get_clipboard_string`: returns an empty
/// string instead of reading the OS clipboard, so the binary never links the Win32
/// clipboard APIs. `rustyline`'s own internal kill-ring (cut/paste within the line)
/// is unaffected — only paste *from the system clipboard* is disabled.
pub fn get_clipboard_string() -> SysResult<String> {
    Ok(String::new())
}
