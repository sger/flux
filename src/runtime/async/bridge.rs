//! Native/LLVM entry stubs for the Rust-owned async runtime.
//!
//! These functions keep native/LLVM integration behind opaque Rust-owned
//! handles. Native code can enter/leave an async context and poll a runtime
//! without learning about the scheduler, backend, or `mio` internals.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use super::{
    backend::{BackendCompletionSource, backend_completion_channel},
    runtime::AsyncRuntime,
    scheduler::SchedulerConfig,
};

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
    static DEFAULT_RUNTIME: Cell<FluxAsyncRuntimeHandle> =
        const { Cell::new(FluxAsyncRuntimeHandle::null()) };
    static NEXT_RUNTIME_HANDLE: Cell<u64> = const { Cell::new(1) };
    static RUNTIMES: RefCell<HashMap<u64, AsyncRuntime<BackendCompletionSource>>> =
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
    let (_sink, source) = backend_completion_channel();
    let runtime = AsyncRuntime::new(SchedulerConfig::default(), source);
    RUNTIMES.with(|runtimes| {
        runtimes.borrow_mut().insert(handle.raw(), runtime);
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
    RUNTIMES.with(|runtimes| {
        let mut runtimes = runtimes.borrow_mut();
        let Some(runtime) = runtimes.get_mut(&runtime.raw()) else {
            return FluxAsyncStatus::InvalidHandle;
        };
        match runtime.poll() {
            Ok(_) => FluxAsyncStatus::Ok,
            Err(_) => FluxAsyncStatus::RuntimeError,
        }
    })
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
}
