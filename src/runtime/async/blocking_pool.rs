//! Small blocking-worker pool for async runtime services.
//!
//! Phase 2 slice 2-viii uses this for DNS resolution. The pool is intentionally
//! generic so later filesystem operations can reuse the same substrate.

use std::fmt;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

type Job = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    Run(Job),
    Shutdown,
}

pub struct BlockingPool {
    sender: Mutex<Option<mpsc::Sender<Message>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl BlockingPool {
    pub fn new(name: &str, size: usize) -> Result<Self, String> {
        let size = size.max(1);
        let (tx, rx) = mpsc::channel::<Message>();
        let rx = std::sync::Arc::new(Mutex::new(rx));
        let mut workers = Vec::with_capacity(size);

        for idx in 0..size {
            let worker_rx = std::sync::Arc::clone(&rx);
            let worker_name = format!("flux-{name}-{idx}");
            let handle = thread::Builder::new()
                .name(worker_name)
                .spawn(move || {
                    loop {
                        let Ok(message) = worker_rx
                            .lock()
                            .expect("blocking pool receiver poisoned")
                            .recv()
                        else {
                            break;
                        };
                        match message {
                            Message::Run(job) => job(),
                            Message::Shutdown => break,
                        }
                    }
                })
                .map_err(|e| format!("spawn blocking worker failed: {e}"))?;
            workers.push(handle);
        }

        Ok(Self {
            sender: Mutex::new(Some(tx)),
            workers: Mutex::new(workers),
        })
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        let sender = self.sender.lock().expect("blocking pool sender poisoned");
        let Some(sender) = sender.as_ref() else {
            return Err("blocking pool is shut down".into());
        };
        sender
            .send(Message::Run(Box::new(job)))
            .map_err(|_| "blocking pool is shut down".to_string())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let mut workers = self.workers.lock().expect("blocking pool workers poisoned");
        let Some(sender) = self
            .sender
            .lock()
            .expect("blocking pool sender poisoned")
            .take()
        else {
            return Ok(());
        };
        for _ in 0..workers.len() {
            let _ = sender.send(Message::Shutdown);
        }
        drop(sender);

        let mut first_error = None;
        for worker in workers.drain(..) {
            if let Err(e) = worker.join()
                && first_error.is_none()
            {
                first_error = Some(format!("blocking worker panicked: {e:?}"));
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

impl Drop for BlockingPool {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl fmt::Debug for BlockingPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let workers = self
            .workers
            .lock()
            .map(|workers| workers.len())
            .unwrap_or_default();
        f.debug_struct("BlockingPool")
            .field("workers", &workers)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn runs_submitted_jobs() {
        let pool = BlockingPool::new("test", 1).unwrap();
        let (tx, rx) = mpsc::channel();
        pool.submit(move || tx.send(7).unwrap()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 7);
        pool.shutdown().unwrap();
    }

    #[test]
    fn shutdown_is_idempotent() {
        let pool = BlockingPool::new("test", 2).unwrap();
        pool.shutdown().unwrap();
        pool.shutdown().unwrap();
    }
}
