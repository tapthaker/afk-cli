use crate::byte_queue::ByteQueue;
use crate::identity::SessionId;
use crate::ipc::{Decoder, ProcessExit, Record, encode};
use crate::limits::{MAX_ATTACHMENT_QUEUE_BYTES, MAX_IPC_PAYLOAD_BYTES};
use crate::output_tail::TRUNCATION_MARKER;
use crate::platform::unix::{RawTerminal, window_size};
use crate::registry::{Registry, SessionMetadata};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::AsFd;
use signal_hook::consts::signal::{SIGHUP, SIGTERM, SIGWINCH};
use signal_hook::flag;
use signal_hook::low_level::unregister;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const IO_CHUNK_BYTES: usize = 16 * 1024;
const POLL_INTERVAL: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

pub(crate) fn attach(session: SessionId, output: &mut impl Write) -> io::Result<ProcessExit> {
    let registry = Registry::open()?;
    match registry.connect(session) {
        Ok(stream) => attach_live(stream, output),
        Err(connect_error) => match registry.read_metadata(session)? {
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
                Ok(process_exit)
            }
            _ => Err(connect_error),
        },
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

fn attach_live(mut stream: UnixStream, output: &mut impl Write) -> io::Result<ProcessExit> {
    stream.set_nonblocking(true)?;
    let stdin = std::io::stdin();
    let dimensions = window_size(stdin.as_fd()).unwrap_or((24, 80));
    let terminal = RawTerminal::enter(stdin.as_fd())?;
    let signals = SignalFlags::register()?;
    let mut socket_output = ByteQueue::new(MAX_ATTACHMENT_QUEUE_BYTES);
    queue_record(
        &mut socket_output,
        &Record::Attach {
            rows: dimensions.0,
            columns: dimensions.1,
        },
    )?;
    let mut decoder = Decoder::default();
    let mut stdin_closed = false;
    let mut input = stdin.lock();

    loop {
        if signals.terminate.load(Ordering::Relaxed) {
            drop(terminal);
            return Ok(ProcessExit::Code(0));
        }
        if signals.resize.swap(false, Ordering::Relaxed) {
            if let Ok((rows, columns)) = window_size(input.as_fd()) {
                queue_record(&mut socket_output, &Record::Resize { rows, columns })?;
            }
        }

        if !stdin_closed {
            stdin_closed = read_input(&mut input, &mut socket_output)?;
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
                return Err(io::Error::from(error));
            }
            descriptors[0].revents()
        };

        if socket_event.contains(PollFlags::OUT) {
            socket_output.flush(&mut stream)?;
        }
        if socket_event.intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR) {
            let records = read_records(&mut stream, &mut decoder)?;
            for record in records {
                match record {
                    Record::Output(bytes) => output.write_all(&bytes)?,
                    Record::Exit(status) => {
                        output.flush()?;
                        drop(terminal);
                        return Ok(status);
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid runner response",
                        ));
                    }
                }
            }
        }
        if stdin_closed && socket_output.is_empty() {
            drop(terminal);
            return Ok(ProcessExit::Code(0));
        }
    }
}

fn read_input(input: &mut impl Read, socket_output: &mut ByteQueue) -> io::Result<bool> {
    let mut bytes = [0_u8; IO_CHUNK_BYTES];
    match input.read(&mut bytes) {
        Ok(0) => Ok(true),
        Ok(length) => {
            queue_record(socket_output, &Record::Input(bytes[..length].to_vec()))?;
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_records(stream: &mut UnixStream, decoder: &mut Decoder) -> io::Result<Vec<Record>> {
    let capacity = decoder.remaining_capacity().min(IO_CHUNK_BYTES);
    if capacity == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC record",
        ));
    }
    let mut buffer = vec![0_u8; capacity];
    match stream.read(&mut buffer) {
        Ok(0) => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "runner closed",
            ));
        }
        Ok(length) => decoder
            .push(&buffer[..length])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(Vec::new()),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut records = Vec::new();
    while let Some(record) = decoder
        .next()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC record"))?
    {
        records.push(record);
    }
    Ok(records)
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
    terminate: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl SignalFlags {
    fn register() -> io::Result<Self> {
        let resize = Arc::new(AtomicBool::new(false));
        let terminate = Arc::new(AtomicBool::new(false));
        let registrations = vec![
            flag::register(SIGWINCH, Arc::clone(&resize))?,
            flag::register(SIGHUP, Arc::clone(&terminate))?,
            flag::register(SIGTERM, Arc::clone(&terminate))?,
        ];
        Ok(Self {
            resize,
            terminate,
            registrations,
        })
    }
}

impl Drop for SignalFlags {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            unregister(registration);
        }
    }
}
