use crate::limits::MAX_TERMINAL_DIMENSION;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::pty::{OpenptFlags, grantpt, ioctl_tiocgptpeer, openpt, unlockpt};
use rustix::termios::{
    OptionalActions, Termios, Winsize, tcgetattr, tcgetwinsize, tcsetattr, tcsetwinsize,
};
use std::io;

pub(crate) fn create_pty(rows: u16, columns: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    validate_dimensions(rows, columns)?;
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .map_err(io::Error::from)?;
    grantpt(&master).map_err(io::Error::from)?;
    unlockpt(&master).map_err(io::Error::from)?;
    let slave = ioctl_tiocgptpeer(
        &master,
        OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC,
    )
    .map_err(io::Error::from)?;
    set_window_size(&slave, rows, columns)?;
    Ok((master, slave))
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

pub(crate) fn restore_flags(fd: impl AsFd, flags: OFlags) -> io::Result<()> {
    fcntl_setfl(fd, flags).map_err(io::Error::from)
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
    original: Termios,
    flags: OFlags,
}

impl RawTerminal {
    pub(crate) fn enter(fd: impl AsFd) -> io::Result<Self> {
        let owned = rustix::io::dup(&fd).map_err(io::Error::from)?;
        let original = tcgetattr(&fd).map_err(io::Error::from)?;
        let mut raw = original.clone();
        raw.make_raw();
        tcsetattr(&fd, OptionalActions::Now, &raw).map_err(io::Error::from)?;
        let flags = match set_nonblocking(&fd) {
            Ok(flags) => flags,
            Err(error) => {
                let _ = tcsetattr(fd, OptionalActions::Now, &original);
                return Err(error);
            }
        };
        Ok(Self {
            fd: owned,
            original,
            flags,
        })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = restore_flags(&self.fd, self.flags);
        let _ = tcsetattr(&self.fd, OptionalActions::Now, &self.original);
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
