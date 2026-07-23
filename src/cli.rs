use std::ffi::OsStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    Help,
    Version,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParseError {
    MissingCommand,
    UnsupportedCommand,
}

pub(crate) fn parse<I, S>(arguments: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut arguments = arguments.into_iter();
    let Some(argument) = arguments.next() else {
        return Err(ParseError::MissingCommand);
    };

    if arguments.next().is_some() {
        return Err(ParseError::UnsupportedCommand);
    }

    match argument.as_ref().as_encoded_bytes() {
        b"--help" | b"-h" => Ok(Command::Help),
        b"--version" | b"-V" => Ok(Command::Version),
        _ => Err(ParseError::UnsupportedCommand),
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, ParseError, parse};
    use std::ffi::OsString;

    #[test]
    fn parses_help_and_version_options() {
        assert_eq!(parse(["--help"]), Ok(Command::Help));
        assert_eq!(parse(["-h"]), Ok(Command::Help));
        assert_eq!(parse(["--version"]), Ok(Command::Version));
        assert_eq!(parse(["-V"]), Ok(Command::Version));
    }

    #[test]
    fn rejects_missing_unsupported_and_trailing_arguments() {
        assert_eq!(
            parse(Vec::<OsString>::new()),
            Err(ParseError::MissingCommand)
        );
        assert_eq!(parse(["stream"]), Err(ParseError::UnsupportedCommand));
        assert_eq!(
            parse(["--version", "unexpected"]),
            Err(ParseError::UnsupportedCommand)
        );
    }
}
