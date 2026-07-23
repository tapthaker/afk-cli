use crate::attach;
use crate::identity::SessionId;
use crate::ipc::ProcessExit;
use crate::platform::linux::window_size;
use crate::registry::{ExitMetadata, Registry, SessionMetadata};
use crate::runner;
use rustix::fd::AsFd;
use std::ffi::OsString;
use std::io::{self, Write};

pub(crate) fn stream(
    session: SessionId,
    command: &[OsString],
    output: &mut impl Write,
) -> io::Result<ProcessExit> {
    let stdin = std::io::stdin();
    let (rows, columns) = window_size(stdin.as_fd()).unwrap_or((24, 80));
    runner::launch_runner(session, command, rows, columns)?;
    attach::attach(session, output)
}

pub(crate) fn attach(session: SessionId, output: &mut impl Write) -> io::Result<ProcessExit> {
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
