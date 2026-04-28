//! Native/LLVM entry stubs for the Rust-owned async runtime.
//!
//! These functions are intentionally inert in Phase 0. They establish the
//! narrow C ABI shape that native code can call later without moving scheduler
//! ownership into the C runtime. Phase 1 will replace the placeholder handles
//! with real scheduler/context objects owned by Rust.

use std::cell::Cell;

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

thread_local! {
    static ACTIVE_CONTEXT: Cell<FluxAsyncContextHandle> =
        const { Cell::new(FluxAsyncContextHandle::null()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_default() -> FluxAsyncRuntimeHandle {
    FluxAsyncRuntimeHandle::null()
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
pub extern "C" fn flux_async_runtime_poll(_runtime: FluxAsyncRuntimeHandle) -> FluxAsyncStatus {
    FluxAsyncStatus::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_is_null_placeholder() {
        assert_eq!(flux_async_runtime_default().raw(), 0);
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
    fn runtime_poll_is_not_implemented_in_phase_0() {
        assert_eq!(
            flux_async_runtime_poll(flux_async_runtime_default()),
            FluxAsyncStatus::Unsupported
        );
    }
}
