use crate::runtime::vm::trace;

#[test]
fn strip_ansi_removes_escape_sequences() {
    let input = "Error: \u{1b}[31mred\u{1b}[0m text";
    let cleaned = trace::strip_ansi(input);
    assert_eq!(cleaned, "Error: red text");
}
