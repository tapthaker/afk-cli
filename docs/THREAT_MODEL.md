# AFK CLI Threat Model

**Status:** Draft

This document covers the small initial AFK CLI design: a persistent user-owned PTY runner, short-lived SSH attachment processes, and owner-only local Unix sockets.

## Assets

AFK protects:

- continuity of the intended shell or command and its PTY;
- terminal input and output while SSH transports it;
- control of a session from other Unix users;
- integrity of session metadata and lifecycle operations;
- host memory and file descriptors from unbounded local IPC;
- release artifact integrity.

AFK does not store terminal contents. Shell history and files created by applications remain governed by those applications.

## Trust boundaries

### Trusted

- the authenticated Unix account;
- SSH after the client verifies the host key;
- the installed `afk` executable and reviewed dependencies;
- operating-system PTY, process, and Unix-socket primitives.

### Untrusted

- every IPC record received by a runner or attach process;
- PTY output produced by applications;
- runtime filesystem entries before validation;
- stale metadata and PIDs;
- session IDs, terminal dimensions, and command-line arguments;
- downloaded release artifacts before verification;
- disconnect timing and partial reads or writes.

### Outside the security claim

- root or host-administrator compromise;
- compromise of the authenticated Unix account;
- mutually untrusted people sharing one Unix UID;
- a malicious SSH server after its host key is trusted;
- host reboot persistence;
- kernel compromise;
- denial of service by the session's own child process within the user's permissions;
- reconstruction of terminal output produced while detached.

## Threats and controls

### Unauthorized local attachment

Threat: another local user connects to a session socket.

Controls:

- mode-0700 runtime directory;
- owner-only socket and lock;
- ownership verification;
- peer credential checks where supported;
- strictly validated 128-bit session IDs;
- no public listener.

### Runtime path replacement

Threat: an attacker replaces metadata, lock, or socket paths through symlinks or races.

Controls:

- descriptor-relative operations where practical;
- no-follow behavior and expected ownership checks;
- exclusive creation;
- atomic metadata replacement;
- bounded path lengths;
- live socket verification before stale cleanup.

### PID reuse

Threat: stale metadata names an unrelated process that reused a PID.

Controls:

- PID is display metadata, not session identity;
- `stop` connects to the owner-only runner socket;
- process identity is verified through the live runner;
- AFK never signals a PID solely because it appeared in a metadata file.

### Malformed IPC

Threat: a malformed or oversized local record causes allocation exhaustion, overflow, or state confusion.

Controls:

- fixed five-byte header;
- checked big-endian payload length;
- 64 KiB maximum record payload enforced before allocation;
- exact payload length for fixed-size records such as resize;
- unknown kinds and invalid state transitions are rejected;
- malformed, truncated, and oversized inputs are tested and fuzzed.

The IPC format is local implementation plumbing, not a network or client integration protocol.

### Slow or disconnected attachment

Threat: client backpressure prevents the runner from draining the PTY and freezes the shell.

Controls:

- PTY reads remain enabled independently of attachment writes;
- per-attachment output queue is byte-bounded;
- a full queue disconnects the attachment;
- with no attachment, output is read and discarded;
- disconnect never stops the child.

### In-flight input during disconnect

Threat: the user cannot know whether the final bytes before a network failure reached the session process.

Control: AFK makes no exactly-once claim and does not automatically resend input. This is the same uncertainty present in an ordinary interrupted SSH terminal. Avoiding automatic retries prevents AFK from duplicating a command.

### Wrong-session fallback

Threat: a failed attach starts a new process and the user mistakes it for the original session.

Controls:

- create and attach are distinct operations;
- `attach` never creates;
- an explicit `stream` action is required to start a process;
- session ID and safe process metadata are shown to the user.

### Shell injection

Threat: a session ID or command argument enters a shell command string.

Controls:

- session IDs accept exactly 32 lowercase hexadecimal characters;
- the default shell is executed directly from a validated absolute path;
- an explicit command is passed as an exact bounded argv vector;
- no `sh -c` wraps user-provided values;
- no IPC value is interpolated into a command string.

### Process-group escape

Threat: `stop` kills an unrelated process or fails to terminate the intended session tree.

Controls:

- the runner creates and owns the PTY process group;
- stop requests go through a verified runner socket;
- signals target the runner-owned process group;
- TERM-to-KILL escalation has a fixed timeout;
- pipelines, descendants, PID reuse, and cleanup are integration-tested.

### Runner killed with SSH login

Threat: host policy kills detached processes when the SSH login ends.

Controls:

- the runner detaches before attachment starts;
- session and descriptor setup is checked;
- integration tests kill the launcher and SSH transport;
- AFK documents that cgroup or administrator policy can override detachment;
- unsupported hosts are reported rather than hidden behind a false success.

### Terminal escape sequences

Threat: child output contains malicious terminal escape sequences.

Control: AFK treats output as opaque bytes and does not parse or persist it. The user's terminal emulator already receives untrusted remote output during ordinary SSH. AFK does not add a terminal parser to the attack surface.

### Sensitive diagnostics

Threat: terminal bytes, input, arguments, environment values, or credentials enter logs or metadata.

Controls:

- no telemetry;
- no terminal recording;
- no terminal bytes in diagnostics;
- bounded static or metadata-only errors;
- owner-only metadata;
- sentinel tests verify sensitive values are not emitted;
- IPC and PTY payload types do not derive unrestricted debug output.

### Malicious release artifact

Threat: an installer executes a modified or incorrectly linked binary.

Controls before release:

- checksums and signed provenance;
- public CI builds;
- SBOM and license inventory;
- `PT_INTERP` and `DT_NEEDED` inspection for musl artifacts;
- clean-image execution tests;
- atomic installation.

## Resource limits

The initial implementation defines and tests at least:

- IPC payload: 64 KiB;
- attachment output queue: 1 MiB;
- session ID: 16 bytes encoded as 32 lowercase hexadecimal characters;
- metadata file: 64 KiB;
- terminal rows and columns: 1 through 4096;
- Unix socket path: checked against the platform limit;
- sessions returned by one listing: 1024;
- stop grace period: five seconds;
- command argv: 256 entries and 64 KiB aggregate;
- PTY bytes processed per event-loop tick: 256 KiB.

There is no replay buffer, scrollback buffer, terminal snapshot, or completed-session retention in the initial design.

## Unsafe code

Unsafe Rust is denied by default. A required exception must have:

- a dedicated Linux platform module;
- documented preconditions and invariants;
- an explanation of why a safe API is insufficient;
- no IPC parsing or runner state in the unsafe block;
- focused platform tests;
- independent review and sanitizer coverage where applicable.

Low-level PTY operations do not justify unsafe code elsewhere.

## Security review gates

Before the process-survival implementation is accepted:

- runtime symlink and ownership tests pass;
- malformed IPC tests cover every record kind;
- launcher termination leaves the runner and shell alive;
- detached high output does not block the child;
- stop is constrained to the intended process group;
- sentinel values do not appear in diagnostics or metadata;
- dependency, advisory, and license checks pass.

Before a public release:

- abrupt OpenSSH TCP loss is tested end to end;
- both musl artifacts have no dynamic dependencies;
- clean-image execution succeeds;
- release checksums and provenance are verifiable;
- the threat model is reconciled with the implementation.

## Reporting

Do not open a public issue for a suspected vulnerability before maintainers have had a reasonable opportunity to respond. Follow the repository's `SECURITY.md` process.
