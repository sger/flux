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
//! Async code goes on the tape through [`async_tape`]: what each run hands to
//! the async backend, and the completions it gets back.
//!
//! When no tape is active, the only cost is the [`is_active`] check.

pub mod async_tape;
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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

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
    Record(Recording),
    Replay {
        tape: LoadedTape,
        cursor: usize,
        finished: bool,
        /// Console events before this index have been printed: a debugger
        /// that goes back and runs them again does not print them twice.
        shown: usize,
    },
    /// A debug session running part of its own recording again (0188 S3):
    /// effects are answered from the kept events until the cursor reaches
    /// their end, and recording goes on from there.
    Rerun {
        recording: Recording,
        cursor: usize,
    },
}

struct Recording {
    /// None for a debug session's recording kept only in memory.
    writer: Option<BufWriter<File>>,
    path: String,
    next_seq: u64,
    finished: bool,
    /// Every event so far, kept while a debugger may go back.
    kept: Option<Vec<TapeEvent>>,
}

/// Whether the recording was started by `flux debug` rather than asked for
/// with `--record`: it can be dropped when the program does something it
/// cannot record.
static IMPLICIT: AtomicBool = AtomicBool::new(false);

/// A place in the tape a debugger can come back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapeMark {
    cursor: usize,
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
    install_recording(Recording {
        writer: Some(writer),
        path: path.display().to_string(),
        next_seq: 0,
        finished: false,
        kept: None,
    });
    Ok(())
}

fn install_recording(recording: Recording) {
    async_tape::reset_ids();
    IMPLICIT.store(false, Ordering::Relaxed);
    *lock() = Some(TapeState::Record(recording));
    MODE.store(ReplayMode::Record as u8, Ordering::Relaxed);
}

/// Starts replaying `tape`.
pub fn start_replay(tape: LoadedTape) {
    async_tape::reset_ids();
    IMPLICIT.store(false, Ordering::Relaxed);
    *lock() = Some(TapeState::Replay {
        tape,
        cursor: 0,
        finished: false,
        shown: 0,
    });
    MODE.store(ReplayMode::Replay as u8, Ordering::Relaxed);
}

/// Starts a recording kept only in memory, for a debug session given no
/// tape: going back runs the program's past from it (0188 S3).
pub fn start_memory_record() {
    install_recording(Recording {
        writer: None,
        path: "<debug session>".to_string(),
        next_seq: 0,
        finished: false,
        kept: Some(Vec::new()),
    });
    IMPLICIT.store(true, Ordering::Relaxed);
}

/// Keeps the events of the recording in progress in memory as well, so a
/// debug session recording to a file can go back.
pub fn keep_events() {
    if let Some(TapeState::Record(recording)) = lock().as_mut() {
        recording.kept.get_or_insert_with(Vec::new);
    }
}

/// Whether the tape is a debug session's own recording, not one asked for.
pub fn is_implicit() -> bool {
    IMPLICIT.load(Ordering::Relaxed) && is_active()
}

/// Drops a debug session's own recording: the program is about to do
/// something it cannot record, and goes on without a tape.
pub fn stop_implicit() {
    if !is_implicit() {
        return;
    }
    MODE.store(ReplayMode::Off as u8, Ordering::Relaxed);
    *lock() = None;
    IMPLICIT.store(false, Ordering::Relaxed);
}

/// Where the tape is now, if a debugger can come back here: a replay, or a
/// recording that keeps its events.
pub fn mark() -> Option<TapeMark> {
    match lock().as_ref()? {
        TapeState::Record(Recording {
            kept: Some(kept), ..
        }) => Some(TapeMark { cursor: kept.len() }),
        TapeState::Record(_) => None,
        TapeState::Replay { cursor, .. } | TapeState::Rerun { cursor, .. } => {
            Some(TapeMark { cursor: *cursor })
        }
    }
}

/// Puts the tape back at `mark`. A recording then answers effects from its
/// kept events until it reaches its end again.
pub fn rewind_to(mark: TapeMark) {
    let mut guard = lock();
    let Some(state) = guard.take() else {
        return;
    };
    let state = match state {
        TapeState::Replay { tape, shown, .. } => TapeState::Replay {
            tape,
            cursor: mark.cursor,
            finished: false,
            shown,
        },
        TapeState::Record(recording) | TapeState::Rerun { recording, .. } => {
            rerun_or_record(recording, mark.cursor)
        }
    };
    *guard = Some(state);
}

/// A recording at `cursor`: re-running its kept events, or recording when
/// the cursor is at their end.
fn rerun_or_record(recording: Recording, cursor: usize) -> TapeState {
    let kept = recording.kept.as_ref().map_or(0, Vec::len);
    if cursor < kept {
        MODE.store(ReplayMode::Replay as u8, Ordering::Relaxed);
        TapeState::Rerun { recording, cursor }
    } else {
        MODE.store(ReplayMode::Record as u8, Ordering::Relaxed);
        TapeState::Record(recording)
    }
}

/// Turns the tape off and forgets it, without finishing it. For tests.
pub fn reset() {
    MODE.store(ReplayMode::Off as u8, Ordering::Relaxed);
    *lock() = None;
    IMPLICIT.store(false, Ordering::Relaxed);
    async_tape::reset_ids();
    positions().clear();
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
    console_shown(request).map(|_| ())
}

/// As [`console`], and says whether to print the text: not when a debugger
/// has gone back and the run reaches text it has already printed.
pub fn console_shown(request: EffectRequest) -> Result<bool, TapeError> {
    debug_assert_eq!(request.class(), EffectClass::Console);
    match mode() {
        ReplayMode::Off => Ok(true),
        ReplayMode::Record => append(request, None).map(|_| true),
        ReplayMode::Replay => {
            let index = take_matching(&request)?.seq as usize;
            Ok(match lock().as_mut() {
                Some(TapeState::Replay { shown, .. }) => {
                    let first_time = index >= *shown;
                    *shown = (*shown).max(index + 1);
                    first_time
                }
                // Re-running a recording: it printed all of it already.
                _ => false,
            })
        }
    }
}

/// Ends the tape: records how the run ended, or checks it against the tape
/// and that nothing recorded was left unreplayed. Calling it again does
/// nothing.
pub fn finish(failed: bool, message: Option<String>) -> Result<(), TapeError> {
    let mut guard = lock();
    // A run ends where its recording ends; should it end while re-running
    // kept events, it records from there.
    if let Some(TapeState::Rerun { .. }) = guard.as_ref()
        && let Some(TapeState::Rerun { recording, .. }) = guard.take()
    {
        MODE.store(ReplayMode::Record as u8, Ordering::Relaxed);
        *guard = Some(TapeState::Record(recording));
    }
    let Some(state) = guard.as_mut() else {
        return Ok(());
    };
    match state {
        TapeState::Record(recording) => {
            if recording.finished {
                return Ok(());
            }
            recording.finished = true;
            let event = TapeEvent {
                seq: recording.next_seq,
                fiber: 0,
                request: EffectRequest::Exit { failed, message },
                result: None,
            };
            recording.next_seq += 1;
            recording.store(event)?;
            recording.flush()
        }
        TapeState::Rerun { .. } => unreachable!("a re-run became a recording above"),
        TapeState::Replay {
            tape,
            cursor,
            finished,
            ..
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
        Some(TapeState::Record(recording) | TapeState::Rerun { recording, .. }) => {
            recording.finished = true;
            let _ = recording.flush();
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

/// The logical fiber an event belongs to. A completion is received by the
/// scheduler, not by a fiber.
fn event_fiber(request: &EffectRequest) -> u32 {
    match request {
        EffectRequest::AsyncComplete { .. } | EffectRequest::Exit { .. } => 0,
        _ => async_tape::current_fiber(),
    }
}

fn append(request: EffectRequest, result: Option<Outcome>) -> Result<(), TapeError> {
    let fiber = event_fiber(&request);
    let mut guard = lock();
    let Some(TapeState::Record(recording)) = guard.as_mut() else {
        return Ok(());
    };
    if recording.finished {
        return Ok(());
    }
    let event = TapeEvent {
        seq: recording.next_seq,
        fiber,
        request,
        result,
    };
    recording.next_seq += 1;
    note_event_position(event.seq as usize);
    recording.store(event)
}

impl Recording {
    fn store(&mut self, event: TapeEvent) -> Result<(), TapeError> {
        if let Some(writer) = self.writer.as_mut() {
            let line = serde_json::to_string(&event).expect("tape events serialize");
            writeln!(writer, "{line}").map_err(|err| cannot_write(Path::new(&self.path), &err))?;
        }
        if let Some(kept) = self.kept.as_mut() {
            kept.push(event);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TapeError> {
        match self.writer.as_mut() {
            Some(writer) => writer
                .flush()
                .map_err(|err| cannot_write(Path::new(&self.path), &err)),
            None => Ok(()),
        }
    }
}

/// Takes the next recorded event, which must be `request`.
fn take_matching(request: &EffectRequest) -> Result<TapeEvent, TapeError> {
    let fiber = event_fiber(request);
    let mut guard = lock();
    let (events, cursor): (&[TapeEvent], &mut usize) = match guard.as_mut() {
        Some(TapeState::Replay { tape, cursor, .. }) => (&tape.events, cursor),
        Some(TapeState::Rerun { recording, cursor }) => {
            (recording.kept.as_deref().unwrap_or_default(), cursor)
        }
        _ => unreachable!("replay mode without a replay tape"),
    };
    let Some(event) = events.get(*cursor) else {
        return Err(TapeError::Exhausted {
            count: events.len() as u64,
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
    if event.fiber != fiber {
        return Err(TapeError::PayloadMismatch {
            index: event.seq,
            what: format!("which fiber performed {request}"),
            tape: describe_fiber(event.fiber),
            program: describe_fiber(fiber),
        });
    }
    *cursor += 1;
    let event = event.clone();
    note_event_position(event.seq as usize);
    // A re-run that reaches the end of its kept events records again.
    if let Some(TapeState::Rerun { recording, cursor }) = guard.as_ref()
        && *cursor >= recording.kept.as_ref().map_or(0, Vec::len)
        && let Some(TapeState::Rerun { recording, cursor }) = guard.take()
    {
        *guard = Some(rerun_or_record(recording, cursor));
    }
    drop(guard);
    async_tape::progressed();
    Ok(event)
}

// ── Where each event happened, for a debugger (0188 S3) ─────────────────

/// The debugger's position in the run, set at each stop site while one is
/// attached.
static POSITION: AtomicU64 = AtomicU64::new(0);
static POSITIONS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn positions() -> std::sync::MutexGuard<'static, Vec<u64>> {
    POSITIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Tells the tape where the debugged program is now.
#[inline]
pub fn set_position(position: u64) {
    POSITION.store(position, Ordering::Relaxed);
}

fn note_event_position(index: usize) {
    let mut positions = positions();
    if index == positions.len() {
        positions.push(POSITION.load(Ordering::Relaxed));
    }
}

/// How many events the run has reached so far.
pub fn events_reached() -> usize {
    positions().len()
}

/// The debugger position at which event `index` happened, once reached.
pub fn event_position(index: usize) -> Option<u64> {
    positions().get(index).copied()
}

/// The tape's events, each with where it happened if the run has reached
/// it: the kept events of a recording, or a replayed tape's.
pub fn event_list() -> Vec<(u64, String, Option<u64>)> {
    let events: Vec<(u64, String)> = match lock().as_ref() {
        Some(TapeState::Replay { tape, .. }) => tape
            .events
            .iter()
            .map(|event| (event.seq, event.request.to_string()))
            .collect(),
        Some(
            TapeState::Record(Recording {
                kept: Some(kept), ..
            })
            | TapeState::Rerun {
                recording: Recording {
                    kept: Some(kept), ..
                },
                ..
            },
        ) => kept
            .iter()
            .map(|event| (event.seq, event.request.to_string()))
            .collect(),
        _ => Vec::new(),
    };
    let positions = positions();
    events
        .into_iter()
        .map(|(seq, text)| (seq, text, positions.get(seq as usize).copied()))
        .collect()
}

fn describe_fiber(fiber: u32) -> String {
    if fiber == 0 {
        "the main program".to_string()
    } else {
        format!("fiber {fiber}")
    }
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

/// The number of workers an async run gets: `requested`, or one while a tape
/// is active, since one worker makes the schedule a function of the order
/// completions arrive in, which the tape records.
pub fn worker_count(requested: usize) -> usize {
    if !is_active() {
        return requested;
    }
    if requested > 1 {
        static NOTED: std::sync::Once = std::sync::Once::new();
        NOTED.call_once(|| {
            eprintln!("note: --record and --replay run async code on one worker");
        });
    }
    1
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
    /// Fibers, channels, events and sockets: they run as usual, and the
    /// async backend puts what they wait on on the tape (`async_tape`).
    Scheduled,
    /// Tasks: they run on threads of their own, which this stage cannot
    /// record (0188 S8).
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
        | TaskBlockingJoin | TaskCancel => TapeClass::Refused,
        FiberRunAsync | FiberRunAsyncWith | FiberSleep | FiberTimeout | FiberYieldNow
        | FiberFork | FiberForkScoped | FiberBoth | FiberRace | FiberFirstOf | TcpConnect
        | TcpRead | TcpWriteAll | TcpClose | TcpListen | TcpAccept | ChanMake | ChanSend
        | ChanRecv | ChanTrySend | ChanTryRecv | ChanSendMove | ChanTrySendMove | EventRecv
        | EventSend | EventSendMove | EventAfter | EventSync | EventPoll | EventWait => {
            TapeClass::Scheduled
        }
        _ => TapeClass::Untaped,
    }
}
