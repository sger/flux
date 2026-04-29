//! Minimal thread-backed timer backend for VM-owned async suspension.
//!
//! This backend is intentionally narrow: it exists so the VM can validate the
//! Phase 1b suspend/resume path without requiring the optional `mio` feature.
//! Only backend completion records cross the spawned timer thread boundary;
//! Flux values and continuations remain owned by the VM thread.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::runtime::r#async::backend::{
    AsyncBackend, AsyncError, AsyncErrorKind, BackendCompletion, BackendCompletionPayload,
    BackendCompletionSink, BackendCompletionSource, CancelHandle, Completion, RequestId,
    RuntimeTarget, backend_completion_channel,
};

#[derive(Debug, Clone)]
pub struct ThreadTimerBackend {
    sink: BackendCompletionSink,
    source: BackendCompletionSource,
    cancelled: Arc<Mutex<HashSet<RequestId>>>,
}

impl Default for ThreadTimerBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadTimerBackend {
    pub fn new() -> Self {
        let (sink, source) = backend_completion_channel();
        Self {
            sink,
            source,
            cancelled: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn pending_completions(&self) -> Result<usize, AsyncError> {
        self.source.pending()
    }
}

impl AsyncBackend for ThreadTimerBackend {
    fn poll_completion(&mut self) -> Option<Completion> {
        self.source.poll_completion().ok().flatten()
    }

    fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.cancelled
            .lock()
            .map_err(|_| AsyncError::new(AsyncErrorKind::Other, "timer cancel set poisoned"))?
            .insert(handle.request_id());
        Ok(())
    }

    fn timer_start(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        let sink = self.sink.clone();
        let cancelled = Arc::clone(&self.cancelled);
        thread::spawn(move || {
            if !duration.is_zero() {
                thread::sleep(duration);
            }
            let is_cancelled = cancelled
                .lock()
                .map(|set| set.contains(&request_id))
                .unwrap_or(true);
            if !is_cancelled {
                let _ = sink.submit(BackendCompletion::ok(
                    request_id,
                    target,
                    BackendCompletionPayload::Unit,
                ));
            }
        });
        Ok(CancelHandle::new(request_id, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::r#async::context::TaskId;

    #[test]
    fn thread_timer_completes_after_duration() {
        let mut backend = ThreadTimerBackend::new();
        let target = RuntimeTarget::Task(TaskId(1));
        backend
            .timer_start(RequestId(1), target, Duration::from_millis(1))
            .expect("timer starts");

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let completion = loop {
            if let Some(completion) = backend.poll_completion() {
                break completion;
            }
            assert!(std::time::Instant::now() < deadline, "timer timed out");
            thread::sleep(Duration::from_millis(1));
        };

        assert_eq!(completion.request_id, RequestId(1));
        assert_eq!(completion.target, target);
        assert_eq!(
            completion.payload,
            Ok(crate::runtime::r#async::backend::CompletionPayload::Unit)
        );
    }

    #[test]
    fn cancelled_thread_timer_does_not_complete() {
        let mut backend = ThreadTimerBackend::new();
        let target = RuntimeTarget::Task(TaskId(1));
        let handle = backend
            .timer_start(RequestId(2), target, Duration::from_millis(10))
            .expect("timer starts");
        backend.cancel(handle).expect("timer cancels");
        thread::sleep(Duration::from_millis(25));

        assert_eq!(backend.poll_completion(), None);
    }
}
