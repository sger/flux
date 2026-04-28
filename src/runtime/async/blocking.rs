//! Blocking service pool for async runtime operations.
//!
//! `mio` does not provide file I/O, DNS, or other blocking services. This pool
//! gives those services a Rust-side home while preserving the async runtime
//! rule that OS threads publish backend-safe completions only.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use super::backend::{
    AsyncError, AsyncErrorKind, BackendCompletion, BackendCompletionPayload, BackendCompletionSink,
    CancelHandle, RequestId, RuntimeTarget,
};

type BlockingResult = Result<BackendCompletionPayload, AsyncError>;
type BlockingWork = Box<dyn FnOnce() -> BlockingResult + Send + 'static>;

struct BlockingJob {
    request_id: RequestId,
    target: RuntimeTarget,
    work: BlockingWork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockingPoolConfig {
    pub worker_count: usize,
}

impl Default for BlockingPoolConfig {
    fn default() -> Self {
        Self { worker_count: 2 }
    }
}

pub struct BlockingPool {
    sender: Option<mpsc::Sender<BlockingJob>>,
    workers: Vec<thread::JoinHandle<()>>,
    cancelled: Arc<Mutex<HashSet<RequestId>>>,
}

impl BlockingPool {
    pub fn new(config: BlockingPoolConfig, sink: BackendCompletionSink) -> Self {
        let worker_count = config.worker_count.max(1);
        let (sender, receiver) = mpsc::channel::<BlockingJob>();
        let receiver = Arc::new(Mutex::new(receiver));
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let mut workers = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let cancelled = Arc::clone(&cancelled);
            let sink = sink.clone();
            workers.push(thread::spawn(move || {
                loop {
                    let job = {
                        let receiver = match receiver.lock() {
                            Ok(receiver) => receiver,
                            Err(_) => return,
                        };
                        match receiver.recv() {
                            Ok(job) => job,
                            Err(_) => return,
                        }
                    };

                    if take_cancelled(&cancelled, job.request_id) {
                        continue;
                    }

                    let completion = match (job.work)() {
                        Ok(payload) => BackendCompletion::ok(job.request_id, job.target, payload),
                        Err(error) => BackendCompletion::err(job.request_id, job.target, error),
                    };

                    if take_cancelled(&cancelled, job.request_id) {
                        continue;
                    }

                    if sink.submit(completion).is_err() {
                        return;
                    }
                }
            }));
        }

        Self {
            sender: Some(sender),
            workers,
            cancelled,
        }
    }

    pub fn submit<F>(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        work: F,
    ) -> Result<CancelHandle, AsyncError>
    where
        F: FnOnce() -> BlockingResult + Send + 'static,
    {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| AsyncError::new(AsyncErrorKind::Closed, "blocking pool is shut down"))?;
        sender
            .send(BlockingJob {
                request_id,
                target,
                work: Box::new(work),
            })
            .map_err(|_| AsyncError::new(AsyncErrorKind::Closed, "blocking pool is closed"))?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn cancel(&self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.cancelled
            .lock()
            .map_err(|_| AsyncError::new(AsyncErrorKind::Other, "blocking pool lock poisoned"))?
            .insert(handle.request_id());
        Ok(())
    }

    pub fn shutdown(mut self) {
        self.sender.take();
        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

fn take_cancelled(cancelled: &Arc<Mutex<HashSet<RequestId>>>, request_id: RequestId) -> bool {
    cancelled
        .lock()
        .map(|mut cancelled| cancelled.remove(&request_id))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::r#async::{
        backend::{RuntimeTarget, backend_completion_channel},
        context::TaskId,
    };

    #[test]
    fn blocking_pool_publishes_backend_completion() {
        let (sink, source) = backend_completion_channel();
        let pool = BlockingPool::new(BlockingPoolConfig { worker_count: 1 }, sink);
        pool.submit(RequestId(1), RuntimeTarget::Task(TaskId(1)), || {
            Ok(BackendCompletionPayload::Count(42))
        })
        .expect("job submits");

        let mut completion = None;
        for _ in 0..100 {
            completion = source
                .poll_backend_completion()
                .expect("completion queue readable");
            if completion.is_some() {
                break;
            }
            thread::yield_now();
        }
        pool.shutdown();

        let completion = completion.expect("job completes");
        assert_eq!(completion.request_id, RequestId(1));
        assert_eq!(completion.payload, Ok(BackendCompletionPayload::Count(42)));
    }

    #[test]
    fn blocking_pool_suppresses_cancelled_job_completion() {
        let (sink, source) = backend_completion_channel();
        let pool = BlockingPool::new(BlockingPoolConfig { worker_count: 1 }, sink);
        let (unblock, wait) = mpsc::channel();
        let handle = pool
            .submit(RequestId(2), RuntimeTarget::Task(TaskId(2)), move || {
                let _ = wait.recv();
                Ok(BackendCompletionPayload::Unit)
            })
            .expect("job submits");
        pool.cancel(handle).expect("job cancels");
        let _ = unblock.send(());

        for _ in 0..100 {
            if source.pending().expect("completion queue readable") > 0 {
                break;
            }
            thread::yield_now();
        }
        pool.shutdown();

        assert_eq!(source.pending().expect("completion queue readable"), 0);
    }
}
