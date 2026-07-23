#![deny(unsafe_code)]

#[cfg(any(target_os = "linux", test))]
mod byte_queue;
mod cli;
mod identity;
#[cfg(any(target_os = "linux", test))]
mod ipc;
mod limits;
#[cfg(any(target_os = "linux", test))]
mod output_tail;
mod platform;

#[cfg(target_os = "linux")]
mod attach;
#[cfg(target_os = "linux")]
mod registry;
#[cfg(target_os = "linux")]
mod runner;
#[cfg(target_os = "linux")]
mod session;

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
    /// The managed process returned this status.
    Child(u8),
}

impl ExitStatus {
    /// Returns the conventional numeric process exit code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Usage => 2,
            Self::Child(code) => code,
        }
    }
}

const HELP: &str = "AFK CLI keeps a remote terminal process alive across SSH disconnections.\n\
\n\
Usage:\n\
    afk --help\n\
    afk --version\n\
    afk stream SESSION_ID [-- COMMAND [ARG...]]\n\
    afk attach SESSION_ID\n\
    afk sessions [--json]\n\
    afk stop SESSION_ID\n\
\n\
Session commands require Linux.\n";

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
        Ok(
            command @ (Command::Stream { .. }
            | Command::Attach { .. }
            | Command::Sessions { .. }
            | Command::Stop { .. }
            | Command::HiddenRunner { .. }
            | Command::HiddenChild { .. }),
        ) => run_session_command(command, &mut stdout, &mut stderr),
        Err(ParseError::MissingCommand) => write_usage_error(&mut stderr, MISSING_COMMAND),
        Err(
            ParseError::UnsupportedCommand
            | ParseError::InvalidSessionId
            | ParseError::InvalidArguments
            | ParseError::CommandTooLarge,
        ) => write_usage_error(&mut stderr, UNSUPPORTED_COMMAND),
    }
}

#[cfg(target_os = "linux")]
fn run_session_command(
    command: Command,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitStatus {
    let result = match command {
        Command::Stream { session, command } => session::stream(session, &command, stdout)
            .map(|status| ExitStatus::Child(status.status_code())),
        Command::Attach { session } => {
            session::attach(session, stdout).map(|status| ExitStatus::Child(status.status_code()))
        }
        Command::Sessions { json } => session::sessions(json, stdout).map(|()| ExitStatus::Success),
        Command::Stop { session } => session::stop(session).map(|()| ExitStatus::Success),
        Command::HiddenRunner { session } => {
            runner::hidden_runner(session).map(|()| ExitStatus::Success)
        }
        Command::HiddenChild { command } => {
            runner::hidden_child(command).map(|()| ExitStatus::Success)
        }
        Command::Help | Command::Version => Ok(ExitStatus::Success),
    };
    match result {
        Ok(status) => status,
        Err(error) => write_session_error(stderr, error.kind()),
    }
}

#[cfg(not(target_os = "linux"))]
fn run_session_command(
    _command: Command,
    _stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitStatus {
    write_failure(stderr, "error: session commands require Linux\n")
}

#[cfg(target_os = "linux")]
fn write_session_error(stderr: &mut impl Write, kind: std::io::ErrorKind) -> ExitStatus {
    let message = match kind {
        std::io::ErrorKind::AlreadyExists => "error: session already exists\n",
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            "error: session not found\n"
        }
        std::io::ErrorKind::PermissionDenied => "error: unsafe session runtime entry\n",
        _ => "error: session operation failed\n",
    };
    write_failure(stderr, message)
}

fn write_failure(output: &mut impl Write, message: &str) -> ExitStatus {
    let _ = output.write_all(message.as_bytes());
    ExitStatus::Failure
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
        assert_eq!(ExitStatus::Child(17).code(), 17);
    }

    #[test]
    fn output_failure_returns_failure_without_secondary_diagnostic() {
        let mut stderr = Vec::new();
        let exit_code = run(["--version"], FailingWriter, &mut stderr);

        assert_eq!(exit_code, ExitStatus::Failure);
        assert!(stderr.is_empty());
    }
}
