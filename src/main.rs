#![deny(unsafe_code)]

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    let exit_code = afk_cli::run(
        std::env::args_os().skip(1),
        io::stdout().lock(),
        io::stderr().lock(),
    );
    ExitCode::from(exit_code.code())
}
