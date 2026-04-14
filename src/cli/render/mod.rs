//! Text rendering helpers used by the CLI.
//!
//! Keeping text output isolated from parsing logic makes command validation easier to test and
//! keeps command-line modules focused on control flow instead of string construction.

pub mod text;
