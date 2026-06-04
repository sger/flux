use crate::driver::{flags::DriverFlags, pipeline, pipeline::RunTarget};
#[cfg(feature = "llvm")]
use crate::llvm::pipeline::toolchain_info;

pub fn run(flags: DriverFlags, target: RunTarget) {
    pipeline::run_pipeline(flags, target);
}

pub fn eval(expr: &str, flags: DriverFlags) {
    pipeline::eval::run_eval(expr, flags);
}

pub fn repl(flags: DriverFlags) {
    #[cfg(feature = "repl")]
    crate::repl::run_repl(flags);
    // Native builds compile without the `repl` feature (it pulls in `rustyline` →
    // `clipboard-win`, whose Win32 imports trip Windows Defender on the small
    // native binaries that link the flux staticlib). Such a build still parses the
    // `repl` subcommand, so fail clearly rather than silently.
    #[cfg(not(feature = "repl"))]
    {
        let _ = flags;
        eprintln!(
            "This build of flux was compiled without REPL support \
             (the `repl` feature is disabled). Rebuild with the default features \
             (`cargo run -- repl`) to use the interactive REPL."
        );
        std::process::exit(2);
    }
}

pub fn init() {
    #[cfg(feature = "llvm")]
    if std::env::var_os("FLUX_PREWARM_TOOLCHAIN").is_some() {
        let _ = toolchain_info();
    }
}
