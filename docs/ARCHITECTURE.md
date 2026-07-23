# AFK CLI Architecture

**Status:** Draft

**Repository:** `afk-cli`

**Installed binary:** `afk`

## 1. Purpose

AFK CLI keeps a user-owned terminal process alive when its SSH connection disappears.

```text
SSH connection
      |
      v
afk attach process             short-lived
      |
      | owner-only Unix socket
      v
afk session runner             persistent
      |
      | PTY master
      v
default shell or explicit command
```

When SSH disconnects, the attach process ends. The runner keeps the inner PTY and its process alive. A later SSH connection starts another attach process and reconnects to the same runner.

AFK does not implement SSH. SSH carries ordinary terminal stdin, stdout, and resize events to the remote `afk` attachment.

The initial promise is process continuity. AFK does not reconstruct output produced while detached.

## 2. Scope

AFK provides:

- one persistent runner per session;
- one active attachment per runner;
- a PTY-backed default shell or explicit command;
- reconnection through an owner-only Unix socket;
- terminal input, output, and resize forwarding;
- session listing and deliberate stop.

AFK does not provide:

- a TCP or UDP listener;
- a hosted service or account;
- a machine-wide daemon;
- a public wire protocol;
- terminal emulation, replay, or recording;
- windows, panes, or scrollback;
- survival across host reboot.

## 3. Commands

```text
afk --help
afk --version
afk stream SESSION_ID [-- COMMAND [ARG...]]
afk attach SESSION_ID
afk sessions [--json]
afk stop SESSION_ID
```

### `afk stream`

`stream` requires a session ID, creates the runner and inner PTY when needed, starts a process, and attaches immediately.

- The ID must contain exactly 32 lowercase hexadecimal characters.
- The caller chooses the ID before starting the SSH command.
- With no command, AFK executes `$SHELL` when it names an absolute executable file, with `/bin/sh` as fallback. AFK does not add login-shell flags.
- A command after `--` is executed as an exact bounded argv vector without `sh -c`.
- The command is used only when creating a session.
- If the ID already has a live runner, `stream` attaches without starting another process.
- There is no detached creation mode.

### `afk attach`

`attach` connects the invoking SSH terminal to an existing runner.

- `attach` never creates a process.
- A new attachment replaces an older attachment so a stale SSH connection cannot block recovery.
- The outer terminal enters raw mode and is restored when attachment ends.
- Closing SSH, stdin, or the Unix socket detaches without stopping the session process.

### `afk sessions`

`sessions` lists live session IDs, runner and child PIDs, start time, and attached state. `--json` emits bounded machine-readable output.

Listings and metadata exclude argv, environment values, credentials, terminal input, and terminal output.

### `afk stop`

`stop` connects to the verified runner and asks it to terminate the inner PTY process group. It never signals a PID solely because that PID appeared in metadata.

## 4. Process model

```text
sshd
  |
  +-- outer PTY
        |
        +-- afk stream/attach       SSH lifetime
              |
              | Unix socket
              v
            afk __runner            session lifetime
              |
              +-- inner PTY
                    |
                    +-- default shell or explicit command
                          |
                          +-- child process tree
```

There is no global AFK daemon. Each session has one independent runner.

### Checked runner startup

The launcher creates a private startup socket pair and starts the same executable in hidden runner mode. The runner:

1. validates the inherited startup descriptor;
2. creates a new Unix session and detaches from the SSH process group;
3. applies umask `077` and closes unrelated descriptors;
4. validates and creates its runtime files;
5. binds an owner-only Unix socket;
6. creates the inner PTY;
7. starts the default shell or explicit argv;
8. writes bounded safe metadata atomically;
9. reports `Ready` or a typed startup error;
10. enters its event loop.

The launcher attaches only after `Ready`. Every system-call result is checked. `nohup` alone is not sufficient.

Host cgroup or administrator policy may still kill detached processes. AFK must not claim survival where host policy prevents it.

## 5. Local Unix socket

The Unix socket is required because the short-lived attachment and persistent runner are different processes. It allows a new SSH connection to reach the inner PTY owned by an existing runner.

The socket is local implementation plumbing, not an app-facing or network protocol.

### Minimal records

Each record has a five-byte header:

```text
kind          u8
payload_len   u32, big-endian
payload       payload_len bytes
```

Initial record kinds are limited to:

- `Attach`: marks an attachment connection and carries initial rows and columns;
- `Input`: terminal bytes from the outer PTY to the inner PTY;
- `Output`: terminal bytes from the inner PTY to the outer PTY;
- `Resize`: new rows and columns;
- `Stop`: deliberate termination from `afk stop`.

The first record on a connection must be `Attach` or `Stop`. After `Attach`, only input and resize records are accepted from the attachment; only output records are sent by the runner.

Socket closure represents attachment loss. When the session process exits, the runner closes the attachment socket and cleans up. No acknowledgement, replay, screen-state, or structured exit protocol is included initially.

### Forwarding

The runner uses one nonblocking event loop for the inner PTY, listener, and active attachment.

- PTY reads never wait for an attachment write.
- With no attachment, PTY output is read and discarded.
- A bounded queue holds output waiting for the active attachment.
- If that queue fills, the attachment is closed and the runner continues draining output.
- Accepting a new `Attach` closes the previous attachment socket.
- Input in flight during a disconnect has the same uncertainty as ordinary SSH and is never automatically resent.

## 6. Resize and signal handling

A normal SSH resize works by storing dimensions on a PTY, not by encoding dimensions in `SIGWINCH`.

For AFK there are two PTYs, so resize is copied between them:

```text
SSH client sends window-change
        |
        v
sshd applies TIOCSWINSZ to outer PTY
        |
        v
kernel sends SIGWINCH to afk attach
        |
        v
afk attach reads outer size with TIOCGWINSZ
        |
        | Resize { rows, columns }
        v
afk runner applies TIOCSWINSZ to inner PTY
        |
        v
kernel sends SIGWINCH to inner foreground process group
```

AFK does not directly forward `SIGWINCH`; it forwards the dimensions and lets the kernel notify the process attached to the inner PTY.

No other interactive signal needs forwarding:

- `Ctrl-C`, `Ctrl-Z`, and `Ctrl-\` are forwarded as bytes; the inner PTY's line discipline generates `SIGINT`, `SIGTSTP`, or `SIGQUIT` when its current terminal mode enables those controls;
- `Ctrl-D` is likewise handled by the inner PTY according to its current terminal mode.

`SIGHUP`, `SIGTERM`, or other termination received by the attachment ends only that attachment. It must not terminate the persistent session process.

The runner uses process APIs internally to observe child exit. `afk stop` causes the runner to send `SIGTERM` to its verified process group, wait five seconds, and then send `SIGKILL` if necessary.

## 7. Runtime files

Default root:

```text
~/.afk/run/
```

Per-session files:

```text
~/.afk/run/<session-id>.sock
~/.afk/run/<session-id>.json
~/.afk/run/<session-id>.lock
```

Requirements:

- root mode 0700;
- owner-only socket and lock;
- expected UID ownership;
- strict session-ID parsing before path construction;
- no symlink traversal;
- exclusive creation;
- atomic metadata replacement;
- Unix socket path-length validation;
- bounded metadata read before JSON parsing;
- stale cleanup verified through the live socket, not PID alone.

The home-relative root does not depend on `$XDG_RUNTIME_DIR`, which may disappear when the final login ends.

## 8. Lifecycle

A session ends when:

- its shell or command exits;
- the user runs `afk stop`;
- the host or administrator kills it;
- the runner or inner PTY fails unrecoverably.

There is no detached-session timeout. After process exit, the runner removes its socket, lock, and metadata and exits. Completed sessions are not retained.

Disconnect, replacement attachment, process exit, and stop are separate tested transitions.

## 9. Bounds and security

Initial fixed bounds are:

```text
IPC payload                  64 KiB
attachment output queue       1 MiB
metadata file                64 KiB
terminal rows/columns         1..=4096
sessions returned per list    1024
PTY bytes processed per tick  256 KiB
stop grace period              5 seconds
command arguments              256 entries / 64 KiB aggregate
```

Security requirements:

- SSH owns remote authentication, encryption, and transport integrity;
- runtime paths and sockets are owner-only and symlink-safe;
- peer credentials are checked where supported;
- record lengths are checked before allocation;
- process startup uses argv and never `sh -c` for supplied values;
- stop signals only the runner-owned process group;
- terminal and IPC payloads are never logged or persisted;
- unsafe Rust is denied except for a narrowly reviewed platform operation when no safe API exists.

AFK does not isolate mutually untrusted people sharing one Unix UID. Detailed controls and exclusions are in [THREAT_MODEL.md](THREAT_MODEL.md).

## 10. Implementation and acceptance

Rust module boundaries, dependencies, and delivery steps are maintained in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).

Concrete disconnect, malformed-input, process, SSH, and static-artifact criteria are maintained in the [acceptance test catalog](../tests/acceptance/README.md).

Before version 1.0, users should stop existing sessions before replacing the binary.
