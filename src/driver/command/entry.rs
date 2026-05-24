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
    crate::repl::run_repl(flags);
}

pub fn init() {
    #[cfg(feature = "llvm")]
    if std::env::var_os("FLUX_PREWARM_TOOLCHAIN").is_some() {
        let _ = toolchain_info();
    }
}
