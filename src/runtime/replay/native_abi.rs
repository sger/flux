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
//! A tape error ends the process: the error goes to stderr as one
//! `flux-tape-error: {json}` line, which the driver renders as a diagnostic.

use std::ffi::{CStr, c_char};
use std::path::Path;
use std::sync::Mutex;

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
            EventKind::ProcRun => EffectRequest::ProcRun {
                cmd: text(0),
                argv: (1..args.len()).map(text).collect(),
            },
            EventKind::ClockNow => EffectRequest::ClockNow,
            EventKind::WriteFile => EffectRequest::WriteFile {
                path: text(0),
                contents: bytes(1),
            },
            EventKind::FsWriteFile => EffectRequest::FsWriteFile {
                path: text(0),
                contents: bytes(1),
            },
            EventKind::FsCreateDirAll => EffectRequest::FsCreateDirAll { path: text(0) },
            EventKind::FsRemoveFile => EffectRequest::FsRemoveFile { path: text(0) },
            EventKind::FsRemoveDirAll => EffectRequest::FsRemoveDirAll { path: text(0) },
            EventKind::FsRename => EffectRequest::FsRename {
                from: text(0),
                to: text(1),
            },
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
/// into, which stay valid until the next `flux_tape_begin`.
struct Pending {
    kind: Option<EventKind>,
    args: Vec<Vec<u8>>,
    list: Vec<Vec<u8>>,
    s0: Vec<u8>,
    s1: Vec<u8>,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending {
    kind: None,
    args: Vec::new(),
    list: Vec::new(),
    s0: Vec::new(),
    s1: Vec::new(),
});

fn pending() -> std::sync::MutexGuard<'static, Pending> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reports a tape error and ends the process.
fn fatal(err: TapeError) -> ! {
    super::abandon();
    eprintln!("{}", err.to_wire());
    std::process::exit(1);
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
/// which the driver sets. Returns the mode: 0 off, 1 record, 2 replay.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_init() -> i32 {
    // Errors found inside the async scheduler reach the driver the same way
    // as the rest: one wire line on stderr.
    super::async_tape::set_fatal_handler(Box::new(|err| eprintln!("{}", err.to_wire())));
    if let Some(path) = std::env::var_os("FLUX_TAPE_RECORD") {
        if let Err(err) = super::start_record_append(Path::new(&path)) {
            fatal(err);
        }
        return 1;
    }
    if let Some(path) = std::env::var_os("FLUX_TAPE_REPLAY") {
        match LoadedTape::load(Path::new(&path)) {
            Ok(tape) => super::start_replay(tape),
            Err(err) => fatal(err),
        }
        return 2;
    }
    0
}

/// Begins describing a request of `kind`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_tape_begin(kind: u32) {
    let mut pending = pending();
    pending.kind = EventKind::from_raw(kind);
    pending.args.clear();
    pending.list.clear();
}

/// Adds the request's next string argument.
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_arg(ptr: *const u8, len: usize) {
    let bytes = unsafe { slice(ptr, len) }.to_vec();
    pending().args.push(bytes);
}

/// Adds the next item of a LIST outcome being recorded.
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_list_push(ptr: *const u8, len: usize) {
    let bytes = unsafe { slice(ptr, len) }.to_vec();
    pending().list.push(bytes);
}

fn take_request(pending: &mut Pending) -> EffectRequest {
    let kind = pending
        .kind
        .take()
        .expect("flux_tape_begin before recording or replaying");
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
    let (request, outcome) = {
        let mut pending = pending();
        let request = take_request(&mut pending);
        let list = std::mem::take(&mut pending.list);
        (request, unsafe { decode(raw, list) })
    };
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
    let request = take_request(&mut pending());
    let outcome = match super::effect(request, || unreachable!("replay never performs")) {
        Ok(outcome) => outcome,
        Err(err) => fatal(err),
    };
    let mut pending = pending();
    // SAFETY: C passes a writable outcome.
    let out = unsafe { &mut *out };
    encode(&outcome, out, &mut pending);
}

/// The `index`th item of the LIST outcome just replayed.
///
/// # Safety
///
/// `len` points to a writable `usize`; the returned bytes are valid until the next `flux_tape_begin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_list_item(index: usize, len: *mut usize) -> *const u8 {
    let pending = pending();
    let item = pending.list.get(index).map(Vec::as_slice).unwrap_or(&[]);
    // SAFETY: C passes a writable length; the bytes live in PENDING until the
    // next begin, and C copies them before then.
    unsafe { *len = item.len() };
    item.as_ptr()
}

/// Records or checks a line of console output (`stream` 0 stdout, 1 stderr).
///
/// # Safety
///
/// `ptr` points to `len` readable bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_tape_console(stream: i32, ptr: *const u8, len: usize) {
    let text = TapeBytes(unsafe { slice(ptr, len) }.to_vec());
    let request = if stream == 1 {
        EffectRequest::Stderr { text }
    } else {
        EffectRequest::Stdout { text }
    };
    if let Err(err) = super::console(request) {
        fatal(err);
    }
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
    let unfinished = {
        let mut pending = pending();
        pending.kind.take().map(|kind| kind.request(&pending.args))
    };
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
        unsafe { CStr::from_ptr(what) }
            .to_string_lossy()
            .into_owned()
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
            kind: IoKind::ALL
                .get(raw.err_kind as usize)
                .copied()
                .unwrap_or(IoKind::Other),
            message: s0().lossy(),
            path: s1().lossy(),
        }),
        OUT_META => Outcome::Meta(FsMeta {
            size: raw.i0,
            modified: raw.i1,
            is_dir: raw.b0 != 0,
            is_file: raw.b1 != 0,
        }),
        OUT_PROC => Outcome::Proc(ProcOut {
            status: raw.i0,
            stdout: s0(),
            stderr: s1(),
        }),
        _ => Outcome::Fail(s0().lossy()),
    };
    if raw.ok != 0 {
        Outcome::Ok(Box::new(inner))
    } else {
        inner
    }
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
