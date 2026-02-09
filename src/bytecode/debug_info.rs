use crate::syntax::position::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub file_id: u32,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstructionLocation {
    pub offset: usize,
    pub location: Option<Location>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionDebugInfo {
    pub name: Option<String>,
    pub files: Vec<String>,
    pub locations: Vec<InstructionLocation>,
}

impl FunctionDebugInfo {
    pub fn new(
        name: Option<String>,
        files: Vec<String>,
        locations: Vec<InstructionLocation>,
    ) -> Self {
        Self {
            name,
            files,
            locations,
        }
    }

    pub fn location_at(&self, ip: usize) -> Option<&Location> {
        match self
            .locations
            .binary_search_by_key(&ip, |entry| entry.offset)
        {
            Ok(index) => self
                .locations
                .get(index)
                .and_then(|entry| entry.location.as_ref()),
            Err(index) => index
                .checked_sub(1)
                .and_then(|prev| self.locations.get(prev))
                .and_then(|entry| entry.location.as_ref()),
        }
    }

    pub fn file_for(&self, file_id: u32) -> Option<&str> {
        self.files.get(file_id as usize).map(|s| s.as_str())
    }
}
