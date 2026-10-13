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

/// What a debugger stop site marks (proposal 0188 §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    /// The start of a function, lambda or handler arm body.
    FnEntry,
    /// The start of a statement in a function body.
    Stmt,
    /// The start of a match arm's body.
    MatchArm,
    /// A call written as a `|>` stage.
    PipeStage,
    /// An effect operation being performed.
    Perform,
}

impl SiteKind {
    pub const ALL: [SiteKind; 5] = [
        SiteKind::FnEntry,
        SiteKind::Stmt,
        SiteKind::MatchArm,
        SiteKind::PipeStage,
        SiteKind::Perform,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SiteKind::FnEntry => "entry",
            SiteKind::Stmt => "statement",
            SiteKind::MatchArm => "match arm",
            SiteKind::PipeStage => "pipe stage",
            SiteKind::Perform => "perform",
        }
    }
}

/// A place a debugger can stop: the `OpBreakpoint` at `ip`.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakSite {
    pub kind: SiteKind,
    pub ip: usize,
    pub location: Option<Location>,
}

/// A local variable's slot and the instructions where it holds the value.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalVar {
    pub slot: u32,
    pub name: String,
    /// The first instruction after the variable is bound.
    pub start_ip: usize,
    /// The end of the variable's scope (exclusive).
    pub end_ip: usize,
    /// Compiler-made: dictionaries, temporaries. Hidden unless the debugger
    /// shows compiler internals.
    pub artificial: bool,
}

impl LocalVar {
    pub fn is_live_at(&self, ip: usize) -> bool {
        self.start_ip <= ip && ip < self.end_ip
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionDebugInfo {
    pub name: Option<String>,
    pub files: Vec<String>,
    pub locations: Vec<InstructionLocation>,
    pub boundary_location: Option<Location>,
    pub effect_summary: EffectSummary,
    /// Debugger stop sites, in instruction order. Empty unless built with `-g`.
    pub sites: Vec<BreakSite>,
    /// Local variables by slot. Empty unless built with `-g`.
    pub locals: Vec<LocalVar>,
    /// Names of the captured values, in `Closure::free` order. Empty unless
    /// built with `-g`.
    pub free_names: Vec<String>,
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
            sites: Vec::new(),
            locals: Vec::new(),
            free_names: Vec::new(),
        }
    }

    /// Adds the debugger tables built for this function under `-g`.
    pub fn with_debug_tables(
        mut self,
        sites: Vec<BreakSite>,
        locals: Vec<LocalVar>,
        free_names: Vec<String>,
    ) -> Self {
        self.sites = sites;
        self.locals = locals;
        self.free_names = free_names;
        self
    }

    /// Moves every instruction offset in the debugger tables by `delta`, as
    /// relocating the function's instructions does.
    pub fn shift_debug_tables(&mut self, delta: isize) {
        let shift = |ip: usize| (ip as isize + delta) as usize;
        for site in &mut self.sites {
            site.ip = shift(site.ip);
        }
        for local in &mut self.locals {
            local.start_ip = shift(local.start_ip);
            local.end_ip = shift(local.end_ip);
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
