//! The debug-info data model (proposal 0188 §1).
//!
//! Everything here is plain data with resolved names, so a backend or a dump
//! can use it without the interner that produced it.

use crate::diagnostics::position::Span;

/// Index into [`ProgramDebugInfo::files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Index into [`ProgramDebugInfo::functions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DebugFnId(pub u32);

/// Index into [`ProgramDebugInfo::types`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DebugTypeId(pub u32);

/// Debug information for one compiled module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramDebugInfo {
    pub files: Vec<DebugFile>,
    pub functions: Vec<DebugFunction>,
    /// Interned types; [`DebugVar::ty`] and ADT fields point into this table.
    pub types: Vec<DebugType>,
    pub effects: Vec<DebugEffect>,
}

/// A source file the module's code came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugFile {
    pub path: String,
}

/// A function as the user sees it: a top-level definition or a lambda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugFunction {
    pub id: DebugFnId,
    /// The Core binder of a top-level definition; `None` for a lambda.
    pub binder: Option<u32>,
    /// The name shown in a call stack: `main`, `Flow.List.map`,
    /// `<lambda app.flx:12:5>`.
    pub source_name: String,
    pub file: FileId,
    pub decl_span: Span,
    pub kind: FnKind,
    /// Compiler-generated: debuggers step through it and hide it unless asked.
    pub artificial: bool,
    /// Parameters first, in order, then the variables bound in the body in
    /// the order they are bound.
    pub vars: Vec<DebugVar>,
}

/// What kind of function a [`DebugFunction`] is.
///
/// Backend-specific kinds (workers, continuation splits, closure wrappers)
/// are added by the stages whose backends create those functions (0188 S2, S5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    /// A definition written by the user.
    User,
    /// A lambda nested inside another function.
    Lambda { parent: DebugFnId },
    /// A type-class dictionary definition.
    DictDef,
    /// A type-class instance method.
    DictMethod,
}

/// A variable in scope somewhere in a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugVar {
    pub name: String,
    /// The Core binder the variable comes from; backends map their own
    /// variables to it.
    pub binder: u32,
    /// The span of the construct that binds the variable: the function or
    /// lambda for a parameter, the `let` for a let binding, the match arm for
    /// a pattern variable, the handler arm for its parameters.
    pub decl_span: Span,
    pub ty: DebugTypeId,
    pub kind: VarKind,
    /// Compiler-generated, such as a dictionary parameter.
    pub artificial: bool,
}

/// How a [`DebugVar`] is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Param,
    Let,
    PatternBound,
    /// A handler arm's parameter, state or `resume`.
    HandlerParam,
    /// A type-class dictionary passed as evidence.
    Evidence,
}

/// A type as the debugger renders it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    List(DebugTypeId),
    Array(DebugTypeId),
    Map(DebugTypeId, DebugTypeId),
    Tuple(Vec<DebugTypeId>),
    Option(DebugTypeId),
    Either(DebugTypeId, DebugTypeId),
    Function(Vec<DebugTypeId>, DebugTypeId),
    /// A user data type; its constructors are listed when the module declares
    /// it, and empty when it comes from another module.
    Adt {
        name: String,
        args: Vec<DebugTypeId>,
        ctors: Vec<DebugCtor>,
    },
    /// Not known statically (a type variable, or no type recorded): render
    /// from the runtime value.
    Dynamic,
}

/// One constructor of a data type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugCtor {
    pub name: String,
    pub fields: Vec<DebugField>,
}

/// One field of a constructor. `name` is `None` for positional fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugField {
    pub name: Option<String>,
    /// The field's declared type, as written in the source.
    pub ty: String,
}

/// An effect the module declares, with its operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugEffect {
    pub name: String,
    pub ops: Vec<String>,
    pub span: Span,
}

impl ProgramDebugInfo {
    pub fn file(&self, id: FileId) -> &DebugFile {
        &self.files[id.0 as usize]
    }

    pub fn ty(&self, id: DebugTypeId) -> &DebugType {
        &self.types[id.0 as usize]
    }
}
