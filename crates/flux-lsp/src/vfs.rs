//! Virtual file system: file identity + content overlay.
//!
//! A minimal, rust-analyzer-inspired VFS. It interns canonicalized absolute
//! paths to [`FileId`]s and tracks per-file content as either a client-owned
//! open buffer or an on-disk file. There is no salsa and no file watching
//! here — just stable identity and an overlay where an open buffer shadows
//! the on-disk copy of the same file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use lsp_types::Uri;

/// Handle to a file tracked by the [`Vfs`].
///
/// A newtype over a dense `u32` index. Ids are never recycled, so a `FileId`
/// stays valid for the whole session even after the file is closed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FileId(pub u32);

/// Bidirectional interner mapping canonicalized absolute paths to [`FileId`]s.
#[derive(Default)]
pub struct PathInterner {
    map: HashMap<PathBuf, FileId>,
    paths: Vec<PathBuf>,
}

impl PathInterner {
    /// Intern `path` (canonicalized) and return its stable id. Idempotent —
    /// interning the same file twice yields the same `FileId`.
    pub fn intern(&mut self, path: &Path) -> FileId {
        let canonical = canonicalize_flux_path(path);
        if let Some(&id) = self.map.get(&canonical) {
            return id;
        }
        let id = FileId(self.paths.len() as u32);
        self.paths.push(canonical.clone());
        self.map.insert(canonical, id);
        id
    }

    /// Look up the id of an already-interned file, without interning it.
    pub fn get(&self, path: &Path) -> Option<FileId> {
        self.map.get(&canonicalize_flux_path(path)).copied()
    }

    /// The canonical path behind a `FileId`.
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.paths.get(id.0 as usize).map(PathBuf::as_path)
    }
}

/// Content of a tracked file. An `Open` buffer is owned by the editor and may
/// differ from disk; `OnDisk` mirrors the file as last read from disk.
#[derive(Clone)]
pub enum FileContent {
    Open { text: Arc<str>, version: i32 },
    OnDisk { text: Arc<str> },
}

impl FileContent {
    fn text(&self) -> &Arc<str> {
        match self {
            FileContent::Open { text, .. } | FileContent::OnDisk { text } => text,
        }
    }
}

/// File identity plus a content overlay. An `Open` buffer always shadows the
/// `OnDisk` copy of the same `FileId`.
#[derive(Default)]
pub struct Vfs {
    interner: PathInterner,
    contents: HashMap<FileId, FileContent>,
}

impl Vfs {
    /// Intern a path, returning its stable `FileId`.
    pub fn intern(&mut self, path: &Path) -> FileId {
        self.interner.intern(path)
    }

    /// Id of an already-interned path.
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.interner.get(path)
    }

    /// Canonical path behind a `FileId`.
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.interner.path(id)
    }

    /// Record an editor-owned open buffer (from `didOpen`/`didChange`).
    pub fn set_open(&mut self, id: FileId, text: Arc<str>, version: i32) {
        self.contents
            .insert(id, FileContent::Open { text, version });
    }

    /// Record an on-disk file the editor has not opened.
    pub fn set_on_disk(&mut self, id: FileId, text: Arc<str>) {
        self.contents.insert(id, FileContent::OnDisk { text });
    }

    /// Drop the open-buffer overlay for `id`. If the file still exists on
    /// disk its content is re-read into an `OnDisk` entry; otherwise the
    /// entry is removed entirely.
    pub fn close(&mut self, id: FileId) {
        let on_disk = self
            .interner
            .path(id)
            .and_then(|p| std::fs::read_to_string(p).ok());
        match on_disk {
            Some(text) => {
                self.contents
                    .insert(id, FileContent::OnDisk { text: text.into() });
            }
            None => {
                self.contents.remove(&id);
            }
        }
    }

    /// Current text of `id` — the open buffer if any, else the on-disk copy.
    pub fn text(&self, id: FileId) -> Option<Arc<str>> {
        self.contents.get(&id).map(|c| Arc::clone(c.text()))
    }

    /// Document version of an open buffer; `None` for on-disk-only files.
    pub fn version(&self, id: FileId) -> Option<i32> {
        match self.contents.get(&id) {
            Some(FileContent::Open { version, .. }) => Some(*version),
            _ => None,
        }
    }

    /// Whether `id` is currently an editor-owned open buffer.
    pub fn is_open(&self, id: FileId) -> bool {
        matches!(self.contents.get(&id), Some(FileContent::Open { .. }))
    }

    /// Every file with known content (open or on-disk).
    pub fn all_files(&self) -> impl Iterator<Item = FileId> + '_ {
        self.contents.keys().copied()
    }
}

/// Canonicalize a path the **same way** `flux::syntax::module_graph::ModuleId`
/// does (plain `fs::canonicalize`, falling back to the path as-is when the
/// file does not exist yet). Keeping these in lock-step is what lets a
/// `FileId` and a `ModuleId` refer to the same file.
pub fn canonicalize_flux_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve a `file://` URI to the absolute path it refers to.
///
/// VS Code emits Windows paths as `file:///e%3A/...`; we percent-decode
/// first, then strip the leading slash so the drive-letter check sees `e:`
/// rather than `%`.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let stripped = s.strip_prefix("file://").unwrap_or(s);
    let decoded = percent_decode(stripped);
    let path_str = if decoded.starts_with('/')
        && decoded
            .chars()
            .nth(2)
            .is_some_and(|c| c == ':' || cfg!(not(windows)))
    {
        if cfg!(windows) {
            decoded.trim_start_matches('/').to_string()
        } else {
            decoded
        }
    } else {
        decoded
    };
    Some(PathBuf::from(path_str))
}

/// Parent directory of the file a `file://` URI points at.
pub fn parent_dir_of_uri(uri: &Uri) -> Option<PathBuf> {
    uri_to_path(uri).and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Build a `file://` URI from an absolute path. Inverse of [`uri_to_path`]
/// for the paths the LSP produces (used for cross-file `Location`s).
///
/// The Windows `\\?\` verbatim prefix that `fs::canonicalize` produces is
/// stripped first — it is not valid in a `file://` URI and editors do not
/// expect it.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let s = strip_verbatim_prefix(&path.to_string_lossy()).replace('\\', "/");
    let with_slash = if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    };
    Uri::from_str(&format!("file://{}", percent_encode_path(&with_slash))).ok()
}

/// Strip the Windows extended-length (`\\?\`) prefix so a path round-trips
/// through a `file://` URI. `\\?\UNC\server\share` becomes `\\server\share`.
fn strip_verbatim_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// Percent-encode a URI path, leaving `/` and `:` literal so Windows drive
/// letters survive as `file:///C:/...`.
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Minimal `%XX` percent-decode for `file://` URIs.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_idempotent() {
        let mut vfs = Vfs::default();
        let a = vfs.intern(Path::new("nonexistent_a.flx"));
        let a2 = vfs.intern(Path::new("nonexistent_a.flx"));
        let b = vfs.intern(Path::new("nonexistent_b.flx"));
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(vfs.file_id(Path::new("nonexistent_a.flx")), Some(a));
    }

    #[test]
    fn open_buffer_shadows_on_disk() {
        let mut vfs = Vfs::default();
        let id = vfs.intern(Path::new("overlay.flx"));
        vfs.set_on_disk(id, Arc::from("disk"));
        assert_eq!(vfs.text(id).as_deref(), Some("disk"));
        assert!(!vfs.is_open(id));

        vfs.set_open(id, Arc::from("buffer"), 7);
        assert_eq!(vfs.text(id).as_deref(), Some("buffer"));
        assert_eq!(vfs.version(id), Some(7));
        assert!(vfs.is_open(id));
    }

    #[test]
    fn close_with_no_disk_file_drops_entry() {
        let mut vfs = Vfs::default();
        let id = vfs.intern(Path::new("ephemeral_unsaved.flx"));
        vfs.set_open(id, Arc::from("buffer"), 1);
        vfs.close(id);
        assert_eq!(vfs.text(id), None);
        assert!(!vfs.is_open(id));
    }

    #[test]
    fn close_rereads_on_disk_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("saved.flx");
        std::fs::write(&file, "from disk").unwrap();

        let mut vfs = Vfs::default();
        let id = vfs.intern(&file);
        vfs.set_open(id, Arc::from("unsaved edits"), 3);
        assert_eq!(vfs.text(id).as_deref(), Some("unsaved edits"));

        vfs.close(id);
        assert_eq!(vfs.text(id).as_deref(), Some("from disk"));
        assert!(!vfs.is_open(id));
    }

    #[test]
    fn canonical_path_collapses_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mod.flx");
        std::fs::write(&file, "x").unwrap();

        let mut vfs = Vfs::default();
        // Reach the same file through a `.` component; canonicalization must
        // collapse both spellings onto one `FileId`.
        let direct = vfs.intern(&file);
        let indirect = vfs.intern(&dir.path().join(".").join("mod.flx"));
        assert_eq!(direct, indirect);
    }

    #[test]
    fn uri_path_round_trip() {
        let uri = Uri::from_str("file:///tmp/sub%20dir/main.flx").unwrap();
        let path = uri_to_path(&uri).unwrap();
        let back = path_to_uri(&path).unwrap();
        assert_eq!(uri_to_path(&back).unwrap(), path);
    }
}
