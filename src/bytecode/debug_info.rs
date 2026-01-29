use crate::frontend::position::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocation {
    pub file: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionDebugInfo {
    pub name: Option<String>,
    pub locations: Vec<Option<SourceLocation>>,
}

impl FunctionDebugInfo {
    pub fn new(name: Option<String>, locations: Vec<Option<SourceLocation>>) -> Self {
        Self { name, locations }
    }

    pub fn location_at(&self, ip: usize) -> Option<&SourceLocation> {
        self.locations.get(ip).and_then(|loc| loc.as_ref())
    }
}
