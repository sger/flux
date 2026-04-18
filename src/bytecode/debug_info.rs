use crate::diagnostics::position::Span;

/// Static metadata for a cost centre, set at compile time.
///
/// Lives in the format layer because it is emitted by the compiler and
/// consumed by the VM — runtime profiling state (`CostCentre`,
/// `CostCentreStackEntry`) stays in `vm::profiling`.
#[derive(Debug, Clone)]
pub struct CostCentreInfo {
    pub name: String,
    pub module: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EffectSummary {
    Pure,
    #[default]
    Unknown,
    HasEffects,
}

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
    pub boundary_location: Option<Location>,
    pub effect_summary: EffectSummary,
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
            boundary_location: None,
            effect_summary: EffectSummary::Unknown,
        }
    }

    pub fn with_boundary_location(mut self, boundary_location: Option<Location>) -> Self {
        self.boundary_location = boundary_location;
        self
    }

    pub fn with_effect_summary(mut self, effect_summary: EffectSummary) -> Self {
        self.effect_summary = effect_summary;
        self
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
