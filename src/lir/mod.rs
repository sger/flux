//! Low-Level IR (LIR) — native backend IR for Flux (Proposal 0132).
//!
//! LIR is a flat, NaN-box-aware CFG with explicit memory operations.  It sits
//! between Core IR (functional, high-level) and LLVM IR (native binary).
//! The VM bytecode path uses CFG (`src/cfg/`) instead.
//!
//! ```text
//! Core IR (functional)
//!   │
//!   └── Core → LIR lowering (single pass)
//!         │
//!         └── LIR → LLVM IR emitter (native)
//! ```

#[cfg(feature = "core_to_llvm")]
pub mod emit_llvm;
pub mod lower;

use std::collections::HashMap;
use std::fmt;

use crate::core::CorePrimOp;

// ── Function identity ──────────────────────────────────────────────────────

/// Stable, unique identifier for a function in the LIR program.
///
/// For top-level functions, this is 1:1 with `CoreBinderId`.
/// For letrec/lambda-generated functions, synthetic IDs are assigned
/// starting above the max `CoreBinderId`.
///
/// Both the bytecode and LLVM backends resolve function references
/// through `LirFuncId`, following GHC's Unique-based Cmm labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LirFuncId(pub u32);

// ── Variables and constants ──────────────────────────────────────────────────

/// An SSA variable in LIR.  Each variable is assigned exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LirVar(pub u32);

/// A block identifier in a function's CFG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// Literal constant values that can appear inline in LIR instructions.
#[derive(Debug, Clone, PartialEq)]
pub enum LirConst {
    /// Raw signed 64-bit integer (before NaN-boxing).
    Int(i64),
    /// IEEE 754 double (before NaN-boxing).
    Float(f64),
    /// Boolean value.
    Bool(bool),
    /// Interned string reference (index into string table).
    String(String),
    /// The None / unit sentinel value.
    None,
    /// NaN-boxed empty list sentinel.
    EmptyList,
    /// A pre-tagged NaN-boxed i64 literal (already in runtime representation).
    Tagged(i64),
}

/// Integer comparison operators for `ICmp` instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
}

// ── Instructions ─────────────────────────────────────────────────────────────

/// A single LIR instruction.  All instructions operate on `LirVar` SSA values
/// representing NaN-boxed `i64` words (the Flux runtime representation).
#[derive(Debug, Clone)]
pub enum LirInstr {
    // ── Memory ──────────────────────────────────────────────────────
    /// Load a NaN-boxed word from `ptr + offset`.
    Load {
        dst: LirVar,
        ptr: LirVar,
        offset: i32,
    },
    /// Store a NaN-boxed word to `ptr + offset`.
    Store {
        ptr: LirVar,
        offset: i32,
        val: LirVar,
    },
    /// Store a raw 32-bit integer to `ptr + offset`.
    StoreI32 {
        ptr: LirVar,
        offset: i32,
        value: i32,
    },
    /// Allocate `size` bytes of heap memory.  Returns a raw pointer
    /// (not yet NaN-boxed).  The caller must `TagPtr` before storing.
    Alloc {
        dst: LirVar,
        size: u32,
        /// Number of pointer-sized fields the GC/RC system should scan.
        scan_fields: u8,
        /// Object type tag for the RC runtime header.
        obj_tag: u8,
    },

    // ── NaN-boxing ──────────────────────────────────────────────────
    /// Tag a raw i64 as a NaN-boxed integer.
    TagInt { dst: LirVar, raw: LirVar },
    /// Untag a NaN-boxed integer to raw i64.
    UntagInt { dst: LirVar, val: LirVar },
    /// Tag a raw f64 as a NaN-boxed float.
    TagFloat { dst: LirVar, raw: LirVar },
    /// Untag a NaN-boxed float to raw f64.
    UntagFloat { dst: LirVar, val: LirVar },
    /// Extract the NaN-box type tag (discriminant) from a value.
    GetTag { dst: LirVar, val: LirVar },
    /// Tag a raw heap pointer as a NaN-boxed pointer value.
    TagPtr { dst: LirVar, ptr: LirVar },
    /// Untag a NaN-boxed pointer to a raw heap pointer.
    UntagPtr { dst: LirVar, val: LirVar },
    /// Tag a boolean (0 or 1) into NaN-boxed form.
    TagBool { dst: LirVar, raw: LirVar },
    /// Untag a NaN-boxed boolean to 0 or 1.
    UntagBool { dst: LirVar, val: LirVar },

    // ── Inline arithmetic (no C call overhead) ──────────────────────
    /// Integer addition on raw (untagged) i64 values.
    IAdd { dst: LirVar, a: LirVar, b: LirVar },
    /// Integer subtraction on raw (untagged) i64 values.
    ISub { dst: LirVar, a: LirVar, b: LirVar },
    /// Integer multiplication on raw (untagged) i64 values.
    IMul { dst: LirVar, a: LirVar, b: LirVar },
    /// Signed integer division on raw i64 values.
    IDiv { dst: LirVar, a: LirVar, b: LirVar },
    /// Signed integer remainder on raw i64 values.
    IRem { dst: LirVar, a: LirVar, b: LirVar },
    /// Integer comparison on raw i64 values.  Result is 0 or 1.
    ICmp {
        dst: LirVar,
        op: CmpOp,
        a: LirVar,
        b: LirVar,
    },

    // ── C runtime calls (CorePrimOp dispatch) ───────────────────────
    /// Call a C runtime function identified by `CorePrimOp`.
    /// Arguments and result are NaN-boxed i64 values.
    PrimCall {
        dst: Option<LirVar>,
        op: CorePrimOp,
        args: Vec<LirVar>,
    },

    // ── Aether reference counting ───────────────────────────────────
    /// Increment the reference count of a NaN-boxed value.
    /// No-op for non-pointer values (Int, Float, Bool, None).
    Dup { val: LirVar },
    /// Decrement the reference count and free if zero.
    /// No-op for non-pointer values.
    Drop { val: LirVar },
    /// Check if a value's refcount is exactly 1 (uniquely owned).
    /// Result is 0 or 1 (raw, not NaN-boxed).
    IsUnique { dst: LirVar, val: LirVar },
    /// Drop for reuse: decrement refcount.  If unique, return the raw
    /// pointer for in-place reuse.  If shared, return null.
    /// `size` is the allocation size in bytes (header + fields) needed for
    /// fresh allocation when the value is shared.
    DropReuse { dst: LirVar, val: LirVar, size: u32 },

    // ── Closures ─────────────────────────────────────────────────────
    /// Create a closure from a nested function and captured values.
    /// `func_id` is the stable unique identity of the target function,
    /// resolved via `LirProgram.func_index` by each backend.
    /// `captures` are outer-scope LirVars whose values are baked into the closure.
    MakeClosure {
        dst: LirVar,
        func_id: LirFuncId,
        captures: Vec<LirVar>,
    },
    /// Create a closure from an externally linked top-level function symbol.
    /// Used by the native backend when an imported public function is referenced
    /// as a first-class value across module boundaries.
    MakeExternClosure {
        dst: LirVar,
        symbol: String,
        arity: usize,
    },

    // ── Collection construction ────────────────────────────────────
    /// Build an array from elements on the stack.
    MakeArray { dst: LirVar, elements: Vec<LirVar> },
    /// Build a tuple from elements.
    MakeTuple { dst: LirVar, elements: Vec<LirVar> },
    /// Build a hash map from interleaved key-value pairs.
    MakeHash { dst: LirVar, pairs: Vec<LirVar> },
    /// Build a cons list from elements (syntactic `[a, b, c]`).
    MakeList { dst: LirVar, elements: Vec<LirVar> },
    /// String interpolation from parts.
    Interpolate { dst: LirVar, parts: Vec<LirVar> },

    // ── Field access ──────────────────────────────────────────────────
    /// Extract a field from a tuple by index.
    ///
    /// High-level instruction: the bytecode emitter maps this to `OpTupleIndex`,
    /// the LLVM emitter expands to UntagPtr + Load at the field offset.
    TupleGet {
        dst: LirVar,
        tuple: LirVar,
        index: usize,
    },

    // ── Constructor creation ────────────────────────────────────────
    /// Build a constructor value from a tag and fields.
    ///
    /// This is a high-level instruction that the bytecode emitter maps to
    /// VM-specific opcodes (OpSome, OpCons, OpMakeAdt, etc.) and the LLVM
    /// emitter expands to Alloc/Store/TagPtr sequences.
    ///
    /// `ctor_tag`: the constructor's integer tag (Some=1, Left=2, Right=3, Cons=4, user=5+)
    /// `ctor_name`: the constructor's string name (for OpMakeAdt which needs it in the constant pool)
    /// `fields`: the field values (already lowered to LirVars)
    MakeCtor {
        dst: LirVar,
        ctor_tag: i32,
        ctor_name: Option<String>,
        fields: Vec<LirVar>,
        /// Per-field runtime representations (Proposal 0123 Phase 7g).
        /// When populated, enables unboxed field storage in ADT payloads.
        /// Empty means all fields are TaggedRep (legacy/unknown).
        field_reps: Vec<crate::core::FluxRep>,
    },

    // ── Variables ───────────────────────────────────────────────────
    /// Copy a value (no ref-count change — use Dup for ownership).
    Copy { dst: LirVar, src: LirVar },
    /// Load an immediate constant.
    Const { dst: LirVar, value: LirConst },

    // ── Globals ────────────────────────────────────────────────────────
    /// Load a value from the VM's global variable table.
    /// Used for imported/prelude functions that were compiled by the
    /// regular CFG pipeline and stored as globals.
    GetGlobal { dst: LirVar, global_idx: usize },
}

// ── Block terminators ────────────────────────────────────────────────────────

/// The terminator of a basic block — exactly one per block.
#[derive(Debug, Clone)]
pub enum LirTerminator {
    /// Return a value from the current function.
    Return(LirVar),
    /// Unconditional jump to a block.
    Jump(BlockId),
    /// Conditional branch on a boolean (0 or 1).
    Branch {
        cond: LirVar,
        then_block: BlockId,
        else_block: BlockId,
    },
    /// Multi-way switch on an integer tag.
    Switch {
        scrutinee: LirVar,
        cases: Vec<(i64, BlockId)>,
        default: BlockId,
    },
    /// Tail call (reuses the current stack frame).
    TailCall {
        func: LirVar,
        args: Vec<LirVar>,
        kind: CallKind,
    },
    /// Non-tail function call with a continuation block.
    /// The result is bound to `dst` in `cont`.
    Call {
        dst: LirVar,
        func: LirVar,
        args: Vec<LirVar>,
        cont: BlockId,
        kind: CallKind,
        /// Optional yield continuation function (Proposal 0134).
        /// When present, the LLVM emitter inserts a yield check after the call:
        /// if `flux_is_yielding()`, build a closure from this function + captured
        /// live vars, call `flux_yield_extend`, and return YIELD_SENTINEL.
        yield_cont: Option<(LirFuncId, Vec<LirVar>)>,
    },
    /// Constructor pattern match on a scrutinee value.
    ///
    /// High-level terminator that the bytecode emitter maps to VM-specific
    /// opcodes (OpIsCons, OpIsEmptyList, OpIsAdtJump, etc.) and the LLVM
    /// emitter expands to GetTag + Switch + Load sequences.
    ///
    /// Each arm has a constructor tag, field binders, and a target block.
    /// Field binders are LirVars that receive the extracted constructor fields
    /// at the start of the target block.
    MatchCtor {
        scrutinee: LirVar,
        arms: Vec<CtorArm>,
        default: BlockId,
    },
    /// Marks unreachable code (after panic, exhaustive match, etc.).
    Unreachable,
}

/// A single arm of a `MatchCtor` terminator.
#[derive(Debug, Clone)]
pub struct CtorArm {
    /// The constructor tag to match against.
    pub tag: CtorTag,
    /// LirVars that receive the extracted fields in the target block.
    pub field_binders: Vec<LirVar>,
    /// Target block if this constructor matches.
    pub target: BlockId,
}

/// Constructor tags for pattern matching.
#[derive(Debug, Clone)]
pub enum CtorTag {
    /// None value (NaN-box tag 0x2).
    None,
    /// Empty list `[]` (NaN-box tag 0x4).
    EmptyList,
    /// `Some(val)` — built-in, ctor_tag = 1.
    Some,
    /// `Left(val)` — built-in, ctor_tag = 2.
    Left,
    /// `Right(val)` — built-in, ctor_tag = 3.
    Right,
    /// `[h | t]` cons cell — built-in, ctor_tag = 4.
    Cons,
    /// User-defined ADT constructor with string name.
    Named(String),
    /// Tuple.
    Tuple,
}

// ── Call kind ───────────────────────────────────────────────────────────────

/// Distinguishes known direct calls from unknown closure dispatch.
/// Following GHC's Cmm approach: known calls use direct `call @func(i64, ...)`
/// while unknown/higher-order calls go through `flux_call_closure`.
#[derive(Debug, Clone)]
pub enum CallKind {
    /// Known top-level function — emit direct call with individual i64 params.
    Direct { func_id: LirFuncId },
    /// Known imported top-level function — emit direct call to an external symbol.
    DirectExtern { symbol: String },
    /// Unknown closure or higher-order value — dispatch via `flux_call_closure`.
    Indirect,
}

// ── Program structure ────────────────────────────────────────────────────────

/// A basic block: a sequence of instructions followed by a terminator.
#[derive(Debug, Clone)]
pub struct LirBlock {
    pub id: BlockId,
    /// Block parameters (like phi-node arguments in SSA).
    pub params: Vec<LirVar>,
    pub instrs: Vec<LirInstr>,
    pub terminator: LirTerminator,
}

/// A function in LIR.
#[derive(Debug, Clone)]
pub struct LirFunction {
    /// Human-readable name for debugging / symbol tables.
    pub name: String,
    /// Stable unique identity (1:1 with CoreBinderId for top-level defs).
    pub id: LirFuncId,
    /// Module-qualified symbol name for LLVM emission.
    /// E.g. `"Flow_List_sort"`, `"main"`, `"lambda_42"`.
    pub qualified_name: String,
    /// Parameter variables.
    pub params: Vec<LirVar>,
    /// The entry block is always `blocks[0]`.
    pub blocks: Vec<LirBlock>,
    /// Next free variable ID for this function (for allocating fresh vars).
    pub next_var: u32,
    /// LirVars in this function that are free (captured from the enclosing scope).
    /// The bytecode emitter maps these to `OpGetFree(index)` instead of `OpGetLocal`.
    pub capture_vars: Vec<LirVar>,
    /// Per-parameter runtime representation (from CoreBinder::rep via HM inference).
    /// Used by the LLVM emitter for worker/wrapper unboxing (Phase 10).
    pub param_reps: Vec<crate::core::FluxRep>,
    /// Return type representation (from CoreDef::result_ty).
    pub result_rep: crate::core::FluxRep,
}

/// A complete LIR program — a collection of functions.
#[derive(Debug, Clone)]
pub struct LirProgram {
    pub functions: Vec<LirFunction>,
    /// String constants referenced by `LirConst::String`.
    pub string_pool: Vec<String>,
    /// Index from stable function ID → position in `functions[]`.
    /// Both backends use this to resolve `LirFuncId` references.
    pub func_index: HashMap<LirFuncId, usize>,
    /// User-defined ADT constructor name → tag ID.
    /// Built-in constructors (Some=1, Left=2, Right=3, Cons=4) are implicit.
    /// User constructors start at 5 and are assigned sequentially.
    pub constructor_tags: HashMap<String, i32>,
    /// Monotonic allocator for synthetic nested-function IDs.
    next_synthetic_func_id: u32,
}

// ── Display ──────────────────────────────────────────────────────────────────

impl fmt::Display for LirFuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn#{}", self.0)
    }
}

impl fmt::Display for LirVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CmpOp::Eq => write!(f, "eq"),
            CmpOp::Ne => write!(f, "ne"),
            CmpOp::Slt => write!(f, "slt"),
            CmpOp::Sle => write!(f, "sle"),
            CmpOp::Sgt => write!(f, "sgt"),
            CmpOp::Sge => write!(f, "sge"),
        }
    }
}

impl LirFunction {
    /// Allocate a fresh `LirVar`.
    pub fn fresh_var(&mut self) -> LirVar {
        let v = LirVar(self.next_var);
        self.next_var += 1;
        v
    }
}

impl LirProgram {
    /// Create an empty program.
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            string_pool: Vec::new(),
            func_index: HashMap::new(),
            constructor_tags: HashMap::new(),
            next_synthetic_func_id: u32::MAX,
        }
    }

    /// Push a function and register it in the index.
    pub fn push_function(&mut self, func: LirFunction) -> usize {
        let idx = self.functions.len();
        self.func_index.insert(func.id, idx);
        self.functions.push(func);
        idx
    }

    /// Look up a function by its stable `LirFuncId`.
    pub fn func_by_id(&self, id: LirFuncId) -> Option<&LirFunction> {
        self.func_index.get(&id).map(|&idx| &self.functions[idx])
    }

    /// Get the position index for a `LirFuncId`.
    pub fn func_idx(&self, id: LirFuncId) -> Option<usize> {
        self.func_index.get(&id).copied()
    }

    /// Allocate a unique synthetic function id for nested lambdas/handlers.
    pub fn alloc_synthetic_func_id(&mut self) -> LirFuncId {
        let id = self.next_synthetic_func_id;
        self.next_synthetic_func_id = self.next_synthetic_func_id.saturating_sub(1);
        LirFuncId(id)
    }

    /// Intern a string constant, returning its index.
    pub fn intern_string(&mut self, s: String) -> usize {
        if let Some(idx) = self.string_pool.iter().position(|existing| *existing == s) {
            idx
        } else {
            let idx = self.string_pool.len();
            self.string_pool.push(s);
            idx
        }
    }
}

impl Default for LirProgram {
    fn default() -> Self {
        Self::new()
    }
}

// ── Pointer-tag helpers for LIR constants ───────────────────────────────────

/// Produce a pre-tagged pointer-tagged integer literal (inline).
/// Encoding: `(raw << 1) | 1` — LSB=1 marks an integer.
/// Used for effect tags and other small compile-time constants.
pub fn nanbox_tag_int(raw: i64) -> i64 {
    (raw << 1) | 1
}
