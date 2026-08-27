//! Top-level driver pipeline orchestration entrypoints.

pub(crate) mod eval;
pub(crate) mod native;
pub(crate) mod parallel_shared;
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
    /// Arguments for the program itself, from after a `--` separator.
    ///
    /// Does **not** include the script path: the runtime prepends that so
    /// `Env.args()` reads like argv everywhere, with the program name first.
    pub program_args: Vec<String>,
}

/// Dispatches a driver invocation to the program or test pipeline.
pub fn run_pipeline(flags: DriverFlags, target: RunTarget) {
    // Install argv before anything runs. The script path leads, matching
    // argv[0] convention, so a Flux tool can name itself in its usage text.
    let mut argv = Vec::with_capacity(target.program_args.len() + 1);
    argv.push(target.path.clone());
    argv.extend(target.program_args.iter().cloned());
    crate::vm::set_program_args(argv);

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
    fn run_target_clone_preserves_path_mode_and_args() {
        let target = RunTarget {
            path: "examples/guide/arithmetic.flx".to_string(),
            mode: RunMode::Program,
            program_args: vec!["--verbose".to_string()],
        };

        let cloned = target.clone();

        assert_eq!(cloned.path, "examples/guide/arithmetic.flx");
        assert_eq!(cloned.mode, RunMode::Program);
        assert_eq!(cloned.program_args, vec!["--verbose".to_string()]);
    }
}
