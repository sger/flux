//! Formatting for the REPL `:browse` command.
//!
//! Pure string building shared by [`super::engine::ReplEngine::browse`]. Renders
//! the in-scope value bindings grouped into the session's own definitions and the
//! auto-exposed prelude, each line `name : type` with the `:` columns aligned.
//! Gathering the names and their type schemes lives in the engine / compiler; this
//! module only lays them out.

/// Render the grouped `:browse` listing. `session` are the user's own bindings,
/// `prelude` the auto-exposed library members (already filtered to exclude any the
/// session shadows). Each group is sorted by the caller.
pub(super) fn format_browse(session: &[(String, String)], prelude: &[(String, String)]) -> String {
    let mut out = String::from("Session:");
    if session.is_empty() {
        out.push_str("\n  (no bindings yet — define some with `let` / `fn`)");
    } else {
        append_members(&mut out, session);
    }
    out.push_str("\n\nPrelude:");
    if prelude.is_empty() {
        out.push_str("\n  (nothing in scope)");
    } else {
        append_members(&mut out, prelude);
    }
    out
}

/// Append `  name : type` lines, left-padding names to a common width so the
/// `:` columns line up within the group.
fn append_members(out: &mut String, members: &[(String, String)]) {
    let width = members
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, ty) in members {
        out.push_str(&format!("\n  {name:<width$} : {ty}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn groups_session_and_prelude_with_aligned_columns() {
        let block = format_browse(
            &pairs(&[("x", "Int"), ("greet", "(String) -> String")]),
            &pairs(&[("map", "forall a b. ((a) -> b, List<a>) -> List<b>")]),
        );
        assert_eq!(
            block,
            "Session:\n  x     : Int\n  greet : (String) -> String\n\n\
             Prelude:\n  map : forall a b. ((a) -> b, List<a>) -> List<b>"
        );
    }

    #[test]
    fn empty_session_shows_a_hint() {
        let block = format_browse(&[], &pairs(&[("abs", "(Int) -> Int")]));
        assert_eq!(
            block,
            "Session:\n  (no bindings yet — define some with `let` / `fn`)\n\n\
             Prelude:\n  abs : (Int) -> Int"
        );
    }
}
