use std::collections::HashMap;
use std::sync::Arc;

use lsp_types::Uri;

use crate::snapshot::Snapshot;

#[derive(Default)]
pub struct DocumentStore {
    docs: HashMap<Uri, Document>,
}

pub struct Document {
    pub version: i32,
    pub snapshot: Snapshot,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, uri: Uri, version: i32, text: String) {
        let snapshot = Snapshot::build(Arc::from(text));
        self.docs.insert(uri, Document { version, snapshot });
    }

    pub fn change(&mut self, uri: Uri, version: i32, text: String) {
        let snapshot = Snapshot::build(Arc::from(text));
        self.docs.insert(uri, Document { version, snapshot });
    }

    pub fn close(&mut self, uri: &Uri) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.docs.get(uri)
    }
}
