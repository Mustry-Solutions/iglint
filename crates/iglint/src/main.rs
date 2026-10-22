#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let environment = iglint::Environment::from_process();
    // Unlocked handles: `iglint server` reads and writes the standard
    // streams from its own threads, which would block on a held lock.
    let mut io = iglint::Io {
        stdout: &mut std::io::stdout(),
        stderr: &mut std::io::stderr(),
        stdin: &mut std::io::stdin(),
        stdout_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdout()),
    };
    ExitCode::from(iglint::run(args, &environment, &mut io) as u8)
}
