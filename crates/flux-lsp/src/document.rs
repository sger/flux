use std::collections::HashMap;
use std::sync::Arc;

use lsp_types::Uri;

use crate::line_index::PositionEncoding;
use crate::prelude::{Prelude, parent_dir_of_uri};
use crate::snapshot::Snapshot;

pub struct DocumentStore {
    docs: HashMap<Uri, Document>,
    prelude: Option<Prelude>,
    encoding: PositionEncoding,
}

pub struct Document {
    pub version: i32,
    pub snapshot: Snapshot,
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new(PositionEncoding::Utf16)
    }
}

impl DocumentStore {
    pub fn new(encoding: PositionEncoding) -> Self {
        Self {
            docs: HashMap::new(),
            prelude: None,
            encoding,
        }
    }

    pub fn set_encoding(&mut self, encoding: PositionEncoding) {
        self.encoding = encoding;
    }

    fn prelude_for(&mut self, uri: &Uri) -> &mut Prelude {
        if self.prelude.is_none() {
            let prelude = match parent_dir_of_uri(uri) {
                Some(dir) => Prelude::try_load_from(&dir),
                None => Prelude::empty(),
            };
            self.prelude = Some(prelude);
        }
        self.prelude.as_mut().unwrap()
    }

    pub fn open(&mut self, uri: Uri, version: i32, text: String) {
        let encoding = self.encoding;
        let prelude = self.prelude_for(&uri);
        let snapshot = Snapshot::build(Arc::from(text), prelude, encoding);
        self.docs.insert(uri, Document { version, snapshot });
    }

    pub fn change(&mut self, uri: Uri, version: i32, text: String) {
        let encoding = self.encoding;
        let prelude = self.prelude_for(&uri);
        let snapshot = Snapshot::build(Arc::from(text), prelude, encoding);
        self.docs.insert(uri, Document { version, snapshot });
    }

    pub fn close(&mut self, uri: &Uri) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.docs.get(uri)
    }
}
