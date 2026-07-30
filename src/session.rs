use crate::attach;
use crate::identity::SessionId;
use crate::ipc::ProcessExit;
use crate::platform::unix::window_size;
use crate::registry::{ExitMetadata, Registry, SessionMetadata};
use crate::runner;
use rustix::fd::AsFd;
use std::ffi::OsString;
use std::io::{self, Write};

pub(crate) fn connect_or_start(
    session: SessionId,
    trace: bool,
    command: &[OsString],
    output: &mut impl Write,
) -> io::Result<ProcessExit> {
    let connect_error = match attach::attach(session, output) {
        Ok(status) => return Ok(status),
        Err(error) => error,
    };
    if !matches!(
        connect_error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    ) {
        return Err(connect_error);
    }

    // A live record whose runner cannot be reached is ambiguous. Fail closed
    // instead of silently replacing a session that the caller meant to resume.
    if Registry::open()?.read_metadata(session)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "recorded session runner is unavailable",
        ));
    }

    let stdin = std::io::stdin();
    let (rows, columns) = window_size(stdin.as_fd()).unwrap_or((24, 80));
    match runner::launch_runner(session, trace, command, rows, columns) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    attach::attach(session, output)
}

pub(crate) fn stop(session: SessionId) -> io::Result<()> {
    attach::stop(session)
}

pub(crate) fn sessions(json: bool, output: &mut impl Write) -> io::Result<()> {
    let sessions = Registry::open()?.list()?;
    if json {
        serde_json::to_writer(&mut *output, &sessions)
            .map_err(|_| io::Error::other("session listing encoding failed"))?;
        output.write_all(b"\n")?;
        return Ok(());
    }

    for metadata in sessions {
        match metadata {
            SessionMetadata::Live {
                session_id,
                runner_pid,
                child_pid,
                started_at,
                attached,
            } => writeln!(
                output,
                "{session_id} live runner={runner_pid} child={child_pid} started={started_at} attached={attached}"
            )?,
            SessionMetadata::Completed {
                session_id,
                started_at,
                finished_at,
                exit,
                output_bytes,
                truncated,
            } => {
                let status = match exit {
                    ExitMetadata::Code(code) => format!("code={code}"),
                    ExitMetadata::Signal(signal) => format!("signal={signal}"),
                };
                writeln!(
                    output,
                    "{session_id} completed started={started_at} finished={finished_at} {status} output_bytes={output_bytes} truncated={truncated}"
                )?;
            }
        }
    }
    Ok(())
}
