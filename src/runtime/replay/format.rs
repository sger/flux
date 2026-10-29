//! The effect tape's file format (proposal 0188 §4.3, version 2).
//!
//! A tape is JSON Lines: one header line, then one line per effect in the
//! order the program performed them.
//!
//! ```text
//! {"magic":"FLXTAPE","version":2,"flux_version":"0.0.7","program_hash":"sha256:…","argv":["app.flx"],"backend":"vm"}
//! {"seq":0,"kind":"read_file","path":"in.txt","result":{"ok":{"text":"hello\n"}}}
//! {"seq":1,"kind":"stdout","text":"hello\n"}
//! {"seq":2,"kind":"exit","failed":false}
//! ```
//!
//! Every payload is host data (text, numbers, error kinds), never a runtime
//! value, so a tape of synchronous effects recorded by one backend replays on
//! the other. A tape with async events replays on the backend that recorded
//! it, whose scheduler it follows.
//!
//! Version 2 (0188 S8c) adds tasks: an event says which task's thread
//! performed it (`task`, absent for the main program), and the tape orders
//! what crosses threads (spawns, task starts and ends, completions handed
//! from one thread to another, channel operations once a task exists), so a
//! replay runs every thread in the recorded order.

use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::TapeError;

pub const TAPE_MAGIC: &str = "FLXTAPE";
pub const TAPE_VERSION: u32 = 3;

/// The first line of a tape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeHeader {
    pub magic: String,
    pub version: u32,
    /// The `flux` that recorded the tape. A different version only warns.
    pub flux_version: String,
    /// `sha256:<hex>` over the program's own modules; a tape replays only the
    /// program it was recorded from.
    pub program_hash: String,
    /// The program's arguments when it was recorded.
    pub argv: Vec<String>,
    /// `vm` or `native`: which backend recorded the tape.
    pub backend: String,
}

impl TapeHeader {
    pub fn new(program_hash: String, argv: Vec<String>, backend: &str) -> Self {
        Self {
            magic: TAPE_MAGIC.to_string(),
            version: TAPE_VERSION,
            flux_version: env!("CARGO_PKG_VERSION").to_string(),
            program_hash,
            argv,
            backend: backend.to_string(),
        }
    }
}

/// One recorded effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeEvent {
    pub seq: u64,
    /// The logical fiber that performed the effect: 0 outside `run_async`,
    /// otherwise fibers numbered from 1 in the order they first appear on
    /// the tape.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fiber: u32,
    /// The logical task whose thread performed the effect: 0 for the main
    /// program, otherwise tasks numbered from 1 in the order they were
    /// spawned (0188 S8c).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub task: u32,
    #[serde(flatten)]
    pub request: EffectRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Outcome>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What the program asked for. Inputs return a recorded [`Outcome`]; outputs
/// are checked and their recorded outcome returned without being re-applied;
/// console text is checked and printed again; `exit` ends the tape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectRequest {
    // ── Inputs ────────────────────────────────────────────────────────
    ReadStdin,
    ReadFile {
        path: String,
    },
    ReadLines {
        path: String,
    },
    TryReadFile {
        path: String,
    },
    FsExists {
        path: String,
    },
    FsIsDir {
        path: String,
    },
    FsIsFile {
        path: String,
    },
    FsListDir {
        path: String,
    },
    FsMetadata {
        path: String,
    },
    Sha256File {
        path: String,
    },
    EnvVar {
        name: String,
    },
    EnvArgs,
    EnvCwd,
    EnvHomeDir,
    ProcRun {
        cmd: String,
        argv: Vec<String>,
    },
    ClockNow,
    /// `time(f)` on the VM: the elapsed time around a callback whose own
    /// effects are recorded before this event.
    Time,
    /// The monotonic clock `Flow.Event` reads for `after` deadlines, in
    /// milliseconds.
    EventClock,
    // ── Outputs ───────────────────────────────────────────────────────
    WriteFile {
        path: String,
        contents: TapeBytes,
    },
    FsWriteFile {
        path: String,
        contents: TapeBytes,
    },
    FsCreateDirAll {
        path: String,
    },
    FsRemoveFile {
        path: String,
    },
    FsRemoveDirAll {
        path: String,
    },
    FsRename {
        from: String,
        to: String,
    },
    // ── Async submissions ─────────────────────────────────────────────
    // Work handed to the async backend. `req` numbers requests from 1 in
    // the order they are first submitted, and the event's `fiber` is the
    // fiber the request belongs to, which is the same on either backend
    // (0188 S9d). Replay checks each submission and does not perform it;
    // the result arrives later as `async_complete`.
    TimerStart {
        req: u64,
        ms: u64,
    },
    DnsResolve {
        req: u64,
        host: String,
        port: u16,
    },
    TcpConnect {
        req: u64,
        addr: String,
    },
    TcpListen {
        req: u64,
        addr: String,
    },
    TcpAccept {
        req: u64,
        handle: u64,
    },
    TcpRead {
        req: u64,
        handle: u64,
        max: u64,
    },
    TcpWrite {
        req: u64,
        handle: u64,
        bytes: TapeBytes,
    },
    TcpClose {
        handle: u64,
    },
    /// The async backend finished request `req`. Where it sits on the tape
    /// is when the scheduler received it.
    AsyncComplete {
        req: u64,
    },
    // ── Threads (0188 S8c) ────────────────────────────────────────────
    // What crosses from one thread to another, in the order it happened, so
    // a replay runs every thread in that order. A task is `id`: tasks are
    // numbered from 1 in the order they are spawned.
    /// The program spawned task `id`.
    TaskSpawn {
        id: u32,
    },
    /// Task `id`'s thread picked it up; the result says whether it ran (a
    /// task cancelled before it started does not).
    TaskStart {
        id: u32,
    },
    /// Task `id` finished: with a value, or failed.
    TaskEnd {
        id: u32,
        failed: bool,
    },
    /// The program cancelled task `id`.
    TaskCancel {
        id: u32,
    },
    /// The program joined task `id`.
    TaskJoin {
        id: u32,
    },
    /// A fiber awaited task `id`; the result says whether the task had
    /// already finished (native code then takes its result at once).
    TaskAwait {
        id: u32,
    },
    /// The scheduler received, from another thread, the completion fiber
    /// `waiter` was waiting for: a task's result, or a value a channel or
    /// event handed over. Replay delivers the real completion at this
    /// point. A fiber waits for one thing at a time, so the fiber names it
    /// on either backend (0188 S9d).
    Wake {
        waiter: u32,
    },
    /// A channel operation once the program has a task: `op` is `make`,
    /// `send`, `recv`, `try_send`, `try_recv` or `close`, and the result
    /// whether it succeeded.
    Channel {
        chan: u64,
        op: String,
    },
    // ── Console ───────────────────────────────────────────────────────
    Stdout {
        text: TapeBytes,
    },
    Stderr {
        text: TapeBytes,
    },
    // ── End of run ────────────────────────────────────────────────────
    Exit {
        failed: bool,
        /// Informational only: the two backends word failures differently,
        /// so replay compares `failed`, not the message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

/// How an [`EffectRequest`] behaves under replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Input,
    Output,
    Console,
    Terminal,
    /// Work handed to the async backend: checked, never performed.
    Submit,
    /// An async completion: replay delivers its recorded result.
    Completion,
    /// Something that crosses threads: replay does it again, at its turn.
    Ordered,
}

impl EffectRequest {
    pub fn class(&self) -> EffectClass {
        use EffectRequest::*;
        match self {
            ReadStdin
            | ReadFile { .. }
            | ReadLines { .. }
            | TryReadFile { .. }
            | FsExists { .. }
            | FsIsDir { .. }
            | FsIsFile { .. }
            | FsListDir { .. }
            | FsMetadata { .. }
            | Sha256File { .. }
            | EnvVar { .. }
            | EnvArgs
            | EnvCwd
            | EnvHomeDir
            | ProcRun { .. }
            | ClockNow
            | Time
            | EventClock => EffectClass::Input,
            WriteFile { .. }
            | FsWriteFile { .. }
            | FsCreateDirAll { .. }
            | FsRemoveFile { .. }
            | FsRemoveDirAll { .. }
            | FsRename { .. } => EffectClass::Output,
            Stdout { .. } | Stderr { .. } => EffectClass::Console,
            Exit { .. } => EffectClass::Terminal,
            TimerStart { .. }
            | DnsResolve { .. }
            | TcpConnect { .. }
            | TcpListen { .. }
            | TcpAccept { .. }
            | TcpRead { .. }
            | TcpWrite { .. }
            | TcpClose { .. } => EffectClass::Submit,
            AsyncComplete { .. } => EffectClass::Completion,
            TaskSpawn { .. }
            | TaskStart { .. }
            | TaskEnd { .. }
            | TaskCancel { .. }
            | TaskJoin { .. }
            | TaskAwait { .. }
            | Wake { .. }
            | Channel { .. } => EffectClass::Ordered,
        }
    }

    /// The request an async submission makes, if it makes one.
    pub fn submitted_req(&self) -> Option<u64> {
        use EffectRequest::*;
        match self {
            TimerStart { req, .. }
            | DnsResolve { req, .. }
            | TcpConnect { req, .. }
            | TcpListen { req, .. }
            | TcpAccept { req, .. }
            | TcpRead { req, .. }
            | TcpWrite { req, .. } => Some(*req),
            _ => None,
        }
    }

    /// The `kind` written to the tape.
    pub fn kind_name(&self) -> &'static str {
        use EffectRequest::*;
        match self {
            ReadStdin => "read_stdin",
            ReadFile { .. } => "read_file",
            ReadLines { .. } => "read_lines",
            TryReadFile { .. } => "try_read_file",
            FsExists { .. } => "fs_exists",
            FsIsDir { .. } => "fs_is_dir",
            FsIsFile { .. } => "fs_is_file",
            FsListDir { .. } => "fs_list_dir",
            FsMetadata { .. } => "fs_metadata",
            Sha256File { .. } => "sha256_file",
            EnvVar { .. } => "env_var",
            EnvArgs => "env_args",
            EnvCwd => "env_cwd",
            EnvHomeDir => "env_home_dir",
            ProcRun { .. } => "proc_run",
            ClockNow => "clock_now",
            Time => "time",
            EventClock => "event_clock",
            WriteFile { .. } => "write_file",
            FsWriteFile { .. } => "fs_write_file",
            FsCreateDirAll { .. } => "fs_create_dir_all",
            FsRemoveFile { .. } => "fs_remove_file",
            FsRemoveDirAll { .. } => "fs_remove_dir_all",
            FsRename { .. } => "fs_rename",
            Stdout { .. } => "stdout",
            Stderr { .. } => "stderr",
            Exit { .. } => "exit",
            TimerStart { .. } => "timer_start",
            DnsResolve { .. } => "dns_resolve",
            TcpConnect { .. } => "tcp_connect",
            TcpListen { .. } => "tcp_listen",
            TcpAccept { .. } => "tcp_accept",
            TcpRead { .. } => "tcp_read",
            TcpWrite { .. } => "tcp_write",
            TcpClose { .. } => "tcp_close",
            AsyncComplete { .. } => "async_complete",
            TaskSpawn { .. } => "task_spawn",
            TaskStart { .. } => "task_start",
            TaskEnd { .. } => "task_end",
            TaskCancel { .. } => "task_cancel",
            TaskJoin { .. } => "task_join",
            TaskAwait { .. } => "task_await",
            Wake { .. } => "wake",
            Channel { .. } => "channel",
        }
    }

    /// Whether a recorded event of this kind must carry a result.
    fn expects_result(&self) -> bool {
        use EffectRequest::*;
        match self {
            TaskStart { .. } | TaskAwait { .. } | Channel { .. } => true,
            _ => matches!(
                self.class(),
                EffectClass::Input | EffectClass::Output | EffectClass::Completion
            ),
        }
    }
}

impl fmt::Display for EffectRequest {
    /// `read_file("in.txt")`, `stdout("hello")`: how a diagnostic names an
    /// effect.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use EffectRequest::*;
        let kind = self.kind_name();
        match self {
            ReadFile { path }
            | ReadLines { path }
            | TryReadFile { path }
            | FsExists { path }
            | FsIsDir { path }
            | FsIsFile { path }
            | FsListDir { path }
            | FsMetadata { path }
            | Sha256File { path }
            | FsCreateDirAll { path }
            | FsRemoveFile { path }
            | FsRemoveDirAll { path } => write!(f, "{kind}({})", quoted(path)),
            EnvVar { name } => write!(f, "{kind}({})", quoted(name)),
            ProcRun { cmd, argv } => {
                let args = argv.iter().map(|a| quoted(a)).collect::<Vec<_>>();
                write!(f, "{kind}({}, [{}])", quoted(cmd), args.join(", "))
            }
            WriteFile { path, contents } | FsWriteFile { path, contents } => {
                write!(f, "{kind}({}, {})", quoted(path), quoted(&contents.lossy()))
            }
            FsRename { from, to } => write!(f, "{kind}({}, {})", quoted(from), quoted(to)),
            Stdout { text } | Stderr { text } => write!(f, "{kind}({})", quoted(&text.lossy())),
            Exit { failed, .. } => {
                write!(f, "{}", if *failed { "a failed exit" } else { "a normal exit" })
            }
            ReadStdin | EnvArgs | EnvCwd | EnvHomeDir | ClockNow | Time | EventClock => {
                write!(f, "{kind}()")
            }
            TimerStart { req, ms } => write!(f, "{kind}(#{req}, {ms} ms)"),
            DnsResolve { req, host, port } => write!(f, "{kind}(#{req}, {}, {port})", quoted(host)),
            TcpConnect { req, addr } | TcpListen { req, addr } => {
                write!(f, "{kind}(#{req}, {})", quoted(addr))
            }
            TcpAccept { req, handle } => write!(f, "{kind}(#{req}, handle {handle})"),
            TcpRead { req, handle, max } => {
                write!(f, "{kind}(#{req}, handle {handle}, max {max})")
            }
            TcpWrite { req, handle, bytes } => {
                write!(f, "{kind}(#{req}, handle {handle}, {})", quoted(&bytes.lossy()))
            }
            TcpClose { handle } => write!(f, "{kind}(handle {handle})"),
            AsyncComplete { req } => write!(f, "the completion of async request #{req}"),
            TaskSpawn { id }
            | TaskStart { id }
            | TaskCancel { id }
            | TaskJoin { id }
            | TaskAwait { id } => {
                write!(f, "{kind}(task {id})")
            }
            TaskEnd { id, failed } => {
                write!(f, "{kind}(task {id}, {})", if *failed { "failed" } else { "done" })
            }
            Wake { waiter } => write!(f, "the hand-over fiber {waiter} waits for"),
            Channel { chan, op } => write!(f, "{kind}(#{chan}, {op})"),
        }
    }
}

/// A string for a diagnostic: quoted, escaped, and cut at 60 characters.
fn quoted(text: &str) -> String {
    const LIMIT: usize = 60;
    let mut shown: String = text.chars().take(LIMIT).collect();
    if text.chars().count() > LIMIT {
        shown.push('…');
    }
    format!("{shown:?}")
}

/// What an effect produced, in backend-neutral form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Unit,
    Bool(bool),
    Int(i64),
    Text(TapeBytes),
    /// A list of strings: `read_lines`, `list_dir`, `env_args`.
    List(Vec<TapeBytes>),
    /// `Some(text)` / `None`: `env_var`, `home_dir`.
    Some(TapeBytes),
    None,
    /// `Ok(..)` of a `Result<_, IoError>`.
    Ok(Box<Outcome>),
    /// `Err(IoError)` of a `Result<_, IoError>`.
    Err(TapeIoError),
    Meta(FsMeta),
    Proc(ProcOut),
    /// The effect failed with a runtime error rather than a value (on the VM a
    /// `read_file` error, on native a panic).
    Fail(String),
    /// Bytes read from a socket.
    Bytes(TapeBytes),
    /// Addresses a DNS lookup returned.
    Addrs(Vec<String>),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Unit => write!(f, "()"),
            Outcome::Bool(b) => write!(f, "{b}"),
            Outcome::Int(i) => write!(f, "{i}"),
            Outcome::Text(t) | Outcome::Some(t) => write!(f, "{}", quoted(&t.lossy())),
            Outcome::List(items) => write!(f, "a list of {} strings", items.len()),
            Outcome::None => write!(f, "None"),
            Outcome::Ok(inner) => write!(f, "Ok({inner})"),
            Outcome::Err(err) => write!(f, "Err({:?})", err.kind),
            Outcome::Meta(_) => write!(f, "file metadata"),
            Outcome::Proc(p) => write!(f, "process exit {}", p.status),
            Outcome::Fail(msg) => write!(f, "error {}", quoted(msg)),
            Outcome::Bytes(bytes) => write!(f, "{} bytes", bytes.0.len()),
            Outcome::Addrs(addrs) => write!(f, "{} addresses", addrs.len()),
        }
    }
}

/// `IoError` as recorded: its kind, message and path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeIoError {
    pub kind: IoKind,
    pub message: String,
    pub path: String,
}

/// The constructors of `Flow.IoError`'s kind, in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    Interrupted,
    Other,
}

impl IoKind {
    pub const ALL: [IoKind; 8] = [
        IoKind::NotFound,
        IoKind::PermissionDenied,
        IoKind::AlreadyExists,
        IoKind::NotADirectory,
        IoKind::IsADirectory,
        IoKind::DirectoryNotEmpty,
        IoKind::Interrupted,
        IoKind::Other,
    ];

    /// The constructor name in `Flow.IoError`.
    pub fn constructor(self) -> &'static str {
        match self {
            IoKind::NotFound => "NotFound",
            IoKind::PermissionDenied => "PermissionDenied",
            IoKind::AlreadyExists => "AlreadyExists",
            IoKind::NotADirectory => "NotADirectory",
            IoKind::IsADirectory => "IsADirectory",
            IoKind::DirectoryNotEmpty => "DirectoryNotEmpty",
            IoKind::Interrupted => "Interrupted",
            IoKind::Other => "Other",
        }
    }

    pub fn from_constructor(name: &str) -> IoKind {
        IoKind::ALL.into_iter().find(|kind| kind.constructor() == name).unwrap_or(IoKind::Other)
    }
}

/// `Flow.Fs.FileMeta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsMeta {
    pub size: i64,
    pub modified: i64,
    pub is_dir: bool,
    pub is_file: bool,
}

/// `Flow.Process.ProcOutput`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcOut {
    pub status: i64,
    pub stdout: TapeBytes,
    pub stderr: TapeBytes,
}

/// Bytes that are written as a JSON string when they are UTF-8, and as
/// `{"hex": "…"}` when they are not, so nothing is lost either way.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TapeBytes(pub Vec<u8>);

impl TapeBytes {
    pub fn lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl From<&str> for TapeBytes {
    fn from(text: &str) -> Self {
        TapeBytes(text.as_bytes().to_vec())
    }
}

impl From<String> for TapeBytes {
    fn from(text: String) -> Self {
        TapeBytes(text.into_bytes())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum BytesRepr {
    Text(String),
    Hex { hex: String },
}

impl Serialize for TapeBytes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match std::str::from_utf8(&self.0) {
            Ok(text) => BytesRepr::Text(text.to_string()).serialize(serializer),
            Err(_) => {
                BytesRepr::Hex { hex: crate::shared::hex::encode(&self.0) }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for TapeBytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match BytesRepr::deserialize(deserializer)? {
            BytesRepr::Text(text) => Ok(TapeBytes(text.into_bytes())),
            BytesRepr::Hex { hex } => decode_hex(&hex)
                .map(TapeBytes)
                .ok_or_else(|| serde::de::Error::custom("invalid hex in tape bytes")),
        }
    }
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// A tape read from disk and checked, ready to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTape {
    pub path: String,
    pub header: TapeHeader,
    pub events: Vec<TapeEvent>,
}

impl LoadedTape {
    /// Reads and checks a whole tape: the header, that events are numbered
    /// from 0 without gaps, and that each input or output carries a result.
    /// A tape that fails any check is rejected before the program starts.
    pub fn load(path: &Path) -> Result<LoadedTape, TapeError> {
        let shown = path.display().to_string();
        let bad = |reason: String| TapeError::BadTape { path: shown.clone(), reason };
        let file =
            std::fs::File::open(path).map_err(|err| bad(format!("cannot read it ({err})")))?;
        let mut lines = BufReader::new(file).lines();

        let header_line = lines
            .next()
            .ok_or_else(|| bad("it is empty".to_string()))?
            .map_err(|err| bad(format!("cannot read it ({err})")))?;
        let header: TapeHeader = serde_json::from_str(&header_line)
            .map_err(|_| bad("it is not a Flux tape".to_string()))?;
        if header.magic != TAPE_MAGIC {
            return Err(bad("it is not a Flux tape".to_string()));
        }
        if header.version != TAPE_VERSION {
            return Err(bad(format!(
                "it uses tape format version {}, and this flux reads version {TAPE_VERSION}",
                header.version
            )));
        }

        let mut events = Vec::new();
        for (index, line) in lines.enumerate() {
            let line_number = index + 2;
            let line = line.map_err(|err| bad(format!("cannot read it ({err})")))?;
            if line.trim().is_empty() {
                continue;
            }
            let event: TapeEvent = serde_json::from_str(&line)
                .map_err(|err| bad(format!("line {line_number} is malformed ({err})")))?;
            if event.seq != events.len() as u64 {
                return Err(bad(format!(
                    "line {line_number} is event #{}, expected #{}",
                    event.seq,
                    events.len()
                )));
            }
            if event.request.expects_result() != event.result.is_some() {
                return Err(bad(format!(
                    "line {line_number} ({}) {} a result",
                    event.request.kind_name(),
                    if event.result.is_some() { "should not have" } else { "is missing" }
                )));
            }
            events.push(event);
        }
        Ok(LoadedTape { path: shown, header, events })
    }

    /// True when the recorded run did not reach its end, for example because
    /// it crashed; replaying it stops with "tape exhausted".
    pub fn is_truncated(&self) -> bool {
        !matches!(self.events.last().map(|event| &event.request), Some(EffectRequest::Exit { .. }))
    }

    /// Checks that this tape can be replayed by the program `program_hash`
    /// names: the same checks whether the driver or a native program opens
    /// it, on either backend, async code included (0188 S9d). An empty
    /// `program_hash` means the program does not know its own, and skips
    /// that check. Returns the warnings to show, each without its
    /// `warning: ` prefix.
    pub fn check_for(&self, program_hash: &str) -> Result<Vec<String>, TapeError> {
        if !program_hash.is_empty() && self.header.program_hash != program_hash {
            return Err(TapeError::BadTape {
                path: self.path.clone(),
                reason: "it was recorded from a different program, or from this one \
                         before it was changed"
                    .to_string(),
            });
        }
        let mut warnings = Vec::new();
        if self.header.flux_version != env!("CARGO_PKG_VERSION") {
            warnings.push(format!(
                "tape `{}` was recorded by flux {}; this is flux {}.",
                self.path,
                self.header.flux_version,
                env!("CARGO_PKG_VERSION")
            ));
        }
        if self.is_truncated() {
            warnings.push(format!(
                "tape `{}` is incomplete: the recorded run did not finish.",
                self.path
            ));
        }
        Ok(warnings)
    }
}
