use crate::byte_queue::ByteQueue;
use crate::identity::SessionId;
use crate::ipc::{Decoder, ProcessExit, Record, encode};
use crate::limits::{MAX_ATTACHMENT_QUEUE_BYTES, MAX_IPC_PAYLOAD_BYTES};
use crate::output_tail::TRUNCATION_MARKER;
use crate::platform::unix::{RawTerminal, window_size};
use crate::registry::{Registry, SessionMetadata};
use crate::trace::Trace;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::AsFd;
use signal_hook::consts::signal::{SIGHUP, SIGTERM, SIGWINCH};
use signal_hook::flag;
use signal_hook::low_level::unregister;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;

const IO_CHUNK_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

pub(crate) fn attach(
    session: SessionId,
    trace_enabled: bool,
    output: &mut impl Write,
) -> io::Result<ProcessExit> {
    let registry = Registry::open()?;
    let mut trace = if trace_enabled {
        let path = registry.paths(session)?.trace;
        match Trace::open(&path, "attachment") {
            Ok(trace) => Some(trace),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    trace_event(&mut trace, "attachment_process_started");
    match registry.connect(session) {
        Ok(stream) => {
            trace_event(&mut trace, "runner_socket_connected");
            attach_live(stream, output, trace)
        }
        Err(connect_error) => {
            trace_io_error(&mut trace, "runner_socket_connect_failed", &connect_error);
            match registry.read_metadata(session)? {
                Some(SessionMetadata::Completed {
                    exit, truncated, ..
                }) => {
                    if truncated {
                        output.write_all(TRUNCATION_MARKER)?;
                    }
                    if let Some(bytes) = registry.read_output(session)? {
                        output.write_all(&bytes)?;
                    }
                    let process_exit = ProcessExit::from(exit);
                    write_completion(output, process_exit)?;
                    trace_process_exit(&mut trace, "completed_session_replayed", process_exit);
                    Ok(process_exit)
                }
                _ => Err(connect_error),
            }
        }
    }
}

pub(crate) fn stop(session: SessionId) -> io::Result<()> {
    let registry = Registry::open()?;
    let mut stream = registry.connect(session)?;
    let encoded = encode(&Record::Stop)?;
    stream.write_all(&encoded)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(7)))?;
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn attach_live(
    mut stream: UnixStream,
    output: &mut impl Write,
    mut trace: Option<Trace>,
) -> io::Result<ProcessExit> {
    stream.set_nonblocking(true)?;
    let stdin = std::io::stdin();
    let dimensions = window_size(stdin.as_fd()).unwrap_or((24, 80));
    let terminal = RawTerminal::enter(stdin.as_fd()).inspect_err(|error| {
        trace_io_error(&mut trace, "raw_terminal_entry_failed", error);
    })?;
    trace_event(&mut trace, "raw_terminal_entered");
    let signals = SignalFlags::register().inspect_err(|error| {
        trace_io_error(&mut trace, "signal_registration_failed", error);
    })?;
    let mut socket_output = ByteQueue::new(MAX_ATTACHMENT_QUEUE_BYTES);
    queue_record(
        &mut socket_output,
        &Record::Attach {
            rows: dimensions.0,
            columns: dimensions.1,
        },
    )?;
    let mut decoder = Decoder::default();
    let input_events = spawn_input_reader();
    let mut stdin_closed = false;

    loop {
        if signals.hangup.load(Ordering::Relaxed) {
            trace_event(&mut trace, "attachment_received_sighup");
            drop(terminal);
            return Ok(ProcessExit::Code(0));
        }
        if signals.terminate.load(Ordering::Relaxed) {
            trace_event(&mut trace, "attachment_received_sigterm");
            drop(terminal);
            return Ok(ProcessExit::Code(0));
        }
        if signals.resize.swap(false, Ordering::Relaxed) {
            if let Ok((rows, columns)) = window_size(stdin.as_fd()) {
                queue_record(&mut socket_output, &Record::Resize { rows, columns })?;
                trace_event(&mut trace, "resize_queued");
            } else {
                trace_event(&mut trace, "resize_dimensions_unavailable");
            }
        }
        if !stdin_closed {
            match receive_input(&input_events, &mut socket_output) {
                Ok(closed) => {
                    if closed {
                        trace_event(&mut trace, "stdin_closed");
                    }
                    stdin_closed = closed;
                }
                Err(error) => {
                    trace_io_error(&mut trace, "stdin_read_failed", &error);
                    return Err(error);
                }
            }
        }

        let socket_event = {
            let mut descriptors = [PollFd::new(
                &stream,
                PollFlags::IN
                    | if socket_output.is_empty() {
                        PollFlags::empty()
                    } else {
                        PollFlags::OUT
                    },
            )];
            if let Err(error) = poll(&mut descriptors, Some(&POLL_INTERVAL)) {
                if error == rustix::io::Errno::INTR {
                    continue;
                }
                let error = io::Error::from(error);
                trace_io_error(&mut trace, "attachment_poll_failed", &error);
                return Err(error);
            }
            descriptors[0].revents()
        };

        if socket_event.contains(PollFlags::OUT) {
            if let Err(error) = socket_output.flush(&mut stream) {
                if attachment_closed(&error) {
                    trace_io_error(&mut trace, "runner_socket_closed_during_write", &error);
                    drop(terminal);
                    return Ok(ProcessExit::Code(0));
                }
                trace_io_error(&mut trace, "runner_socket_write_failed", &error);
                return Err(error);
            }
        }
        if stdin_closed && socket_output.is_empty() {
            trace_event(&mut trace, "attachment_ended_after_stdin_close");
            drop(terminal);
            return Ok(ProcessExit::Code(0));
        }
        if socket_event.intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR) {
            if socket_event.contains(PollFlags::HUP) {
                trace_event(&mut trace, "runner_socket_poll_hangup");
            }
            if socket_event.contains(PollFlags::ERR) {
                trace_event(&mut trace, "runner_socket_poll_error");
            }
            let records = match read_records(&mut stream, &mut decoder) {
                Ok(Some(records)) => records,
                Ok(None) => {
                    trace_event(&mut trace, "runner_socket_eof");
                    drop(terminal);
                    return Ok(ProcessExit::Code(0));
                }
                Err(error) => {
                    trace_io_error(&mut trace, "runner_socket_read_failed", &error);
                    return Err(error);
                }
            };
            let mut wrote_output = false;
            for record in records {
                match record {
                    Record::Output(bytes) => {
                        output.write_all(&bytes)?;
                        wrote_output = true;
                    }
                    Record::Exit(status) => {
                        trace_process_exit(&mut trace, "runner_reported_exit", status);
                        output.flush()?;
                        drop(terminal);
                        return Ok(status);
                    }
                    _ => {
                        trace_event(&mut trace, "invalid_runner_response");
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid runner response",
                        ));
                    }
                }
            }
            if wrote_output {
                // `StdoutLock` is line-buffered on a terminal. Prompts and partial output
                // commonly contain no newline, so flush every received output batch instead
                // of waiting for later input or process exit to make those bytes visible.
                output.flush()?;
            }
        }
    }
}

enum InputEvent {
    Bytes(Vec<u8>),
    Closed,
    Failed(io::Error),
}

fn spawn_input_reader() -> Receiver<InputEvent> {
    let capacity = MAX_ATTACHMENT_QUEUE_BYTES / IO_CHUNK_BYTES;
    let (sender, receiver) = sync_channel(capacity);
    thread::spawn(move || read_input(sender));
    receiver
}

fn read_input(sender: SyncSender<InputEvent>) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut bytes = [0_u8; IO_CHUNK_BYTES];
    loop {
        let event = match input.read(&mut bytes) {
            Ok(0) => InputEvent::Closed,
            Ok(length) => InputEvent::Bytes(bytes[..length].to_vec()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => InputEvent::Failed(error),
        };
        let finished = !matches!(event, InputEvent::Bytes(_));
        if sender.send(event).is_err() || finished {
            return;
        }
    }
}

fn receive_input(
    receiver: &Receiver<InputEvent>,
    socket_output: &mut ByteQueue,
) -> io::Result<bool> {
    match receiver.try_recv() {
        Ok(InputEvent::Bytes(bytes)) => {
            queue_record(socket_output, &Record::Input(bytes))?;
            Ok(false)
        }
        Ok(InputEvent::Closed) | Err(TryRecvError::Disconnected) => Ok(true),
        Ok(InputEvent::Failed(error)) => Err(error),
        Err(TryRecvError::Empty) => Ok(false),
    }
}

fn read_records(stream: &mut UnixStream, decoder: &mut Decoder) -> io::Result<Option<Vec<Record>>> {
    let capacity = decoder.remaining_capacity().min(IO_CHUNK_BYTES);
    if capacity == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC record",
        ));
    }
    let mut buffer = vec![0_u8; capacity];
    match stream.read(&mut buffer) {
        Ok(0) => return Ok(None),
        Ok(length) => decoder
            .push(&buffer[..length])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(Some(Vec::new())),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(Some(Vec::new())),
        Err(error) if attachment_closed(&error) => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut records = Vec::new();
    while let Some(record) = decoder
        .next()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?
    {
        records.push(record);
    }
    Ok(Some(records))
}

fn attachment_closed(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof
    )
}

fn queue_record(queue: &mut ByteQueue, record: &Record) -> io::Result<()> {
    match record {
        Record::Input(bytes) | Record::Output(bytes) if bytes.len() > MAX_IPC_PAYLOAD_BYTES => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPC payload too large",
            ));
        }
        _ => {}
    }
    let encoded = encode(record)?;
    if queue.push(&encoded) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "attachment queue full",
        ))
    }
}

fn write_completion(output: &mut impl Write, status: ProcessExit) -> io::Result<()> {
    match status {
        ProcessExit::Code(code) => writeln!(output, "\r\n[afk: process exited with code {code}]")?,
        ProcessExit::Signal(signal) => {
            writeln!(output, "\r\n[afk: process exited with signal {signal}]")?
        }
    }
    output.flush()
}

struct SignalFlags {
    resize: Arc<AtomicBool>,
    hangup: Arc<AtomicBool>,
    terminate: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl SignalFlags {
    fn register() -> io::Result<Self> {
        let resize = Arc::new(AtomicBool::new(false));
        let hangup = Arc::new(AtomicBool::new(false));
        let terminate = Arc::new(AtomicBool::new(false));
        let registrations = vec![
            flag::register(SIGWINCH, Arc::clone(&resize))?,
            flag::register(SIGHUP, Arc::clone(&hangup))?,
            flag::register(SIGTERM, Arc::clone(&terminate))?,
        ];
        Ok(Self {
            resize,
            hangup,
            terminate,
            registrations,
        })
    }
}

fn trace_event(trace: &mut Option<Trace>, event: &'static str) {
    if let Some(trace) = trace {
        trace.event(event);
    }
}

fn trace_io_error(trace: &mut Option<Trace>, event: &'static str, error: &io::Error) {
    if let Some(trace) = trace {
        trace.io_error(event, error);
    }
}

fn trace_process_exit(trace: &mut Option<Trace>, event: &'static str, exit: ProcessExit) {
    if let Some(trace) = trace {
        match exit {
            ProcessExit::Code(code) => trace.metric(event, "exit_code", u64::from(code)),
            ProcessExit::Signal(signal) => trace.metric(event, "signal", u64::from(signal)),
        }
    }
}

impl Drop for SignalFlags {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            unregister(registration);
        }
    }
}
