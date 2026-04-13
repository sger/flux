use std::{ffi::OsString, process::ExitCode};

pub mod render;

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    main::init();
    match parse_args(args) {
        Ok(command) => {
            match command {}
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::SUCCESS
        }
    }
}
