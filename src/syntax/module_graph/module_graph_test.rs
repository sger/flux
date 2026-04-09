use std::path::{Path, PathBuf};

use crate::syntax::expression::ExprId;
use crate::syntax::{
    block::Block, expression::Expression, interner::Interner, program::Program,
    statement::Statement,
};

use crate::diagnostics::{
    IMPORT_NOT_FOUND, INVALID_MODULE_ALIAS, INVALID_MODULE_NAME, MODULE_PATH_MISMATCH,
    MULTIPLE_MODULES, SCRIPT_NOT_IMPORTABLE,
    position::{Position, Span},
};

use super::{
    ImportEdge, ModuleGraph, ModuleId, ModuleKind, ModuleNode, import_binding_name,
    is_valid_module_alias, is_valid_module_name, module_binding_name,
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
fn validate_file_kind_module_path_mismatch_uses_stable_display_path() {
    let mut interner = Interner::new();
    let debug_sym = interner.intern("Debug.IceDemo");
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let path_str = if let Some(rest) = format!("{cwd}/examples/Debug/IceDemo.flx").strip_prefix('/')
    {
        format!("//?/{rest}")
    } else {
        format!("//?/{cwd}/examples/Debug/IceDemo.flx")
    };
    let path_buf = PathBuf::from(path_str);

    let program = Program {
        statements: vec![Statement::Module {
            name: debug_sym,
            body: Block {
                statements: vec![],
                span: Span::default(),
            },
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots = vec![
        PathBuf::from(
            if let Some(rest) = format!("{cwd}/examples/Debug").strip_prefix('/') {
                format!("//?/{rest}")
            } else {
                format!("//?/{cwd}/examples/Debug")
            },
        ),
        PathBuf::from(if let Some(rest) = format!("{cwd}/src").strip_prefix('/') {
            format!("//?/{rest}")
        } else {
            format!("//?/{cwd}/src")
        }),
    ];

    let err = validate_file_kind(&path_buf, &program, false, &roots, &interner).unwrap_err();
    assert_eq!(err[0].code(), Some(MODULE_PATH_MISMATCH.code));
    let rendered = err[0].render(None, None);

    assert!(rendered.contains(
        "Module name `Debug.IceDemo` doesn't match file path `examples/Debug/IceDemo.flx`."
    ));
}

#[test]
fn resolve_imports_invalid_name() {
    let mut interner = Interner::new();
    let foo_sym = interner.intern("foo");

    let program = Program {
        statements: vec![Statement::Import {
            name: foo_sym,
            alias: None,
            except: vec![],
            exposing: crate::syntax::statement::ImportExposing::None,
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
            except: vec![],
            exposing: crate::syntax::statement::ImportExposing::None,
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
            except: vec![],
            exposing: crate::syntax::statement::ImportExposing::None,
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
fn topo_levels_group_independent_dependencies() {
    let leaf_a = ModuleId("/tmp/A.flx".to_string());
    let leaf_b = ModuleId("/tmp/B.flx".to_string());
    let entry = ModuleId("/tmp/Main.flx".to_string());

    let graph = ModuleGraph {
        entry: entry.clone(),
        nodes: std::collections::HashMap::from([
            (
                leaf_a.clone(),
                ModuleNode {
                    id: leaf_a.clone(),
                    path: PathBuf::from("/tmp/A.flx"),
                    kind: ModuleKind::User,
                    program: Program::new(),
                    imports: vec![],
                },
            ),
            (
                leaf_b.clone(),
                ModuleNode {
                    id: leaf_b.clone(),
                    path: PathBuf::from("/tmp/B.flx"),
                    kind: ModuleKind::User,
                    program: Program::new(),
                    imports: vec![],
                },
            ),
            (
                entry.clone(),
                ModuleNode {
                    id: entry.clone(),
                    path: PathBuf::from("/tmp/Main.flx"),
                    kind: ModuleKind::User,
                    program: Program::new(),
                    imports: vec![
                        ImportEdge {
                            name: "A".to_string(),
                            position: Position::default(),
                            target: leaf_a.clone(),
                            target_path: PathBuf::from("/tmp/A.flx"),
                        },
                        ImportEdge {
                            name: "B".to_string(),
                            position: Position::default(),
                            target: leaf_b.clone(),
                            target_path: PathBuf::from("/tmp/B.flx"),
                        },
                    ],
                },
            ),
        ]),
        order: vec![leaf_a, leaf_b, entry],
    };

    let levels = graph.topo_levels();
    assert_eq!(levels.len(), 2);
    assert_eq!(levels[0].len(), 2);
    assert_eq!(levels[1].len(), 1);
    assert_eq!(levels[1][0].path, PathBuf::from("/tmp/Main.flx"));
}

#[test]
fn build_graph_marks_flow_modules_as_stdlib() {
    let root = temp_dir("flow_kind");
    let lib_root = root.join("lib");
    let flow_dir = lib_root.join("Flow");
    std::fs::create_dir_all(&flow_dir).unwrap();
    let entry_path = flow_dir.join("List.flx");
    std::fs::write(&entry_path, "module Flow.List {}\n").unwrap();

    let mut interner = Interner::new();
    let module_sym = interner.intern("Flow.List");
    let program = Program {
        statements: vec![Statement::Module {
            name: module_sym,
            body: Block {
                statements: vec![],
                span: Span::default(),
            },
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[lib_root]);
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    let node = result.graph.topo_order()[0];
    assert_eq!(node.kind, ModuleKind::FlowStdlib);
}

#[test]
fn build_graph_marks_non_flow_modules_as_user() {
    let root = temp_dir("user_kind");
    let app_root = root.join("app");
    let app_module_dir = app_root.join("App");
    std::fs::create_dir_all(&app_module_dir).unwrap();
    let entry_path = app_module_dir.join("Main.flx");
    std::fs::write(&entry_path, "module App.Main {}\n").unwrap();

    let mut interner = Interner::new();
    let module_sym = interner.intern("App.Main");
    let program = Program {
        statements: vec![Statement::Module {
            name: module_sym,
            body: Block {
                statements: vec![],
                span: Span::default(),
            },
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[app_root]);
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    let node = result.graph.topo_order()[0];
    assert_eq!(node.kind, ModuleKind::User);
}

#[test]
fn resolve_imports_missing_module_hint_uses_stable_display_paths() {
    let mut interner = Interner::new();
    let module_sym = interner.intern("Debug.ModuleGraphCycleA");

    let program = Program {
        statements: vec![Statement::Import {
            name: module_sym,
            alias: None,
            except: vec![],
            exposing: crate::syntax::statement::ImportExposing::None,
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let make_verbatim = |suffix: &str| {
        let joined = format!("{cwd}/{suffix}");
        if let Some(rest) = joined.strip_prefix('/') {
            PathBuf::from(format!("//?/{rest}"))
        } else {
            PathBuf::from(format!("//?/{joined}"))
        }
    };

    let source_path = make_verbatim("examples/Debug/module_cycle_error.flx");
    let roots = vec![make_verbatim("examples/Debug"), make_verbatim("src")];

    let err = resolve_imports(&source_path, &program, &roots, &interner).unwrap_err();
    let rendered = err[0].render(None, None);

    assert!(rendered.contains(
        "Looked for module `Debug.ModuleGraphCycleA` under roots: examples/Debug, src (imported from examples/Debug/module_cycle_error.flx)."
    ));
}

#[test]
fn resolve_imports_no_imports_returns_empty() {
    let interner = Interner::new();

    let program = Program {
        statements: vec![Statement::Expression {
            expression: Expression::Integer {
                value: 1,
                span: span(1, 0),
                id: ExprId::UNSET,
            },
            has_semicolon: false,
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/main.flx");

    let imports = resolve_imports(path, &program, &roots, &interner).unwrap();
    assert!(imports.is_empty());
}

#[test]
fn resolve_imports_ignores_synthetic_flow_import() {
    let mut interner = Interner::new();
    let base_sym = interner.intern("Flow");
    let print_sym = interner.intern("print");

    let program = Program {
        statements: vec![Statement::Import {
            name: base_sym,
            alias: None,
            except: vec![print_sym],
            exposing: crate::syntax::statement::ImportExposing::None,
            span: span(1, 0),
        }],
        span: Span::default(),
    };

    let roots: Vec<PathBuf> = Vec::new();
    let path = Path::new("/tmp/main.flx");

    let imports = resolve_imports(path, &program, &roots, &interner).unwrap();
    assert!(imports.is_empty());
}
