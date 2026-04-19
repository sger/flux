use crate as flux;
use crate::driver::mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat};
use crate::driver::support::shared::{DiagnosticRenderRequest, emit_diagnostics};
use flux::syntax::program::Program;

pub(crate) struct DumpRequest<'a> {
    pub(crate) compiler: &'a mut flux::compiler::Compiler,
    pub(crate) merged_program: &'a Program,
    pub(crate) path: &'a str,
    pub(crate) source: &'a str,
    pub(crate) is_multimodule: bool,
    pub(crate) max_errors: usize,
    pub(crate) diagnostics_format: DiagnosticOutputFormat,
    pub(crate) all_errors: bool,
    pub(crate) enable_optimize: bool,
    pub(crate) dump_aether: AetherDumpMode,
    pub(crate) dump_core: CoreDumpMode,
    pub(crate) dump_lir: bool,
    pub(crate) dump_cfg: bool,
    #[cfg_attr(not(feature = "llvm"), allow(unused))]
    pub(crate) dump_lir_llvm: bool,
}

pub(crate) fn handle_dumps(request: DumpRequest<'_>) -> bool {
    if request.dump_aether != AetherDumpMode::None {
        match request.compiler.dump_aether_report(
            request.merged_program,
            request.enable_optimize,
            request.dump_aether == AetherDumpMode::Debug,
        ) {
            Ok(report) => println!("{report}"),
            Err(diag) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: &[diag],
                    default_file: Some(request.path),
                    default_source: Some(request.source),
                    show_file_headers: request.is_multimodule,
                    max_errors: request.max_errors,
                    format: request.diagnostics_format,
                    all_errors: request.all_errors,
                    text_to_stderr: true,
                });
                std::process::exit(1);
            }
        }
        return true;
    }

    if !matches!(request.dump_core, CoreDumpMode::None) {
        let dumped = request.compiler.dump_core_with_opts(
            request.merged_program,
            request.enable_optimize,
            match request.dump_core {
                CoreDumpMode::Readable => flux::core::display::CoreDisplayMode::Readable,
                CoreDumpMode::Debug => flux::core::display::CoreDisplayMode::Debug,
                CoreDumpMode::None => unreachable!("checked above"),
            },
        );
        match dumped {
            Ok(dumped) => println!("{dumped}"),
            Err(diag) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: &[diag],
                    default_file: Some(request.path),
                    default_source: Some(request.source),
                    show_file_headers: request.is_multimodule,
                    max_errors: request.max_errors,
                    format: request.diagnostics_format,
                    all_errors: request.all_errors,
                    text_to_stderr: true,
                });
                std::process::exit(1);
            }
        }
        return true;
    }

    if request.dump_lir {
        let dumped = request
            .compiler
            .dump_lir(request.merged_program, request.enable_optimize);
        match dumped {
            Ok(dumped) => println!("{dumped}"),
            Err(diag) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: &[diag],
                    default_file: Some(request.path),
                    default_source: Some(request.source),
                    show_file_headers: request.is_multimodule,
                    max_errors: request.max_errors,
                    format: request.diagnostics_format,
                    all_errors: request.all_errors,
                    text_to_stderr: true,
                });
                std::process::exit(1);
            }
        }
        return true;
    }

    if request.dump_cfg {
        let dumped = request
            .compiler
            .dump_cfg(request.merged_program, request.enable_optimize);
        match dumped {
            Ok(dumped) => println!("{dumped}"),
            Err(diag) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: &[diag],
                    default_file: Some(request.path),
                    default_source: Some(request.source),
                    show_file_headers: request.is_multimodule,
                    max_errors: request.max_errors,
                    format: request.diagnostics_format,
                    all_errors: request.all_errors,
                    text_to_stderr: true,
                });
                std::process::exit(1);
            }
        }
        return true;
    }

    #[cfg(feature = "llvm")]
    if request.dump_lir_llvm {
        match request
            .compiler
            .dump_lir_llvm(request.merged_program, request.enable_optimize)
        {
            Ok(ir_text) => println!("{ir_text}"),
            Err(diag) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: &[diag],
                    default_file: Some(request.path),
                    default_source: Some(request.source),
                    show_file_headers: request.is_multimodule,
                    max_errors: request.max_errors,
                    format: request.diagnostics_format,
                    all_errors: request.all_errors,
                    text_to_stderr: true,
                });
                std::process::exit(1);
            }
        }
        return true;
    }

    false
}
