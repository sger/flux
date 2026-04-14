//! Top-level driver pipeline orchestration entrypoints.

pub(crate) mod native;
pub mod program;
pub(crate) mod vm;

use crate::driver::{
    flags::DriverFlags,
    mode::RunMode,
    pipeline::program::{RunProgramRequest, run_file},
    run_tests::{TestRunRequest, run_test_file},
    session::DriverSession,
};

#[derive(Debug, Clone)]
/// Fully resolved run target selected by the driver.
pub struct RunTarget {
    pub path: String,
    pub mode: RunMode,
}

/// Dispatches a driver invocation to the program or test pipeline.
pub fn run_pipeline(flags: DriverFlags, target: RunTarget) {
    let session = DriverSession::from(&flags);
    match target.mode {
        RunMode::Program => run_file(RunProgramRequest {
            path: &target.path,
            flags: &flags,
            session: &session,
        }),
        RunMode::Tests => run_test_file(
            &target.path,
            TestRunRequest {
                flags: &flags,
                session: &session,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::RunTarget;
    use crate::driver::mode::RunMode;

    #[test]
    fn run_target_clone_preserves_path_and_mode() {
        let target = RunTarget {
            path: "examples/basics/arithmetic.flx".to_string(),
            mode: RunMode::Program,
        };

        let cloned = target.clone();

        assert_eq!(cloned.path, "examples/basics/arithmetic.flx");
        assert_eq!(cloned.mode, RunMode::Program);
    }
}
