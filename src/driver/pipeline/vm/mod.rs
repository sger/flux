//! VM-specific pipeline entrypoints for parallel module compilation.

use std::path::PathBuf;

use crate as flux;
use crate::{cache_paths::CacheLayout, syntax::module_graph::ModuleGraph};

mod parallel;

/// Summary of a parallel VM build ready for execution.
pub(crate) struct ParallelVmBuild {
    pub(crate) bytecode: flux::bytecode::bytecode::Bytecode,
    pub(crate) symbol_table: flux::bytecode::symbol_table::SymbolTable,
    pub(crate) cached_count: usize,
    pub(crate) compiled_count: usize,
}

/// Inputs required to compile a module graph into VM bytecode in parallel.
pub(crate) struct VmCompileRequest<'a> {
    pub(crate) graph: &'a ModuleGraph,
    pub(crate) entry_canonical: Option<&'a PathBuf>,
    pub(crate) graph_interner: &'a flux::syntax::interner::Interner,
    pub(crate) cache_layout: &'a CacheLayout,
    pub(crate) no_cache: bool,
    pub(crate) strict_mode: bool,
    pub(crate) strict_types: bool,
    pub(crate) enable_optimize: bool,
    pub(crate) enable_analyze: bool,
    pub(crate) verbose: bool,
}

/// Compiles a module graph into a linked VM program using the parallel VM pipeline.
pub(crate) fn compile_vm_modules_parallel(
    request: VmCompileRequest<'_>,
    all_diagnostics: &mut Vec<flux::diagnostics::Diagnostic>,
) -> Result<ParallelVmBuild, String> {
    parallel::compile_vm_modules_parallel(request, all_diagnostics)
}

#[cfg(test)]
mod tests {
    use super::ParallelVmBuild;
    use crate::bytecode::{bytecode::Bytecode, symbol_table::SymbolTable};

    #[test]
    fn parallel_vm_build_preserves_cache_counters() {
        let build = ParallelVmBuild {
            bytecode: Bytecode {
                instructions: Vec::new(),
                constants: Vec::new(),
                debug_info: None,
            },
            symbol_table: SymbolTable::default(),
            cached_count: 3,
            compiled_count: 2,
        };

        assert_eq!(build.cached_count, 3);
        assert_eq!(build.compiled_count, 2);
        assert_eq!(build.symbol_table.num_definitions, 0);
        assert!(build.bytecode.instructions.is_empty());
    }
}
