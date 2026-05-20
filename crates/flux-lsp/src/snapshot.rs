use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use flux::ast::type_infer::{InferProgramResult, infer_program};
use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::lsp_support;
use flux::syntax::Identifier;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;

use crate::instance_index::InstanceIndex;
use crate::line_index::{PositionEncoding, PositionMap};
use crate::prelude::Prelude;
use crate::symbol_index::SymbolIndex;

pub struct Snapshot {
    pub text: Arc<str>,
    pub program: Program,
    pub interner: Interner,
    pub infer: Option<InferProgramResult>,
    pub symbol_index: SymbolIndex,
    /// Per-snapshot index of `instance` blocks for class-method
    /// goto-definition. Looked up by the `ClassDispatch` tuple
    /// (`class_name`, `head_type_ctor`, `method_name`) the type
    /// checker stored in `InferProgramResult::class_method_dispatch`.
    pub instance_index: InstanceIndex,
    pub position_map: PositionMap,
    pub diagnostics: Vec<FluxDiagnostic>,
    /// Final segment of every module known to this session's prelude (e.g.
    /// `"String"` for `Flow.String`). Used by hover to label module-prefixed
    /// references like `String.join`. `Arc`-shared from the prelude — these
    /// names do not change between keystrokes.
    pub module_short_names: Arc<HashSet<String>>,
    /// Short module name → member names (e.g. `"String" -> ["join", "split", ...]`).
    /// `Arc`-shared from the prelude so completion after `String.` needs no
    /// per-keystroke rebuild.
    pub module_members: Arc<HashMap<String, Vec<String>>>,
    /// Variant `Identifier` → list of `(field_name, field_type)`. Empty for
    /// positional variants. Drives hover content for record fields
    /// (`alice.name`, `Person { name: ... }`) and completion inside record
    /// literals.
    pub variant_fields: HashMap<Identifier, Vec<(Identifier, TypeExpr)>>,
    /// Variant `Identifier` → list of positional field types. Populated
    /// alongside `variant_fields` for ADT constructors that don't use
    /// named fields (`Left(err)`, `Some(42)`).
    pub variant_positional_fields: HashMap<Identifier, Vec<TypeExpr>>,
    /// Short module name (e.g. `"Math"`) → (parsed program, source text, file
    /// path). Cloned from the prelude at snapshot build time so goto-definition
    /// can jump into imported module files. The `Arc<Program>` makes that
    /// per-keystroke clone a ref-count bump rather than an AST deep copy.
    pub module_programs: HashMap<String, (Arc<Program>, Arc<str>, PathBuf)>,
    /// Identifiers available to the buffer unqualified from outside it: the
    /// prelude/builtin schemes inference was seeded with, plus every Flow name
    /// it recognizes. The name-resolution pass treats these as resolved so it
    /// never flags `print`, `len`, and friends.
    pub base_names: HashSet<Identifier>,
}

impl Snapshot {
    /// Build a snapshot from source text. Inference runs through the shared
    /// `Compiler` held by `prelude`, which already has Flow prelude schemes
    /// loaded into its `cached_member_schemes`. That's what lets `print`,
    /// `Console`, etc. resolve to their real types in this buffer.
    /// `workspace_modules` is the full name of every `module` declared across
    /// the workspace (from [`Workspace`](crate::workspace::Workspace)'s
    /// incremental index). It lets the unimported-module pass flag a path into
    /// a not-yet-imported *sibling* — those never enter `module_programs`
    /// because the module graph only follows existing imports. Pass an empty
    /// slice when no workspace context is available (file-at-a-time mode).
    pub fn build(
        text: Arc<str>,
        prelude: &mut Prelude,
        encoding: PositionEncoding,
        workspace_modules: &[String],
    ) -> Self {
        // Swap the compiler's interner into the buffer's lexer so identifiers
        // in the buffer share IDs with the preloaded schemes. Swap the
        // enriched interner back when parsing finishes.
        let main_interner = std::mem::take(&mut prelude.compiler.interner);
        let lexer = Lexer::new_with_interner(text.as_ref().to_string(), main_interner);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let mut diagnostics = std::mem::take(&mut parser.errors);
        diagnostics.extend(parser.take_warnings());
        prelude.compiler.interner = parser.take_interner();

        let symbol_index = SymbolIndex::build(&program, &prelude.compiler.interner);
        let instance_index = InstanceIndex::build(&program);
        let position_map = PositionMap::new(Arc::clone(&text), encoding);

        // Walk buffer-level `import Flow.*` statements and lazily preload any
        // Flow module not in the auto-prelude (e.g. `Flow.Async`,
        // `Flow.Tcp`). Without this, identifiers from those modules collapse
        // to free type variables during inference.
        load_buffer_imports(&program, prelude);

        let (infer, base_names) = run_inference(&program, &mut prelude.compiler);
        if let Some(result) = &infer {
            diagnostics.extend(result.diagnostics.iter().cloned());
        }

        // The snapshot keeps a clone of the (now enriched) interner so it can
        // resolve symbols independently of subsequent buffer edits.
        let interner = prelude.compiler.interner.clone();

        // Prelude-derived indexes are computed once on the `Prelude` and
        // shared by `Arc` — they change only when a Flow module loads, never
        // between keystrokes.
        let module_short_names = Arc::clone(&prelude.module_short_names);
        let module_members = Arc::clone(&prelude.module_members);

        let (variant_fields, variant_positional_fields) = build_variant_indexes(&program);

        // Clone the prelude's module-program cache so goto-definition can jump
        // to definitions in imported Flow modules without re-parsing them.
        // Cheap now: the values hold `Arc<Program>`, so this is a map of
        // ref-count bumps, not AST deep copies.
        let module_programs = prelude.module_programs.clone();

        let mut snapshot = Snapshot {
            text,
            program,
            interner,
            infer,
            symbol_index,
            instance_index,
            position_map,
            diagnostics,
            module_short_names,
            module_members,
            variant_fields,
            variant_positional_fields,
            module_programs,
            base_names,
        };

        // Inference does not report undefined names (it recovers with fresh type
        // variables), so a dedicated lexical-scope pass supplies the squiggles —
        // and the diagnostics the auto-import quick fix keys off.
        let unresolved = crate::name_resolution::unresolved_name_diagnostics(&snapshot);
        snapshot.diagnostics.extend(unresolved);
        // Flag module-qualified paths whose module isn't imported (Flow stdlib,
        // already-loaded siblings, and — via `workspace_modules` — not-yet-
        // imported siblings); the auto-import quick fix on the squiggle resolves
        // them.
        let missing =
            crate::handlers::auto_import::missing_import_diagnostics(&snapshot, workspace_modules);
        snapshot.diagnostics.extend(missing);
        snapshot
    }
}

/// Walk all `Statement::Data` declarations in `program` and emit two indexes:
/// `(variant_name -> [(field_name, ty)])` for named-field variants, and
/// `(variant_name -> [ty])` for positional variants. Used by hover and
/// completion to look up a field's declared type given just the variant
/// constructor identifier.
type NamedFieldsIndex = HashMap<Identifier, Vec<(Identifier, TypeExpr)>>;
type PositionalFieldsIndex = HashMap<Identifier, Vec<TypeExpr>>;

fn build_variant_indexes(program: &Program) -> (NamedFieldsIndex, PositionalFieldsIndex) {
    let mut named: HashMap<Identifier, Vec<(Identifier, TypeExpr)>> = HashMap::new();
    let mut positional: HashMap<Identifier, Vec<TypeExpr>> = HashMap::new();
    for stmt in &program.statements {
        if let Statement::Data {
            name: data_name,
            variants,
            ..
        } = stmt
        {
            for variant in variants {
                if let Some(field_names) = &variant.field_names {
                    let pairs: Vec<(Identifier, TypeExpr)> = field_names
                        .iter()
                        .zip(variant.fields.iter())
                        .map(|(n, t)| (*n, t.clone()))
                        .collect();
                    named.insert(variant.name, pairs.clone());
                    // Also key by the parent data-type name when it differs
                    // from the variant. Lets `alice.name` (where `alice: Person`)
                    // resolve the same as `Person { name: ... }`. The common
                    // `data X { X { ... } }` pattern lands the same value
                    // under the same key — `insert` is fine.
                    if *data_name != variant.name {
                        named.entry(*data_name).or_insert(pairs);
                    }
                } else if !variant.fields.is_empty() {
                    positional.insert(variant.name, variant.fields.clone());
                }
            }
        }
    }
    (named, positional)
}

fn run_inference(
    program: &Program,
    compiler: &mut flux::compiler::Compiler,
) -> (Option<InferProgramResult>, HashSet<Identifier>) {
    // Clear per-file scratch from the previous buffer (errors, scope state,
    // function effects) without dropping the prelude's `cached_member_schemes`.
    lsp_support::reset_per_file_state(compiler);
    compiler.set_file_path("<buffer>".to_string());

    let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Populate `class_env` from the buffer's class declarations so
        // class-method dispatch resolves during inference. Without this,
        // `lookup_class_method` would return None for buffer-declared
        // classes and the LSP's class-method goto-definition would not
        // fire.
        lsp_support::collect_classes_for_program(compiler, program);
        let config = lsp_support::build_infer_config_for_program(compiler, program);
        // Capture the unqualified names the buffer inherits (prelude/builtin
        // schemes + recognized Flow names) before `config` is consumed; the
        // name-resolution pass treats these as resolved.
        let mut base_names: HashSet<Identifier> =
            config.preloaded_base_schemes.keys().copied().collect();
        base_names.extend(config.known_flow_names.iter().copied());
        let result = infer_program(program, &compiler.interner, config);
        (result, base_names)
    }))
    .ok();
    match captured {
        Some((result, base_names)) => (Some(result), base_names),
        None => (None, HashSet::new()),
    }
}

fn load_buffer_imports(program: &Program, prelude: &mut Prelude) {
    let module_names: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::Import { name, .. } => prelude
                .compiler
                .interner
                .try_resolve(*name)
                .filter(|s| s.starts_with("Flow."))
                .map(|s| s.to_string()),
            _ => None,
        })
        .collect();

    for name in module_names {
        prelude.preload_module_if_needed(&name);
    }
}

#[cfg(test)]
mod tests {
    use super::Snapshot;

    /// The worker thread receives `Arc<Snapshot>`; this locks in that
    /// `Snapshot` stays `Send + Sync` (a stray `Rc` anywhere in the AST,
    /// interner, or inference result would break the build here).
    #[test]
    fn snapshot_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Snapshot>();
    }
}
