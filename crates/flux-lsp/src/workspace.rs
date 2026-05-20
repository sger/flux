//! Project model and analysis orchestration.
//!
//! The `Workspace` owns the [`Vfs`] (file identity + content), the shared
//! [`Prelude`] (Flow stdlib compiler), and a per-file [`Snapshot`] cache. It
//! replaces the old document-first `DocumentStore`: documents are keyed by
//! interned [`FileId`] rather than `Uri`.
//!
//! When workspace roots are known and a buffer participates in the Flux
//! module system (it declares a `module` or imports a user module), the
//! workspace builds the module graph, infers every reachable user module in
//! topological order, and threads each module's schemes into the next — so
//! diagnostics, hover, and goto-definition work across files. Each member
//! file still gets its own per-file `Snapshot`, so the request handlers are
//! unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::Uri;

use flux::ast::type_infer::InferProgramResult;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::module_graph::{ModuleGraph, ModuleKind};
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::line_index::PositionEncoding;
use crate::prelude::Prelude;
use crate::snapshot::Snapshot;
use crate::vfs::{FileId, Vfs, path_to_uri, uri_to_path};

/// A connected set of user-module files discovered from one entry file.
pub struct GraphComponent {
    /// The file the graph was rooted at (the edited buffer).
    pub entry: FileId,
    /// User-module member files in topological order — dependencies first,
    /// entry last. Cross-file references/rename scope to this set.
    pub members: Vec<FileId>,
}

pub struct Workspace {
    vfs: Vfs,
    /// Client-supplied workspace folders (from `initialize` /
    /// `didChangeWorkspaceFolders`). Empty when the LSP runs file-at-a-time.
    roots: Vec<PathBuf>,
    /// Best guess at the enclosing project root — the prelude (`lib/Flow/`)
    /// and on-disk file discovery anchor here.
    project_root: Option<PathBuf>,
    /// Shared Flow-stdlib compiler. Loaded once the project root is known
    /// (or lazily off the first opened file when there are no roots).
    prelude: Option<Prelude>,
    encoding: PositionEncoding,
    snapshots: HashMap<FileId, Arc<Snapshot>>,
    /// Module-graph components, keyed by entry file. Rebuilt whenever a
    /// participating buffer changes.
    components: HashMap<FileId, GraphComponent>,
    /// Original client `Uri` for each interned file, so responses echo back
    /// exactly the spelling the editor sent rather than a re-derived one.
    uris: HashMap<FileId, Uri>,
}

/// Directory names skipped when walking a workspace root for `.flx` files.
/// Also reused by the file-watch [`loader`](crate::loader) to drop change
/// events for generated/VCS paths.
pub(crate) const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git"];

/// Maximum directory depth walked during file discovery — a guard against
/// pathological trees and symlink loops.
const MAX_DISCOVERY_DEPTH: usize = 32;

impl Default for Workspace {
    fn default() -> Self {
        Self::new(PositionEncoding::Utf16)
    }
}

impl Workspace {
    pub fn new(encoding: PositionEncoding) -> Self {
        Self {
            vfs: Vfs::default(),
            roots: Vec::new(),
            project_root: None,
            prelude: None,
            encoding,
            snapshots: HashMap::new(),
            components: HashMap::new(),
            uris: HashMap::new(),
        }
    }

    pub fn set_encoding(&mut self, encoding: PositionEncoding) {
        self.encoding = encoding;
    }

    /// Adopt the client's workspace folders. Picks a project root and walks
    /// the roots to register every `.flx` file with the [`Vfs`] as on-disk
    /// content (cross-file analysis reads dependency text from there).
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
        self.project_root = self
            .roots
            .iter()
            .find_map(|r| flux::shared::cache_paths::find_project_root(r))
            .or_else(|| self.roots.first().cloned());
        self.discover_files();
    }

    /// Workspace roots the client supplied.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// `(uri, text)` for every file the VFS knows — open buffers and
    /// discovered on-disk files alike. Cheap (`Arc` clones + URI lookups);
    /// the `workspace/symbol` handler parses these off the main thread.
    pub fn all_file_texts(&self) -> Vec<(Uri, Arc<str>)> {
        self.vfs
            .all_files()
            .filter_map(|id| Some((self.uri_of(id)?, self.vfs.text(id)?)))
            .collect()
    }

    // ── queries ────────────────────────────────────────────────────────────

    /// Id of an already-interned file behind `uri`, if the workspace has seen
    /// it. Does not intern — use [`Workspace::open`]/[`change`](Self::change)
    /// for that.
    pub fn file_id(&self, uri: &Uri) -> Option<FileId> {
        let path = uri_to_path(uri)?;
        self.vfs.file_id(&path)
    }

    /// Best `Uri` for a file — the exact spelling the client sent if known,
    /// otherwise one derived from the canonical path.
    pub fn uri_of(&self, id: FileId) -> Option<Uri> {
        if let Some(uri) = self.uris.get(&id) {
            return Some(uri.clone());
        }
        self.vfs.path(id).and_then(path_to_uri)
    }

    /// Cached snapshot for `id`, without building one. Used for internal
    /// reads right after a rebuild; request handlers want
    /// [`ensure_snapshot`](Self::ensure_snapshot) instead.
    pub fn snapshot(&self, id: FileId) -> Option<&Arc<Snapshot>> {
        self.snapshots.get(&id)
    }

    pub fn snapshot_for_uri(&self, uri: &Uri) -> Option<&Arc<Snapshot>> {
        self.snapshot(self.file_id(uri)?)
    }

    /// Snapshot for `id`, building it on demand from VFS content when the
    /// file has not been analyzed yet. This is what request handlers use, so
    /// a closed or never-opened project file still answers queries.
    pub fn ensure_snapshot(&mut self, id: FileId) -> Option<&Arc<Snapshot>> {
        if !self.snapshots.contains_key(&id) && self.vfs.text(id).is_some() {
            self.rebuild(id);
        }
        self.snapshots.get(&id)
    }

    pub fn ensure_snapshot_for_uri(&mut self, uri: &Uri) -> Option<&Arc<Snapshot>> {
        let id = self.file_id(uri)?;
        self.ensure_snapshot(id)
    }

    /// Document version of an open buffer; `None` for files known only on disk.
    pub fn version(&self, id: FileId) -> Option<i32> {
        self.vfs.version(id)
    }

    /// Files in the module-graph component `id` belongs to — the set
    /// cross-file references and rename are allowed to span. A file that is
    /// not part of any component scopes to just itself.
    pub fn component_scope(&self, id: FileId) -> Vec<FileId> {
        for comp in self.components.values() {
            if comp.members.contains(&id) {
                return comp.members.clone();
            }
        }
        vec![id]
    }

    // ── document lifecycle ─────────────────────────────────────────────────

    /// Handle `textDocument/didOpen`. Returns every file whose snapshot was
    /// (re)built — more than one when the buffer pulls in user modules.
    pub fn open(&mut self, uri: &Uri, version: i32, text: String) -> Vec<FileId> {
        self.upsert(uri, version, text)
    }

    /// Handle `textDocument/didChange` (full-text sync).
    pub fn change(&mut self, uri: &Uri, version: i32, text: String) -> Vec<FileId> {
        self.upsert(uri, version, text)
    }

    /// Handle `textDocument/didClose`: swap the open buffer back to its
    /// on-disk content and drop the cached snapshot/component — a later query
    /// rebuilds it lazily via [`ensure_snapshot`](Self::ensure_snapshot).
    /// Returns the dependent files re-analyzed because the buffer→disk swap
    /// may have changed their cross-file diagnostics.
    pub fn close(&mut self, uri: &Uri) -> Vec<FileId> {
        let Some(id) = self.file_id(uri) else {
            return vec![];
        };
        self.vfs.close(id);
        self.snapshots.remove(&id);
        self.components.remove(&id);
        self.reanalyze_dependents(id)
    }

    /// Handle a `workspace/didChangeWatchedFiles` event for one file: refresh
    /// its on-disk content and re-analyze every component that depends on it.
    /// Open buffers shadow disk, so an event for an open file is ignored.
    pub fn on_disk_changed(&mut self, uri: &Uri) -> Vec<FileId> {
        let Some(path) = uri_to_path(uri) else {
            return vec![];
        };
        let id = self.vfs.intern(&path);
        if self.vfs.is_open(id) {
            return vec![];
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => self.vfs.set_on_disk(id, Arc::from(text)),
            // Deleted/unreadable — keep the last-known content; a dependent
            // re-analysis will surface the resulting errors.
            Err(_) => return vec![],
        }
        self.reanalyze_dependents(id)
    }

    /// Re-analyze every component that lists `id` as a member. Returns the
    /// rebuilt files.
    fn reanalyze_dependents(&mut self, id: FileId) -> Vec<FileId> {
        let entries: Vec<FileId> = self
            .components
            .values()
            .filter(|comp| comp.members.contains(&id))
            .map(|comp| comp.entry)
            .collect();
        let mut rebuilt: Vec<FileId> = Vec::new();
        for entry in entries {
            for fid in self.analyze_entry(entry) {
                if !rebuilt.contains(&fid) {
                    rebuilt.push(fid);
                }
            }
        }
        rebuilt
    }

    fn upsert(&mut self, uri: &Uri, version: i32, text: String) -> Vec<FileId> {
        let Some(path) = uri_to_path(uri) else {
            return vec![];
        };
        let id = self.vfs.intern(&path);
        self.uris.entry(id).or_insert_with(|| uri.clone());
        self.vfs.set_open(id, Arc::from(text), version);
        self.rebuild(id)
    }

    /// Re-analyze `id` as a graph entry, plus every component that lists `id`
    /// as a member (cross-file dependent invalidation: editing a module
    /// refreshes the diagnostics of files that import it).
    fn rebuild(&mut self, id: FileId) -> Vec<FileId> {
        let mut entries: Vec<FileId> = vec![id];
        for comp in self.components.values() {
            if comp.entry != id && comp.members.contains(&id) {
                entries.push(comp.entry);
            }
        }
        let mut rebuilt: Vec<FileId> = Vec::new();
        for entry in entries {
            for fid in self.analyze_entry(entry) {
                if !rebuilt.contains(&fid) {
                    rebuilt.push(fid);
                }
            }
        }
        rebuilt
    }

    // ── analysis ───────────────────────────────────────────────────────────

    /// Analyze the file `entry_id` and, if it participates in the module
    /// system, every user module reachable from it. Returns the rebuilt files.
    fn analyze_entry(&mut self, entry_id: FileId) -> Vec<FileId> {
        let Some(entry_path) = self.vfs.path(entry_id).map(Path::to_path_buf) else {
            return vec![];
        };
        let Some(entry_text) = self.vfs.text(entry_id) else {
            return vec![];
        };

        let (entry_program, entry_interner) = parse_standalone(&entry_text);
        let cross_file =
            !self.roots.is_empty() && participates_in_graph(&entry_program, &entry_interner);
        if !cross_file {
            self.components.remove(&entry_id);
            self.analyze_single(entry_id, &entry_path, entry_text);
            return vec![entry_id];
        }

        // Discover the user modules reachable from this entry, in topological
        // order. The graph reads dependency *structure* from disk; the actual
        // inference below uses VFS content so unsaved edits are honored. The
        // graph must be handed the very interner `entry_program` was parsed
        // with, or its identifier symbols are meaningless.
        let roots = self.module_search_roots(&entry_path);
        let build = ModuleGraph::build_with_entry_and_roots(
            &entry_path,
            &entry_program,
            entry_interner,
            &roots,
        );
        let user_nodes: Vec<(PathBuf, Option<String>)> = build
            .graph
            .topo_order()
            .iter()
            .filter(|node| node.kind == ModuleKind::User)
            .map(|node| {
                (
                    node.path.clone(),
                    module_name_of(&node.program, &build.interner),
                )
            })
            .collect();

        if user_nodes.len() <= 1 {
            // No cross-file dependencies — analyze the entry standalone.
            self.components.remove(&entry_id);
            self.analyze_single(entry_id, &entry_path, entry_text);
            return vec![entry_id];
        }

        // Pass 1: build each member's snapshot in topological order, stashing
        // every module's exported schemes so later members — and the entry —
        // resolve `import`s and qualified `Module.member` references.
        let mut built: Vec<(FileId, Snapshot, PathBuf, Option<String>)> = Vec::new();
        for (path, module_name) in &user_nodes {
            let fid = self.vfs.intern(path);
            let Some(text) = self.text_or_read(fid, path) else {
                continue;
            };
            let snapshot = self.build_snapshot(&entry_path, text);
            if let (Some(name), Some(infer)) = (module_name.as_deref(), &snapshot.infer) {
                self.stash_schemes(name, infer);
            }
            built.push((fid, snapshot, path.clone(), module_name.clone()));
        }

        // Sibling-module program cache: lets goto-definition jump from the
        // entry (or any member) into a user module's declaration site, the
        // same way it already jumps into Flow prelude modules.
        let user_modules: HashMap<String, (Arc<Program>, Arc<str>, PathBuf)> = built
            .iter()
            .filter_map(|(_, snapshot, path, module_name)| {
                module_name.as_ref().map(|name| {
                    (
                        name.clone(),
                        (
                            // One AST copy, then `Arc`-shared into every
                            // member's `module_programs` below.
                            Arc::new(snapshot.program.clone()),
                            Arc::clone(&snapshot.text),
                            path.clone(),
                        ),
                    )
                })
            })
            .collect();

        // Pass 2: hand every member the sibling-module cache, then publish.
        let mut members: Vec<FileId> = Vec::new();
        for (fid, mut snapshot, _, _) in built {
            for (name, program) in &user_modules {
                snapshot
                    .module_programs
                    .entry(name.clone())
                    .or_insert_with(|| program.clone());
            }
            self.snapshots.insert(fid, Arc::new(snapshot));
            members.push(fid);
        }

        self.components.insert(
            entry_id,
            GraphComponent {
                entry: entry_id,
                members: members.clone(),
            },
        );
        members
    }

    /// Build a single-file snapshot (no cross-file context).
    fn analyze_single(&mut self, id: FileId, hint: &Path, text: Arc<str>) {
        let snapshot = self.build_snapshot(hint, text);
        self.snapshots.insert(id, Arc::new(snapshot));
    }

    fn build_snapshot(&mut self, hint: &Path, text: Arc<str>) -> Snapshot {
        let encoding = self.encoding;
        let prelude = self.prelude_for(hint);
        Snapshot::build(text, prelude, encoding)
    }

    /// Publish a user module's inferred schemes into the shared compiler so
    /// downstream modules resolve `import`s of it.
    fn stash_schemes(&mut self, module_name: &str, infer: &InferProgramResult) {
        if let Some(prelude) = self.prelude.as_mut() {
            flux::lsp_support::stash_module_schemes(&mut prelude.compiler, module_name, infer);
        }
    }

    /// Current text of `fid` — the open buffer or on-disk copy, reading from
    /// disk (and caching) when the VFS has not seen the file yet.
    fn text_or_read(&mut self, fid: FileId, path: &Path) -> Option<Arc<str>> {
        if let Some(text) = self.vfs.text(fid) {
            return Some(text);
        }
        let text: Arc<str> = Arc::from(std::fs::read_to_string(path).ok()?);
        self.vfs.set_on_disk(fid, Arc::clone(&text));
        Some(text)
    }

    /// Whether the Flow prelude has been loaded yet.
    pub fn prelude_loaded(&self) -> bool {
        self.prelude.is_some()
    }

    /// The leading `///` doc comment of `member` declared in the module cached
    /// under `module_key`, as markdown — backs `completionItem/resolve`.
    ///
    /// Resolve requests carry no document URI, so the source is found without a
    /// file context: first in the Flow prelude (always cached), then in any
    /// analyzed snapshot's sibling-module cache (which is where user modules
    /// live). Returns `None` for an unknown module or an undocumented member.
    pub fn member_doc(&self, module_key: &str, member: &str) -> Option<String> {
        if let Some(prelude) = self.prelude.as_ref()
            && let Some(doc) =
                prelude
                    .module_programs
                    .get(module_key)
                    .and_then(|(program, source, _)| {
                        let span = crate::doc_comments::member_decl_span(
                            program,
                            &prelude.compiler.interner,
                            member,
                        )?;
                        crate::doc_comments::doc_comment_above(source, span, self.encoding)
                    })
        {
            return Some(doc);
        }
        // User/sibling modules: any snapshot in the module's component holds
        // the same parsed source, so the first hit wins.
        self.snapshots.values().find_map(|snap| {
            let (program, source, _) = snap.module_programs.get(module_key)?;
            let span = crate::doc_comments::member_decl_span(program, &snap.interner, member)?;
            crate::doc_comments::doc_comment_above(source, span, self.encoding)
        })
    }

    /// Eagerly load the Flow prelude (parse + infer the stdlib, index every
    /// `lib/Flow/*.flx`) anchored at the project root, unless it is already
    /// loaded. Called at `initialized` so the cost happens up front — and can
    /// be reported via `$/progress` — rather than lazily on the first edit.
    /// A no-op without workspace roots; there the prelude loads lazily off
    /// the first opened file's directory.
    pub fn warm_prelude(&mut self) {
        if self.prelude.is_some() {
            return;
        }
        if let Some(start) = self
            .project_root
            .clone()
            .or_else(|| self.roots.first().cloned())
        {
            self.prelude = Some(Prelude::try_load_from(&start));
        }
    }

    fn prelude_for(&mut self, hint: &Path) -> &mut Prelude {
        if self.prelude.is_none() {
            // Prefer the discovered project root so the prelude resolves
            // `lib/Flow/` from a stable anchor; fall back to the edited
            // file's directory when the LSP runs without workspace folders.
            let start = self.project_root.as_deref().unwrap_or(hint);
            self.prelude = Some(Prelude::try_load_from(start));
        }
        self.prelude.as_mut().unwrap()
    }

    /// Search roots for module-graph resolution of `entry_path`'s imports.
    ///
    /// Flux resolves `import A.B.C` to `<root>/A/B/C.flx` for each search
    /// root. A workspace folder is rarely the directory that *directly*
    /// holds the `A/` package: a project's modules often sit in a subtree
    /// (`examples/type_classes/ClassMatchableEffects/...`). So the workspace
    /// roots alone are not enough — the entry's own ancestor directories are
    /// added too, from its parent up to (and including) the enclosing
    /// workspace root. Whichever ancestor actually holds the package
    /// directory then resolves the import; `resolve_import_path` dedups by
    /// canonical path, so an extra root that resolves to the same file is
    /// harmless.
    fn module_search_roots(&self, entry_path: &Path) -> Vec<PathBuf> {
        let mut roots = self.roots.clone();
        let mut dir = entry_path.parent();
        let mut depth = 0;
        while let Some(d) = dir {
            depth += 1;
            if depth > MAX_DISCOVERY_DEPTH {
                break;
            }
            if !roots.iter().any(|r| r == d) {
                roots.push(d.to_path_buf());
            }
            // Stop at a workspace root; with no roots configured, the
            // entry's own directory is the only sensible anchor.
            if self.roots.is_empty() || self.roots.iter().any(|r| r == d) {
                break;
            }
            dir = d.parent();
        }
        roots
    }

    // ── discovery ──────────────────────────────────────────────────────────

    /// Register every `.flx` file under the roots with the VFS as on-disk
    /// content, without overwriting any open buffer.
    fn discover_files(&mut self) {
        let mut found: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            collect_flx_files(root, &mut found, 0);
        }
        for path in found {
            let id = self.vfs.intern(&path);
            if self.vfs.text(id).is_none()
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                self.vfs.set_on_disk(id, Arc::from(text));
            }
        }
    }
}

/// Whether `program` takes part in the Flux module system — it either
/// declares a `module` or imports something other than the Flow stdlib.
/// Plain scripts skip the (more expensive) module-graph path entirely.
fn participates_in_graph(program: &Program, interner: &Interner) -> bool {
    program.statements.iter().any(|stmt| match stmt {
        Statement::Module { .. } => true,
        Statement::Import { name, .. } => interner
            .try_resolve(*name)
            .is_some_and(|n| !n.starts_with("Flow")),
        _ => false,
    })
}

/// Name of the `module` a program declares, if any.
fn module_name_of(program: &Program, interner: &Interner) -> Option<String> {
    program.statements.iter().find_map(|stmt| match stmt {
        Statement::Module { name, .. } => interner.try_resolve(*name).map(str::to_string),
        _ => None,
    })
}

/// Parse `text` with a throwaway interner — used only for module-graph
/// discovery, where the resulting `Program`/`Interner` are inspected for
/// imports and module declarations and then dropped.
fn parse_standalone(text: &str) -> (Program, Interner) {
    let lexer = Lexer::new(text.to_string());
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    (program, parser.take_interner())
}

/// Recursively collect `*.flx` files under `dir`, skipping build/VCS
/// directories and hidden directories, bounded by [`MAX_DISCOVERY_DEPTH`].
fn collect_flx_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_DISCOVERY_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_flx_files(&path, out, depth + 1);
        } else if file_type.is_file() && path.extension().is_some_and(|e| e == "flx") {
            out.push(path);
        }
    }
}
