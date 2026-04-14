#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOutputFormat {
    Text,
    Json,
    JsonCompact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreDumpMode {
    None,
    Readable,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AetherDumpMode {
    None,
    Summary,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Program,
    Tests,
}
