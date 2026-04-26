//! Drift check between the compiler's built-in effect registry and the
//! `lib/Flow/Effects.flx` stdlib spec file (Proposal 0161 Phase 1).
//!
//! The compiler seeds built-in effect aliases (`IO`, `Time`) and operational
//! labels (`Console`, `FileSystem`, `Stdin`, `Clock`) programmatically in
//! `Compiler::seed_builtin_effect_aliases` and
//! `Compiler::seed_builtin_effect_operations`. The `Flow.Effects` source file
//! is documentation only — it is not compiled and not auto-imported. If the
//! two sources drift (a new label in the seed that the spec doesn't mention,
//! or vice versa), neither test nor user will notice until the next time
//! someone reads the file.
//!
//! This test parses `lib/Flow/Effects.flx` and asserts that the set of
//! `effect Foo { ... }` declarations, their operation names, and the set of
//! `alias Foo = ...` declarations exactly match the authoritative registry
//! hand-mirrored here from `src/syntax/builtin_effects.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use flux::syntax::{lexer::Lexer, parser::Parser, statement::Statement};

/// The authoritative set of built-in effect *labels* the compiler seeds.
/// Mirrored from `src/syntax/builtin_effects.rs` constants and
/// `Compiler::seed_builtin_effect_aliases` /
/// `Compiler::seed_builtin_effect_operations` in `src/compiler/mod.rs`.
///
/// Values: the set of operation names declared on each label (empty set for
/// reserved / phantom labels).
fn authoritative_labels() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let mut out = BTreeMap::new();
    out.insert("Console", BTreeSet::from(["print", "println"]));
    out.insert(
        "FileSystem",
        BTreeSet::from(["read_file", "read_lines", "write_file"]),
    );
    out.insert("Stdin", BTreeSet::from(["read_stdin"]));
    out.insert("Clock", BTreeSet::from(["clock_now", "now_ms"]));
    // Reserved / phantom labels — no operations yet. Seeded by the compiler
    // for classification purposes; `Flow.Effects` documents them as effect
    // declarations with no body.
    out.insert("Random", BTreeSet::new());
    out.insert("NonDet", BTreeSet::new());
    out.insert("Div", BTreeSet::new());
    out.insert("Exn", BTreeSet::new());
    out.insert("Panic", BTreeSet::new());
    // Developer tracing — one operation backed by the DebugTrace primop.
    out.insert("Debug", BTreeSet::from(["trace"]));
    out
}

/// The authoritative set of built-in effect-row *aliases* the compiler seeds.
/// The values are unordered multi-sets of atom names, since the spec file may
/// order the alias body differently from the seed without semantic change.
fn authoritative_aliases() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let mut out = BTreeMap::new();
    out.insert("IO", BTreeSet::from(["Console", "FileSystem", "Stdin"]));
    out.insert("Time", BTreeSet::from(["Clock"]));
    out
}

/// Parse `lib/Flow/Effects.flx` and extract its declared labels + ops and
/// aliases.
fn parse_spec_file() -> (
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec_path = workspace_root.join("lib").join("Flow").join("Effects.flx");
    let source = std::fs::read_to_string(&spec_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", spec_path.display()));

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser rejected lib/Flow/Effects.flx: {:?}",
        parser.errors
    );

    let interner = parser.take_interner();
    let mut labels: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    fn walk(
        stmts: &[Statement],
        interner: &flux::syntax::interner::Interner,
        labels: &mut BTreeMap<String, BTreeSet<String>>,
        aliases: &mut BTreeMap<String, BTreeSet<String>>,
    ) {
        for stmt in stmts {
            match stmt {
                Statement::Module { body, .. } => {
                    walk(&body.statements, interner, labels, aliases);
                }
                Statement::EffectDecl { name, ops, .. } => {
                    let label_name = interner.resolve(*name).to_string();
                    let op_names: BTreeSet<String> = ops
                        .iter()
                        .map(|op| interner.resolve(op.name).to_string())
                        .collect();
                    labels.insert(label_name, op_names);
                }
                Statement::EffectAlias {
                    name, expansion, ..
                } => {
                    let alias_name = interner.resolve(*name).to_string();
                    let atoms: BTreeSet<String> = expansion
                        .normalized_concrete_names()
                        .iter()
                        .map(|id| interner.resolve(*id).to_string())
                        .collect();
                    aliases.insert(alias_name, atoms);
                }
                _ => {}
            }
        }
    }

    walk(&program.statements, &interner, &mut labels, &mut aliases);
    (labels, aliases)
}

fn to_owned(
    src: &BTreeMap<&'static str, BTreeSet<&'static str>>,
) -> BTreeMap<String, BTreeSet<String>> {
    src.iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect(),
            )
        })
        .collect()
}

#[test]
fn flow_effects_spec_labels_match_seed() {
    let (spec_labels, _spec_aliases) = parse_spec_file();
    let expected = to_owned(&authoritative_labels());

    let spec_keys: BTreeSet<_> = spec_labels.keys().cloned().collect();
    let expected_keys: BTreeSet<_> = expected.keys().cloned().collect();

    let missing_from_spec: Vec<_> = expected_keys.difference(&spec_keys).cloned().collect();
    let extra_in_spec: Vec<_> = spec_keys.difference(&expected_keys).cloned().collect();

    assert!(
        missing_from_spec.is_empty() && extra_in_spec.is_empty(),
        "\n\
         Flow.Effects spec drift:\n  \
           labels missing from lib/Flow/Effects.flx (present in compiler seed): {:?}\n  \
           labels in lib/Flow/Effects.flx but not seeded by compiler: {:?}\n\
         \n\
         Keep `lib/Flow/Effects.flx` and `Compiler::seed_builtin_effect_aliases` / \
         `seed_builtin_effect_operations` in sync. See proposal 0161.\n",
        missing_from_spec,
        extra_in_spec,
    );

    for (label, expected_ops) in &expected {
        let spec_ops = spec_labels
            .get(label)
            .expect("label presence was checked above");
        let missing_ops: Vec<_> = expected_ops.difference(spec_ops).cloned().collect();
        let extra_ops: Vec<_> = spec_ops.difference(expected_ops).cloned().collect();
        assert!(
            missing_ops.is_empty() && extra_ops.is_empty(),
            "\n\
             Flow.Effects spec drift on effect `{}`:\n  \
               ops missing from lib/Flow/Effects.flx (present in compiler seed): {:?}\n  \
               ops in lib/Flow/Effects.flx but not seeded by compiler: {:?}\n\
             \n\
             Keep the operation list in sync with \
             `Compiler::seed_builtin_effect_operations`.\n",
            label,
            missing_ops,
            extra_ops,
        );
    }
}

#[test]
fn flow_effects_spec_aliases_match_seed() {
    let (_spec_labels, spec_aliases) = parse_spec_file();
    let expected = to_owned(&authoritative_aliases());

    let spec_keys: BTreeSet<_> = spec_aliases.keys().cloned().collect();
    let expected_keys: BTreeSet<_> = expected.keys().cloned().collect();

    let missing_from_spec: Vec<_> = expected_keys.difference(&spec_keys).cloned().collect();
    let extra_in_spec: Vec<_> = spec_keys.difference(&expected_keys).cloned().collect();

    assert!(
        missing_from_spec.is_empty() && extra_in_spec.is_empty(),
        "\n\
         Flow.Effects alias drift:\n  \
           aliases missing from lib/Flow/Effects.flx: {:?}\n  \
           aliases in lib/Flow/Effects.flx but not seeded by compiler: {:?}\n\
         \n\
         Keep `lib/Flow/Effects.flx` and `Compiler::seed_builtin_effect_aliases` in sync.\n",
        missing_from_spec,
        extra_in_spec,
    );

    for (alias, expected_atoms) in &expected {
        let spec_atoms = spec_aliases
            .get(alias)
            .expect("alias presence was checked above");
        let missing_atoms: Vec<_> = expected_atoms.difference(spec_atoms).cloned().collect();
        let extra_atoms: Vec<_> = spec_atoms.difference(expected_atoms).cloned().collect();
        assert!(
            missing_atoms.is_empty() && extra_atoms.is_empty(),
            "\n\
             Flow.Effects alias `{}` expansion drift:\n  \
               atoms missing from lib/Flow/Effects.flx: {:?}\n  \
               atoms in lib/Flow/Effects.flx but not in compiler seed: {:?}\n",
            alias,
            missing_atoms,
            extra_atoms,
        );
    }
}
