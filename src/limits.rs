#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) const IPC_HEADER_BYTES: usize = 5;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) const MAX_IPC_PAYLOAD_BYTES: usize = 64 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) const MAX_IPC_RECORD_BYTES: usize = IPC_HEADER_BYTES + MAX_IPC_PAYLOAD_BYTES;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const MAX_ATTACHMENT_QUEUE_BYTES: usize = 1024 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const OUTPUT_TAIL_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const MAX_METADATA_BYTES: usize = 64 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) const MAX_TERMINAL_DIMENSION: u16 = 4096;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const MAX_SESSION_LIST_ENTRIES: usize = 1024;
pub(crate) const MAX_COMMAND_ARGUMENTS: usize = 256;
pub(crate) const MAX_COMMAND_BYTES: usize = 64 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const MAX_PTY_BYTES_PER_TICK: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const COMPLETED_RETENTION_SECONDS: u64 = 24 * 60 * 60;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const STOP_GRACE_SECONDS: u64 = 5;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const UNIX_SOCKET_PATH_BYTES: usize = 103;
