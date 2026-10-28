//! The effect tape for native programs (proposal 0188 S1).
//!
//! A native program's effects are C functions in `runtime/c`. Each one, while
//! a tape is active, describes its request here (`flux_tape_begin` and
//! `flux_tape_arg`) and then either hands over what it produced
//! (`flux_tape_record`) or asks for what was recorded (`flux_tape_replay`).
//! Everything crosses as plain C data — byte slices, integers, and the flat
//! [`FluxTapeOutcome`] — and C builds its own values from it, so this side
//! never touches the native heap.
//!
//! A tape error ends the process. It goes to stderr rendered as a
//! diagnostic and then as one `flux-tape-error: {json}` line for tools; run
//! by the driver (`FLUX_TAPE_WIRE`), only the line, which the driver renders.

use std::cell::RefCell;
use std::ffi::{CStr, c_char};
use std::path::Path;

use super::{
    EffectRequest, FsMeta, IoKind, LoadedTape, Outcome, ProcOut, TapeBytes, TapeError, TapeIoError,
};

/// The request kinds C can begin. The values are part of the C ABI
/// (`FLUX_TAPE_EV_*` in `flux_rt.h`); never renumber them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EventKind {
    ReadStdin = 0,
    ReadFile = 1,
    ReadLines = 2,
    TryReadFile = 3,
    FsExists = 4,
    FsIsDir = 5,
    FsIsFile = 6,
    FsListDir = 7,
    FsMetadata = 8,
    Sha256File = 9,
    EnvVar = 10,
    EnvArgs = 11,
    EnvCwd = 12,
    EnvHomeDir = 13,
    ProcRun = 14,
    ClockNow = 15,
    WriteFile = 16,
    FsWriteFile = 17,
    FsCreateDirAll = 18,
    FsRemoveFile = 19,
    FsRemoveDirAll = 20,
    FsRename = 21,
    EventClock = 22,
}

impl EventKind {
    pub const ALL: [EventKind; 23] = [
        EventKind::ReadStdin,
        EventKind::ReadFile,
        EventKind::ReadLines,
        EventKind::TryReadFile,
        EventKind::FsExists,
        EventKind::FsIsDir,
        EventKind::FsIsFile,
        EventKind::FsListDir,
        EventKind::FsMetadata,
        EventKind::Sha256File,
        EventKind::EnvVar,
        EventKind::EnvArgs,
        EventKind::EnvCwd,
        EventKind::EnvHomeDir,
        EventKind::ProcRun,
        EventKind::ClockNow,
        EventKind::WriteFile,
        EventKind::FsWriteFile,
        EventKind::FsCreateDirAll,
        EventKind::FsRemoveFile,
        EventKind::FsRemoveDirAll,
        EventKind::FsRename,
        EventKind::EventClock,
    ];

    fn from_raw(raw: u32) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|kind| *kind as u32 == raw)
    }

    /// The request, from the string arguments C passed in order.
    fn request(self, args: &[Vec<u8>]) -> EffectRequest {
        let text = |index: usize| {
            args.get(index)
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default()
        };
        let bytes = |index: usize| TapeBytes(args.get(index).cloned().unwrap_or_default());
        match self {
            EventKind::ReadStdin => EffectRequest::ReadStdin,
            EventKind::ReadFile => EffectRequest::ReadFile { path: text(0) },
            EventKind::ReadLines => EffectRequest::ReadLines { path: text(0) },
            EventKind::TryReadFile => EffectRequest::TryReadFile { path: text(0) },
            EventKind::FsExists => EffectRequest::FsExists { path: text(0) },
            EventKind::FsIsDir => EffectRequest::FsIsDir { path: text(0) },
            EventKind::FsIsFile => EffectRequest::FsIsFile { path: text(0) },
            EventKind::FsListDir => EffectRequest::FsListDir { path: text(0) },
            EventKind::FsMetadata => EffectRequest::FsMetadata { path: text(0) },
            EventKind::Sha256File => EffectRequest::Sha256File { path: text(0) },
            EventKind::EnvVar => EffectRequest::EnvVar { name: text(0) },
            EventKind::EnvArgs => EffectRequest::EnvArgs,
            EventKind::EnvCwd => EffectRequest::EnvCwd,
            EventKind::EnvHomeDir => EffectRequest::EnvHomeDir,
            EventKind::ProcRun => {
                EffectRequest::ProcRun { cmd: text(0), argv: (1..args.len()).map(text).collect() }
            }
            EventKind::ClockNow => EffectRequest::ClockNow,
            EventKind::WriteFile => EffectRequest::WriteFile { path: text(0), contents: bytes(1) },
            EventKind::FsWriteFile => {
                EffectRequest::FsWriteFile { path: text(0), contents: bytes(1) }
            }
            EventKind::FsCreateDirAll => EffectRequest::FsCreateDirAll { path: text(0) },
            EventKind::FsRemoveFile => EffectRequest::FsRemoveFile { path: text(0) },
            EventKind::FsRemoveDirAll => EffectRequest::FsRemoveDirAll { path: text(0) },
            EventKind::FsRename => EffectRequest::FsRename { from: text(0), to: text(1) },
            EventKind::EventClock => EffectRequest::EventClock,
        }
    }
}

/// Outcome tags shared with C (`FLUX_TAPE_OUT_*`).
pub const OUT_UNIT: i32 = 0;
pub const OUT_BOOL: i32 = 1;
pub const OUT_INT: i32 = 2;
pub const OUT_TEXT: i32 = 3;
pub const OUT_LIST: i32 = 4;
pub const OUT_SOME: i32 = 5;
pub const OUT_NONE: i32 = 6;
pub const OUT_ERR: i32 = 7;
pub const OUT_META: i32 = 8;
pub const OUT_PROC: i32 = 9;
pub const OUT_FAIL: i32 = 10;

/// An [`Outcome`] as C sees it. `ok` wraps the outcome in `Ok(..)`. The byte
/// fields carry, by tag: TEXT/SOME/FAIL `s0`; ERR `s0` message and `s1` path;
/// PROC `s0` stdout and `s1` stderr. LIST items travel separately.
#[repr(C)]
#[derive(Debug)]
pub struct FluxTapeOutcome {
    pub tag: i32,
    pub ok: i32,
    pub err_kind: i32,
    pub b0: i32,
    pub b1: i32,
    pub pad: i32,
    pub i0: i64,
    pub i1: i64,
    pub s0: *const u8,
    pub n0: usize,
    pub s1: *const u8,
    pub n1: usize,
    pub list_len: usize,
}

/// The request being described, and the buffers a replayed outcome points
/// into, which stay valid until the next `flux_tape_begin`. One per thread:
/// a task's thread describes its own requests while the main program
/// describes others (0188 S8c).
#[derive(Default)]
struct Pending {
    kind: Option<EventKind>,
    args: Vec<Vec<u8>>,
    list: Vec<Vec<u8>>,
    s0: Vec<u8>,
    s1: Vec<u8>,
}

thread_local! {
    static PENDING: RefCell<Pending> = const {
        RefCell::new(Pending {
            kind: None,
            args: Vec::new(),
            list: Vec::new(),
            s0: Vec::new(),
            s1: Vec::new(),
        })
    };
}

fn with_pending<R>(f: impl FnOnce(&mut Pending) -> R) -> R {
    let mut f = Some(f);
    if let Ok(result) =
        PENDING.try_with(|pending| (f.take().expect("runs once"))(&mut pending.borrow_mut()))
    {
        return result;
    }
    // At process exit, after this thread's locals are gone (the tape's
    // atexit hook): nothing is being described any more.
    (f.take().expect("did not run"))(&mut Pending::default())
}

/// Set by the driver for the native program it runs: tape errors go to
/// stderr only as wire lines, which the driver renders itself.
pub const WIRE_ENV: &str = "FLUX_TAPE_WIRE";

/// The program's name for rendered errors: its `argv[0]`.
static PROGRAM: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Reports a tape error and ends the process.
fn fatal(err: TapeError) -> ! {
    super::abandon();
    report(&err);
    std::process::exit(1);
}

/// Prints a tape error: rendered, unless the driver will render it, and
/// always as the wire line tools read.
fn report(err: &TapeError) {
    if std::env::var_os(WIRE_ENV).is_none() {
        let program = PROGRAM.get().map(String::as_str).unwrap_or("program");
        let diagnostic = err.to_diagnostic(program, None);
        eprintln!(
            "{}",
            crate::diagnostics::render_runtime_diagnostic(&diagnostic, program, None, &[])
        );
    }
    eprintln!("{}", err.to_wire());
}

/// Reads C's `argv`.
///
/// # Safety
///
/// `argv` holds `argc` NUL-terminated strings (or is null).
unsafe fn read_argv(argc: i32, argv: *const *const c_char) -> Vec<String> {
    if argv.is_null() {
        return Vec::new();
    }
    (0..argc.max(0) as usize)
        .map(|i| {
            // SAFETY: the caller passes `argc` valid pointers.
            let arg = unsafe { *argv.add(i) };
            if arg.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(arg) }.to_string_lossy().into_owned()
            }
        })
        .collect()
}

unsafe fn slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: C passes a pointer to `len` readable bytes.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

/// Starts recording or replaying from `FLUX_TAPE_RECORD` / `FLUX_TAPE_REPLAY`,
/// set by the driver, an editor or by hand. Returns the mode: 0 off, 1
/// record, 2 replay.
///
/// The program needs no driver for either: it writes a new tape's header
/// itself, from `program_hash` (the `flux_program_hash` its build embeds)
/// and its own `argv`, and checks a tape it replays the way the driver does.
///
/// # Safety
///
/// `program_hash` is a NUL-terminated string or null; `argv` holds `argc`
/// NUL-terminated strings or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_init(
    program_hash: *const c_char,
    argc: i32,
    argv: *const *const c_char,
) -> i32 {
    let argv = unsafe { read_argv(argc, argv) };
    let _ = PROGRAM.set(argv.first().cloned().unwrap_or_default());
    let program_hash = if program_hash.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(program_hash) }.to_string_lossy().into_owned()
    };
    // Errors found inside the async scheduler are reported the same way as
    // the rest.
    super::async_tape::set_fatal_handler(Box::new(report));
    if let Some(path) = std::env::var_os("FLUX_TAPE_RECORD") {
        let header = super::TapeHeader::new(program_hash, argv, "native");
        if let Err(err) = super::start_record(Path::new(&path), &header) {
            fatal(err);
        }
        super::keep_titles();
        return 1;
    }
    if let Some(path) = std::env::var_os("FLUX_TAPE_REPLAY") {
        let tape = LoadedTape::load(Path::new(&path)).unwrap_or_else(|err| fatal(err));
        match tape.check_for(&program_hash) {
            // The driver already showed these.
            Ok(warnings) if std::env::var_os(WIRE_ENV).is_none() => {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
            }
            Ok(_) => {}
            Err(err) => fatal(err),
        }
        super::start_replay(tape);
        return 2;
    }
    0
}

/// The tape's events for a debugger, as JSON: up to a page of them from
/// event `from`, with where the next page starts and the next event the run
/// reaches —
/// `{"events":[{"seq":0,"text":"read_file(\"in.txt\")"}],"next":null,"cursor":1,"mode":"replay"}`,
/// or `{"events":null}` without a tape. lldb calls it while the program is
/// stopped (`flux events`, 0188 S9b); like `flux_debug_fibers` it answers
/// in a static buffer, valid until the next call, and takes no lock it
/// could wait for.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn flux_debug_tape_events(from: u64) -> *const c_char {
    use crate::runtime::r#async::native_abi::debug_json;
    use std::fmt::Write as _;
    // Escaped, an event's text fits in a few hundred bytes; a page this
    // long stays well inside the buffer.
    const PAGE: usize = 100;
    let Ok(page) = std::panic::catch_unwind(|| super::try_event_page(from, PAGE)) else {
        return debug_json::write(|out| out.text("{\"busy\":true}"));
    };
    debug_json::write(|out| match page {
        None => out.text("{\"busy\":true}"),
        Some(None) => out.text("{\"events\":null}"),
        Some(Some(page)) => {
            out.text("{\"events\":[")?;
            for (i, (seq, text)) in page.events.iter().enumerate() {
                if i > 0 {
                    out.text(",")?;
                }
                let text = serde_json::to_string(text).map_err(|_| std::fmt::Error)?;
                write!(out, "{{\"seq\":{seq},\"text\":{text}}}")?;
            }
            match page.next {
                Some(next) => write!(out, "],\"next\":{next}")?,
                None => out.text("],\"next\":null")?,
            }
            write!(
                out,
                ",\"cursor\":{},\"mode\":\"{}\"}}",
                page.cursor,
                if page.replaying { "replay" } else { "record" }
            )
        }
    })
}

/// Begins describing a request of `kind`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_begin(kind: u32) {
    with_pending(|pending| {
        pending.kind = EventKind::from_raw(kind);
        pending.args.clear();
        pending.list.clear();
    });
}

/// Adds the request's next string argument.
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_arg(ptr: *const u8, len: usize) {
    let bytes = unsafe { slice(ptr, len) }.to_vec();
    with_pending(|pending| pending.args.push(bytes));
}

/// Adds the next item of a LIST outcome being recorded.
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_list_push(ptr: *const u8, len: usize) {
    let bytes = unsafe { slice(ptr, len) }.to_vec();
    with_pending(|pending| pending.list.push(bytes));
}

fn take_request(pending: &mut Pending) -> EffectRequest {
    let kind = pending.kind.take().expect("flux_tape_begin before recording or replaying");
    kind.request(&pending.args)
}

/// Records the outcome of the request just described.
///
/// # Safety
///
/// `outcome` points to a valid `FluxTapeOutcome` whose byte fields point to `n0`/`n1` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_record(outcome: *const FluxTapeOutcome) {
    // SAFETY: C passes a valid outcome.
    let raw = unsafe { &*outcome };
    let (request, outcome) = with_pending(|pending| {
        let request = take_request(pending);
        let list = std::mem::take(&mut pending.list);
        (request, unsafe { decode(raw, list) })
    });
    if let Err(err) = super::effect(request, || outcome) {
        fatal(err);
    }
}

/// Fills `out` with the recorded outcome of the request just described. Its
/// bytes and list items stay valid until the next `flux_tape_begin`.
///
/// # Safety
///
/// `out` points to a writable `FluxTapeOutcome`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_replay(out: *mut FluxTapeOutcome) {
    let request = with_pending(take_request);
    let outcome = match super::effect(request, || unreachable!("replay never performs")) {
        Ok(outcome) => outcome,
        Err(err) => fatal(err),
    };
    // SAFETY: C passes a writable outcome.
    let out = unsafe { &mut *out };
    with_pending(|pending| encode(&outcome, out, pending));
}

/// The `index`th item of the LIST outcome just replayed.
///
/// # Safety
///
/// `len` points to a writable `usize`; the returned bytes are valid until the next `flux_tape_begin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_list_item(index: usize, len: *mut usize) -> *const u8 {
    with_pending(|pending| {
        let item = pending.list.get(index).map(Vec::as_slice).unwrap_or(&[]);
        // SAFETY: C passes a writable length; the bytes live in this thread's
        // PENDING until its next begin, and C copies them before then.
        unsafe { *len = item.len() };
        item.as_ptr()
    })
}

/// Records or checks a line of console output (`stream` 0 stdout, 1 stderr),
/// and returns whether to print it. C prints it, then calls
/// `flux_tape_console_done`: while replaying, this thread keeps its turn in
/// between, so threads print in the recorded order (0188 S8c).
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_console(stream: i32, ptr: *const u8, len: usize) -> i32 {
    let text = TapeBytes(unsafe { slice(ptr, len) }.to_vec());
    let request =
        if stream == 1 { EffectRequest::Stderr { text } } else { EffectRequest::Stdout { text } };
    match super::console_begin(request) {
        Ok(shown) => shown as i32,
        Err(err) => fatal(err),
    }
}

/// Ends the turn `flux_tape_console` took, once the text is printed.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_console_done() {
    super::console_end();
}

/// What `flux_tape_ordered_begin` begins (0188 S8c).
const ORDERED_TASK_SPAWN: u32 = 1;
const ORDERED_TASK_START: u32 = 2;
const ORDERED_TASK_END: u32 = 3;
const ORDERED_TASK_CANCEL: u32 = 4;
const ORDERED_TASK_JOIN: u32 = 5;
const ORDERED_TASK_AWAIT: u32 = 6;
const ORDERED_CHANNEL: u32 = 7;

/// The channel operations `flux_tape_ordered_begin` names by number.
const CHANNEL_OPS: [&str; 8] =
    ["make", "send", "recv", "try_send", "try_recv", "close", "len", "is_closed"];

/// Begins something that crosses threads while a tape is active: `kind` is
/// one of the `ORDERED_*` numbers, `a` the logical task (or the real
/// channel id), `b` a task end's `failed` (or a channel's operation, an
/// index in `CHANNEL_OPS`). C does it, then calls `flux_tape_ordered_end`.
/// Returns a spawned task's logical number, or a channel's.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_ordered_begin(kind: u32, a: u64, b: u64) -> u64 {
    let id = a as u32;
    let channel_op = || CHANNEL_OPS.get(b as usize).copied().unwrap_or("?").to_string();
    let hint = match kind {
        ORDERED_TASK_SPAWN => EffectRequest::TaskSpawn { id: 0 },
        ORDERED_TASK_START => EffectRequest::TaskStart { id },
        ORDERED_TASK_END => EffectRequest::TaskEnd { id, failed: b != 0 },
        ORDERED_TASK_CANCEL => EffectRequest::TaskCancel { id },
        ORDERED_TASK_JOIN => EffectRequest::TaskJoin { id },
        ORDERED_TASK_AWAIT => EffectRequest::TaskAwait { id },
        _ => EffectRequest::Channel { chan: 0, op: channel_op() },
    };
    let made = super::ordered_begin(&hint, || match kind {
        ORDERED_TASK_SPAWN => EffectRequest::TaskSpawn { id: super::async_tape::next_task() },
        ORDERED_CHANNEL => {
            EffectRequest::Channel { chan: super::async_tape::logical_channel(a), op: channel_op() }
        }
        _ => hint.clone(),
    });
    match made {
        Ok(EffectRequest::TaskSpawn { id }) => id as u64,
        Ok(EffectRequest::Channel { chan, .. }) => chan,
        Ok(_) => 0,
        Err(err) => fatal(err),
    }
}

/// Ends what `flux_tape_ordered_begin` began, with what it returned:
/// `shape` 0 nothing, 1 a Bool, 2 an Int, in `value`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_ordered_end(shape: i32, value: i64) {
    let outcome = match shape {
        1 => Some(Outcome::Bool(value != 0)),
        2 => Some(Outcome::Int(value)),
        _ => None,
    };
    if let Err(err) = super::ordered_end(outcome) {
        fatal(err);
    }
}

/// Whether the program has spawned a task under the tape: from then on its
/// channel operations are ordered.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_has_tasks() -> i32 {
    super::async_tape::has_tasks() as i32
}

/// This thread runs logical task `task` (0188 S8c): what it records is that
/// task's.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_enter_task(task: u32) {
    super::async_tape::enter_task(task);
}

/// Ends the tape: `failed` when the program panicked. A request begun but not
/// finished (an effect that panicked, like a `read_file` of a missing file)
/// is recorded first, as that failure.
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_finish(failed: i32, ptr: *const u8, len: usize) {
    let message = (!ptr.is_null() && len > 0)
        .then(|| String::from_utf8_lossy(unsafe { slice(ptr, len) }).into_owned());
    let unfinished =
        with_pending(|pending| pending.kind.take().map(|kind| kind.request(&pending.args)));
    if let (Some(request), true) = (unfinished, failed != 0) {
        let failure = Outcome::Fail(message.clone().unwrap_or_default());
        if let Err(err) = super::effect(request, || failure) {
            fatal(err);
        }
    }
    if let Err(err) = super::finish(failed != 0, message) {
        fatal(err);
    }
}

/// Stops the run: `what` cannot be recorded or replayed yet.
///
/// # Safety
///
/// `what` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_refuse(what: *const c_char) {
    let what = if what.is_null() {
        "this effect".to_string()
    } else {
        // SAFETY: C passes a NUL-terminated string.
        unsafe { CStr::from_ptr(what) }.to_string_lossy().into_owned()
    };
    let err = super::unsupported(&what);
    let _ = super::finish(true, Some(err.message()));
    fatal(err);
}

/// C's outcome as an [`Outcome`].
unsafe fn decode(raw: &FluxTapeOutcome, list: Vec<Vec<u8>>) -> Outcome {
    let s0 = || TapeBytes(unsafe { slice(raw.s0, raw.n0) }.to_vec());
    let s1 = || TapeBytes(unsafe { slice(raw.s1, raw.n1) }.to_vec());
    let inner = match raw.tag {
        OUT_UNIT => Outcome::Unit,
        OUT_BOOL => Outcome::Bool(raw.b0 != 0),
        OUT_INT => Outcome::Int(raw.i0),
        OUT_TEXT => Outcome::Text(s0()),
        OUT_LIST => Outcome::List(list.into_iter().map(TapeBytes).collect()),
        OUT_SOME => Outcome::Some(s0()),
        OUT_NONE => Outcome::None,
        OUT_ERR => Outcome::Err(TapeIoError {
            kind: IoKind::ALL.get(raw.err_kind as usize).copied().unwrap_or(IoKind::Other),
            message: s0().lossy(),
            path: s1().lossy(),
        }),
        OUT_META => Outcome::Meta(FsMeta {
            size: raw.i0,
            modified: raw.i1,
            is_dir: raw.b0 != 0,
            is_file: raw.b1 != 0,
        }),
        OUT_PROC => Outcome::Proc(ProcOut { status: raw.i0, stdout: s0(), stderr: s1() }),
        _ => Outcome::Fail(s0().lossy()),
    };
    if raw.ok != 0 { Outcome::Ok(Box::new(inner)) } else { inner }
}

/// An [`Outcome`] as C's flat struct, its bytes kept in `pending`.
fn encode(outcome: &Outcome, out: &mut FluxTapeOutcome, pending: &mut Pending) {
    *out = FluxTapeOutcome {
        tag: OUT_UNIT,
        ok: 0,
        err_kind: 0,
        b0: 0,
        b1: 0,
        pad: 0,
        i0: 0,
        i1: 0,
        s0: std::ptr::null(),
        n0: 0,
        s1: std::ptr::null(),
        n1: 0,
        list_len: 0,
    };
    let outcome = match outcome {
        Outcome::Ok(inner) => {
            out.ok = 1;
            inner.as_ref()
        }
        other => other,
    };
    pending.s0.clear();
    pending.s1.clear();
    pending.list.clear();
    match outcome {
        Outcome::Unit | Outcome::Ok(_) => out.tag = OUT_UNIT,
        Outcome::Bool(b) => {
            out.tag = OUT_BOOL;
            out.b0 = i32::from(*b);
        }
        Outcome::Int(i) => {
            out.tag = OUT_INT;
            out.i0 = *i;
        }
        Outcome::Text(text) => {
            out.tag = OUT_TEXT;
            pending.s0 = text.0.clone();
        }
        Outcome::List(items) => {
            out.tag = OUT_LIST;
            out.list_len = items.len();
            pending.list = items.iter().map(|item| item.0.clone()).collect();
        }
        Outcome::Some(text) => {
            out.tag = OUT_SOME;
            pending.s0 = text.0.clone();
        }
        Outcome::None => out.tag = OUT_NONE,
        Outcome::Err(error) => {
            out.tag = OUT_ERR;
            out.err_kind = IoKind::ALL
                .iter()
                .position(|kind| *kind == error.kind)
                .unwrap_or(IoKind::ALL.len() - 1) as i32;
            pending.s0 = error.message.clone().into_bytes();
            pending.s1 = error.path.clone().into_bytes();
        }
        Outcome::Meta(meta) => {
            out.tag = OUT_META;
            out.i0 = meta.size;
            out.i1 = meta.modified;
            out.b0 = i32::from(meta.is_dir);
            out.b1 = i32::from(meta.is_file);
        }
        Outcome::Proc(proc) => {
            out.tag = OUT_PROC;
            out.i0 = proc.status;
            pending.s0 = proc.stdout.0.clone();
            pending.s1 = proc.stderr.0.clone();
        }
        Outcome::Fail(message) => {
            out.tag = OUT_FAIL;
            pending.s0 = message.clone().into_bytes();
        }
        // Async completions go straight to the scheduler, never through C.
        Outcome::Bytes(_) | Outcome::Addrs(_) => {
            out.tag = OUT_FAIL;
            pending.s0 =
                format!("the tape has {outcome} where a synchronous effect returns").into_bytes();
        }
    }
    out.s0 = pending.s0.as_ptr();
    out.n0 = pending.s0.len();
    out.s1 = pending.s1.as_ptr();
    out.n1 = pending.s1.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are the C ABI: `FLUX_TAPE_EV_*` in `runtime/c/flux_rt.h`.
    #[test]
    fn event_kinds_keep_their_c_values() {
        for (index, kind) in EventKind::ALL.into_iter().enumerate() {
            assert_eq!(kind as u32, index as u32, "{kind:?}");
            assert_eq!(EventKind::from_raw(index as u32), Some(kind));
        }
        assert_eq!(EventKind::from_raw(EventKind::ALL.len() as u32), None);
    }

    #[test]
    fn outcomes_survive_the_c_struct() {
        let outcomes = [
            Outcome::Unit,
            Outcome::Bool(true),
            Outcome::Int(42),
            Outcome::Text("text".into()),
            Outcome::List(vec!["a".into(), "b".into()]),
            Outcome::Some("s".into()),
            Outcome::None,
            Outcome::Ok(Box::new(Outcome::Unit)),
            Outcome::Ok(Box::new(Outcome::Text("ok".into()))),
            Outcome::Err(TapeIoError {
                kind: IoKind::IsADirectory,
                message: "is a directory".into(),
                path: "p".into(),
            }),
            Outcome::Ok(Box::new(Outcome::Meta(FsMeta {
                size: 1,
                modified: 2,
                is_dir: true,
                is_file: false,
            }))),
            Outcome::Ok(Box::new(Outcome::Proc(ProcOut {
                status: 7,
                stdout: "o".into(),
                stderr: "e".into(),
            }))),
            Outcome::Fail("boom".into()),
        ];
        for outcome in outcomes {
            let mut pending = Pending {
                kind: None,
                args: Vec::new(),
                list: Vec::new(),
                s0: Vec::new(),
                s1: Vec::new(),
            };
            let mut raw = FluxTapeOutcome {
                tag: -1,
                ok: 0,
                err_kind: 0,
                b0: 0,
                b1: 0,
                pad: 0,
                i0: 0,
                i1: 0,
                s0: std::ptr::null(),
                n0: 0,
                s1: std::ptr::null(),
                n1: 0,
                list_len: 0,
            };
            encode(&outcome, &mut raw, &mut pending);
            let list = pending.list.clone();
            let back = unsafe { decode(&raw, list) };
            assert_eq!(back, outcome);
        }
    }
}
