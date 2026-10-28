//! Native LLVM Flow.Json parser/stringifier parity smoke tests.

#![cfg(feature = "llvm")]

use std::sync::{Mutex, OnceLock};

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use flux_runner::flux_source;

static NATIVE_JSON_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn run_source(source: &str) -> (String, String, bool) {
    let _guard = NATIVE_JSON_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native JSON test lock poisoned");
    let ran = flux_source(source, "native_json_tests").native().run();
    (ran.stdout, ran.stderr, ran.success)
}

#[test]
fn native_json_value_constructors_stringify() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json

fn main() with IO {
    print(Json.stringify(Json.array([|Json.string("a"), Json.bool(false), Json.null()|])))
}
"#,
    );
    assert!(
        ok,
        "native JSON fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("\"[\"a\",false,null]\""), "{stdout}");
}
