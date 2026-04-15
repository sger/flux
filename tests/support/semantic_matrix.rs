use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use flux::{
    ast::type_infer::{InferProgramConfig, InferProgramResult, infer_program},
    bytecode::compiler::Compiler,
    core::display::CoreDisplayMode,
    diagnostics::{Diagnostic, DiagnosticsAggregator, render_diagnostics},
    syntax::{
        interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser,
        statement::Statement, type_expr::TypeExpr,
    },
    types::{
        class_env::ClassEnv, infer_effect_row::InferEffectRow, infer_type::InferType,
        scheme::Scheme, type_constructor::TypeConstructor,
    },
};

pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

pub fn semantic_fixture_root() -> PathBuf {
    workspace_root().join("tests/fixtures/semantic_types")
}

pub fn semantic_fixture_path(rel: &str) -> PathBuf {
    semantic_fixture_root().join(rel)
}

pub fn combined_output(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

pub fn first_error_code(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .find_map(|diag| diag.code().map(str::to_string))
        .unwrap_or_default()
}

pub struct InferredFixture {
    pub result: InferProgramResult,
    pub interner: Interner,
    pub source: String,
}

pub struct CompiledFixture {
    pub compiler: Compiler,
    pub interner: Interner,
    pub program: flux::syntax::program::Program,
    pub source: String,
}

fn parse_source(source: &str) -> (flux::syntax::program::Program, Interner) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );
    (program, parser.take_interner())
}

fn collect_effect_sigs(
    statements: &[Statement],
    out: &mut HashMap<(flux::syntax::Identifier, flux::syntax::Identifier), TypeExpr>,
) {
    for statement in statements {
        match statement {
            Statement::EffectDecl { name, ops, .. } => {
                for op in ops {
                    out.insert((*name, op.name), op.type_expr.clone());
                }
            }
            Statement::Module { body, .. } => collect_effect_sigs(&body.statements, out),
            _ => {}
        }
    }
}

fn build_class_env(
    program: &flux::syntax::program::Program,
    interner: &mut Interner,
) -> (ClassEnv, Vec<Diagnostic>) {
    let mut class_env = ClassEnv::new();
    class_env.register_builtins(interner);
    let diags = class_env.collect_from_statements(&program.statements, interner);
    (class_env, diags)
}

pub fn infer_fixture(rel: &str) -> InferredFixture {
    let source = std::fs::read_to_string(semantic_fixture_path(rel))
        .unwrap_or_else(|err| panic!("failed to read fixture {rel}: {err}"));
    let (program, mut interner) = parse_source(&source);
    let mut effect_op_sigs = HashMap::new();
    collect_effect_sigs(&program.statements, &mut effect_op_sigs);
    let (class_env, class_diags) = build_class_env(&program, &mut interner);
    assert!(
        class_diags.is_empty(),
        "class env diagnostics for {rel}:\n{}",
        render_diagnostics(&class_diags, Some(&source), None)
    );
    let flow_module_symbol = interner.intern("Flow");
    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: Some(rel.into()),
            strict_inference: false,
            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            known_flow_names: HashSet::new(),
            flow_module_symbol,
            preloaded_effect_op_signatures: effect_op_sigs,
            class_env: Some(class_env),
        },
    );
    InferredFixture {
        result,
        interner,
        source,
    }
}

pub fn compile_single_file_fixture(rel: &str) -> Result<CompiledFixture, Vec<Diagnostic>> {
    let source = std::fs::read_to_string(semantic_fixture_path(rel))
        .unwrap_or_else(|err| panic!("failed to read fixture {rel}: {err}"));
    let (program, interner) = parse_source(&source);
    let mut compiler = Compiler::new_with_interner(rel, interner.clone());
    match compiler.compile(&program) {
        Ok(()) => Ok(CompiledFixture {
            compiler,
            interner,
            program,
            source,
        }),
        Err(diags) => Err(diags),
    }
}

pub fn dump_core_debug_fixture(rel: &str) -> String {
    let compiled = compile_single_file_fixture(rel)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(rel), None)));
    let mut compiler = compiled.compiler;
    compiler
        .dump_core_with_opts(&compiled.program, false, CoreDisplayMode::Debug)
        .unwrap_or_else(|diag| {
            panic!(
                "{}",
                render_diagnostics(&[diag], Some(&compiled.source), None)
            )
        })
}

pub fn compile_module_fixture(rel: &str) -> Result<CompiledFixture, Vec<Diagnostic>> {
    let entry_path = semantic_fixture_path(rel);
    let source = std::fs::read_to_string(&entry_path)
        .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", entry_path.display()));
    let (program, interner) = parse_source(&source);
    let fixture_root = semantic_fixture_root();
    let entry_parent = entry_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fixture_root.clone());
    let roots = vec![entry_parent, fixture_root, workspace_root().join("lib")];
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &roots);

    let mut all_diags = graph_result.diagnostics.clone();
    if DiagnosticsAggregator::new(&all_diags)
        .report()
        .counts
        .errors
        > 0
    {
        return Err(all_diags);
    }

    let mut compiler = Compiler::new_with_interner(
        entry_path.to_string_lossy().to_string(),
        graph_result.interner,
    );
    for node in graph_result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        if let Err(diags) = compiler.compile(&node.program) {
            all_diags.extend(diags);
            return Err(all_diags);
        }
    }

    Ok(CompiledFixture {
        interner: compiler.interner.clone(),
        compiler,
        program,
        source,
    })
}

pub fn run_fixture(rel: &str, native: bool) -> Output {
    let fixture = semantic_fixture_path(rel);
    let fixture_root = semantic_fixture_root();
    let entry_parent = fixture
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fixture_root.clone());
    let lib_root = workspace_root().join("lib");
    let mut args: Vec<String> = Vec::new();
    if native {
        args.push("--native".into());
    }
    args.push("--no-cache".into());
    args.push(fixture.display().to_string());
    args.push("--roots-only".into());
    args.push("--root".into());
    args.push(entry_parent.display().to_string());
    args.push("--root".into());
    args.push(fixture_root.display().to_string());
    args.push("--root".into());
    args.push(lib_root.display().to_string());

    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|err| panic!("failed to run fixture {rel}: {err}"))
}

fn alpha_name(index: usize) -> String {
    let letter = ((index % 26) as u8 + b'a') as char;
    let suffix = index / 26;
    if suffix == 0 {
        letter.to_string()
    } else {
        format!("{letter}{suffix}")
    }
}

struct SchemeFormatter<'a> {
    interner: &'a Interner,
    names: HashMap<u32, String>,
    next: usize,
}

impl<'a> SchemeFormatter<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            names: HashMap::new(),
            next: 0,
        }
    }

    fn intern_var_name(&mut self, id: u32) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let name = alpha_name(self.next);
        self.next += 1;
        self.names.insert(id, name.clone());
        name
    }

    fn adt_name(&self, sym: flux::syntax::Identifier) -> String {
        self.interner
            .try_resolve(sym)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{sym}"))
    }

    fn effect_name(&self, sym: flux::syntax::Identifier) -> String {
        self.interner
            .try_resolve(sym)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{sym}"))
    }

    fn format_constructor(&self, constructor: &TypeConstructor) -> String {
        match constructor {
            TypeConstructor::Adt(sym) => self.adt_name(*sym),
            other => other.to_string(),
        }
    }

    fn format_effects(&mut self, effects: &InferEffectRow) -> String {
        let mut concrete: Vec<_> = effects
            .concrete()
            .iter()
            .map(|effect| self.effect_name(*effect))
            .collect();
        concrete.sort();
        match effects.tail() {
            Some(tail) if concrete.is_empty() => format!("|{}", self.intern_var_name(tail)),
            Some(tail) => format!("{}, |{}", concrete.join(", "), self.intern_var_name(tail)),
            None => concrete.join(", "),
        }
    }

    fn format_type(&mut self, infer_type: &InferType) -> String {
        match infer_type {
            InferType::Var(id) => self.intern_var_name(*id),
            InferType::Con(constructor) => self.format_constructor(constructor),
            InferType::App(constructor, args) => format!(
                "{}<{}>",
                self.format_constructor(constructor),
                args.iter()
                    .map(|arg| self.format_type(arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InferType::Fun(params, ret, effects) => {
                let params = params
                    .iter()
                    .map(|param| self.format_type(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut rendered = format!("({params}) -> {}", self.format_type(ret));
                if !effects.concrete().is_empty() || effects.tail().is_some() {
                    rendered.push_str(" with ");
                    rendered.push_str(&self.format_effects(effects));
                }
                rendered
            }
            InferType::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| self.format_type(element))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InferType::HktApp(head, args) => format!(
                "{}<{}>",
                self.format_type(head),
                args.iter()
                    .map(|arg| self.format_type(arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn format_scheme(mut self, scheme: &Scheme) -> String {
        let mut forall = scheme.forall.clone();
        forall.extend(scheme.infer_type.free_vars());
        for constraint in &scheme.constraints {
            forall.extend(constraint.type_vars.iter().copied());
        }
        forall.sort_unstable();
        forall.dedup();

        let constraints = if scheme.constraints.is_empty() {
            String::new()
        } else {
            let mut rendered = scheme
                .constraints
                .iter()
                .map(|constraint| {
                    let class_name = self
                        .interner
                        .try_resolve(constraint.class_name)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{}", constraint.class_name));
                    let args = constraint
                        .type_vars
                        .iter()
                        .map(|var| self.intern_var_name(*var))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{class_name}<{args}>")
                })
                .collect::<Vec<_>>();
            rendered.sort();
            format!("{} => ", rendered.join(", "))
        };

        let ty = self.format_type(&scheme.infer_type);
        if forall.is_empty() {
            format!("{constraints}{ty}")
        } else {
            for var in &forall {
                self.intern_var_name(*var);
            }
            let mut forall_named = forall
                .iter()
                .map(|var| (*var, self.intern_var_name(*var)))
                .collect::<Vec<_>>();
            forall_named.sort_by(|left, right| left.1.cmp(&right.1));
            let vars = forall_named
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!("forall {vars}. {constraints}{ty}")
        }
    }
}

pub fn normalize_scheme(interner: &Interner, scheme: &Scheme) -> String {
    SchemeFormatter::new(interner).format_scheme(scheme)
}

pub fn assert_named_schemes(rel: &str, expected: &[(&str, &str)]) {
    let inferred = infer_fixture(rel);
    assert!(
        inferred.result.diagnostics.is_empty(),
        "unexpected inference diagnostics for {rel}:\n{}",
        render_diagnostics(&inferred.result.diagnostics, Some(&inferred.source), None)
    );
    for (name, expected_scheme) in expected {
        let symbol = inferred
            .interner
            .lookup(name)
            .unwrap_or_else(|| panic!("binding `{name}` is not interned in {rel}"));
        let scheme = inferred
            .result
            .type_env
            .lookup(symbol)
            .unwrap_or_else(|| panic!("missing inferred binding `{name}` in {rel}"));
        let got = normalize_scheme(&inferred.interner, scheme);
        assert_eq!(got, *expected_scheme, "binding `{name}` in {rel}");
    }
}

pub fn assert_module_member_schemes(rel: &str, expected: &[(&str, &str, &str)]) {
    let compiled = compile_module_fixture(rel).unwrap_or_else(|diags| {
        panic!(
            "{}",
            render_diagnostics(
                &diags,
                Some(&semantic_fixture_path(rel).display().to_string()),
                None
            )
        )
    });
    for (module_name, member_name, expected_scheme) in expected {
        let module = compiled
            .interner
            .lookup(module_name)
            .unwrap_or_else(|| panic!("module `{module_name}` is not interned in {rel}"));
        let member = compiled
            .interner
            .lookup(member_name)
            .unwrap_or_else(|| panic!("member `{member_name}` is not interned in {rel}"));
        let scheme = compiled
            .compiler
            .cached_member_schemes()
            .get(&(module, member))
            .unwrap_or_else(|| {
                panic!("missing cached module scheme `{module_name}.{member_name}` in {rel}")
            });
        let got = normalize_scheme(&compiled.interner, scheme);
        assert_eq!(
            got, *expected_scheme,
            "`{module_name}.{member_name}` in {rel}"
        );
    }
}
