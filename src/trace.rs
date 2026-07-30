use crate::registry::now_seconds;
use rustix::process::getpid;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(crate) const MAX_TRACE_BYTES: u64 = 1024 * 1024;

/// Bounded, owner-only lifecycle diagnostics. Events must never contain terminal
/// bytes, command arguments, environment values, credentials, or error text.
pub(crate) struct Trace {
    file: File,
    component: &'static str,
    written: u64,
}

impl Trace {
    pub(crate) fn create(path: &Path, component: &'static str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        let mut trace = Self {
            file,
            component,
            written: 0,
        };
        trace.event("trace_started");
        Ok(trace)
    }

    pub(crate) fn open(path: &Path, component: &'static str) -> io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.len() > MAX_TRACE_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe trace file",
            ));
        }
        let file = OpenOptions::new().append(true).open(path)?;
        let mut trace = Self {
            file,
            component,
            written: metadata.len(),
        };
        trace.event("trace_opened");
        Ok(trace)
    }

    pub(crate) fn event(&mut self, event: &'static str) {
        self.write(event, None);
    }

    pub(crate) fn io_error(&mut self, event: &'static str, error: &io::Error) {
        let detail = match error.raw_os_error() {
            Some(code) => format!(" error_kind={} os_error={code}", error_kind(error.kind())),
            None => format!(" error_kind={}", error_kind(error.kind())),
        };
        self.write(event, Some(&detail));
    }

    pub(crate) fn metric(&mut self, event: &'static str, name: &'static str, value: u64) {
        self.write(event, Some(&format!(" {name}={value}")));
    }

    fn write(&mut self, event: &'static str, detail: Option<&str>) {
        if self.written >= MAX_TRACE_BYTES {
            return;
        }
        let timestamp = now_seconds().unwrap_or(0);
        let pid = getpid().as_raw_nonzero().get();
        let detail = detail.unwrap_or("");
        let line = format!(
            "timestamp={timestamp} pid={pid} component={} event={event}{detail}\n",
            self.component
        );
        let Ok(length) = u64::try_from(line.len()) else {
            return;
        };
        if self.written.saturating_add(length) > MAX_TRACE_BYTES {
            return;
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += length;
            let _ = self.file.flush();
        }
    }
}

fn error_kind(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::ConnectionRefused => "connection_refused",
        io::ErrorKind::ConnectionReset => "connection_reset",
        io::ErrorKind::ConnectionAborted => "connection_aborted",
        io::ErrorKind::NotConnected => "not_connected",
        io::ErrorKind::BrokenPipe => "broken_pipe",
        io::ErrorKind::AlreadyExists => "already_exists",
        io::ErrorKind::WouldBlock => "would_block",
        io::ErrorKind::InvalidInput => "invalid_input",
        io::ErrorKind::InvalidData => "invalid_data",
        io::ErrorKind::TimedOut => "timed_out",
        io::ErrorKind::WriteZero => "write_zero",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::UnexpectedEof => "unexpected_eof",
        io::ErrorKind::OutOfMemory => "out_of_memory",
        _ => "other",
    }
}
