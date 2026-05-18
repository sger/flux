use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use lsp_types::{
    Diagnostic, Location, OneOf, OptionalVersionedTextDocumentIdentifier, PublishDiagnosticsParams,
    SymbolInformation, SymbolKind, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::convert::diagnostic_to_lsp;
use crate::handlers::references::collect_all_uses;
use crate::line_index::PositionEncoding;
use crate::navigation_target::NavigationTarget;
use crate::prelude::Prelude;
use crate::snapshot::Snapshot;
use crate::symbol_index::Entry;

#[derive(Clone, Debug)]
pub struct WorkspaceRoot {
    pub path: PathBuf,
    pub uri: Uri,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSource {
    Disk,
    Open,
}

pub struct WorkspaceFile {
    pub id: FileId,
    pub path: PathBuf,
    pub uri: Uri,
    pub version: Option<i32>,
    pub text_source: TextSource,
    pub snapshot: Snapshot,
}

#[derive(Default)]
pub struct WorkspaceIndex {
    pub symbols_by_name: HashMap<String, Vec<FileId>>,
    pub symbols_by_file: HashMap<FileId, Vec<String>>,
    pub module_by_name: HashMap<String, FileId>,
    pub file_by_uri: HashMap<Uri, FileId>,
}

pub struct Workspace {
    roots: Vec<WorkspaceRoot>,
    files: HashMap<FileId, WorkspaceFile>,
    path_to_id: HashMap<PathBuf, FileId>,
    next_id: usize,
    prelude: Prelude,
    encoding: PositionEncoding,
    discover_on_first_open: bool,
    pub index: WorkspaceIndex,
}

impl Workspace {
    pub fn empty(encoding: PositionEncoding) -> Self {
        Self {
            roots: Vec::new(),
            files: HashMap::new(),
            path_to_id: HashMap::new(),
            next_id: 0,
            prelude: Prelude::empty(),
            encoding,
            discover_on_first_open: false,
            index: WorkspaceIndex::default(),
        }
    }

    pub fn new(roots: Vec<WorkspaceRoot>, encoding: PositionEncoding) -> Self {
        let prelude = roots
            .first()
            .map(|root| Prelude::try_load_from(&root.path))
            .unwrap_or_else(Prelude::empty);
        let mut workspace = Self {
            roots,
            files: HashMap::new(),
            path_to_id: HashMap::new(),
            next_id: 0,
            prelude,
            encoding,
            discover_on_first_open: false,
            index: WorkspaceIndex::default(),
        };
        workspace.scan_roots();
        workspace
    }

    pub fn enable_first_open_discovery(&mut self) {
        self.discover_on_first_open = true;
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn ensure_root_for_uri(&mut self, uri: &Uri) {
        if !self.roots.is_empty() {
            return;
        }
        if !self.discover_on_first_open {
            return;
        }
        let Some(path) = uri_to_path(uri).and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            return;
        };
        if !path.is_dir() || path.parent().is_none() {
            return;
        }
        let uri = path_to_uri(&path).unwrap_or_else(|| uri.clone());
        self.roots.push(WorkspaceRoot {
            path: path.clone(),
            uri,
        });
        self.prelude = Prelude::try_load_from(&path);
        self.scan_roots();
    }

    pub fn open(&mut self, uri: &Uri, version: i32, text: String) {
        self.ensure_root_for_uri(uri);
        let Some(path) = canonical_uri_path(uri) else {
            return;
        };
        self.upsert(
            path,
            uri.clone(),
            Some(version),
            TextSource::Open,
            Arc::from(text),
        );
    }

    pub fn change(&mut self, uri: &Uri, version: i32, text: String) {
        let Some(path) = canonical_uri_path(uri) else {
            return;
        };
        self.upsert(
            path,
            uri.clone(),
            Some(version),
            TextSource::Open,
            Arc::from(text),
        );
    }

    pub fn close(&mut self, uri: &Uri) {
        let Some(path) = canonical_uri_path(uri) else {
            return;
        };
        let Some(id) = self.path_to_id.get(&path).copied() else {
            return;
        };
        if path.is_file()
            && let Ok(text) = std::fs::read_to_string(&path)
            && let Some(uri) = path_to_uri(&path)
        {
            self.upsert(path, uri, None, TextSource::Disk, Arc::from(text));
        } else {
            self.remove_id(id);
        }
    }

    pub fn rescan_path(&mut self, uri: &Uri) {
        let Some(path) = canonical_uri_path(uri) else {
            return;
        };
        if path.is_file()
            && path.extension().is_some_and(|ext| ext == "flx")
            && let Ok(text) = std::fs::read_to_string(&path)
            && let Some(uri) = path_to_uri(&path)
        {
            self.upsert(path, uri, None, TextSource::Disk, Arc::from(text));
        } else if let Some(id) = self
            .path_to_id
            .get(&path)
            .copied()
            .or_else(|| self.index.file_by_uri.get(uri).copied())
            .or_else(|| {
                self.files
                    .iter()
                    .find(|(_, file)| {
                        file.uri == *uri || file.path == path || same_path_suffix(&file.path, &path)
                    })
                    .map(|(id, _)| *id)
            })
        {
            self.remove_id(id);
        }
    }

    pub fn file_by_uri(&self, uri: &Uri) -> Option<&WorkspaceFile> {
        let id = self.index.file_by_uri.get(uri)?;
        self.files.get(id)
    }

    pub fn diagnostics(&self) -> Vec<PublishDiagnosticsParams> {
        self.files
            .values()
            .map(|file| diagnostics_for_file(file))
            .collect()
    }

    pub fn symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let query = query.to_ascii_lowercase();
        let mut out = Vec::new();
        for file in self.files.values() {
            for entry in file.snapshot.symbol_index.entries() {
                if !query.is_empty() && !entry.name.to_ascii_lowercase().contains(&query) {
                    continue;
                }
                out.push(symbol_information(file, entry));
            }
        }
        out.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.location.uri.cmp(&b.location.uri))
        });
        out
    }

    pub fn definition_by_name(&self, name: &str) -> Option<NavigationTarget> {
        let ids = self.index.symbols_by_name.get(name)?;
        for id in ids {
            let file = self.files.get(id)?;
            if let Some(entry) = file.snapshot.symbol_index.lookup(name) {
                return Some(target_from_entry(
                    &file.uri,
                    &file.snapshot.position_map,
                    entry,
                ));
            }
        }
        None
    }

    pub fn references_by_name(&self, name: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for file in self.files.values() {
            let Some(sym) = file.snapshot.interner.lookup(name) else {
                continue;
            };
            let mut spans = Vec::new();
            collect_all_uses(&file.snapshot.program, sym, &mut spans);
            out.extend(spans.into_iter().map(|span| Location {
                uri: file.uri.clone(),
                range: file.snapshot.position_map.flux_span_to_range(span),
            }));
        }
        out
    }

    pub fn rename_by_name(&self, name: &str, new_name: String) -> Option<WorkspaceEdit> {
        let mut edits_by_uri: HashMap<
            Uri,
            (
                Option<i32>,
                Vec<OneOf<TextEdit, lsp_types::AnnotatedTextEdit>>,
            ),
        > = HashMap::new();
        for file in self.files.values() {
            let Some(sym) = file.snapshot.interner.lookup(name) else {
                continue;
            };
            let mut spans = Vec::new();
            collect_all_uses(&file.snapshot.program, sym, &mut spans);
            if spans.is_empty() {
                continue;
            }
            let entry = edits_by_uri
                .entry(file.uri.clone())
                .or_insert_with(|| (file.version, Vec::new()));
            for span in spans {
                entry.1.push(OneOf::Left(TextEdit {
                    range: file.snapshot.position_map.flux_span_to_range(span),
                    new_text: new_name.clone(),
                }));
            }
        }
        if edits_by_uri.is_empty() {
            return None;
        }
        let document_edits = edits_by_uri
            .into_iter()
            .map(|(uri, (version, edits))| TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                edits,
            })
            .collect();
        Some(WorkspaceEdit {
            document_changes: Some(lsp_types::DocumentChanges::Edits(document_edits)),
            ..Default::default()
        })
    }

    fn scan_roots(&mut self) {
        let roots: Vec<PathBuf> = self.roots.iter().map(|root| root.path.clone()).collect();
        for root in roots {
            self.scan_dir(&root);
        }
        self.rebuild_index();
    }

    fn scan_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if should_skip_dir(&path) {
                    continue;
                }
                self.scan_dir(&path);
            } else if path.extension().is_some_and(|ext| ext == "flx")
                && let Ok(text) = std::fs::read_to_string(&path)
                && let Some(uri) = path_to_uri(&path)
            {
                let path = canonical_path(&path);
                if !self.path_to_id.contains_key(&path) {
                    self.upsert(path, uri, None, TextSource::Disk, Arc::from(text));
                }
            }
        }
    }

    fn upsert(
        &mut self,
        path: PathBuf,
        uri: Uri,
        version: Option<i32>,
        text_source: TextSource,
        text: Arc<str>,
    ) {
        let id = self.id_for_path(path.clone());
        let snapshot = Snapshot::build(text, &mut self.prelude, self.encoding);
        self.files.insert(
            id,
            WorkspaceFile {
                id,
                path: path.clone(),
                uri,
                version,
                text_source,
                snapshot,
            },
        );
        self.path_to_id.insert(path, id);
        self.rebuild_index();
    }

    fn id_for_path(&mut self, path: PathBuf) -> FileId {
        if let Some(id) = self.path_to_id.get(&path) {
            return *id;
        }
        let id = FileId(self.next_id);
        self.next_id += 1;
        self.path_to_id.insert(path, id);
        id
    }

    fn remove_id(&mut self, id: FileId) {
        if let Some(file) = self.files.remove(&id) {
            self.path_to_id.remove(&file.path);
        }
        self.rebuild_index();
    }

    fn rebuild_index(&mut self) {
        let mut index = WorkspaceIndex::default();
        for file in self.files.values() {
            index.file_by_uri.insert(file.uri.clone(), file.id);
            let mut names = Vec::new();
            for entry in file.snapshot.symbol_index.entries() {
                index
                    .symbols_by_name
                    .entry(entry.name.clone())
                    .or_default()
                    .push(file.id);
                names.push(entry.name.clone());
                if looks_like_module_name(&entry.name) {
                    index.module_by_name.insert(entry.name.clone(), file.id);
                    if let Some(short) = entry.name.rsplit('.').next() {
                        index.module_by_name.insert(short.to_string(), file.id);
                    }
                }
            }
            index.symbols_by_file.insert(file.id, names);
        }
        self.index = index;
    }
}

pub fn workspace_roots_from_initialize(params: &lsp_types::InitializeParams) -> Vec<WorkspaceRoot> {
    let mut roots = Vec::new();
    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Some(path) = uri_to_path(&folder.uri) {
                roots.push(WorkspaceRoot {
                    path: canonical_path(&path),
                    uri: folder.uri.clone(),
                });
            }
        }
    }
    if roots.is_empty() {
        #[allow(deprecated)]
        let root_uri = params.root_uri.as_ref();
        if let Some(uri) = root_uri
            && let Some(path) = uri_to_path(uri)
        {
            roots.push(WorkspaceRoot {
                path: canonical_path(&path),
                uri: uri.clone(),
            });
        }
    }
    roots
}

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let stripped = s.strip_prefix("file://").unwrap_or(s);
    let decoded = percent_decode(stripped);
    let path_str = if cfg!(windows)
        && decoded.starts_with('/')
        && decoded.chars().nth(2).is_some_and(|c| c == ':')
    {
        decoded.trim_start_matches('/').to_string()
    } else {
        decoded
    };
    Some(PathBuf::from(path_str))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let path = canonical_path(path);
    let mut s = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) && !s.starts_with('/') {
        s = format!("/{s}");
    }
    Uri::from_str(&format!("file://{}", percent_encode_path(&s))).ok()
}

fn canonical_uri_path(uri: &Uri) -> Option<PathBuf> {
    uri_to_path(uri).map(|path| canonical_path(&path))
}

fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn diagnostics_for_file(file: &WorkspaceFile) -> PublishDiagnosticsParams {
    let diagnostics: Vec<Diagnostic> = file
        .snapshot
        .diagnostics
        .iter()
        .map(|diag| diagnostic_to_lsp(diag, &file.snapshot.position_map))
        .collect();
    PublishDiagnosticsParams {
        uri: file.uri.clone(),
        diagnostics,
        version: file.version,
    }
}

fn target_from_entry(
    uri: &Uri,
    dest_map: &crate::line_index::PositionMap,
    entry: &Entry,
) -> NavigationTarget {
    NavigationTarget {
        uri: uri.clone(),
        full_range: dest_map.flux_span_to_range(entry.full_span),
        focus_range: dest_map.flux_span_to_range(entry.focus_span),
        name: entry.name.clone(),
    }
}

fn symbol_information(file: &WorkspaceFile, entry: &Entry) -> SymbolInformation {
    #[allow(deprecated)]
    SymbolInformation {
        name: entry.name.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        deprecated: None,
        location: Location {
            uri: file.uri.clone(),
            range: file
                .snapshot
                .position_map
                .flux_span_to_range(entry.focus_span),
        },
        container_name: None,
    }
}

fn should_skip_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(name, ".git" | "target" | "node_modules" | ".vscode") || name.starts_with('.')
}

fn same_path_suffix(left: &Path, right: &Path) -> bool {
    left.file_name() == right.file_name()
        && left.parent().and_then(Path::file_name) == right.parent().and_then(Path::file_name)
}

fn looks_like_module_name(name: &str) -> bool {
    name.contains('.')
        || name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
