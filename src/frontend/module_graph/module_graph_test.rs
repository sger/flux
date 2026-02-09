use std::path::{Path, PathBuf};

use crate::frontend::{
    block::Block,
    diagnostics::{
        IMPORT_NOT_FOUND, INVALID_MODULE_ALIAS, INVALID_MODULE_NAME, MULTIPLE_MODULES,
        SCRIPT_NOT_IMPORTABLE,
    },
    expression::Expression,
    interner::Interner,
    position::{Position, Span},
    program::Program,
    statement::Statement,
};

use super::{
    import_binding_name, is_valid_module_alias, is_valid_module_name, module_binding_name,
    module_resolution::{normalize_roots, resolve_imports, validate_file_kind},
};

fn pos(line: usize, column: usize) -> Position {
    Position::new(line, column)
}

fn span(line: usize, column: usize) -> Span {
    Span::new(pos(line, column), pos(line, column))
}

fn temp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    path.push(format!("flux_module_graph_{}_{}_{}", name, pid, nanos));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn module_binding_helpers() {
    assert_eq!(module_binding_name("Foo"), "Foo");
    assert_eq!(import_binding_name("Foo", None), "Foo");
    assert_eq!(import_binding_name("Foo", Some("Bar")), "Bar");

    assert!(is_valid_module_name("Foo"));
    assert!(is_valid_module_name("Foo.Bar"));
    assert!(!is_valid_module_name("foo"));
    assert!(!is_valid_module_name("Foo.bar"));

    assert!(is_valid_module_alias("Foo"));
    assert!(!is_valid_module_alias("foo"));
}

#[test]
fn normalize_roots_dedups() {
    let root = temp_dir("roots");
    let roots = normalize_roots(&[root.clone(), root.clone()]);
    assert_eq!(roots.len(), 1);
}

#[test]
fn validate_file_kind_multiple_modules() {
    let mut interner = Interner::new();
    let foo_sym = interner.intern("Foo");
    let bar_sym = interner.intern("Bar");

    let program = Program {
        statements: vec![
            Statement::Module {
                name: foo_sym,
                body: Block {
                    statements: vec![],
                    span: Span::default(),
                },
                span: span(1, 0),
            },
            Statement::Module {
                name: bar_sym,
                body: Block {
                    statements: vec![],
                    span: Span::default(),
                },
                span: span(2, 0),
            },
        ],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/Module.flx");

    let err = validate_file_kind(path, &program, true, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(MULTIPLE_MODULES.code));
}

#[test]
fn validate_file_kind_script_not_importable() {
    let interner = Interner::new();
    let program = Program::new();
    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/Script.flx");

    let err = validate_file_kind(path, &program, false, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(SCRIPT_NOT_IMPORTABLE.code));
}

#[test]
fn resolve_imports_invalid_name() {
    let mut interner = Interner::new();
    let foo_sym = interner.intern("foo");

    let program = Program {
        statements: vec![Statement::Import {
            name: foo_sym,
            alias: None,
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/main.flx");

    let err = resolve_imports(path, &program, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(INVALID_MODULE_NAME.code));
}

#[test]
fn resolve_imports_invalid_alias() {
    let mut interner = Interner::new();
    let foo_sym = interner.intern("Foo");
    let bar_sym = interner.intern("bar");

    let program = Program {
        statements: vec![Statement::Import {
            name: foo_sym,
            alias: Some(bar_sym),
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/main.flx");

    let err = resolve_imports(path, &program, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(INVALID_MODULE_ALIAS.code));
}

#[test]
fn resolve_imports_missing_module() {
    let mut interner = Interner::new();
    let foo_bar_sym = interner.intern("Foo.Bar");

    let program = Program {
        statements: vec![Statement::Import {
            name: foo_bar_sym,
            alias: None,
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let root = temp_dir("missing_module");
    let roots = vec![root];
    let path = Path::new("/tmp/main.flx");

    let err = resolve_imports(path, &program, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(IMPORT_NOT_FOUND.code));
}

#[test]
fn resolve_imports_no_imports_returns_empty() {
    let interner = Interner::new();

    let program = Program {
        statements: vec![Statement::Expression {
            expression: Expression::Integer {
                value: 1,
                span: span(1, 0),
            },
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/main.flx");

    let imports = resolve_imports(path, &program, &roots, &interner).unwrap();
    assert!(imports.is_empty());
}
