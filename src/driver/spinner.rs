//! A terminal spinner for work whose duration cannot be predicted.
//!
//! Used while a git dependency downloads: the fetch is a subprocess talking to
//! a network, so there is no percentage to report and no way to know in advance
//! whether it will take a tenth of a second or a minute. What the user needs is
//! not a measurement but evidence of life — that the toolchain is waiting on
//! something rather than wedged.
//!
//! The spinner animates on a background thread because the thread that started
//! the work is blocked reading the child's output. It repaints one line in
//! place with a carriage return, and erases it on stop, so the finished output
//! contains no trace of the animation:
//!
//! ```text
//!     Updating https://github.com/sger/flux-greeter
//!   ⠹ fetching…                          <- while downloading, then erased
//!      Fetched https://github.com/sger/flux-greeter (50543e1)
//! ```
//!
//! It draws nothing unless stderr is a terminal. Redirected to a file or a CI
//! log, in-place repainting produces thousands of lines of control characters
//! rather than an animation, so a non-interactive stream gets silence and the
//! surrounding `Updating` / `Fetched` lines carry the whole story.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Braille frames, which animate in place without changing width.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How often the frame advances. Fast enough to read as motion, slow enough
/// that a long fetch is not thousands of writes to a terminal.
const TICK: Duration = Duration::from_millis(80);

/// A running spinner. Stops when `stop` is called, or when dropped.
pub(crate) struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner labelled `message`, or a no-op when stderr is not a
    /// terminal.
    pub(crate) fn start(message: &str) -> Self {
        if !should_animate() {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                handle: None,
            };
        }

        let stop = Arc::new(AtomicBool::new(false));
        let label = message.to_string();
        let flag = Arc::clone(&stop);
        let handle = thread::spawn(move || animate(&label, &flag));
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stop the animation and erase the line.
    ///
    /// Joins the animating thread before returning, so the caller's next write
    /// cannot interleave with a final repaint and leave a half-drawn frame on
    /// screen.
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether an animation would be seen by a human.
///
/// `NO_COLOR` is honoured alongside the terminal check: a user who has asked
/// for plain output is asking not to be repainted at, and the toolchain already
/// reads that variable for diagnostics.
fn should_animate() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Sleep for `total`, waking early once stopped.
///
/// Sleeping the whole tick in one call would let a fetch that finishes just
/// after a repaint hold the line for the rest of the frame, delaying the
/// `Fetched` line behind it by up to a tick for no reason.
fn sleep_until_stopped(total: Duration, stop: &AtomicBool) {
    const SLICE: Duration = Duration::from_millis(10);
    let mut slept = Duration::ZERO;
    while slept < total && !stop.load(Ordering::Relaxed) {
        thread::sleep(SLICE);
        slept += SLICE;
    }
}

/// Repaint the spinner until told to stop, then erase it.
fn animate(message: &str, stop: &AtomicBool) {
    let mut frame = 0usize;
    while !stop.load(Ordering::Relaxed) {
        let mut err = std::io::stderr().lock();
        // `\r` returns to the start of the line without a newline, so the next
        // write lands on top of this one. `\x1b[K` clears to end of line first,
        // so a shorter frame cannot leave the tail of a longer one behind.
        let _ = write!(err, "\r\x1b[K  {} {message}", FRAMES[frame % FRAMES.len()]);
        let _ = err.flush();
        drop(err);

        frame = frame.wrapping_add(1);
        sleep_until_stopped(TICK, stop);
    }

    let mut err = std::io::stderr().lock();
    let _ = write!(err, "\r\x1b[K");
    let _ = err.flush();
}
