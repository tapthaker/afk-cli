use crate::byte_queue::ByteQueue;
use crate::identity::SessionId;
use crate::ipc::{Decoder, ProcessExit, Record, encode};
use crate::limits::{
    MAX_ATTACHMENT_QUEUE_BYTES, MAX_COMMAND_ARGUMENTS, MAX_COMMAND_BYTES, MAX_IPC_PAYLOAD_BYTES,
    MAX_PTY_BYTES_PER_TICK, OUTPUT_TAIL_BYTES, STOP_GRACE_SECONDS,
};
use crate::output_tail::{OutputTail, TRUNCATION_MARKER};
use crate::platform::linux::{become_session_leader, create_pty, set_nonblocking, set_window_size};
use crate::registry::{Registry, SessionFiles, SessionMetadata, now_seconds};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::AsFd;
use rustix::fs::Mode;
use rustix::net::sockopt::socket_peercred;
use rustix::process::{Pid, Signal, getpid, getuid, kill_process, umask};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP_READY: u8 = 0;
const STARTUP_EXISTS: u8 = 1;
const STARTUP_FAILED: u8 = 2;
const IO_CHUNK_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

pub(crate) fn launch_runner(
    session: SessionId,
    command: &[OsString],
    rows: u16,
    columns: u16,
) -> io::Result<()> {
    let (mut launcher, runner) = UnixStream::pair()?;
    let runner_fd: rustix::fd::OwnedFd = runner.into();
    let runner_stdio = Stdio::from(runner_fd);
    let mut process = Command::new(std::env::current_exe()?)
        .arg("__runner")
        .arg(session.to_string())
        .stdin(runner_stdio)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    write_startup_config(&mut launcher, command, rows, columns)?;
    let mut response = [STARTUP_FAILED];
    if let Err(error) = launcher.read_exact(&mut response) {
        let _ = process.wait();
        return Err(error);
    }
    match response[0] {
        STARTUP_READY => Ok(()),
        STARTUP_EXISTS => {
            let _ = process.wait();
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "session exists",
            ))
        }
        _ => {
            let _ = process.wait();
            Err(io::Error::other("session runner failed to start"))
        }
    }
}

pub(crate) fn hidden_runner(session: SessionId) -> io::Result<()> {
    let config = match read_startup_config() {
        Ok(config) => config,
        Err(error) => {
            let _ = write_startup_response(STARTUP_FAILED);
            return Err(error);
        }
    };
    match start_runner(session, config) {
        Ok(()) => Ok(()),
        Err(error) => {
            let response = if error.kind() == io::ErrorKind::AlreadyExists {
                STARTUP_EXISTS
            } else {
                STARTUP_FAILED
            };
            let _ = write_startup_response(response);
            Err(error)
        }
    }
}

pub(crate) fn hidden_child(command: Vec<OsString>) -> io::Result<()> {
    become_session_leader()?;
    let stdin = std::io::stdin();
    crate::platform::linux::acquire_controlling_terminal(stdin.as_fd())?;
    let Some(program) = command.first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing child program",
        ));
    };
    let error = Command::new(program).args(&command[1..]).exec();
    Err(error)
}

struct StartupConfig {
    command: Vec<OsString>,
    rows: u16,
    columns: u16,
}

fn start_runner(session: SessionId, config: StartupConfig) -> io::Result<()> {
    become_session_leader()?;
    umask(Mode::RWXG | Mode::RWXO);
    let registry = Registry::open()?;
    let files = registry.bind_session(session)?;
    let (master, slave) = create_pty(config.rows, config.columns)?;
    let command = if config.command.is_empty() {
        vec![default_shell()]
    } else {
        config.command
    };
    let slave_stdout = rustix::io::dup(&slave).map_err(io::Error::from)?;
    let slave_stderr = rustix::io::dup(&slave).map_err(io::Error::from)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("__child")
        .args(command)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::from(slave_stdout))
        .stderr(Stdio::from(slave_stderr))
        .spawn()?;
    let started_at = now_seconds()?;
    let child_pid = child.id();
    let runner_pid = u32::try_from(getpid().as_raw_nonzero().get())
        .map_err(|_| io::Error::other("invalid runner PID"))?;
    let ready = registry
        .write_metadata(
            &files.paths,
            &live_metadata(session, runner_pid, child_pid, started_at, false),
        )
        .and_then(|()| write_startup_response(STARTUP_READY));
    if let Err(error) = ready {
        drop(master);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    let master = File::from(master);
    set_nonblocking(&master)?;
    run_event_loop(
        session, registry, files, master, &mut child, runner_pid, child_pid, started_at,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_event_loop(
    session: SessionId,
    registry: Registry,
    files: SessionFiles,
    mut master: File,
    child: &mut Child,
    runner_pid: u32,
    child_pid: u32,
    started_at: u64,
) -> io::Result<()> {
    let mut tail = OutputTail::new(OUTPUT_TAIL_BYTES);
    let mut pty_input = ByteQueue::new(MAX_ATTACHMENT_QUEUE_BYTES);
    let mut active: Option<Connection> = None;
    let mut candidate: Option<Connection> = None;
    let mut stop_requested = false;

    let process_exit = loop {
        let had_attachment = active.is_some();
        drain_pty(&mut master, &mut tail, &mut active)?;
        if had_attachment && active.is_none() {
            registry.write_metadata(
                &files.paths,
                &live_metadata(session, runner_pid, child_pid, started_at, false),
            )?;
        }
        if let Some(status) = child.try_wait()? {
            break process_exit(status);
        }

        let active_events = active.as_ref().map_or(PollFlags::empty(), |connection| {
            PollFlags::IN
                | if connection.output.is_empty() {
                    PollFlags::empty()
                } else {
                    PollFlags::OUT
                }
        });
        let mut descriptors = vec![
            PollFd::new(
                &master,
                PollFlags::IN
                    | if pty_input.is_empty() {
                        PollFlags::empty()
                    } else {
                        PollFlags::OUT
                    },
            ),
            PollFd::new(&files.listener, PollFlags::IN),
        ];
        let candidate_index = candidate.as_ref().map(|connection| {
            let index = descriptors.len();
            descriptors.push(PollFd::new(&connection.stream, PollFlags::IN));
            index
        });
        let active_index = active.as_ref().map(|connection| {
            let index = descriptors.len();
            descriptors.push(PollFd::new(&connection.stream, active_events));
            index
        });
        let poll_result = poll(&mut descriptors, Some(&POLL_INTERVAL));
        if let Err(error) = poll_result {
            if error == rustix::io::Errno::INTR {
                continue;
            }
            return Err(io::Error::from(error));
        }
        let events: Vec<PollFlags> = descriptors.iter().map(PollFd::revents).collect();
        drop(descriptors);

        if events[0].contains(PollFlags::OUT) && pty_input.flush(&mut master).is_err() {
            active = None;
        }
        if let Some(index) = active_index {
            let event = events[index];
            if event.contains(PollFlags::OUT)
                && active.as_mut().is_some_and(|connection| {
                    connection.output.flush(&mut connection.stream).is_err()
                })
            {
                active = None;
            }
            if active.is_some() && event.intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
            {
                match read_records(active.as_mut()) {
                    Ok(records) => {
                        if !apply_active_records(records, &mut pty_input, &master)? {
                            active = None;
                        }
                    }
                    Err(_) => active = None,
                }
            }
            if active.is_none() {
                registry.write_metadata(
                    &files.paths,
                    &live_metadata(session, runner_pid, child_pid, started_at, false),
                )?;
            }
        }
        if events[1].contains(PollFlags::IN) {
            accept_candidate(&files, &mut candidate)?;
        }
        if let Some(index) = candidate_index {
            if events[index].intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR) {
                match read_records(candidate.as_mut()) {
                    Ok(records) if !records.is_empty() => {
                        let mut records = records.into_iter();
                        match records.next() {
                            Some(Record::Attach { rows, columns }) => {
                                if let Some(mut connection) = candidate.take() {
                                    set_window_size(&master, rows, columns)?;
                                    enqueue_replay(&mut connection, &tail)?;
                                    let remaining: Vec<Record> = records.collect();
                                    if apply_active_records(remaining, &mut pty_input, &master)? {
                                        active = Some(connection);
                                        registry.write_metadata(
                                            &files.paths,
                                            &live_metadata(
                                                session, runner_pid, child_pid, started_at, true,
                                            ),
                                        )?;
                                    }
                                }
                            }
                            Some(Record::Stop) if records.next().is_none() => {
                                candidate = None;
                                stop_requested = true;
                            }
                            _ => candidate = None,
                        }
                    }
                    Ok(_) => {}
                    Err(_) => candidate = None,
                }
            }
        }
        if stop_requested {
            drop(master);
            let status = process_exit(stop_child(child)?);
            return complete_session(session, registry, files, tail, active, started_at, status);
        }
    };

    drain_pty(&mut master, &mut tail, &mut active)?;
    complete_session(
        session,
        registry,
        files,
        tail,
        active,
        started_at,
        process_exit,
    )
}

fn accept_candidate(files: &SessionFiles, candidate: &mut Option<Connection>) -> io::Result<()> {
    loop {
        match files.listener.accept() {
            Ok((stream, _)) => {
                let credentials = socket_peercred(&stream).map_err(io::Error::from)?;
                if credentials.uid != getuid() {
                    continue;
                }
                stream.set_nonblocking(true)?;
                *candidate = Some(Connection::new(stream));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn drain_pty(
    master: &mut File,
    tail: &mut OutputTail,
    active: &mut Option<Connection>,
) -> io::Result<()> {
    let mut processed = 0;
    let mut buffer = [0_u8; IO_CHUNK_BYTES];
    while processed < MAX_PTY_BYTES_PER_TICK {
        match master.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(length) => {
                let bytes = &buffer[..length];
                tail.extend(bytes);
                if let Some(connection) = active.as_mut() {
                    if enqueue_output(connection, bytes).is_err() {
                        *active = None;
                    }
                }
                processed += length;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.raw_os_error() == Some(5) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_records(connection: Option<&mut Connection>) -> io::Result<Vec<Record>> {
    let Some(connection) = connection else {
        return Ok(Vec::new());
    };
    let capacity = connection.decoder.remaining_capacity().min(IO_CHUNK_BYTES);
    if capacity == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC record",
        ));
    }
    let mut buffer = vec![0_u8; capacity];
    match connection.stream.read(&mut buffer) {
        Ok(0) => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "attachment closed",
            ));
        }
        Ok(length) => connection
            .decoder
            .push(&buffer[..length])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(Vec::new()),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut records = Vec::new();
    while let Some(record) = connection
        .decoder
        .next()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?
    {
        records.push(record);
    }
    Ok(records)
}

fn apply_active_records(
    records: Vec<Record>,
    pty_input: &mut ByteQueue,
    master: &File,
) -> io::Result<bool> {
    for record in records {
        match record {
            Record::Input(bytes) => {
                if !pty_input.push(&bytes) {
                    return Ok(false);
                }
            }
            Record::Resize { rows, columns } => set_window_size(master, rows, columns)?,
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn enqueue_replay(connection: &mut Connection, tail: &OutputTail) -> io::Result<()> {
    if tail.is_truncated() {
        enqueue_output(connection, TRUNCATION_MARKER)?;
    }
    enqueue_output(connection, &tail.snapshot())
}

fn enqueue_output(connection: &mut Connection, output: &[u8]) -> io::Result<()> {
    for chunk in output.chunks(MAX_IPC_PAYLOAD_BYTES) {
        let encoded = encode(&Record::Output(chunk.to_vec()))?;
        if !connection.output.push(&encoded) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "attachment queue full",
            ));
        }
    }
    Ok(())
}

fn stop_child(child: &mut Child) -> io::Result<std::process::ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    let raw_pid = i32::try_from(child.id()).map_err(|_| io::Error::other("invalid child PID"))?;
    let pid = Pid::from_raw(raw_pid).ok_or_else(|| io::Error::other("invalid child PID"))?;
    kill_process(pid, Signal::TERM).map_err(io::Error::from)?;
    let deadline = Instant::now() + Duration::from_secs(STOP_GRACE_SECONDS);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(50));
    }
    if child.try_wait()?.is_none() {
        kill_process(pid, Signal::KILL).map_err(io::Error::from)?;
    }
    child.wait()
}

fn complete_session(
    session: SessionId,
    registry: Registry,
    files: SessionFiles,
    tail: OutputTail,
    mut active: Option<Connection>,
    started_at: u64,
    process_exit: ProcessExit,
) -> io::Result<()> {
    let output = tail.snapshot();
    registry.write_output(&files.paths, &output)?;
    let metadata = SessionMetadata::Completed {
        session_id: session.to_string(),
        started_at,
        finished_at: now_seconds()?,
        exit: process_exit.into(),
        output_bytes: output.len(),
        truncated: tail.is_truncated(),
    };
    registry.write_metadata(&files.paths, &metadata)?;
    registry.remove_live_files(&files.paths);
    drop(files);

    if let Some(connection) = &mut active {
        let _ = connection.stream.set_nonblocking(false);
        let _ = connection
            .stream
            .set_write_timeout(Some(Duration::from_secs(1)));
        if connection.output.flush(&mut connection.stream).is_ok() {
            if let Ok(encoded) = encode(&Record::Exit(process_exit)) {
                if connection.output.push(&encoded) {
                    let _ = connection.output.flush(&mut connection.stream);
                }
            }
        }
    }
    Ok(())
}

struct Connection {
    stream: UnixStream,
    decoder: Decoder,
    output: ByteQueue,
}

impl Connection {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            decoder: Decoder::default(),
            output: ByteQueue::new(MAX_ATTACHMENT_QUEUE_BYTES),
        }
    }
}

fn live_metadata(
    session: SessionId,
    runner_pid: u32,
    child_pid: u32,
    started_at: u64,
    attached: bool,
) -> SessionMetadata {
    SessionMetadata::Live {
        session_id: session.to_string(),
        runner_pid,
        child_pid,
        started_at,
        attached,
    }
}

fn default_shell() -> OsString {
    std::env::var_os("SHELL")
        .filter(|shell| {
            let path = std::path::Path::new(shell);
            path.is_absolute()
                && path.metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
        })
        .unwrap_or_else(|| OsString::from("/bin/sh"))
}

fn process_exit(status: std::process::ExitStatus) -> ProcessExit {
    if let Some(code) = status.code().and_then(|code| u8::try_from(code).ok()) {
        ProcessExit::Code(code)
    } else if let Some(signal) = status.signal().and_then(|signal| u8::try_from(signal).ok()) {
        ProcessExit::Signal(signal)
    } else {
        ProcessExit::Code(1)
    }
}

fn write_startup_config(
    stream: &mut UnixStream,
    command: &[OsString],
    rows: u16,
    columns: u16,
) -> io::Result<()> {
    let count = u16::try_from(command.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many command arguments"))?;
    stream.write_all(&rows.to_be_bytes())?;
    stream.write_all(&columns.to_be_bytes())?;
    stream.write_all(&count.to_be_bytes())?;
    for argument in command {
        let bytes = argument.as_os_str().as_bytes();
        let length = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "command argument too large")
        })?;
        stream.write_all(&length.to_be_bytes())?;
        stream.write_all(bytes)?;
    }
    stream.flush()
}

fn read_startup_config() -> io::Result<StartupConfig> {
    let mut stdin = std::io::stdin().lock();
    let rows = read_u16(&mut stdin)?;
    let columns = read_u16(&mut stdin)?;
    if rows == 0 || columns == 0 || rows > 4096 || columns > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid terminal dimensions",
        ));
    }
    let count = usize::from(read_u16(&mut stdin)?);
    if count > MAX_COMMAND_ARGUMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many command arguments",
        ));
    }
    let mut aggregate = 0_usize;
    let mut command = Vec::with_capacity(count);
    for _ in 0..count {
        let length = usize::try_from(read_u32(&mut stdin)?).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "command argument too large")
        })?;
        aggregate = aggregate
            .checked_add(length)
            .filter(|total| *total <= MAX_COMMAND_BYTES)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "command arguments too large")
            })?;
        let mut bytes = vec![0_u8; length];
        stdin.read_exact(&mut bytes)?;
        command.push(OsString::from_vec(bytes));
    }
    Ok(StartupConfig {
        command,
        rows,
        columns,
    })
}

fn read_u16(input: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0_u8; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn write_startup_response(response: u8) -> io::Result<()> {
    let stdin = std::io::stdin();
    let mut remaining = &[response][..];
    while !remaining.is_empty() {
        match rustix::io::write(stdin.as_fd(), remaining) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "startup response failed",
                ));
            }
            Ok(written) => remaining = &remaining[written..],
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Ok(())
}

use std::os::unix::fs::PermissionsExt;
