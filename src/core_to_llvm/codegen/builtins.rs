//! Base function (built-in) support for core_to_llvm.
//!
//! Maps Flux base function names to C runtime function declarations.
//! Only functions with a direct C runtime equivalent are supported;
//! others require closure wrapping (future work).

use crate::core_to_llvm::{
    CallConv, GlobalId, LlvmDecl, LlvmFunctionSig, LlvmModule, LlvmType,
};
use crate::core_to_llvm::Linkage;

/// Describes a base function's C runtime mapping.
pub struct BuiltinMapping {
    /// The Flux base function name (e.g., "print").
    pub flux_name: &'static str,
    /// The C runtime function name (e.g., "flux_println").
    pub c_name: &'static str,
    /// Number of parameters.
    pub arity: usize,
    /// Whether the function returns a value (i64) or void.
    pub returns_value: bool,
}

/// Known base function → C runtime mappings.
static BUILTIN_MAPPINGS: &[BuiltinMapping] = &[
    // I/O
    BuiltinMapping { flux_name: "print",      c_name: "flux_print",           arity: 1, returns_value: false },
    BuiltinMapping { flux_name: "println",     c_name: "flux_println",         arity: 1, returns_value: false },
    // String conversion
    BuiltinMapping { flux_name: "to_string",   c_name: "flux_int_to_string",   arity: 1, returns_value: true },
    // String operations
    BuiltinMapping { flux_name: "str_concat",  c_name: "flux_string_concat",   arity: 2, returns_value: true },
    BuiltinMapping { flux_name: "str_length",  c_name: "flux_string_length",   arity: 1, returns_value: true },
    BuiltinMapping { flux_name: "str_slice",   c_name: "flux_string_slice",    arity: 3, returns_value: true },
    // HAMT
    BuiltinMapping { flux_name: "hamt_empty",  c_name: "flux_hamt_empty",      arity: 0, returns_value: true },
    BuiltinMapping { flux_name: "hamt_get",    c_name: "flux_hamt_get",        arity: 2, returns_value: true },
    BuiltinMapping { flux_name: "hamt_set",    c_name: "flux_hamt_set",        arity: 3, returns_value: true },
    BuiltinMapping { flux_name: "hamt_delete", c_name: "flux_hamt_delete",     arity: 2, returns_value: true },
    // I/O (file)
    BuiltinMapping { flux_name: "read_file",   c_name: "flux_read_file",       arity: 1, returns_value: true },
    BuiltinMapping { flux_name: "read_stdin",  c_name: "flux_read_line",       arity: 0, returns_value: true },
];

/// Look up a base function's C runtime mapping by Flux name.
pub fn find_builtin(name: &str) -> Option<&'static BuiltinMapping> {
    BUILTIN_MAPPINGS.iter().find(|m| m.flux_name == name)
}

/// Ensure the C runtime declaration for a builtin exists in the module.
pub fn ensure_builtin_declared(module: &mut LlvmModule, mapping: &BuiltinMapping) {
    let name = mapping.c_name;
    // Check if already declared.
    if module.declarations.iter().any(|d| d.name.0 == name)
        || module.functions.iter().any(|f| f.name.0 == name)
    {
        return;
    }

    let params = vec![LlvmType::i64(); mapping.arity];
    let ret = if mapping.returns_value {
        LlvmType::i64()
    } else {
        LlvmType::Void
    };

    module.declarations.push(LlvmDecl {
        linkage: Linkage::External,
        name: GlobalId(name.into()),
        sig: LlvmFunctionSig {
            ret,
            params,
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".into()],
    });
}

/// Check if a name (resolved via interner) is a known base function.
pub fn is_known_builtin(name: &str) -> bool {
    find_builtin(name).is_some()
}
