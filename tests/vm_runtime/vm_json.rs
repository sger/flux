//! VM Flow.Json parser/stringifier smoke tests.

use crate::support::flux_runner;

fn run_source(source: &str) -> (String, String, bool) {
    flux_runner::run_flux(source, "json")
}

#[test]
fn value_constructors_cover_core_kinds() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json
import Flow.Map as Map

fn main() with IO {
    let obj = Json.object(Map.set(Map.set({}, "flag", Json.bool(true)), "name", Json.string("flux")))
    print(Json.stringify(Json.null()))
    print(Json.stringify(Json.number(3.5)))
    print(Json.stringify(Json.array([|Json.string("a"), Json.bool(false)|])))
    print(Json.stringify(obj))
}
"#,
    );
    assert!(ok, "JSON fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("\"null\""), "{stdout}");
    assert!(stdout.contains("\"3.5\""), "{stdout}");
    assert!(stdout.contains("\"[\"a\",false]\""), "{stdout}");
    assert!(stdout.contains("\"{\"flag\":true,\"name\":\"flux\"}\""), "{stdout}");
}

#[test]
fn same_class_contextual_json_encoders_dispatch_elements() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Json as Json
import Flow.Json exposing (encode)

fn main() with IO {
    print(Json.encode_json(encode(Some(42))))
    print(Json.encode_json(encode([1, 2])))
    print(Json.encode_json(encode([|1, 2|])))
}
"#,
    );
    assert!(ok, "contextual JSON encoder fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(stdout, "\"42\"\n\"[1,2]\"\n\"[1,2]\"\n", "{stdout}");
}

/// KI-051: an imported instance's generated methods are defined
/// module-qualified, so marking the bare `__tc_*` name `Imported` made the
/// cached parallel linker demand a global no module defines. Only the cached
/// path catches this — `--no-cache` always printed the right answer.
#[test]
fn imported_contextual_instances_link_on_the_cached_path() {
    let (stdout, stderr, ok) = flux_runner::run_flux_cached(
        r#"
import Flow.Json as Json
import Flow.Json exposing (encode)

fn main() with IO {
    print(Json.encode_json(encode([1, 2])))
    print(Json.encode_json(encode(Some(42))))
}
"#,
        "ki051",
    );
    assert!(
        ok,
        "cached run of imported contextual instances failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "\"[1,2]\"\n\"42\"\n", "{stdout}");
}
