//! Native/LLVM C ABI shims for proposal 0174 Phase 1b-vi-d.
//!
//! These symbols are linked into LLVM-generated native binaries and provide
//! the narrow entry surface from the C runtime into the Rust async backend.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::backend::{AsyncBackend, CompletionPayload, RequestId};
use super::backends::mio::MioBackend;

static BACKEND: OnceLock<MioBackend> = OnceLock::new();
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static READY_TIMERS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();

fn backend() -> &'static MioBackend {
    BACKEND.get_or_init(MioBackend::new)
}

fn ready_timers() -> &'static Mutex<HashSet<u64>> {
    READY_TIMERS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Initialize the process-global native async backend.
///
/// Returns 0 on success, -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_init() -> i32 {
    match backend().start() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Shut down the process-global native async backend.
///
/// The current native driver runs a short-lived executable per Flux program,
/// so this is mostly a test/diagnostic hook. It is intentionally idempotent.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_shutdown() -> i32 {
    match backend().shutdown() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Allocate a timer request and submit it to the Rust `mio` backend.
///
/// `ms` is an untagged millisecond count from the C runtime. Returns the raw
/// request id, or 0 on failure. RequestId(0) is reserved as a sentinel.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_timer_start(ms: i64) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let req = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let delay = if ms < 0 { 0 } else { ms as u64 };
    backend().timer_start(RequestId(req), delay);
    req
}

/// Poll the backend dispatch path until `req` has completed.
///
/// Returns:
/// - 1 when the request completed successfully,
/// - 0 when no matching completion is currently available,
/// - -1 when the request completed with an error or the backend failed.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_poll_dispatch(req: u64) -> i32 {
    if req == 0 {
        return -1;
    }

    if let Ok(mut ready) = ready_timers().lock()
        && ready.remove(&req)
    {
        return 1;
    }

    if flux_async_runtime_init() != 0 {
        return -1;
    }

    loop {
        if let Some(completion) = backend().next_completion() {
            match completion.payload {
                CompletionPayload::Unit if completion.request_id.0 == req => return 1,
                CompletionPayload::Error(_) if completion.request_id.0 == req => return -1,
                CompletionPayload::Unit => {
                    if let Ok(mut ready) = ready_timers().lock() {
                        ready.insert(completion.request_id.0);
                    }
                }
                CompletionPayload::Bytes(_)
                | CompletionPayload::TcpHandle(_)
                | CompletionPayload::Error(_) => {
                    if completion.request_id.0 == req {
                        return -1;
                    }
                }
            }
        } else {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}
