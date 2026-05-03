//! `mio`-backed async reactor (proposal 0174 Phase 1a).
//!
//! Reactor lifecycle (1a-i):
//!   - `start()` spawns a dedicated reactor thread that owns a `mio::Poll`.
//!   - `shutdown()` flips a flag, wakes the reactor, and joins.
//!   - `Drop` calls `shutdown` so a forgotten teardown can't leak the thread.
//!
//! Timer service (1a-ii):
//!   - `timer_start(req, ms)` pushes a deadline into a shared min-heap and
//!     wakes the reactor so it can recompute its next `poll` timeout.
//!   - Expired entries push a `CompletionPayload::Unit` into the shared
//!     completions queue.
//!   - `cancel(req)` records the request as cancelled; the reactor drops
//!     fires and already-queued completions for it.
//!
//! TCP readiness state machines (1a-vii):
//!   - The owning thread submits TCP intent through a command queue:
//!     `tcp_connect` / `tcp_read` / `tcp_write` / `tcp_close`.
//!   - The reactor drains the queue each iteration, registers/updates poll
//!     interests, and tracks a per-handle [`TcpConnState`] holding the
//!     pending connect/read/write requests for that connection.
//!   - On a readable event with a pending read, the reactor reads up to
//!     `max` bytes (handling `WouldBlock` as "no data this iteration") and
//!     pushes `CompletionPayload::Bytes(buf)` (empty buffer = EOF).
//!   - On a writable event with a pending write, the reactor writes as
//!     much as it can; when the entire buffer has been accepted by the OS
//!     it pushes `CompletionPayload::Unit`. Partial writes stay parked
//!     under the same `RequestId`.
//!   - For a pending connect the reactor uses the writable event plus
//!     `take_error()` to decide success/failure.
//!
//! Threading model:
//!   - One reactor thread per [`MioBackend`]; it is the only thread that
//!     touches `Poll`, the timer heap, or the per-connection state.
//!   - The owning thread shares the timer heap, completions queue, cancel
//!     set, command queue, a `mio::Waker`, and an `AtomicBool` shutdown
//!     flag with the reactor.
//!   - `Waker::wake` is the one and only mechanism used to nudge the
//!     reactor out of a blocking `poll` — used for shutdown, new timers,
//!     and new TCP commands.

use super::super::backend::{AsyncBackend, Completion, CompletionPayload, IoHandle, RequestId};
use mio::net::TcpStream as MioTcpStream;
use mio::{Events, Interest, Poll, Token, Waker};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Token reserved for the cross-thread `Waker`. Phase 1a-vii allocates per-
/// TCP-source tokens above this; the registry will own that allocation.
const WAKER_TOKEN: Token = Token(0);

/// One scheduled timer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TimerEntry {
    deadline: Instant,
    request_id: RequestId,
}

// `BinaryHeap` is a max-heap; reverse the comparison so the soonest deadline
// is at the top. Ties broken by request_id for stable ordering.
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.request_id.cmp(&self.request_id))
    }
}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

/// Owning-thread → reactor command. The reactor drains these once per
/// iteration so all `mio::Poll` access stays single-threaded.
#[derive(Debug)]
enum TcpCommand {
    Connect {
        req: RequestId,
        handle: IoHandle,
        addr: SocketAddr,
    },
    Read {
        req: RequestId,
        handle: IoHandle,
        max: usize,
    },
    Write {
        req: RequestId,
        handle: IoHandle,
        bytes: Vec<u8>,
    },
    Close {
        handle: IoHandle,
    },
}

/// Shared state between the owning thread and the reactor thread.
#[derive(Debug)]
struct ReactorShared {
    waker: Waker,
    shutdown: AtomicBool,
    timers: Mutex<BinaryHeap<TimerEntry>>,
    cancelled: Mutex<HashSet<RequestId>>,
    completions: Mutex<VecDeque<Completion>>,
    tcp_commands: Mutex<VecDeque<TcpCommand>>,
    next_handle: AtomicU64,
}

/// `mio`-backed async backend.
#[derive(Debug, Default)]
pub struct MioBackend {
    inner: Mutex<Option<Inner>>,
}

#[derive(Debug)]
struct Inner {
    shared: Arc<ReactorShared>,
    handle: Option<JoinHandle<()>>,
}

impl MioBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn start_inner(&self) -> io::Result<()> {
        let mut slot = self.inner.lock().expect("MioBackend mutex poisoned");
        if slot.is_some() {
            return Ok(());
        }

        let poll = Poll::new()?;
        let waker = Waker::new(poll.registry(), WAKER_TOKEN)?;
        let shared = Arc::new(ReactorShared {
            waker,
            shutdown: AtomicBool::new(false),
            timers: Mutex::new(BinaryHeap::new()),
            cancelled: Mutex::new(HashSet::new()),
            completions: Mutex::new(VecDeque::new()),
            tcp_commands: Mutex::new(VecDeque::new()),
            next_handle: AtomicU64::new(1),
        });

        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("flux-mio-reactor".into())
            .spawn(move || run_reactor(poll, thread_shared))
            .map_err(io::Error::other)?;

        *slot = Some(Inner {
            shared,
            handle: Some(handle),
        });
        Ok(())
    }

    fn shutdown_inner(&self) -> io::Result<()> {
        let mut inner = match self.inner.lock().expect("MioBackend mutex poisoned").take() {
            Some(inner) => inner,
            None => return Ok(()),
        };

        inner.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = inner.shared.waker.wake();

        if let Some(handle) = inner.handle.take()
            && let Err(e) = handle.join()
        {
            return Err(io::Error::other(format!(
                "mio reactor thread panicked: {e:?}"
            )));
        }
        Ok(())
    }

    /// Run `f` against the live shared state, or no-op if the backend is
    /// stopped. Avoids exposing `Inner`/`Arc` to the trait impls.
    fn with_shared<R>(&self, f: impl FnOnce(&Arc<ReactorShared>) -> R) -> Option<R> {
        self.inner
            .lock()
            .expect("MioBackend mutex poisoned")
            .as_ref()
            .map(|inner| f(&inner.shared))
    }

    /// True when the reactor thread is currently running. Test-only helper.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.inner
            .lock()
            .expect("MioBackend mutex poisoned")
            .is_some()
    }
}

impl AsyncBackend for MioBackend {
    fn start(&self) -> Result<(), String> {
        self.start_inner().map_err(|e| e.to_string())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.shutdown_inner().map_err(|e| e.to_string())
    }

    fn cancel(&self, req: RequestId) {
        // Record the cancellation so the reactor skips firing it. Also drop
        // any already-queued completion for the request — once the registry
        // has marked the entry cancelled, the synthetic-cancelled-error path
        // owns delivery, not the original timer fire.
        if let Some(()) = self.with_shared(|shared| {
            shared
                .cancelled
                .lock()
                .expect("cancelled-set poisoned")
                .insert(req);
            shared
                .completions
                .lock()
                .expect("completions poisoned")
                .retain(|c| c.request_id != req);
            // Wake the reactor so it can prune cancelled head-of-heap entries
            // promptly rather than waiting on its next deadline.
            let _ = shared.waker.wake();
        }) {
            // ran
        }
    }

    fn timer_start(&self, req: RequestId, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        if let Some(()) = self.with_shared(|shared| {
            shared
                .timers
                .lock()
                .expect("timer-heap poisoned")
                .push(TimerEntry {
                    deadline,
                    request_id: req,
                });
            // Wake the reactor so it recomputes its next poll timeout against
            // the new deadline.
            let _ = shared.waker.wake();
        }) {
            // ran
        }
    }

    fn next_completion(&self) -> Option<Completion> {
        self.with_shared(|shared| {
            shared
                .completions
                .lock()
                .expect("completions poisoned")
                .pop_front()
        })
        .flatten()
    }

    fn tcp_connect(&self, req: RequestId, addr: SocketAddr) {
        let _ = self.with_shared(|shared| {
            let handle = IoHandle(shared.next_handle.fetch_add(1, Ordering::Relaxed));
            shared
                .tcp_commands
                .lock()
                .expect("tcp commands poisoned")
                .push_back(TcpCommand::Connect { req, handle, addr });
            let _ = shared.waker.wake();
        });
    }

    fn tcp_read(&self, req: RequestId, handle: IoHandle, max: usize) {
        let _ = self.with_shared(|shared| {
            shared
                .tcp_commands
                .lock()
                .expect("tcp commands poisoned")
                .push_back(TcpCommand::Read { req, handle, max });
            let _ = shared.waker.wake();
        });
    }

    fn tcp_write(&self, req: RequestId, handle: IoHandle, bytes: Vec<u8>) {
        let _ = self.with_shared(|shared| {
            shared
                .tcp_commands
                .lock()
                .expect("tcp commands poisoned")
                .push_back(TcpCommand::Write { req, handle, bytes });
            let _ = shared.waker.wake();
        });
    }

    fn tcp_close(&self, handle: IoHandle) {
        let _ = self.with_shared(|shared| {
            shared
                .tcp_commands
                .lock()
                .expect("tcp commands poisoned")
                .push_back(TcpCommand::Close { handle });
            let _ = shared.waker.wake();
        });
    }
}

impl Drop for MioBackend {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

/// One pending read against a TCP handle.
#[derive(Debug)]
struct PendingRead {
    request_id: RequestId,
    max: usize,
}

/// One pending write against a TCP handle. `written` tracks how many bytes
/// the OS has already accepted; partial writes resume from this offset on
/// the next writable event.
#[derive(Debug)]
struct PendingWrite {
    request_id: RequestId,
    bytes: Vec<u8>,
    written: usize,
}

/// Per-connection reactor-thread state. Lives in the reactor's local
/// `HashMap<IoHandle, TcpConnState>`; never touched by the owning thread.
struct TcpConnState {
    stream: MioTcpStream,
    token: Token,
    pending_connect: Option<RequestId>,
    pending_read: Option<PendingRead>,
    pending_write: Option<PendingWrite>,
}

/// Reactor thread entry point.
fn run_reactor(mut poll: Poll, shared: Arc<ReactorShared>) {
    let mut events = Events::with_capacity(64);
    let mut conns: HashMap<IoHandle, TcpConnState> = HashMap::new();
    let mut handles_by_token: HashMap<Token, IoHandle> = HashMap::new();
    // Token(0) is reserved for the Waker; allocate connection tokens above.
    let mut next_token: usize = 1;

    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }

        drain_tcp_commands(
            &shared,
            &mut conns,
            &mut handles_by_token,
            &mut next_token,
            poll.registry(),
        );

        let timeout = next_live_deadline(&shared).map(|d| {
            let now = Instant::now();
            d.checked_duration_since(now).unwrap_or(Duration::ZERO)
        });

        match poll.poll(&mut events, timeout) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("flux mio reactor: poll failed: {e}");
                return;
            }
        }

        for event in events.iter() {
            if event.token() == WAKER_TOKEN {
                continue;
            }
            let Some(&handle) = handles_by_token.get(&event.token()) else {
                continue;
            };
            handle_tcp_event(
                &shared,
                &mut conns,
                &mut handles_by_token,
                handle,
                event.is_readable(),
                event.is_writable(),
                poll.registry(),
            );
        }

        fire_due_timers(&shared);
    }
}

fn drain_tcp_commands(
    shared: &ReactorShared,
    conns: &mut HashMap<IoHandle, TcpConnState>,
    handles_by_token: &mut HashMap<Token, IoHandle>,
    next_token: &mut usize,
    registry: &mio::Registry,
) {
    let commands: Vec<TcpCommand> = {
        let mut q = shared.tcp_commands.lock().expect("tcp commands poisoned");
        q.drain(..).collect()
    };
    for cmd in commands {
        match cmd {
            TcpCommand::Connect { req, handle, addr } => {
                let token = Token(*next_token);
                *next_token += 1;
                match MioTcpStream::connect(addr) {
                    Ok(mut stream) => {
                        if let Err(e) = registry.register(
                            &mut stream,
                            token,
                            Interest::READABLE | Interest::WRITABLE,
                        ) {
                            push_completion(
                                shared,
                                req,
                                CompletionPayload::Error(format!("register failed: {e}")),
                            );
                            continue;
                        }
                        conns.insert(
                            handle,
                            TcpConnState {
                                stream,
                                token,
                                pending_connect: Some(req),
                                pending_read: None,
                                pending_write: None,
                            },
                        );
                        handles_by_token.insert(token, handle);
                    }
                    Err(e) => {
                        push_completion(
                            shared,
                            req,
                            CompletionPayload::Error(format!("connect failed: {e}")),
                        );
                    }
                }
            }
            TcpCommand::Read { req, handle, max } => {
                let Some(c) = conns.get_mut(&handle) else {
                    push_completion(
                        shared,
                        req,
                        CompletionPayload::Error("read on unknown handle".into()),
                    );
                    continue;
                };
                c.pending_read = Some(PendingRead {
                    request_id: req,
                    max,
                });
                // The connection is registered for READABLE | WRITABLE
                // already; we'll service the read on the next readable
                // event (or attempt one immediately for already-buffered
                // data).
                try_progress_read(shared, c);
            }
            TcpCommand::Write { req, handle, bytes } => {
                let Some(c) = conns.get_mut(&handle) else {
                    push_completion(
                        shared,
                        req,
                        CompletionPayload::Error("write on unknown handle".into()),
                    );
                    continue;
                };
                c.pending_write = Some(PendingWrite {
                    request_id: req,
                    bytes,
                    written: 0,
                });
                try_progress_write(shared, c);
            }
            TcpCommand::Close { handle } => {
                if let Some(mut c) = conns.remove(&handle) {
                    handles_by_token.remove(&c.token);
                    // Best-effort deregister; an already-broken socket may
                    // fail this and there's nothing useful to do about it.
                    let _ = registry.deregister(&mut c.stream);
                    // Pending requests against a closed handle are dropped
                    // silently per the docs on AsyncBackend::tcp_close.
                }
            }
        }
    }
}

fn handle_tcp_event(
    shared: &ReactorShared,
    conns: &mut HashMap<IoHandle, TcpConnState>,
    handles_by_token: &mut HashMap<Token, IoHandle>,
    handle: IoHandle,
    is_readable: bool,
    is_writable: bool,
    registry: &mio::Registry,
) {
    let Some(c) = conns.get_mut(&handle) else {
        return;
    };

    // Connect resolution: a pending connect resolves on writable, success
    // iff `take_error()` returns None.
    if c.pending_connect.is_some() && is_writable {
        let req = c.pending_connect.take().expect("guarded above");
        match c.stream.take_error() {
            Ok(None) => {
                push_completion(shared, req, CompletionPayload::TcpHandle(handle));
            }
            Ok(Some(e)) | Err(e) => {
                push_completion(
                    shared,
                    req,
                    CompletionPayload::Error(format!("connect failed: {e}")),
                );
                handles_by_token.remove(&c.token);
                let mut to_remove = conns.remove(&handle).expect("entry was just borrowed");
                let _ = registry.deregister(&mut to_remove.stream);
                return;
            }
        }
    }

    if is_readable {
        try_progress_read(shared, c);
    }
    if is_writable {
        try_progress_write(shared, c);
    }
}

fn try_progress_read(shared: &ReactorShared, c: &mut TcpConnState) {
    let Some(pending) = c.pending_read.as_mut() else {
        return;
    };
    let mut buf = vec![0u8; pending.max];
    match c.stream.read(&mut buf) {
        Ok(n) => {
            buf.truncate(n);
            let req = pending.request_id;
            c.pending_read = None;
            push_completion(shared, req, CompletionPayload::Bytes(buf));
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // Wait for the next readable event.
        }
        Err(e) => {
            let req = pending.request_id;
            c.pending_read = None;
            push_completion(
                shared,
                req,
                CompletionPayload::Error(format!("read failed: {e}")),
            );
        }
    }
}

fn try_progress_write(shared: &ReactorShared, c: &mut TcpConnState) {
    let Some(pending) = c.pending_write.as_mut() else {
        return;
    };
    loop {
        let to_write = &pending.bytes[pending.written..];
        if to_write.is_empty() {
            let req = pending.request_id;
            c.pending_write = None;
            push_completion(shared, req, CompletionPayload::Unit);
            return;
        }
        match c.stream.write(to_write) {
            Ok(0) => {
                // Some platforms surface a closed connection as Ok(0). Treat
                // as an error so the request doesn't hang forever.
                let req = pending.request_id;
                c.pending_write = None;
                push_completion(
                    shared,
                    req,
                    CompletionPayload::Error("write returned 0".into()),
                );
                return;
            }
            Ok(n) => {
                pending.written += n;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return;
            }
            Err(e) => {
                let req = pending.request_id;
                c.pending_write = None;
                push_completion(
                    shared,
                    req,
                    CompletionPayload::Error(format!("write failed: {e}")),
                );
                return;
            }
        }
    }
}

fn push_completion(shared: &ReactorShared, req: RequestId, payload: CompletionPayload) {
    shared
        .completions
        .lock()
        .expect("completions poisoned")
        .push_back(Completion {
            request_id: req,
            payload,
        });
}

/// Pop expired and cancelled entries off the front of the heap, returning
/// the soonest live deadline (or `None` if the heap is empty).
fn next_live_deadline(shared: &ReactorShared) -> Option<Instant> {
    let mut timers = shared.timers.lock().expect("timer-heap poisoned");
    let cancelled = shared.cancelled.lock().expect("cancelled-set poisoned");
    while let Some(top) = timers.peek() {
        if cancelled.contains(&top.request_id) {
            timers.pop();
            continue;
        }
        return Some(top.deadline);
    }
    None
}

/// Pop all entries with `deadline <= now` and produce completions for the
/// ones whose request was not cancelled.
fn fire_due_timers(shared: &ReactorShared) {
    let now = Instant::now();
    let mut timers = shared.timers.lock().expect("timer-heap poisoned");
    let mut cancelled = shared.cancelled.lock().expect("cancelled-set poisoned");
    let mut completions = shared.completions.lock().expect("completions poisoned");

    while let Some(top) = timers.peek() {
        if top.deadline > now {
            break;
        }
        let entry = timers.pop().expect("peek matched");
        if cancelled.remove(&entry.request_id) {
            // Cancelled before fire — drop without producing a completion.
            continue;
        }
        completions.push_back(Completion {
            request_id: entry.request_id,
            payload: CompletionPayload::Unit,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Spin-poll `next_completion` for up to `timeout`. Returns the
    /// completion or `None` if the timeout elapses. Used because the
    /// scheduler/worker pool does not exist yet — owners poll today.
    fn wait_for(backend: &MioBackend, timeout: Duration) -> Option<Completion> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(c) = backend.next_completion() {
                return Some(c);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn start_then_shutdown_joins_cleanly() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        assert!(backend.is_running());
        backend.shutdown().unwrap();
        assert!(!backend.is_running());
    }

    #[test]
    fn start_is_idempotent() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        backend.start().unwrap();
        backend.start().unwrap();
        assert!(backend.is_running());
        backend.shutdown().unwrap();
    }

    #[test]
    fn shutdown_is_idempotent() {
        let backend = MioBackend::new();
        backend.shutdown().unwrap(); // before any start
        backend.start().unwrap();
        backend.shutdown().unwrap();
        backend.shutdown().unwrap(); // after shutdown
        assert!(!backend.is_running());
    }

    #[test]
    fn drop_shuts_down_reactor_thread() {
        let start = Instant::now();
        {
            let backend = MioBackend::new();
            backend.start().unwrap();
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Drop must join the reactor thread without blocking the test"
        );
    }

    #[test]
    fn many_start_shutdown_cycles_do_not_leak() {
        let backend = MioBackend::new();
        for _ in 0..8 {
            backend.start().unwrap();
            backend.shutdown().unwrap();
        }
        assert!(!backend.is_running());
    }

    #[test]
    fn timer_fires_after_delay() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        let scheduled = Instant::now();
        backend.timer_start(RequestId(1), 30);

        let completion =
            wait_for(&backend, Duration::from_secs(2)).expect("timer must fire within 2s");
        assert_eq!(completion.request_id, RequestId(1));
        assert_eq!(completion.payload, CompletionPayload::Unit);
        // Sanity: the timer didn't fire instantly. Allow generous slack for
        // CI scheduling jitter — we mostly want to catch a "fire immediately"
        // bug, not enforce tight timing.
        assert!(
            scheduled.elapsed() >= Duration::from_millis(20),
            "timer fired suspiciously early: {:?}",
            scheduled.elapsed()
        );
        backend.shutdown().unwrap();
    }

    #[test]
    fn two_timers_fire_in_deadline_order() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        // Schedule the longer timer first, the shorter one second. The short
        // one must fire first regardless of registration order.
        backend.timer_start(RequestId(2), 120);
        backend.timer_start(RequestId(1), 30);

        let first =
            wait_for(&backend, Duration::from_secs(2)).expect("first timer fires within 2s");
        let second =
            wait_for(&backend, Duration::from_secs(2)).expect("second timer fires within 2s");

        assert_eq!(first.request_id, RequestId(1));
        assert_eq!(second.request_id, RequestId(2));
        backend.shutdown().unwrap();
    }

    #[test]
    fn cancel_before_fire_suppresses_completion() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        backend.timer_start(RequestId(1), 200);
        backend.timer_start(RequestId(2), 30);

        // Cancel the long timer before it could fire.
        backend.cancel(RequestId(1));

        // The short timer fires.
        let c = wait_for(&backend, Duration::from_secs(2)).expect("short timer fires within 2s");
        assert_eq!(c.request_id, RequestId(2));

        // No further completion should arrive within a reasonable grace period.
        let extra = wait_for(&backend, Duration::from_millis(400));
        assert!(
            extra.is_none(),
            "cancelled timer must not produce a completion: got {extra:?}"
        );
        backend.shutdown().unwrap();
    }

    #[test]
    fn tcp_loopback_connect_write_read_roundtrip() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        // Bind a synchronous loopback listener and accept on a thread that
        // echoes the first message back. The mio backend drives the client.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf).unwrap();
            conn.write_all(&buf[..n]).unwrap();
            // Linger briefly so the client's read sees the bytes before we
            // drop the socket on platforms that surface RST aggressively.
            thread::sleep(Duration::from_millis(20));
        });

        let backend = MioBackend::new();
        backend.start().unwrap();

        // Connect.
        let req_connect = RequestId(101);
        backend.tcp_connect(req_connect, addr);
        let connected = wait_for(&backend, Duration::from_secs(5)).expect("connect must complete");
        let handle = match connected.payload {
            CompletionPayload::TcpHandle(h) => h,
            other => panic!("expected TcpHandle, got {other:?}"),
        };
        assert_eq!(connected.request_id, req_connect);

        // Write.
        let req_write = RequestId(102);
        let payload = b"hello-flux".to_vec();
        backend.tcp_write(req_write, handle, payload.clone());
        let written = wait_for(&backend, Duration::from_secs(5)).expect("write must complete");
        assert_eq!(written.request_id, req_write);
        assert_eq!(written.payload, CompletionPayload::Unit);

        // Read echo.
        let req_read = RequestId(103);
        backend.tcp_read(req_read, handle, 64);
        let read = wait_for(&backend, Duration::from_secs(5)).expect("read must complete");
        assert_eq!(read.request_id, req_read);
        assert_eq!(read.payload, CompletionPayload::Bytes(payload));

        backend.tcp_close(handle);
        backend.shutdown().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn tcp_connect_to_closed_port_reports_error() {
        use std::net::TcpListener;
        // Bind then drop, leaving the address unbound and refusing
        // subsequent connects. (Linux may be slow to reject; we give a
        // generous timeout.)
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let backend = MioBackend::new();
        backend.start().unwrap();
        backend.tcp_connect(RequestId(7), addr);
        let completion =
            wait_for(&backend, Duration::from_secs(5)).expect("connect must report something");
        assert_eq!(completion.request_id, RequestId(7));
        match completion.payload {
            CompletionPayload::Error(_) => {}
            other => panic!("expected Error, got {other:?}"),
        }
        backend.shutdown().unwrap();
    }

    #[test]
    fn shutdown_with_pending_timers_does_not_hang() {
        let backend = MioBackend::new();
        backend.start().unwrap();
        // Schedule a timer well beyond the test budget.
        backend.timer_start(RequestId(1), 60_000);

        let start = Instant::now();
        backend.shutdown().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "shutdown must wake the reactor out of its deadline-sized poll"
        );
        assert!(!backend.is_running());
    }
}
