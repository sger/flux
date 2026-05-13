//! Structured fiber event log for concurrency debugging.
//!
//! Activated by setting `FLUX_FIBER_TRACE=1` in the environment before running
//! a Flux program. When disabled the hot path is a single relaxed atomic bool
//! load with no allocation and no I/O.
//!
//! ## Output format
//!
//! One line per event on stderr, prefixed by category tag:
//!
//! ```text
//! [FIBER] spawn    fid=1  worker=0  t=0µs
//! [FIBER] suspend  fid=2  worker=0  req=100  t=12µs
//! [FIBER] resume   fid=2  worker=0  req=100  t=205µs
//! [FIBER] cancel   fid=3  worker=1  t=210µs
//! [FIBER] complete fid=2  worker=0  t=206µs
//! [TASK]  spawn    tid=1  parent_fid=1  t=50µs
//! [TASK]  await    tid=1  fid=1  t=51µs
//! [TASK]  cancel   tid=1  t=55µs
//! [TASK]  done     tid=1  t=62µs
//! [SCOPE] create   scope=10  fid=4  t=70µs
//! [SCOPE] cancel   scope=10  t=300µs
//! [CHAN]  send     ch=7  fid=2  backpressure=false  t=80µs
//! [CHAN]  recv     ch=7  fid=3  was_empty=false  t=81µs
//! [CHAN]  close    ch=7  t=90µs
//! ```

use std::sync::OnceLock;
use std::time::Instant;

// ── Activation ────────────────────────────────────────────────────────────────

static ENABLED: OnceLock<bool> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

pub fn is_enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("FLUX_FIBER_TRACE")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

fn elapsed_us() -> u128 {
    START.get_or_init(Instant::now).elapsed().as_micros()
}

// ── Event enum ────────────────────────────────────────────────────────────────

pub enum FiberEvent {
    // Fiber lifecycle
    Spawn {
        fid: u64,
        worker: u32,
    },
    Suspend {
        fid: u64,
        worker: u32,
        request_id: u64,
    },
    Resume {
        fid: u64,
        worker: u32,
        request_id: u64,
    },
    Cancel {
        fid: u64,
        worker: u32,
    },
    Complete {
        fid: u64,
        worker: u32,
    },
    // Task lifecycle
    TaskSpawn {
        tid: i64,
        parent_fid: u64,
    },
    TaskAwait {
        tid: i64,
        fid: u64,
    },
    TaskCancel {
        tid: i64,
    },
    TaskDone {
        tid: i64,
    },
    // Scope lifecycle
    ScopeCreate {
        scope_id: u64,
        fid: u64,
    },
    ScopeCancel {
        scope_id: u64,
    },
    // Channel operations
    ChanSend {
        ch: i64,
        fid: u64,
        backpressure: bool,
    },
    ChanRecv {
        ch: i64,
        fid: u64,
        was_empty: bool,
    },
    ChanClose {
        ch: i64,
    },
}

// ── Emit ──────────────────────────────────────────────────────────────────────

pub fn emit(event: FiberEvent) {
    if !is_enabled() {
        return;
    }
    let t = elapsed_us();
    match event {
        FiberEvent::Spawn { fid, worker } => {
            eprintln!("[FIBER] spawn    fid={fid}  worker={worker}  t={t}µs")
        }
        FiberEvent::Suspend {
            fid,
            worker,
            request_id,
        } => eprintln!("[FIBER] suspend  fid={fid}  worker={worker}  req={request_id}  t={t}µs"),
        FiberEvent::Resume {
            fid,
            worker,
            request_id,
        } => eprintln!("[FIBER] resume   fid={fid}  worker={worker}  req={request_id}  t={t}µs"),
        FiberEvent::Cancel { fid, worker } => {
            eprintln!("[FIBER] cancel   fid={fid}  worker={worker}  t={t}µs")
        }
        FiberEvent::Complete { fid, worker } => {
            eprintln!("[FIBER] complete fid={fid}  worker={worker}  t={t}µs")
        }
        FiberEvent::TaskSpawn { tid, parent_fid } => {
            eprintln!("[TASK]  spawn    tid={tid}  parent_fid={parent_fid}  t={t}µs")
        }
        FiberEvent::TaskAwait { tid, fid } => {
            eprintln!("[TASK]  await    tid={tid}  fid={fid}  t={t}µs")
        }
        FiberEvent::TaskCancel { tid } => eprintln!("[TASK]  cancel   tid={tid}  t={t}µs"),
        FiberEvent::TaskDone { tid } => eprintln!("[TASK]  done     tid={tid}  t={t}µs"),
        FiberEvent::ScopeCreate { scope_id, fid } => {
            eprintln!("[SCOPE] create   scope={scope_id}  fid={fid}  t={t}µs")
        }
        FiberEvent::ScopeCancel { scope_id } => {
            eprintln!("[SCOPE] cancel   scope={scope_id}  t={t}µs")
        }
        FiberEvent::ChanSend {
            ch,
            fid,
            backpressure,
        } => eprintln!("[CHAN]  send     ch={ch}  fid={fid}  backpressure={backpressure}  t={t}µs"),
        FiberEvent::ChanRecv { ch, fid, was_empty } => {
            eprintln!("[CHAN]  recv     ch={ch}  fid={fid}  was_empty={was_empty}  t={t}µs")
        }
        FiberEvent::ChanClose { ch } => eprintln!("[CHAN]  close    ch={ch}  t={t}µs"),
    }
}
