use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct LeakStats {
    pub compiled_functions: usize,
    pub closures: usize,
    pub arrays: usize,
    pub hashes: usize,
    pub somes: usize,
}

static COMPILED_FUNCTIONS: AtomicUsize = AtomicUsize::new(0);
static CLOSURES: AtomicUsize = AtomicUsize::new(0);
static ARRAYS: AtomicUsize = AtomicUsize::new(0);
static HASHES: AtomicUsize = AtomicUsize::new(0);
static SOMES: AtomicUsize = AtomicUsize::new(0);

pub fn record_compiled_function() {
    COMPILED_FUNCTIONS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_closure() {
    CLOSURES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_array() {
    ARRAYS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_hash() {
    HASHES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_some() {
    SOMES.fetch_add(1, Ordering::Relaxed);
}

pub fn snapshot() -> LeakStats {
    LeakStats {
        compiled_functions: COMPILED_FUNCTIONS.load(Ordering::Relaxed),
        closures: CLOSURES.load(Ordering::Relaxed),
        arrays: ARRAYS.load(Ordering::Relaxed),
        hashes: HASHES.load(Ordering::Relaxed),
        somes: SOMES.load(Ordering::Relaxed),
    }
}
