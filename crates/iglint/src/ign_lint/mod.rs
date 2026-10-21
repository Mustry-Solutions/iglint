//! `iglint ign-lint`: a drop-in replacement for the ign-lint 0.6.4 CLI.

use std::ffi::OsString;

use crate::{Environment, ExitStatus, Io};

/// Runs ign-lint compatible mode. `args[0]` is the program name as ign-lint
/// would show it (`ign-lint`).
pub fn run(args: Vec<OsString>, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    let _ = (args, environment, io);
    unimplemented!("ign-lint mode")
}
