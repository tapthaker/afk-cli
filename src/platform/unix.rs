use crate::limits::MAX_TERMINAL_DIMENSION;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::{
    OptionalActions, Termios, Winsize, tcgetattr, tcgetwinsize, tcsetattr, tcsetwinsize,
};
use std::io;
use std::os::unix::net::UnixStream;

pub(crate) fn create_pty(rows: u16, columns: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    validate_dimensions(rows, columns)?;
    let pty = rustix_openpty::openpty(
        None,
        Some(&Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
    )
    .map_err(io::Error::from)?;
    Ok((pty.controller, pty.user))
}

pub(crate) fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    unix_cred::get_peer_ids(stream).map(|(uid, _)| uid)
}

pub(crate) fn become_session_leader() -> io::Result<()> {
    setsid().map(|_| ()).map_err(io::Error::from)
}

pub(crate) fn acquire_controlling_terminal(fd: impl AsFd) -> io::Result<()> {
    ioctl_tiocsctty(fd).map_err(io::Error::from)
}

pub(crate) fn set_nonblocking(fd: impl AsFd) -> io::Result<OFlags> {
    let previous = fcntl_getfl(&fd).map_err(io::Error::from)?;
    fcntl_setfl(fd, previous | OFlags::NONBLOCK).map_err(io::Error::from)?;
    Ok(previous)
}

pub(crate) fn window_size(fd: impl AsFd) -> io::Result<(u16, u16)> {
    let size = tcgetwinsize(fd).map_err(io::Error::from)?;
    validate_dimensions(size.ws_row, size.ws_col)?;
    Ok((size.ws_row, size.ws_col))
}

pub(crate) fn set_window_size(fd: impl AsFd, rows: u16, columns: u16) -> io::Result<()> {
    validate_dimensions(rows, columns)?;
    tcsetwinsize(
        fd,
        Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .map_err(io::Error::from)
}

pub(crate) struct RawTerminal {
    fd: OwnedFd,
    original: Option<Termios>,
}

impl RawTerminal {
    pub(crate) fn enter(fd: impl AsFd) -> io::Result<Self> {
        let owned = rustix::io::dup(&fd).map_err(io::Error::from)?;
        let original = match tcgetattr(&fd) {
            Ok(original) => {
                let mut raw = original.clone();
                raw.make_raw();
                tcsetattr(&fd, OptionalActions::Now, &raw).map_err(io::Error::from)?;
                Some(original)
            }
            Err(rustix::io::Errno::NOTTY | rustix::io::Errno::NODEV) => None,
            Err(error) => return Err(io::Error::from(error)),
        };
        Ok(Self {
            fd: owned,
            original,
        })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(original) = &self.original {
            let _ = tcsetattr(&self.fd, OptionalActions::Now, original);
        }
    }
}

fn validate_dimensions(rows: u16, columns: u16) -> io::Result<()> {
    if (1..=MAX_TERMINAL_DIMENSION).contains(&rows)
        && (1..=MAX_TERMINAL_DIMENSION).contains(&columns)
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "terminal dimensions are outside AFK limits",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{RawTerminal, peer_uid};
    use rustix::fs::{OFlags, fcntl_getfl};
    use rustix::process::getuid;
    use rustix::termios::Winsize;
    use std::os::unix::net::UnixStream;

    #[test]
    fn reads_peer_uid_from_unix_stream() -> std::io::Result<()> {
        let (stream, _peer) = UnixStream::pair()?;

        assert_eq!(peer_uid(&stream)?, getuid().as_raw());
        Ok(())
    }

    #[test]
    fn raw_terminal_keeps_shared_pty_output_blocking() -> std::io::Result<()> {
        let pty = rustix_openpty::openpty(
            None,
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )?;
        let shared_output = rustix::io::dup(&pty.user)?;

        let _terminal = RawTerminal::enter(&pty.user)?;

        assert!(!fcntl_getfl(&shared_output)?.contains(OFlags::NONBLOCK));
        Ok(())
    }
}
