/// Format error message by replacing {} placeholders with values
///
/// # Example
/// ```
/// use flux::frontend::diagnostics::format_message;
/// let msg = format_message("Expected {}, got {}.", &["Int", "String"]);
/// assert_eq!(msg, "Expected Int, got String.");
/// ```
pub fn format_message(template: &str, values: &[&str]) -> String {
    let mut result = template.to_string();
    for value in values {
        result = result.replacen("{}", value, 1);
    }
    result
}

/// Format message using named placeholders (future enhancement)
///
/// # Example
/// ```
/// use flux::frontend::diagnostics::format_message_named;
/// let msg = format_message_named("Cannot access {member} in {module}.",
///     &[("member", "foo"), ("module", "Bar")]);
/// ```
#[allow(dead_code)]
pub fn format_message_named(template: &str, args: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in args {
        let placeholder = format!("{{{}}}", name);
        result = result.replace(&placeholder, value);
    }
    result
}
