//! A global allocator that counts allocations, for `flux --metrics` (proposal
//! 0190 S11).
//!
//! Only the `flux` binary installs it (`src/main.rs`). The library is also a
//! staticlib linked into native programs, and those keep the system allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// The system allocator, counting every `alloc`, `alloc_zeroed` and
/// `realloc`. A `realloc` counts because growing a `Vec` is one.
pub struct CountingAlloc;

// SAFETY: every method forwards to `System` with the same arguments; the
// counter is a relaxed atomic that never touches the memory handed out.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Allocations so far, or `None` when `CountingAlloc` is not the global
/// allocator: a process that has run any Rust code has allocated by now.
pub fn allocations() -> Option<u64> {
    match ALLOCATIONS.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}
