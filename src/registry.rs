use crate::identity::SessionId;
use crate::ipc::ProcessExit;
use crate::limits::{
    COMPLETED_RETENTION_SECONDS, LINUX_UNIX_SOCKET_PATH_BYTES, MAX_METADATA_BYTES,
    MAX_SESSION_LIST_ENTRIES, OUTPUT_TAIL_BYTES,
};
use rustix::net::sockopt::socket_peercred;
use rustix::process::getuid;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::linux::fs::MetadataExt as LinuxMetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub(crate) enum SessionMetadata {
    Live {
        session_id: String,
        runner_pid: u32,
        child_pid: u32,
        started_at: u64,
        attached: bool,
    },
    Completed {
        session_id: String,
        started_at: u64,
        finished_at: u64,
        exit: ExitMetadata,
        output_bytes: usize,
        truncated: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(tag = "reason", content = "value", rename_all = "lowercase")]
pub(crate) enum ExitMetadata {
    Code(u8),
    Signal(u8),
}

impl From<ProcessExit> for ExitMetadata {
    fn from(value: ProcessExit) -> Self {
        match value {
            ProcessExit::Code(code) => Self::Code(code),
            ProcessExit::Signal(signal) => Self::Signal(signal),
        }
    }
}

impl From<ExitMetadata> for ProcessExit {
    fn from(value: ExitMetadata) -> Self {
        match value {
            ExitMetadata::Code(code) => Self::Code(code),
            ExitMetadata::Signal(signal) => Self::Signal(signal),
        }
    }
}

impl SessionMetadata {
    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Live { session_id, .. } | Self::Completed { session_id, .. } => session_id,
        }
    }

    pub(crate) fn is_expired(&self, now: u64) -> bool {
        match self {
            Self::Completed { finished_at, .. } => {
                now.saturating_sub(*finished_at) >= COMPLETED_RETENTION_SECONDS
            }
            Self::Live { .. } => false,
        }
    }
}

pub(crate) struct Registry {
    root: PathBuf,
    uid: u32,
}

pub(crate) struct SessionFiles {
    pub(crate) listener: UnixListener,
    pub(crate) _lock: File,
    pub(crate) paths: SessionPaths,
}

impl Drop for SessionFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.paths.socket);
        let _ = fs::remove_file(&self.paths.lock);
    }
}

#[derive(Clone)]
pub(crate) struct SessionPaths {
    pub(crate) socket: PathBuf,
    pub(crate) metadata: PathBuf,
    pub(crate) lock: PathBuf,
    pub(crate) output: PathBuf,
}

impl Registry {
    pub(crate) fn open() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| invalid("HOME must name an absolute directory"))?;
        let afk = home.join(".afk");
        ensure_private_directory(&afk)?;
        let root = afk.join("run");
        ensure_private_directory(&root)?;
        let registry = Self {
            root,
            uid: getuid().as_raw(),
        };
        registry.cleanup_expired()?;
        Ok(registry)
    }

    pub(crate) fn paths(&self, session: SessionId) -> io::Result<SessionPaths> {
        let prefix = session.to_string();
        let paths = SessionPaths {
            socket: self.root.join(format!("{prefix}.sock")),
            metadata: self.root.join(format!("{prefix}.json")),
            lock: self.root.join(format!("{prefix}.lock")),
            output: self.root.join(format!("{prefix}.out")),
        };
        if paths.socket.as_os_str().as_bytes().len() > LINUX_UNIX_SOCKET_PATH_BYTES {
            return Err(invalid("AFK runtime socket path is too long"));
        }
        Ok(paths)
    }

    pub(crate) fn bind_session(&self, session: SessionId) -> io::Result<SessionFiles> {
        let paths = self.paths(session)?;
        if let Some(metadata) = self.read_metadata(session)? {
            if metadata.is_expired(now_seconds()?) {
                self.remove_completed(&paths)?;
            } else {
                match metadata {
                    SessionMetadata::Completed { .. } => {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "session exists",
                        ));
                    }
                    SessionMetadata::Live { .. } => {
                        if self.connect(session).is_ok() {
                            return Err(io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "session exists",
                            ));
                        }
                        self.remove_stale_live(&paths)?;
                    }
                }
            }
        } else if path_entry_exists(&paths.socket)? || path_entry_exists(&paths.lock)? {
            if self.connect(session).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "session exists",
                ));
            }
            self.remove_stale_live(&paths)?;
        }

        match UnixListener::bind(&paths.socket) {
            Ok(listener) => {
                fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
                listener.set_nonblocking(true)?;
                let lock = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&paths.lock)
                    .inspect_err(|_| {
                        let _ = fs::remove_file(&paths.socket);
                    })?;
                Ok(SessionFiles {
                    listener,
                    _lock: lock,
                    paths,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "session exists",
            )),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn connect(&self, session: SessionId) -> io::Result<UnixStream> {
        let paths = self.paths(session)?;
        verify_path(&paths.socket, self.uid, PathKind::Socket, 0o600)?;
        let stream = UnixStream::connect(&paths.socket)?;
        let credentials = socket_peercred(&stream).map_err(io::Error::from)?;
        if credentials.uid.as_raw() != self.uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "session peer owner mismatch",
            ));
        }
        Ok(stream)
    }

    pub(crate) fn read_metadata(&self, session: SessionId) -> io::Result<Option<SessionMetadata>> {
        let paths = self.paths(session)?;
        let Some(bytes) = read_bounded_file(
            &paths.metadata,
            self.uid,
            PathKind::Regular,
            0o600,
            MAX_METADATA_BYTES,
        )?
        else {
            return Ok(None);
        };
        let metadata: SessionMetadata =
            serde_json::from_slice(&bytes).map_err(|_| invalid("invalid session metadata"))?;
        if metadata.session_id() != session.to_string() {
            return Err(invalid("session metadata identity mismatch"));
        }
        Ok(Some(metadata))
    }

    pub(crate) fn read_output(&self, session: SessionId) -> io::Result<Option<Vec<u8>>> {
        let paths = self.paths(session)?;
        read_bounded_file(
            &paths.output,
            self.uid,
            PathKind::Regular,
            0o600,
            OUTPUT_TAIL_BYTES,
        )
    }

    pub(crate) fn write_metadata(
        &self,
        paths: &SessionPaths,
        metadata: &SessionMetadata,
    ) -> io::Result<()> {
        let encoded =
            serde_json::to_vec(metadata).map_err(|_| invalid("metadata encoding failed"))?;
        if encoded.len() > MAX_METADATA_BYTES {
            return Err(invalid("session metadata is too large"));
        }
        atomic_write(&paths.metadata, &encoded)
    }

    pub(crate) fn write_output(&self, paths: &SessionPaths, output: &[u8]) -> io::Result<()> {
        if output.len() > OUTPUT_TAIL_BYTES {
            return Err(invalid("completed output is too large"));
        }
        atomic_write(&paths.output, output)
    }

    pub(crate) fn remove_live_files(&self, paths: &SessionPaths) {
        let _ = fs::remove_file(&paths.socket);
        let _ = fs::remove_file(&paths.lock);
    }

    pub(crate) fn list(&self) -> io::Result<Vec<SessionMetadata>> {
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            if sessions.len() == MAX_SESSION_LIST_ENTRIES {
                break;
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(session) = session_from_metadata_name(&name) else {
                continue;
            };
            if let Some(metadata) = self.read_metadata(session)? {
                sessions.push(metadata);
            }
        }
        sessions.sort_by_key(|metadata| match metadata {
            SessionMetadata::Live { started_at, .. }
            | SessionMetadata::Completed { started_at, .. } => *started_at,
        });
        Ok(sessions)
    }

    fn cleanup_expired(&self) -> io::Result<()> {
        let now = now_seconds()?;
        for entry in fs::read_dir(&self.root)?.take(MAX_SESSION_LIST_ENTRIES) {
            let entry = entry?;
            let name = entry.file_name();
            let Some(session) = session_from_metadata_name(&name) else {
                continue;
            };
            let paths = self.paths(session)?;
            if self
                .read_metadata(session)?
                .is_some_and(|metadata| metadata.is_expired(now))
            {
                self.remove_completed(&paths)?;
            }
        }
        Ok(())
    }

    fn remove_completed(&self, paths: &SessionPaths) -> io::Result<()> {
        remove_if_present(&paths.output)?;
        remove_if_present(&paths.metadata)
    }

    fn remove_stale_live(&self, paths: &SessionPaths) -> io::Result<()> {
        remove_owned_entry_if_present(&paths.socket, self.uid, PathKind::Socket, 0o600)?;
        remove_owned_entry_if_present(&paths.lock, self.uid, PathKind::Regular, 0o600)?;
        remove_owned_entry_if_present(&paths.metadata, self.uid, PathKind::Regular, 0o600)
    }
}

pub(crate) fn now_seconds() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| invalid("system clock is before the Unix epoch"))
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(invalid("AFK runtime path is not a directory"));
            }
            if metadata.st_uid() != getuid().as_raw() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "AFK runtime directory owner mismatch",
                ));
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)
        }
        Err(error) => Err(error),
    }
}

fn verify_path(path: &Path, uid: u32, kind: PathKind, mode: u32) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || metadata.st_uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe AFK runtime entry",
        ));
    }
    let type_matches = match kind {
        PathKind::Regular => metadata.is_file(),
        PathKind::Socket => metadata.file_type().is_socket(),
    };
    if !type_matches || metadata.st_mode() & 0o777 != mode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid AFK runtime entry",
        ));
    }
    Ok(metadata)
}

fn read_bounded_file(
    path: &Path,
    uid: u32,
    kind: PathKind,
    mode: u32,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    match verify_path(path, uid, kind, mode) {
        Ok(metadata) => {
            if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
                return Err(invalid("AFK runtime file is too large"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("AFK runtime file is too large"));
    }
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("invalid AFK runtime filename"))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn session_from_metadata_name(name: &OsStr) -> Option<SessionId> {
    let bytes = name.as_bytes();
    if bytes.len() != 37 || &bytes[32..] != b".json" {
        return None;
    }
    SessionId::parse_bytes(&bytes[..32]).ok()
}

fn path_entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_owned_entry_if_present(
    path: &Path,
    uid: u32,
    kind: PathKind,
    mode: u32,
) -> io::Result<()> {
    match verify_path(path, uid, kind, mode) {
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Copy)]
enum PathKind {
    Regular,
    Socket,
}
