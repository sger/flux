use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

// NO_COLOR is a process-global environment variable; guard access to avoid
// racy tests when diagnostics are rendered in parallel.
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub struct EnvGuard {
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set_no_color(value: Option<&str>) -> Self {
        let prev = std::env::var_os("NO_COLOR");
        unsafe {
            match value {
                Some(val) => std::env::set_var("NO_COLOR", val),
                None => std::env::remove_var("NO_COLOR"),
            }
        }
        Self { prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(val) => std::env::set_var("NO_COLOR", val),
                None => std::env::remove_var("NO_COLOR"),
            }
        }
    }
}

pub fn with_no_color(value: Option<&str>) -> (MutexGuard<'static, ()>, EnvGuard) {
    let lock = ENV_LOCK.lock().unwrap();
    let guard = EnvGuard::set_no_color(value);
    (lock, guard)
}
