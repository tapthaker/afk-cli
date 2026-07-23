#![deny(unsafe_code)]

mod cli;

use cli::{Command, ParseError};
use std::ffi::OsStr;
use std::io::Write;

/// Stable process outcome returned by the CLI entry point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    /// The requested operation completed successfully.
    Success,
    /// AFK could not complete the operation.
    Failure,
    /// The command line was missing or invalid.
    Usage,
}

impl ExitStatus {
    /// Returns the conventional numeric process exit code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Usage => 2,
        }
    }
}

const HELP: &str = "AFK CLI keeps a remote terminal process alive across SSH disconnections.\n\
\n\
Usage:\n\
    afk --help\n\
    afk --version\n\
\n\
Session commands are not implemented in this development build.\n";

const MISSING_COMMAND: &str = "error: a command or option is required\n\
Try 'afk --help' for usage.\n";

const UNSUPPORTED_COMMAND: &str = "error: unsupported command or option\n\
Try 'afk --help' for usage.\n";

/// Runs AFK CLI with explicit streams and returns a typed process outcome.
///
/// Arguments are never included in diagnostics. Keeping streams explicit makes
/// CLI behavior testable without mutating process-global output handles.
pub fn run<I, S, O, E>(arguments: I, mut stdout: O, mut stderr: E) -> ExitStatus
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    O: Write,
    E: Write,
{
    match cli::parse(arguments) {
        Ok(Command::Help) => write_static(&mut stdout, HELP),
        Ok(Command::Version) => write_version(&mut stdout),
        Err(ParseError::MissingCommand) => write_usage_error(&mut stderr, MISSING_COMMAND),
        Err(ParseError::UnsupportedCommand) => write_usage_error(&mut stderr, UNSUPPORTED_COMMAND),
    }
}

fn write_static(output: &mut impl Write, message: &str) -> ExitStatus {
    match output.write_all(message.as_bytes()) {
        Ok(()) => ExitStatus::Success,
        Err(_) => ExitStatus::Failure,
    }
}

fn write_version(output: &mut impl Write) -> ExitStatus {
    if output.write_all(b"afk ").is_err()
        || output
            .write_all(env!("CARGO_PKG_VERSION").as_bytes())
            .is_err()
        || output.write_all(b"\n").is_err()
    {
        ExitStatus::Failure
    } else {
        ExitStatus::Success
    }
}

fn write_usage_error(stderr: &mut impl Write, message: &str) -> ExitStatus {
    match stderr.write_all(message.as_bytes()) {
        Ok(()) => ExitStatus::Usage,
        Err(_) => ExitStatus::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::{ExitStatus, run};
    use std::io::{self, Write};

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("synthetic write failure"))
        }
    }

    #[test]
    fn exit_status_codes_are_stable() {
        assert_eq!(ExitStatus::Success.code(), 0);
        assert_eq!(ExitStatus::Failure.code(), 1);
        assert_eq!(ExitStatus::Usage.code(), 2);
    }

    #[test]
    fn output_failure_returns_failure_without_secondary_diagnostic() {
        let mut stderr = Vec::new();
        let exit_code = run(["--version"], FailingWriter, &mut stderr);

        assert_eq!(exit_code, ExitStatus::Failure);
        assert!(stderr.is_empty());
    }
}
