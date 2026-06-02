//! Formatting helpers for the REPL `:info` command.
//!
//! Pure, side-effect-free string building shared by [`super::engine::ReplEngine::info`].
//! The orchestration (classifying a name, querying the compiler, inferring
//! constructor/value signatures) lives in the engine; this module only knows how to
//! render the resulting member lists and which built-in types carry constructors.

/// Built-in types that are `TypeConstructor` variants rather than registry ADTs.
/// `Option` / `Either` have constructors (listed as `(name, arity)` so the engine
/// can eta-expand them for signature inference); the rest are abstract built-ins
/// with no user constructors and render as a bare `-- built-in type` note.
const BUILTIN_TYPES: &[(&str, &[(&str, usize)])] = &[
    ("Option", &[("None", 0), ("Some", 1)]),
    ("Either", &[("Left", 1), ("Right", 1)]),
    ("List", &[]),
    ("Array", &[]),
    ("Map", &[]),
    ("Int", &[]),
    ("Float", &[]),
    ("Bool", &[]),
    ("String", &[]),
    ("Char", &[]),
    ("Unit", &[]),
];

/// The constructors of a built-in type as `(name, arity)` pairs (`Option` →
/// `[("None", 0), ("Some", 1)]`), or `None` if `name` is not a known built-in type.
/// An empty slice means a known abstract built-in with no user constructors
/// (`List`, `Int`, …).
pub(super) fn builtin_type_constructors(name: &str) -> Option<&'static [(&'static str, usize)]> {
    BUILTIN_TYPES
        .iter()
        .find(|(ty, _)| *ty == name)
        .map(|(_, ctors)| *ctors)
}

/// If `name` is a constructor of a built-in type (`Some` → `Option`), the owning
/// type and the constructor's arity; `None` otherwise. Built-in constructors
/// (`Some`/`None`/`Left`/`Right`) aren't in the ADT registry, so `:info` resolves
/// their owning type from this table.
pub(super) fn builtin_constructor_parent(name: &str) -> Option<(&'static str, usize)> {
    BUILTIN_TYPES.iter().find_map(|(ty, ctors)| {
        ctors
            .iter()
            .find(|(ctor, _)| *ctor == name)
            .map(|(_, arity)| (*ty, *arity))
    })
}

/// Render a `type <name>` block listing each constructor with its signature. An
/// empty member list renders the header with a `-- built-in type` note.
pub(super) fn format_type_block(name: &str, ctor_sigs: &[(String, String)]) -> String {
    format_members(format!("type {name}"), ctor_sigs, "built-in type")
}

/// Render an `effect <name>` block listing each operation with its signature. An
/// empty member list renders the header with a `-- built-in effect` note.
pub(super) fn format_effect_block(name: &str, ops: &[(String, String)]) -> String {
    format_members(format!("effect {name}"), ops, "built-in effect")
}

/// Shared layout for a header plus `  name : signature` member lines, with the
/// member names left-padded to a common width so the `:` columns align. An empty
/// member list yields `<header>    -- <builtin_note>`.
fn format_members(header: String, members: &[(String, String)], builtin_note: &str) -> String {
    if members.is_empty() {
        return format!("{header}    -- {builtin_note}");
    }
    let width = members
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = header;
    for (name, sig) in members {
        out.push_str(&format!("\n  {name:<width$} : {sig}"));
    }
    out
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
    fn builtin_lookup_hits_and_misses() {
        assert_eq!(
            builtin_type_constructors("Option"),
            Some(&[("None", 0), ("Some", 1)][..])
        );
        assert_eq!(
            builtin_type_constructors("Either"),
            Some(&[("Left", 1), ("Right", 1)][..])
        );
        assert_eq!(builtin_type_constructors("List"), Some(&[][..]));
        assert_eq!(builtin_type_constructors("Color"), None);
    }

    #[test]
    fn builtin_constructor_parent_resolves_owning_type() {
        assert_eq!(builtin_constructor_parent("Some"), Some(("Option", 1)));
        assert_eq!(builtin_constructor_parent("None"), Some(("Option", 0)));
        assert_eq!(builtin_constructor_parent("Right"), Some(("Either", 1)));
        assert_eq!(builtin_constructor_parent("Red"), None);
    }

    #[test]
    fn type_block_lists_constructors() {
        let block = format_type_block(
            "Option",
            &pairs(&[("None", "Option a"), ("Some", "(a) -> Option a")]),
        );
        assert_eq!(
            block,
            "type Option\n  None : Option a\n  Some : (a) -> Option a"
        );
    }

    #[test]
    fn empty_type_block_is_a_builtin_note() {
        assert_eq!(
            format_type_block("List", &[]),
            "type List    -- built-in type"
        );
    }

    #[test]
    fn effect_block_lists_operations_aligned() {
        let block = format_effect_block(
            "Console",
            &pairs(&[
                ("print", "(a) -> () with Console"),
                ("println", "(a) -> () with Console"),
            ]),
        );
        // Names padded to the widest ("println") so the colons line up.
        assert_eq!(
            block,
            "effect Console\n  print   : (a) -> () with Console\n  println : (a) -> () with Console"
        );
    }

    #[test]
    fn empty_effect_block_is_a_builtin_note() {
        assert_eq!(
            format_effect_block("Time", &[]),
            "effect Time    -- built-in effect"
        );
    }
}
