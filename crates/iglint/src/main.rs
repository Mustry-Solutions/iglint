use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let environment = iglint::Environment::from_process();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let stdin = std::io::stdin();
    let mut io = iglint::Io {
        stdout: &mut stdout.lock(),
        stderr: &mut stderr.lock(),
        stdin: &mut stdin.lock(),
        stdout_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdout()),
    };
    ExitCode::from(iglint::run(args, &environment, &mut io) as u8)
}
