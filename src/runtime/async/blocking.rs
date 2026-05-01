//! Blocking service pool for async runtime operations.
//!
//! `mio` does not provide file I/O, DNS, or other blocking services. This pool
//! gives those services a Rust-side home while preserving the async runtime
//! rule that OS threads publish backend-safe completions only.

use std::{
    collections::{HashMap, HashSet},
    fs,
    net::ToSocketAddrs,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use super::backend::{
    AsyncError, AsyncErrorKind, BackendCompletion, BackendCompletionPayload, BackendCompletionSink,
    CancelHandle, RequestId, RuntimeTarget,
};

/// Per-request route table shared with the reactor: completions for a
/// `RequestId` are delivered to the registered sink (the originating worker
/// VM's source) rather than the backend's primary sink.
pub type BlockingRouteTable = Arc<Mutex<HashMap<RequestId, BackendCompletionSink>>>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockingServicesConfig {
    pub fs_workers: usize,
    pub dns_workers: usize,
}

impl Default for BlockingServicesConfig {
    fn default() -> Self {
        let workers = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2);
        Self {
            fs_workers: workers.min(4).max(1),
            dns_workers: 2,
        }
    }
}

pub struct BlockingServices {
    fs_pool: BlockingPool,
    dns_pool: BlockingPool,
}

impl BlockingPool {
    pub fn new(
        config: BlockingPoolConfig,
        sink: BackendCompletionSink,
        routes: BlockingRouteTable,
    ) -> Self {
        let worker_count = config.worker_count.max(1);
        let (sender, receiver) = mpsc::channel::<BlockingJob>();
        let receiver = Arc::new(Mutex::new(receiver));
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let mut workers = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let cancelled = Arc::clone(&cancelled);
            let sink = sink.clone();
            let routes = Arc::clone(&routes);
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

                    let routed = routes
                        .lock()
                        .ok()
                        .and_then(|mut routes| routes.remove(&completion.request_id));
                    let result = match routed {
                        Some(target_sink) => target_sink.submit(completion),
                        None => sink.submit(completion),
                    };
                    if result.is_err() {
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

impl BlockingServices {
    pub fn new(
        config: BlockingServicesConfig,
        sink: BackendCompletionSink,
        routes: BlockingRouteTable,
    ) -> Self {
        Self {
            fs_pool: BlockingPool::new(
                BlockingPoolConfig {
                    worker_count: config.fs_workers,
                },
                sink.clone(),
                Arc::clone(&routes),
            ),
            dns_pool: BlockingPool::new(
                BlockingPoolConfig {
                    worker_count: config.dns_workers,
                },
                sink,
                routes,
            ),
        }
    }

    pub fn read_file(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        self.fs_pool.submit(request_id, target, move || {
            fs::read(&path)
                .map(BackendCompletionPayload::Bytes)
                .map_err(|err| io_error("file read failed", err))
        })
    }

    pub fn write_file(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.fs_pool.submit(request_id, target, move || {
            fs::write(&path, &bytes)
                .map(|()| BackendCompletionPayload::Count(bytes.len()))
                .map_err(|err| io_error("file write failed", err))
        })
    }

    pub fn resolve_dns(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.dns_pool.submit(request_id, target, move || {
            (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| BackendCompletionPayload::AddressList(addresses.collect()))
                .map_err(|err| io_error("dns resolve failed", err))
        })
    }

    pub fn cancel(&self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.fs_pool.cancel(handle)?;
        self.dns_pool.cancel(handle)?;
        Ok(())
    }
}

fn io_error(context: &str, error: std::io::Error) -> AsyncError {
    AsyncError::new(AsyncErrorKind::Other, format!("{context}: {error}"))
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
        backend::{
            BackendCompletion, BackendCompletionPayload, RuntimeTarget, backend_completion_channel,
        },
        context::TaskId,
    };
    use std::{
        fs,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn poll_backend_completion(
        source: &crate::runtime::r#async::backend::BackendCompletionSource,
    ) -> BackendCompletion {
        for _ in 0..100 {
            if let Some(completion) = source
                .poll_backend_completion()
                .expect("completion queue readable")
            {
                return completion;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for blocking completion")
    }

    fn temp_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("flux_async_{name}_{unique}"))
    }

    fn empty_routes() -> BlockingRouteTable {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn blocking_pool_publishes_backend_completion() {
        let (sink, source) = backend_completion_channel();
        let pool = BlockingPool::new(BlockingPoolConfig { worker_count: 1 }, sink, empty_routes());
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
            thread::sleep(Duration::from_millis(1));
        }
        pool.shutdown();

        let completion = completion.expect("job completes");
        assert_eq!(completion.request_id, RequestId(1));
        assert_eq!(completion.payload, Ok(BackendCompletionPayload::Count(42)));
    }

    #[test]
    fn blocking_pool_suppresses_cancelled_job_completion() {
        let (sink, source) = backend_completion_channel();
        let pool = BlockingPool::new(BlockingPoolConfig { worker_count: 1 }, sink, empty_routes());
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
            thread::sleep(Duration::from_millis(1));
        }
        pool.shutdown();

        assert_eq!(source.pending().expect("completion queue readable"), 0);
    }

    #[test]
    fn blocking_services_read_and_write_files_as_backend_bytes() {
        let (sink, source) = backend_completion_channel();
        let services = BlockingServices::new(
            BlockingServicesConfig {
                fs_workers: 1,
                dns_workers: 1,
            },
            sink,
            empty_routes(),
        );
        let path = temp_path("service_file");

        services
            .write_file(
                RequestId(3),
                RuntimeTarget::Task(TaskId(3)),
                path.clone(),
                b"flux".to_vec(),
            )
            .expect("write job submits");
        let write = poll_backend_completion(&source);
        assert_eq!(write.payload, Ok(BackendCompletionPayload::Count(4)));

        services
            .read_file(RequestId(4), RuntimeTarget::Task(TaskId(4)), path.clone())
            .expect("read job submits");
        let read = poll_backend_completion(&source);
        assert_eq!(
            read.payload,
            Ok(BackendCompletionPayload::Bytes(b"flux".to_vec()))
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn blocking_services_resolves_dns_to_backend_addresses() {
        let (sink, source) = backend_completion_channel();
        let services = BlockingServices::new(
            BlockingServicesConfig {
                fs_workers: 1,
                dns_workers: 1,
            },
            sink,
            empty_routes(),
        );
        services
            .resolve_dns(
                RequestId(5),
                RuntimeTarget::Task(TaskId(5)),
                "localhost".to_string(),
                80,
            )
            .expect("dns job submits");

        let completion = poll_backend_completion(&source);
        match completion.payload {
            Ok(BackendCompletionPayload::AddressList(addresses)) => {
                assert!(!addresses.is_empty());
            }
            other => panic!("expected address list, got {other:?}"),
        }
    }

    #[test]
    fn blocking_pool_routes_completion_to_per_request_sink() {
        let (primary_sink, primary_source) = backend_completion_channel();
        let (worker_sink, worker_source) = backend_completion_channel();
        let routes: BlockingRouteTable = Arc::new(Mutex::new(HashMap::new()));
        let routed_request = RequestId(101);
        routes
            .lock()
            .expect("route table lock")
            .insert(routed_request, worker_sink);

        let pool = BlockingPool::new(
            BlockingPoolConfig { worker_count: 1 },
            primary_sink,
            Arc::clone(&routes),
        );
        pool.submit(routed_request, RuntimeTarget::Task(TaskId(7)), || {
            Ok(BackendCompletionPayload::Count(99))
        })
        .expect("job submits");

        let completion = poll_backend_completion(&worker_source);
        assert_eq!(completion.request_id, routed_request);
        assert_eq!(completion.payload, Ok(BackendCompletionPayload::Count(99)));
        assert_eq!(
            primary_source.pending().expect("primary source readable"),
            0,
            "routed completion must not appear on the primary sink",
        );
        assert!(
            !routes
                .lock()
                .expect("route table lock")
                .contains_key(&routed_request),
            "route entry must be removed after delivery",
        );
        pool.shutdown();
    }
}
