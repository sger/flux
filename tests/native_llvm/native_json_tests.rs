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
    let ran = flux_source(&source, "native_json_tests").native().run();
    (ran.stdout, ran.stderr, ran.success)
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
fn native_json_integer_numbers_round_trip_without_precision_loss() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json
import Flow.Json exposing (..)

fn main() with IO {
    print(Json.stringify(Json.parse("9007199254740993")))
    print(Json.stringify(Json.int(9007199254740993)))
}
"#,
    );
    assert!(
        ok,
        "native JSON integer fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.matches("\"9007199254740993\"").count(),
        2,
        "{stdout}"
    );
}

#[test]
fn native_json_int_decode_rejects_fractional_and_unsafe_float_numbers() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json
import Flow.Json exposing (..)

fn main() with IO {
    print(Json.result_or(Json.as_int(Json.parse("42"), "$"), -1))
    print(Json.result_or(Json.as_int(Json.number(42.0), "$"), -1))
    let fractional = Json.as_int(Json.number(42.5), "$")
    print(Json.result_is_ok(fractional))
    print(Json.error_message(fractional))
    print(Json.result_or(Json.as_int(Json.number(9007199254740994.0), "$"), -1))
}
"#,
    );
    assert!(
        ok,
        "native JSON int decode fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("42"), "{stdout}");
    assert!(stdout.contains("-1"), "{stdout}");
    assert!(stdout.contains("false"), "{stdout}");
    assert!(
        stdout.contains("$: expected safe integral JSON number"),
        "{stdout}"
    );
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

#[test]
fn native_json_derived_decoders_return_structured_json_errors() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json
import Flow.Json exposing (..)

data Person { Person { name: String, age: Int } } deriving (Json.Encode, Json.Decode)

fn error_or(result, fallback) -> String {
    let _fallback = Json.result_or(result, fallback)
    Json.error_message(result)
}

fn main() with IO {
    let fallback = Person { name: "fallback", age: -1 }
    print(error_or(decode(Json.parse("{\"tag\":\"Nope\",\"fields\":[]}")), fallback))
    print(error_or(decode(Json.parse("{\"tag\":\"Person\",\"fields\":{\"name\":\"Ada\"}}")), fallback))
    print(error_or(decode(Json.parse("{\"tag\":\"Person\",\"fields\":{\"name\":\"Ada\",\"age\":\"old\"}}")), fallback))
}
"#,
    );
    assert!(
        ok,
        "native JSON structured error fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("unknown constructor"), "{stdout}");
    assert!(stdout.contains("missing JSON field"), "{stdout}");
    assert!(stdout.contains("expected JSON number"), "{stdout}");
}
