use crate::identity::SessionId;
use crate::limits::{MAX_COMMAND_ARGUMENTS, MAX_COMMAND_BYTES};
use std::ffi::{OsStr, OsString};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    Help,
    Version,
    Stream {
        session: SessionId,
        command: Vec<OsString>,
    },
    Attach {
        session: SessionId,
    },
    Sessions {
        json: bool,
    },
    Stop {
        session: SessionId,
    },
    HiddenRunner {
        session: SessionId,
    },
    HiddenChild {
        command: Vec<OsString>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParseError {
    MissingCommand,
    UnsupportedCommand,
    InvalidSessionId,
    InvalidArguments,
    CommandTooLarge,
}

pub(crate) fn parse<I, S>(arguments: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let arguments: Vec<OsString> = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_os_string())
        .collect();
    let Some(first) = arguments.first() else {
        return Err(ParseError::MissingCommand);
    };

    match first.as_encoded_bytes() {
        b"--help" | b"-h" if arguments.len() == 1 => Ok(Command::Help),
        b"--version" | b"-V" if arguments.len() == 1 => Ok(Command::Version),
        b"stream" => parse_stream(&arguments[1..]),
        b"attach" => parse_session_command(&arguments[1..], |session| Command::Attach { session }),
        b"sessions" => parse_sessions(&arguments[1..]),
        b"stop" => parse_session_command(&arguments[1..], |session| Command::Stop { session }),
        b"__runner" => {
            parse_session_command(&arguments[1..], |session| Command::HiddenRunner { session })
        }
        b"__child" => parse_hidden_child(&arguments[1..]),
        _ => Err(ParseError::UnsupportedCommand),
    }
}

fn parse_stream(arguments: &[OsString]) -> Result<Command, ParseError> {
    let Some(session) = arguments.first() else {
        return Err(ParseError::InvalidArguments);
    };
    let session = parse_session_id(session)?;
    let command = match &arguments[1..] {
        [] => Vec::new(),
        [separator, command @ ..]
            if separator.as_encoded_bytes() == b"--" && !command.is_empty() =>
        {
            validate_command(command)?;
            command.to_vec()
        }
        _ => return Err(ParseError::InvalidArguments),
    };
    Ok(Command::Stream { session, command })
}

fn parse_session_command(
    arguments: &[OsString],
    command: impl FnOnce(SessionId) -> Command,
) -> Result<Command, ParseError> {
    let [session] = arguments else {
        return Err(ParseError::InvalidArguments);
    };
    parse_session_id(session).map(command)
}

fn parse_sessions(arguments: &[OsString]) -> Result<Command, ParseError> {
    match arguments {
        [] => Ok(Command::Sessions { json: false }),
        [option] if option.as_encoded_bytes() == b"--json" => Ok(Command::Sessions { json: true }),
        _ => Err(ParseError::InvalidArguments),
    }
}

fn parse_hidden_child(arguments: &[OsString]) -> Result<Command, ParseError> {
    if arguments.is_empty() {
        return Err(ParseError::InvalidArguments);
    }
    validate_command(arguments)?;
    Ok(Command::HiddenChild {
        command: arguments.to_vec(),
    })
}

fn parse_session_id(value: &OsStr) -> Result<SessionId, ParseError> {
    SessionId::parse_bytes(value.as_encoded_bytes()).map_err(|_| ParseError::InvalidSessionId)
}

fn validate_command(command: &[OsString]) -> Result<(), ParseError> {
    if command.len() > MAX_COMMAND_ARGUMENTS {
        return Err(ParseError::CommandTooLarge);
    }
    let aggregate = command.iter().try_fold(0_usize, |total, argument| {
        total.checked_add(argument.as_encoded_bytes().len())
    });
    match aggregate {
        Some(length) if length <= MAX_COMMAND_BYTES => Ok(()),
        _ => Err(ParseError::CommandTooLarge),
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, ParseError, parse};
    use crate::limits::{MAX_COMMAND_ARGUMENTS, MAX_COMMAND_BYTES};
    use std::ffi::OsString;

    const SESSION: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn parses_help_and_version_options() {
        assert_eq!(parse(["--help"]), Ok(Command::Help));
        assert_eq!(parse(["-h"]), Ok(Command::Help));
        assert_eq!(parse(["--version"]), Ok(Command::Version));
        assert_eq!(parse(["-V"]), Ok(Command::Version));
    }

    #[test]
    fn parses_public_session_commands() {
        let session = SESSION.parse();
        assert!(session.is_ok());
        let session = session.unwrap_or_else(|_| unreachable!());
        assert_eq!(
            parse(["stream", SESSION]),
            Ok(Command::Stream {
                session,
                command: Vec::new()
            })
        );
        assert_eq!(
            parse(["stream", SESSION, "--", "/bin/echo", "hello"]),
            Ok(Command::Stream {
                session,
                command: vec![OsString::from("/bin/echo"), OsString::from("hello")],
            })
        );
        assert_eq!(parse(["attach", SESSION]), Ok(Command::Attach { session }));
        assert_eq!(parse(["sessions"]), Ok(Command::Sessions { json: false }));
        assert_eq!(
            parse(["sessions", "--json"]),
            Ok(Command::Sessions { json: true })
        );
        assert_eq!(parse(["stop", SESSION]), Ok(Command::Stop { session }));
    }

    #[test]
    fn rejects_missing_unsupported_and_malformed_arguments() {
        assert_eq!(
            parse(Vec::<OsString>::new()),
            Err(ParseError::MissingCommand)
        );
        assert_eq!(parse(["unknown"]), Err(ParseError::UnsupportedCommand));
        assert_eq!(
            parse(["--version", "unexpected"]),
            Err(ParseError::UnsupportedCommand)
        );
        assert_eq!(parse(["stream"]), Err(ParseError::InvalidArguments));
        assert_eq!(
            parse(["stream", SESSION, "--"]),
            Err(ParseError::InvalidArguments)
        );
        assert_eq!(
            parse(["stream", SESSION, "/bin/sh"]),
            Err(ParseError::InvalidArguments)
        );
        assert_eq!(
            parse(["attach", "not-an-id"]),
            Err(ParseError::InvalidSessionId)
        );
        assert_eq!(
            parse(["sessions", "--other"]),
            Err(ParseError::InvalidArguments)
        );
    }

    #[test]
    fn enforces_command_count_and_byte_bounds() {
        let mut too_many = vec![
            OsString::from("stream"),
            OsString::from(SESSION),
            OsString::from("--"),
        ];
        too_many.extend((0..=MAX_COMMAND_ARGUMENTS).map(|_| OsString::from("x")));
        assert_eq!(parse(too_many), Err(ParseError::CommandTooLarge));

        let too_large = "x".repeat(MAX_COMMAND_BYTES + 1);
        assert_eq!(
            parse([
                OsString::from("stream"),
                OsString::from(SESSION),
                OsString::from("--"),
                OsString::from(too_large)
            ]),
            Err(ParseError::CommandTooLarge)
        );
    }
}
