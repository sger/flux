//! The effect tape: record a run's effects and replay them (proposal 0188
//! S1, implementing 0038).
//!
//! `flux run --record <tape>` writes every effect the program performs, with
//! its result, to a tape. `flux run --replay <tape>` runs the program again and
//! answers every effect from the tape: inputs (files, stdin, environment,
//! processes, the clock) return what was recorded, outputs (file writes,
//! renames, removals) are checked and not re-applied, and console text is
//! checked and printed again. The first place the program and the tape
//! disagree stops the replay with a diagnostic.
//!
//! Both backends use this module: the VM calls it from its primop dispatch,
//! and a native program through the `flux_tape_*` exports in `native_abi`.
//! When no tape is active, the only cost is the [`is_active`] check.

pub mod error;
pub mod format;
#[cfg(feature = "llvm")]
pub mod native_abi;

#[cfg(test)]
mod tests;

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::core::CorePrimOp;

pub use error::TapeError;
pub use format::{
    EffectClass, EffectRequest, FsMeta, IoKind, LoadedTape, Outcome, ProcOut, TapeBytes, TapeEvent,
    TapeHeader, TapeIoError,
};

/// Whether this process records, replays, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReplayMode {
    Off = 0,
    Record = 1,
    Replay = 2,
}

static MODE: AtomicU8 = AtomicU8::new(ReplayMode::Off as u8);
static STATE: Mutex<Option<TapeState>> = Mutex::new(None);

enum TapeState {
    Record {
        writer: BufWriter<File>,
        path: String,
        next_seq: u64,
        finished: bool,
    },
    Replay {
        tape: LoadedTape,
        cursor: usize,
        finished: bool,
    },
}

/// The current mode: one relaxed load.
#[inline(always)]
pub fn mode() -> ReplayMode {
    match MODE.load(Ordering::Relaxed) {
        1 => ReplayMode::Record,
        2 => ReplayMode::Replay,
        _ => ReplayMode::Off,
    }
}

/// True while a tape is recording or replaying.
#[inline(always)]
pub fn is_active() -> bool {
    MODE.load(Ordering::Relaxed) != ReplayMode::Off as u8
}

fn lock() -> std::sync::MutexGuard<'static, Option<TapeState>> {
    // A panic while the lock is held cannot leave the tape half-written in a
    // way that matters more than the panic itself, so recover the guard.
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Starts recording into a new tape at `path`, writing `header` first.
pub fn start_record(path: &Path, header: &TapeHeader) -> Result<(), TapeError> {
    create_tape(path, header)?;
    start_record_append(path)
}

/// Creates a tape holding only its header. The driver does this for a native
/// program, which then records into it with [`start_record_append`].
pub fn create_tape(path: &Path, header: &TapeHeader) -> Result<(), TapeError> {
    let line = serde_json::to_string(header).expect("tape header serializes");
    std::fs::write(path, format!("{line}\n")).map_err(|err| cannot_write(path, &err))
}

/// Continues recording into a tape whose header has already been written: a
/// native program records into the tape the driver created for it.
pub fn start_record_append(path: &Path) -> Result<(), TapeError> {
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|err| cannot_write(path, &err))?;
    install_record(BufWriter::new(file), path)
}

fn install_record(writer: BufWriter<File>, path: &Path) -> Result<(), TapeError> {
    *lock() = Some(TapeState::Record {
        writer,
        path: path.display().to_string(),
        next_seq: 0,
        finished: false,
    });
    MODE.store(ReplayMode::Record as u8, Ordering::Relaxed);
    Ok(())
}

/// Starts replaying `tape`.
pub fn start_replay(tape: LoadedTape) {
    *lock() = Some(TapeState::Replay {
        tape,
        cursor: 0,
        finished: false,
    });
    MODE.store(ReplayMode::Replay as u8, Ordering::Relaxed);
}

/// Turns the tape off and forgets it, without finishing it. For tests.
pub fn reset() {
    MODE.store(ReplayMode::Off as u8, Ordering::Relaxed);
    *lock() = None;
}

fn cannot_write(path: &Path, err: &std::io::Error) -> TapeError {
    TapeError::BadTape {
        path: path.display().to_string(),
        reason: format!("it cannot be written ({err})"),
    }
}

/// Performs an input or output effect through the tape.
///
/// Off: runs `perform`. Record: runs `perform` and writes the request and its
/// outcome. Replay: checks the request against the next recorded event and
/// returns the recorded outcome; `perform` never runs, so replay does not
/// read or change anything outside the program.
///
/// The tape lock is not held while `perform` runs: `time(f)` runs program
/// code, and that code performs effects of its own.
pub fn effect(
    request: EffectRequest,
    perform: impl FnOnce() -> Outcome,
) -> Result<Outcome, TapeError> {
    match mode() {
        ReplayMode::Off => Ok(perform()),
        ReplayMode::Record => {
            let outcome = perform();
            append(request, Some(outcome.clone()))?;
            Ok(outcome)
        }
        ReplayMode::Replay => {
            let event = take_matching(&request)?;
            Ok(event
                .result
                .expect("a loaded tape gives every input and output a result"))
        }
    }
}

/// Records or checks console text. The caller prints it in every mode, so a
/// replay shows the same output as the recorded run.
pub fn console(request: EffectRequest) -> Result<(), TapeError> {
    debug_assert_eq!(request.class(), EffectClass::Console);
    match mode() {
        ReplayMode::Off => Ok(()),
        ReplayMode::Record => append(request, None),
        ReplayMode::Replay => take_matching(&request).map(|_| ()),
    }
}

/// Ends the tape: records how the run ended, or checks it against the tape
/// and that nothing recorded was left unreplayed. Calling it again does
/// nothing.
pub fn finish(failed: bool, message: Option<String>) -> Result<(), TapeError> {
    let mut guard = lock();
    let Some(state) = guard.as_mut() else {
        return Ok(());
    };
    match state {
        TapeState::Record {
            writer,
            path,
            next_seq,
            finished,
        } => {
            if *finished {
                return Ok(());
            }
            *finished = true;
            let event = TapeEvent {
                seq: *next_seq,
                fiber: 0,
                request: EffectRequest::Exit { failed, message },
                result: None,
            };
            *next_seq += 1;
            write_event(writer, path, &event)?;
            writer
                .flush()
                .map_err(|err| cannot_write(Path::new(path), &err))
        }
        TapeState::Replay {
            tape,
            cursor,
            finished,
        } => {
            if *finished {
                return Ok(());
            }
            *finished = true;
            let ending = EffectRequest::Exit {
                failed,
                message: None,
            };
            match tape.events.get(*cursor) {
                None => {
                    return Err(TapeError::Exhausted {
                        count: tape.events.len() as u64,
                        actual: ending.to_string(),
                    });
                }
                Some(TapeEvent {
                    request:
                        EffectRequest::Exit {
                            failed: recorded, ..
                        },
                    seq,
                    ..
                }) => {
                    if *recorded != failed {
                        return Err(TapeError::PayloadMismatch {
                            index: *seq,
                            what: "how the program ended".to_string(),
                            tape: describe_ending(*recorded),
                            program: describe_ending(failed),
                        });
                    }
                    *cursor += 1;
                }
                Some(_) => {}
            }
            let remaining = &tape.events[*cursor..];
            if let Some(first) = remaining.first() {
                return Err(TapeError::Leftover {
                    count: remaining.len() as u64,
                    first_index: first.seq,
                    first: first.request.to_string(),
                });
            }
            Ok(())
        }
    }
}

fn describe_ending(failed: bool) -> String {
    if failed {
        "a failed exit".to_string()
    } else {
        "a normal exit".to_string()
    }
}

/// Marks the tape finished without recording or checking anything: the run
/// is stopping on a tape error, and nothing after it should report another.
pub fn abandon() {
    match lock().as_mut() {
        Some(TapeState::Record {
            writer, finished, ..
        }) => {
            *finished = true;
            let _ = writer.flush();
        }
        Some(TapeState::Replay { finished, .. }) => *finished = true,
        None => {}
    }
}

/// An effect this stage cannot record or replay.
pub fn unsupported(what: &str) -> TapeError {
    TapeError::Unsupported {
        what: what.to_string(),
    }
}

fn append(request: EffectRequest, result: Option<Outcome>) -> Result<(), TapeError> {
    let mut guard = lock();
    let Some(TapeState::Record {
        writer,
        path,
        next_seq,
        finished: false,
    }) = guard.as_mut()
    else {
        return Ok(());
    };
    let event = TapeEvent {
        seq: *next_seq,
        fiber: 0,
        request,
        result,
    };
    *next_seq += 1;
    write_event(writer, path, &event)
}

fn write_event(
    writer: &mut BufWriter<File>,
    path: &str,
    event: &TapeEvent,
) -> Result<(), TapeError> {
    let line = serde_json::to_string(event).expect("tape events serialize");
    writeln!(writer, "{line}").map_err(|err| cannot_write(Path::new(path), &err))
}

/// Takes the next recorded event, which must be `request`.
fn take_matching(request: &EffectRequest) -> Result<TapeEvent, TapeError> {
    let mut guard = lock();
    let Some(TapeState::Replay { tape, cursor, .. }) = guard.as_mut() else {
        unreachable!("replay mode without a replay tape");
    };
    let Some(event) = tape.events.get(*cursor) else {
        return Err(TapeError::Exhausted {
            count: tape.events.len() as u64,
            actual: request.to_string(),
        });
    };
    if event.request.kind_name() != request.kind_name() {
        return Err(TapeError::KindMismatch {
            index: event.seq,
            expected: event.request.to_string(),
            actual: request.to_string(),
        });
    }
    if event.request != *request {
        let (what, tape_shown, program_shown) = difference(&event.request, request);
        return Err(TapeError::PayloadMismatch {
            index: event.seq,
            what,
            tape: tape_shown,
            program: program_shown,
        });
    }
    *cursor += 1;
    Ok(event.clone())
}

/// Names what differs between two requests of the same kind.
fn difference(tape: &EffectRequest, program: &EffectRequest) -> (String, String, String) {
    match (tape, program) {
        (
            EffectRequest::Stdout { text: recorded } | EffectRequest::Stderr { text: recorded },
            EffectRequest::Stdout { text: printed } | EffectRequest::Stderr { text: printed },
        ) => {
            let (recorded_line, printed_line) = first_differing_line(recorded, printed);
            ("the printed text".to_string(), recorded_line, printed_line)
        }
        _ => (
            format!("the `{}` effect", tape.kind_name()),
            tape.to_string(),
            program.to_string(),
        ),
    }
}

/// The first line where two outputs differ, quoted, for a diagnostic.
fn first_differing_line(recorded: &TapeBytes, printed: &TapeBytes) -> (String, String) {
    let recorded = recorded.lossy();
    let printed = printed.lossy();
    let mut recorded_lines = recorded.lines();
    let mut printed_lines = printed.lines();
    loop {
        match (recorded_lines.next(), printed_lines.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (a, b) => {
                let show = |line: Option<&str>| match line {
                    Some(line) => format!("{:?}", truncate(line)),
                    None => "no further output".to_string(),
                };
                return (show(a), show(b));
            }
        }
    }
}

fn truncate(line: &str) -> String {
    const LIMIT: usize = 120;
    if line.chars().count() <= LIMIT {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(LIMIT).collect();
    cut.push('…');
    cut
}

/// The arguments the recorded run was given, when replaying.
pub fn replay_argv() -> Option<Vec<String>> {
    match lock().as_ref() {
        Some(TapeState::Replay { tape, .. }) => Some(tape.header.argv.clone()),
        _ => None,
    }
}

/// How the tape treats a primop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapeClass {
    /// Deterministic: never on the tape.
    Untaped,
    /// Reads the outside world; replay returns the recorded result.
    Input,
    /// `time(f)`: runs program code, then reports the elapsed time.
    Time,
    /// Writes to stdout or stderr; replay checks it and prints it again.
    Console,
    /// Changes the outside world; replay checks it and does not re-apply it.
    Output,
    /// Async, tasks, channels and network I/O: refused until 0188 S1b.
    Refused,
}

/// How the tape treats `op`.
pub fn tape_class(op: CorePrimOp) -> TapeClass {
    use CorePrimOp::*;
    match op {
        ReadStdin | ReadFile | ReadLines | TryReadFile | FsExists | FsIsDir | FsIsFile
        | FsListDir | FsMetadata | Sha256File | EnvVar | EnvArgs | EnvCwd | EnvHomeDir
        | ProcRun | ClockNow => TapeClass::Input,
        Time => TapeClass::Time,
        Print | Println | DebugTrace => TapeClass::Console,
        WriteFile | FsWriteFile | FsCreateDirAll | FsRemoveFile | FsRemoveDirAll | FsRename => {
            TapeClass::Output
        }
        TaskSpawn | TaskSpawnMove | TaskSpawnScoped | TaskSpawnScopedMove | TaskAwait
        | TaskBlockingJoin | TaskCancel | FiberRunAsync | FiberRunAsyncWith | FiberSleep
        | FiberTimeout | FiberYieldNow | FiberFork | FiberForkScoped | FiberBoth | FiberRace
        | FiberFirstOf | TcpConnect | TcpRead | TcpWriteAll | TcpClose | TcpListen | TcpAccept
        | ChanMake | ChanSend | ChanRecv | ChanTrySend | ChanTryRecv | ChanSendMove
        | ChanTrySendMove | EventRecv | EventSend | EventSendMove | EventAfter | EventSync
        | EventPoll | EventWait => TapeClass::Refused,
        _ => TapeClass::Untaped,
    }
}
