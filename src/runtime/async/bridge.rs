//! Native/LLVM entry stubs for the Rust-owned async runtime.
//!
//! These functions keep native/LLVM integration behind opaque Rust-owned
//! handles. Native code can enter/leave an async context, poll a runtime, and
//! drive blocking-style awaits (sleep, etc.) without learning about the
//! scheduler, backend, or `mio` internals.
//!
//! Each Rust thread that calls into the bridge gets its own thread-local
//! runtime backed by a real `mio` reactor running on a sibling thread. That
//! mirrors the VM's setup (see `src/vm/mod.rs`) so native code reaches the
//! same scheduler architecture as the VM.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    slice,
    time::Duration,
};

use super::{
    backend::{IoHandle, RequestId, RuntimeTarget},
    runtime::AsyncRuntime,
    scheduler::SchedulerConfig,
};

#[cfg(feature = "async-mio")]
use super::backend::CompletionPayload;

#[cfg(feature = "async-mio")]
use super::backends::mio::{
    MioBackend, MioBackendHandle, MioDriverBackend, MioReactorRunLimit, MioReactorRunReport,
    spawn_mio_reactor_until_stopped,
};

use crate::runtime::value::Value;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FluxAsyncRuntimeHandle {
    raw: u64,
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FluxAsyncContextHandle {
    raw: u64,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluxAsyncStatus {
    Ok = 0,
    InvalidHandle = 1,
    Unsupported = 2,
    RuntimeError = 3,
    Cancelled = 4,
}

impl FluxAsyncRuntimeHandle {
    pub const fn null() -> Self {
        Self { raw: 0 }
    }

    pub const fn raw(self) -> u64 {
        self.raw
    }
}

impl FluxAsyncContextHandle {
    pub const fn null() -> Self {
        Self { raw: 0 }
    }

    pub const fn raw(self) -> u64 {
        self.raw
    }
}

#[cfg(feature = "async-mio")]
struct BridgeRuntime {
    runtime: AsyncRuntime<MioDriverBackend>,
    reactor_handle: MioBackendHandle,
    reactor_thread: Option<std::thread::JoinHandle<Result<MioReactorRunReport, super::backend::AsyncError>>>,
}

#[cfg(feature = "async-mio")]
impl BridgeRuntime {
    fn new() -> Self {
        let backend =
            MioBackend::new().expect("native bridge mio backend initialization cannot fail");
        let driver_backend = backend.driver_backend();
        let reactor_handle = backend.handle();
        let reactor_thread = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: usize::MAX,
                timeout: Some(Duration::from_millis(10)),
            },
        );
        let runtime = AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, driver_backend);
        Self {
            runtime,
            reactor_handle,
            reactor_thread: Some(reactor_thread),
        }
    }
}

#[cfg(feature = "async-mio")]
impl Drop for BridgeRuntime {
    fn drop(&mut self) {
        let _ = self.reactor_handle.stop();
        if let Some(thread) = self.reactor_thread.take() {
            let _ = thread.join();
        }
    }
}

// Without the `async-mio` feature there is no real reactor, so the bridge
// runtime just owns a no-op runtime. The blocking shims will report
// `Unsupported`.
#[cfg(not(feature = "async-mio"))]
struct BridgeRuntime {
    _phantom: (),
}

#[cfg(not(feature = "async-mio"))]
impl BridgeRuntime {
    fn new() -> Self {
        Self { _phantom: () }
    }
}

thread_local! {
    static ACTIVE_CONTEXT: Cell<FluxAsyncContextHandle> =
        const { Cell::new(FluxAsyncContextHandle::null()) };
    static DEFAULT_RUNTIME: Cell<FluxAsyncRuntimeHandle> =
        const { Cell::new(FluxAsyncRuntimeHandle::null()) };
    static NEXT_RUNTIME_HANDLE: Cell<u64> = const { Cell::new(1) };
    static RUNTIMES: RefCell<HashMap<u64, BridgeRuntime>> =
        RefCell::new(HashMap::new());
}

fn allocate_runtime_handle() -> FluxAsyncRuntimeHandle {
    NEXT_RUNTIME_HANDLE.with(|next| {
        let raw = next.get();
        next.set(raw.wrapping_add(1).max(1));
        FluxAsyncRuntimeHandle { raw }
    })
}

fn create_runtime() -> FluxAsyncRuntimeHandle {
    let handle = allocate_runtime_handle();
    RUNTIMES.with(|runtimes| {
        runtimes.borrow_mut().insert(handle.raw(), BridgeRuntime::new());
    });
    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_default() -> FluxAsyncRuntimeHandle {
    DEFAULT_RUNTIME.with(|default| {
        let existing = default.get();
        if existing.raw() != 0 {
            return existing;
        }
        let handle = create_runtime();
        default.set(handle);
        handle
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_create() -> FluxAsyncRuntimeHandle {
    create_runtime()
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_destroy(runtime: FluxAsyncRuntimeHandle) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    let removed = RUNTIMES.with(|runtimes| runtimes.borrow_mut().remove(&runtime.raw()).is_some());
    if !removed {
        return FluxAsyncStatus::InvalidHandle;
    }
    DEFAULT_RUNTIME.with(|default| {
        if default.get() == runtime {
            default.set(FluxAsyncRuntimeHandle::null());
        }
    });
    FluxAsyncStatus::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_context_current() -> FluxAsyncContextHandle {
    ACTIVE_CONTEXT.with(Cell::get)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_context_enter(
    context: FluxAsyncContextHandle,
) -> FluxAsyncContextHandle {
    ACTIVE_CONTEXT.with(|active| {
        let previous = active.get();
        active.set(context);
        previous
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_context_leave(previous: FluxAsyncContextHandle) -> FluxAsyncStatus {
    ACTIVE_CONTEXT.with(|active| active.set(previous));
    FluxAsyncStatus::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_poll(runtime: FluxAsyncRuntimeHandle) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        RUNTIMES.with(|runtimes| {
            let mut runtimes = runtimes.borrow_mut();
            let Some(bridge) = runtimes.get_mut(&runtime.raw()) else {
                return FluxAsyncStatus::InvalidHandle;
            };
            match bridge.runtime.poll() {
                Ok(_) => FluxAsyncStatus::Ok,
                Err(_) => FluxAsyncStatus::RuntimeError,
            }
        })
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = runtime;
        FluxAsyncStatus::Unsupported
    }
}

/// Block the calling thread until the reactor reports the timer for `ms`
/// milliseconds has fired. Designed for the native (LLVM) backend, which is
/// synchronous and runs on a real OS thread — the thread parks on the mio
/// reactor's completion channel rather than spinning a `usleep` loop.
///
/// Negative or zero `ms` returns `Ok` immediately. The runtime handle must
/// have been obtained from `flux_async_runtime_default` /
/// `flux_async_runtime_create`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_sleep_blocking(
    runtime: FluxAsyncRuntimeHandle,
    ms: i64,
) -> FluxAsyncStatus {
    if ms <= 0 {
        return FluxAsyncStatus::Ok;
    }
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let duration = Duration::from_millis(ms as u64);
        run_blocking_sleep(runtime, duration)
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = ms;
        FluxAsyncStatus::Unsupported
    }
}

#[cfg(feature = "async-mio")]
fn run_blocking_sleep(
    runtime: FluxAsyncRuntimeHandle,
    duration: Duration,
) -> FluxAsyncStatus {
    match run_blocking::<_, ()>(runtime, |bridge, target| {
        bridge
            .runtime
            .start_timer(target, Value::None, duration)
            .ok()
    }, |_| Some(())) {
        BlockingOutcome::Ok(_) => FluxAsyncStatus::Ok,
        BlockingOutcome::InvalidHandle => FluxAsyncStatus::InvalidHandle,
        BlockingOutcome::Cancelled => FluxAsyncStatus::Cancelled,
        BlockingOutcome::RuntimeError(_) => FluxAsyncStatus::RuntimeError,
    }
}

#[cfg(feature = "async-mio")]
enum BlockingOutcome<T> {
    Ok(T),
    InvalidHandle,
    Cancelled,
    /// Backend or scheduler error; carries an optional message from the
    /// completion's `AsyncError` if the failure surfaced from the reactor.
    RuntimeError(Option<String>),
}

/// Spawn a one-shot task on the runtime, invoke `start` to register a backend
/// wait, then drive the poll loop until the wait completes. `extract` decodes
/// the payload into the caller's expected return shape.
///
/// Returns `BlockingOutcome` so caller-specific FFI shims can map runtime
/// errors to status codes and copy out payload data without holding the
/// `RUNTIMES` borrow.
#[cfg(feature = "async-mio")]
fn run_blocking<F, T>(
    runtime: FluxAsyncRuntimeHandle,
    start: F,
    extract: impl FnOnce(CompletionPayload) -> Option<T>,
) -> BlockingOutcome<T>
where
    F: FnOnce(&mut BridgeRuntime, RuntimeTarget) -> Option<RequestId>,
{
    RUNTIMES.with(|runtimes| {
        let mut runtimes = runtimes.borrow_mut();
        let Some(bridge) = runtimes.get_mut(&runtime.raw()) else {
            return BlockingOutcome::InvalidHandle;
        };
        let Ok((task_id, _worker_id)) = bridge.runtime.spawn_task() else {
            return BlockingOutcome::RuntimeError(None);
        };
        let target = RuntimeTarget::Task(task_id);
        let Some(request_id) = start(bridge, target) else {
            return BlockingOutcome::RuntimeError(None);
        };
        drain_until_request(bridge, request_id, extract)
    })
}

#[cfg(feature = "async-mio")]
fn drain_until_request<T>(
    bridge: &mut BridgeRuntime,
    request_id: RequestId,
    extract: impl FnOnce(CompletionPayload) -> Option<T>,
) -> BlockingOutcome<T> {
    let mut extract = Some(extract);
    loop {
        if bridge.runtime.poll().is_err() {
            return BlockingOutcome::RuntimeError(None);
        }
        if let Some(resumed) = bridge.runtime.pop_resumed_continuation() {
            if resumed.request_id != request_id {
                // A stray completion from another in-flight request — drop and keep waiting.
                continue;
            }
            return match resumed.completion {
                Some(Ok(payload)) => match (extract.take().unwrap())(payload) {
                    Some(value) => BlockingOutcome::Ok(value),
                    None => BlockingOutcome::RuntimeError(Some(
                        "unexpected completion payload shape".into(),
                    )),
                },
                Some(Err(error)) => BlockingOutcome::RuntimeError(Some(error.message)),
                None => BlockingOutcome::Cancelled,
            };
        }
        // Park briefly to avoid burning CPU while the reactor thread is at
        // work. The reactor itself is on epoll/kqueue, so this is purely a
        // scheduling courtesy for our own thread.
        std::thread::yield_now();
    }
}

// ── TCP blocking shims ────────────────────────────────────────────────
//
// Each shim takes typed out-pointers for the result and a separate
// `*mut FluxAsyncBuffer` for an error message string. Bytes/text payloads
// are handed back as Rust-allocated heap buffers; C copies them and calls
// `flux_async_runtime_free_buffer` to release.

/// Heap buffer handed across the FFI boundary. The C side reads `ptr`/`len`,
/// copies the contents into a Flux-managed object, then calls
/// `flux_async_runtime_free_buffer` to release the Rust allocation.
#[repr(C)]
pub struct FluxAsyncBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl FluxAsyncBuffer {
    fn from_vec(mut bytes: Vec<u8>) -> Self {
        bytes.shrink_to_fit();
        let len = bytes.len();
        if len == 0 {
            // An empty buffer still has to round-trip safely — give C a
            // non-null but zero-length pointer so it can distinguish "buffer
            // present" from "buffer absent" without inspecting `len`.
            let mut placeholder = vec![0u8];
            let ptr = placeholder.as_mut_ptr();
            std::mem::forget(placeholder);
            return Self { ptr, len: 0 };
        }
        let ptr = bytes.as_mut_ptr();
        std::mem::forget(bytes);
        Self { ptr, len }
    }

    fn from_string(text: String) -> Self {
        Self::from_vec(text.into_bytes())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_free_buffer(buffer: FluxAsyncBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    // Reconstruct the Vec the same way it was leaked: capacity == len after
    // `shrink_to_fit`, except for the empty-buffer placeholder where capacity
    // is 1.
    let cap = if buffer.len == 0 { 1 } else { buffer.len };
    unsafe {
        let _ = Vec::from_raw_parts(buffer.ptr, buffer.len, cap);
    }
}

/// Read a `(ptr, len)` UTF-8 view from the C side. The buffer is borrowed —
/// this function never frees it.
unsafe fn read_str(ptr: *const u8, len: usize) -> Option<String> {
    if ptr.is_null() && len != 0 {
        return None;
    }
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(ptr, len) }
    };
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

unsafe fn read_bytes(ptr: *const u8, len: usize) -> Option<Vec<u8>> {
    if ptr.is_null() && len != 0 {
        return None;
    }
    if len == 0 {
        return Some(Vec::new());
    }
    Some(unsafe { slice::from_raw_parts(ptr, len) }.to_vec())
}

#[cfg(feature = "async-mio")]
fn write_error(error: Option<String>, out_err: *mut FluxAsyncBuffer) {
    if out_err.is_null() {
        return;
    }
    let message = error.unwrap_or_else(|| "async runtime error".into());
    unsafe {
        *out_err = FluxAsyncBuffer::from_string(message);
    }
}

#[cfg(feature = "async-mio")]
fn outcome_to_status<T>(
    outcome: BlockingOutcome<T>,
    on_ok: impl FnOnce(T),
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    match outcome {
        BlockingOutcome::Ok(value) => {
            on_ok(value);
            FluxAsyncStatus::Ok
        }
        BlockingOutcome::InvalidHandle => FluxAsyncStatus::InvalidHandle,
        BlockingOutcome::Cancelled => FluxAsyncStatus::Cancelled,
        BlockingOutcome::RuntimeError(message) => {
            write_error(message, out_err);
            FluxAsyncStatus::RuntimeError
        }
    }
}

/// TCP address kinds for `flux_async_runtime_tcp_addr_blocking`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluxAsyncTcpAddrKind {
    Local = 0,
    Remote = 1,
    ListenerLocal = 2,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_listen_blocking(
    runtime: FluxAsyncRuntimeHandle,
    host_ptr: *const u8,
    host_len: usize,
    port: u16,
    out_handle: *mut u64,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let Some(host) = (unsafe { read_str(host_ptr, host_len) }) else {
            write_error(Some("invalid host string".into()), out_err);
            return FluxAsyncStatus::RuntimeError;
        };
        let outcome = run_blocking::<_, u64>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_listen(target, Value::None, host, port)
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Handle(handle) => Some(handle),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |handle| {
                if !out_handle.is_null() {
                    unsafe { *out_handle = handle };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (host_ptr, host_len, port, out_handle, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_connect_blocking(
    runtime: FluxAsyncRuntimeHandle,
    host_ptr: *const u8,
    host_len: usize,
    port: u16,
    out_handle: *mut u64,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let Some(host) = (unsafe { read_str(host_ptr, host_len) }) else {
            write_error(Some("invalid host string".into()), out_err);
            return FluxAsyncStatus::RuntimeError;
        };
        let outcome = run_blocking::<_, u64>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_connect(target, Value::None, host, port)
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Handle(handle) => Some(handle),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |handle| {
                if !out_handle.is_null() {
                    unsafe { *out_handle = handle };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (host_ptr, host_len, port, out_handle, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_accept_blocking(
    runtime: FluxAsyncRuntimeHandle,
    listener: u64,
    out_handle: *mut u64,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let outcome = run_blocking::<_, u64>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_accept(target, Value::None, IoHandle(listener))
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Handle(handle) => Some(handle),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |handle| {
                if !out_handle.is_null() {
                    unsafe { *out_handle = handle };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (listener, out_handle, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_read_blocking(
    runtime: FluxAsyncRuntimeHandle,
    handle: u64,
    max: usize,
    out_buf: *mut FluxAsyncBuffer,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let outcome = run_blocking::<_, Vec<u8>>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_read(target, Value::None, IoHandle(handle), max)
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Bytes(bytes) => Some(bytes),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |bytes| {
                if !out_buf.is_null() {
                    unsafe { *out_buf = FluxAsyncBuffer::from_vec(bytes) };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (handle, max, out_buf, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_write_blocking(
    runtime: FluxAsyncRuntimeHandle,
    handle: u64,
    buf_ptr: *const u8,
    buf_len: usize,
    out_count: *mut usize,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let Some(bytes) = (unsafe { read_bytes(buf_ptr, buf_len) }) else {
            write_error(Some("invalid bytes buffer".into()), out_err);
            return FluxAsyncStatus::RuntimeError;
        };
        let outcome = run_blocking::<_, usize>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_write(target, Value::None, IoHandle(handle), bytes)
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Count(count) => Some(count),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |count| {
                if !out_count.is_null() {
                    unsafe { *out_count = count };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (handle, buf_ptr, buf_len, out_count, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_close_blocking(
    runtime: FluxAsyncRuntimeHandle,
    handle: u64,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let outcome = run_blocking::<_, ()>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_close(target, Value::None, IoHandle(handle))
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Unit => Some(()),
                _ => None,
            },
        );
        outcome_to_status(outcome, |_| {}, out_err)
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (handle, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_close_listener_blocking(
    runtime: FluxAsyncRuntimeHandle,
    listener: u64,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let outcome = run_blocking::<_, ()>(
            runtime,
            move |bridge, target| {
                bridge
                    .runtime
                    .start_tcp_close_listener(target, Value::None, IoHandle(listener))
                    .ok()
            },
            |payload| match payload {
                CompletionPayload::Unit => Some(()),
                _ => None,
            },
        );
        outcome_to_status(outcome, |_| {}, out_err)
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (listener, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flux_async_runtime_tcp_addr_blocking(
    runtime: FluxAsyncRuntimeHandle,
    handle: u64,
    kind: FluxAsyncTcpAddrKind,
    out_text: *mut FluxAsyncBuffer,
    out_err: *mut FluxAsyncBuffer,
) -> FluxAsyncStatus {
    if runtime.raw() == 0 {
        return FluxAsyncStatus::InvalidHandle;
    }
    #[cfg(feature = "async-mio")]
    {
        let outcome = run_blocking::<_, String>(
            runtime,
            move |bridge, target| match kind {
                FluxAsyncTcpAddrKind::Local => bridge
                    .runtime
                    .start_tcp_local_addr(target, Value::None, IoHandle(handle))
                    .ok(),
                FluxAsyncTcpAddrKind::Remote => bridge
                    .runtime
                    .start_tcp_remote_addr(target, Value::None, IoHandle(handle))
                    .ok(),
                FluxAsyncTcpAddrKind::ListenerLocal => bridge
                    .runtime
                    .start_tcp_listener_local_addr(target, Value::None, IoHandle(handle))
                    .ok(),
            },
            |payload| match payload {
                CompletionPayload::Text(text) => Some(text),
                _ => None,
            },
        );
        outcome_to_status(
            outcome,
            |text| {
                if !out_text.is_null() {
                    unsafe { *out_text = FluxAsyncBuffer::from_string(text) };
                }
            },
            out_err,
        )
    }
    #[cfg(not(feature = "async-mio"))]
    {
        let _ = (handle, kind, out_text, out_err);
        FluxAsyncStatus::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_is_real_thread_local_handle() {
        let runtime = flux_async_runtime_default();
        assert_ne!(runtime.raw(), 0);
        assert_eq!(flux_async_runtime_default(), runtime);
        assert_eq!(flux_async_runtime_poll(runtime), FluxAsyncStatus::Ok);
    }

    #[test]
    fn context_enter_leave_restores_previous_context() {
        let original = flux_async_context_current();
        let next = FluxAsyncContextHandle { raw: 42 };

        let previous = flux_async_context_enter(next);
        assert_eq!(previous, original);
        assert_eq!(flux_async_context_current(), next);

        assert_eq!(flux_async_context_leave(previous), FluxAsyncStatus::Ok);
        assert_eq!(flux_async_context_current(), original);
    }

    #[test]
    fn runtime_create_poll_destroy_lifecycle() {
        let runtime = flux_async_runtime_create();
        assert_ne!(runtime.raw(), 0);
        assert_eq!(flux_async_runtime_poll(runtime), FluxAsyncStatus::Ok);
        assert_eq!(flux_async_runtime_destroy(runtime), FluxAsyncStatus::Ok);
        assert_eq!(
            flux_async_runtime_poll(runtime),
            FluxAsyncStatus::InvalidHandle
        );
    }

    #[test]
    fn runtime_poll_rejects_null_handle() {
        assert_eq!(
            flux_async_runtime_poll(FluxAsyncRuntimeHandle::null()),
            FluxAsyncStatus::InvalidHandle
        );
    }

    #[cfg(feature = "async-mio")]
    #[test]
    fn sleep_blocking_zero_ms_returns_immediately() {
        let runtime = flux_async_runtime_default();
        assert_eq!(
            flux_async_runtime_sleep_blocking(runtime, 0),
            FluxAsyncStatus::Ok
        );
        assert_eq!(
            flux_async_runtime_sleep_blocking(runtime, -10),
            FluxAsyncStatus::Ok
        );
    }

    #[cfg(feature = "async-mio")]
    #[test]
    fn sleep_blocking_waits_at_least_requested_duration() {
        let runtime = flux_async_runtime_default();
        let start = std::time::Instant::now();
        let status = flux_async_runtime_sleep_blocking(runtime, 25);
        let elapsed = start.elapsed();
        assert_eq!(status, FluxAsyncStatus::Ok);
        assert!(
            elapsed >= Duration::from_millis(20),
            "sleep returned too early: {elapsed:?}"
        );
    }

    #[cfg(feature = "async-mio")]
    #[test]
    fn sleep_blocking_rejects_invalid_handle() {
        assert_eq!(
            flux_async_runtime_sleep_blocking(FluxAsyncRuntimeHandle::null(), 5),
            FluxAsyncStatus::InvalidHandle
        );
    }
}
