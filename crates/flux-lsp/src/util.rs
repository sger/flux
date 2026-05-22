//! Small cross-cutting helpers shared by otherwise-unrelated modules.

/// Best-effort human-readable text from a caught panic payload — the value
/// [`std::panic::catch_unwind`] hands back in its `Err` arm. Recovers the
/// `&str` or `String` a `panic!` carries, or a placeholder for anything else.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::panic_message;

    #[test]
    fn recovers_str_and_string_payloads() {
        // A string-literal panic carries a `&'static str` payload.
        let literal = std::panic::catch_unwind(|| panic!("boom")).unwrap_err();
        assert_eq!(panic_message(literal.as_ref()), "boom");

        // A formatted panic carries an owned `String` payload.
        let formatted =
            std::panic::catch_unwind(|| panic!("{}", String::from("dynamic"))).unwrap_err();
        assert_eq!(panic_message(formatted.as_ref()), "dynamic");

        // An exotic payload still yields a placeholder, not a panic of its own.
        let exotic = std::panic::catch_unwind(|| std::panic::panic_any(42u8)).unwrap_err();
        assert_eq!(panic_message(exotic.as_ref()), "unknown panic");
    }
}
