use crate::registry::now_seconds;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const MAX_TRACE_BYTES: usize = 1024 * 1024;

/// Bounded, owner-only lifecycle diagnostics. Events must never contain terminal
/// bytes, command arguments, environment values, or credentials.
pub(crate) struct Trace {
    file: File,
    written: usize,
}

impl Trace {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        let mut trace = Self { file, written: 0 };
        trace.event("trace_started");
        Ok(trace)
    }

    pub(crate) fn event(&mut self, event: &'static str) {
        if self.written >= MAX_TRACE_BYTES {
            return;
        }
        let timestamp = now_seconds().unwrap_or(0);
        let line = format!("timestamp={timestamp} event={event}\n");
        if self.written.saturating_add(line.len()) > MAX_TRACE_BYTES {
            return;
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += line.len();
            let _ = self.file.flush();
        }
    }
}
