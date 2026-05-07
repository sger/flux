//! Native LLVM Flow.Json parser/stringifier parity smoke tests.

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NATIVE_JSON_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn write_fixture(source: &str) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-native-json-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: &str) -> (String, String, bool) {
    let _guard = NATIVE_JSON_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native JSON test lock poisoned");
    let path = write_fixture(source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run native flux");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn native_json_parse_and_stringify_object_array() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json

fn main() with IO {
    print(Json.encode_json(Json.parse("{\"z\":null,\"a\":[1,true]}")))
}
"#,
    );
    assert!(
        ok,
        "native JSON fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("\"{\"a\":[1,true],\"z\":null}\""),
        "{stdout}"
    );
}

#[test]
fn native_json_rejects_malformed_input() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json

fn main() with IO {
    print(Json.result_is_ok(Json.parse_result("[1,")))
    print(Json.error_message(Json.parse_result("[1,")))
}
"#,
    );
    assert!(
        ok,
        "native JSON fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("false"), "{stdout}");
    assert!(stdout.contains("unterminated array"), "{stdout}");
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

#[test]
fn native_json_derived_record_and_sum_codecs() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json

data Person { Person { name: String, age: Int } } deriving (Json.Encode, Json.Decode)
data Shape { Dot, Circle(Float) } deriving (Encode, Decode)

fn main() with IO {
    let person = Person { name: "Ada", age: 42 }
    let person_json = encode(person)
    print(Json.encode_json(person_json))
    let decoded_person = Json.result_or(decode(person_json), Person { name: "", age: 0 })
    match decoded_person {
        Person { name, age } -> print(name + ":" + to_string(age))
    }

    let circle_json = encode(Circle(2.5))
    print(Json.encode_json(circle_json))
    let decoded_circle = Json.result_or(decode(circle_json), Dot)
    match decoded_circle {
        Circle(r) -> print(to_string(r)),
        _ -> print("not-circle")
    }

    let bad_person = Json.result_or(decode(Json.parse("{\"tag\":\"Nope\",\"fields\":[]}")), Person { name: "fallback", age: -1 })
    match bad_person {
        Person { name, age } -> print(name + ":" + to_string(age))
    }
}
"#,
    );
    assert!(
        ok,
        "native JSON deriving fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("\"{\"fields\":{\"age\":42,\"name\":\"Ada\"},\"tag\":\"Person\"}\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"Ada:42\""), "{stdout}");
    assert!(
        stdout.contains("\"{\"fields\":[2.5],\"tag\":\"Circle\"}\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"2.5\""), "{stdout}");
    assert!(stdout.contains("\"fallback:-1\""), "{stdout}");
}
