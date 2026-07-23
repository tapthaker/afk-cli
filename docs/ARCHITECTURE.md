# AFK CLI Architecture

**Status:** Initial implementation complete; hardening in progress

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

The initial promise is process continuity with bounded raw history. Each live attachment receives the retained output tail before new PTY output; AFK does not reconstruct terminal screen state.

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
- terminal emulation or screen-state reconstruction;
- windows, panes, or rendered scrollback;
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
- The process inherits the cwd and environment of `afk stream`; AFK does not persist either.
- The command is used only when creating a session.
- If the ID already exists as a live or retained completed session, `stream` returns `SessionExists` and does not attach or start another process.
- There is no detached creation mode.

### `afk attach`

`attach` connects the invoking SSH terminal to an existing runner.

- `attach` never creates a process.
- For a live session, `attach` first receives the retained raw output tail and then continues with new PTY output.
- For a retained completed session, `attach` prints the retained raw output tail, a truncation marker when needed, and the completion summary, then returns the recorded exit status.
- A new attachment replaces an older attachment so a stale SSH connection cannot block recovery.
- The outer terminal enters raw mode and is restored when attachment ends.
- Closing SSH, stdin, or the Unix socket detaches without stopping the session process.

### `afk sessions`

`sessions` lists live and retained completed sessions. Live entries include runner and child PIDs, start time, and attached state. Completed entries include start time, finish time, and exit code or signal. `--json` emits bounded machine-readable output.

Listings and metadata exclude argv, environment values, credentials, terminal input, and terminal output.

### `afk stop`

`stop` connects to the verified runner and asks it to close the inner PTY and terminate the child session leader. It never signals a PID solely because that PID appeared in metadata.

Stop is best effort for descendants. Ordinary shell jobs receive terminal hangup and exit, but a process that deliberately ignores signals or creates a new Unix session may survive.

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
- `Stop`: deliberate termination from `afk stop`;
- `Exit`: a two-byte reason (`code` or `signal`) and `u8` value.

The first record on a connection must be `Attach` or `Stop`. After `Attach`, only input and resize records are accepted from the attachment; output and one final exit record may be sent by the runner.

Socket closure without an exit record represents attachment loss. No acknowledgement, replay-cursor, or screen-state protocol is included.

### Forwarding

The runner uses one nonblocking event loop for the inner PTY, listener, and active attachment.

- PTY reads never wait for an attachment write.
- Every PTY output byte is added to a 256 KiB in-memory tail ring.
- With no attachment, output is still drained into that bounded ring.
- On attach, the runner snapshots the tail into the attachment queue before reading more PTY output, preserving replay-before-live ordering.
- A separate bounded queue holds replay and new output waiting for the active attachment.
- If that queue fills, the attachment is closed and the runner continues draining output.
- Accepting a new `Attach` closes the previous attachment socket.
- Input in flight during a disconnect has the same uncertainty as ordinary SSH and is never automatically resent.

If the ring has wrapped, AFK sends a static truncation marker before the raw tail. Because there is no output acknowledgement or replay cursor, a new attachment may see bytes that its previous terminal had already received. The tail is raw PTY output, not terminal rows or reconstructed screen state.

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

The runner uses process APIs internally to observe child exit. `afk stop` closes the inner PTY, sends `SIGTERM` to the verified child session leader, waits five seconds, and sends `SIGKILL` if that process remains. It does not scan unrelated processes through `/proc`.

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
~/.afk/run/<session-id>.out
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
- completed output mode 0600 and size at most 256 KiB;
- atomic completed-output replacement;
- stale cleanup verified through the live socket, not PID alone.

The home-relative root does not depend on `$XDG_RUNTIME_DIR`, which may disappear when the final login ends.

## 8. Lifecycle

A session completes when:

- its shell or command exits;
- the user runs `afk stop`;
- the host or administrator kills it;
- the runner or inner PTY fails unrecoverably.

There is no detached-session timeout. After process exit, the runner:

1. drains remaining PTY output into the tail ring;
2. atomically writes the last 256 KiB of raw PTY output to the owner-only output file;
3. records the exit code or signal, finish time, retained byte count, and truncation flag atomically;
4. sends an exit record to an active attachment;
5. removes its socket and lock;
6. retains the output and metadata tombstone for 24 hours;
7. exits.

Any later AFK command lazily removes expired output and metadata. AFK does not persist a separate input stream, although terminal echo may place input bytes in captured output. If the runner is killed before observing process completion, its in-memory tail may be lost. An attached command returns the child exit code; a signal exit returns the conventional `128 + signal` status.

Disconnect, replacement attachment, process exit, and stop are separate tested transitions.

## 9. Bounds and security

Initial fixed bounds are:

```text
IPC payload                  64 KiB
attachment output queue       1 MiB
in-memory output tail        256 KiB
completed output file        256 KiB
metadata file                64 KiB
terminal rows/columns         1..=4096
sessions returned per list    1024
PTY bytes processed per tick  256 KiB
stop grace period              5 seconds
command arguments              256 entries / 64 KiB aggregate
completed metadata retention   24 hours
```

Security requirements:

- SSH owns remote authentication, encryption, and transport integrity;
- runtime paths and sockets are owner-only and symlink-safe;
- peer credentials are checked where supported;
- record lengths are checked before allocation;
- process startup uses argv and never `sh -c` for supplied values;
- stop signals only the verified child session leader and documents best-effort descendant cleanup;
- terminal and IPC payloads never enter diagnostics or metadata;
- only the bounded completed-output file may persist raw terminal output;
- unsafe Rust is denied except for a narrowly reviewed platform operation when no safe API exists.

AFK does not isolate mutually untrusted people sharing one Unix UID. Detailed controls and exclusions are in [THREAT_MODEL.md](THREAT_MODEL.md).

## 10. Implementation and acceptance

Rust module boundaries, dependencies, and delivery steps are maintained in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md). Linux PTY, signal, thread, stop, and socket-path experiments are recorded in [SPIKE_RESULTS.md](SPIKE_RESULTS.md).

Concrete disconnect, malformed-input, process, SSH, and static-artifact criteria are maintained in the [acceptance test catalog](../tests/acceptance/README.md).

Before version 1.0, users should stop existing sessions before replacing the binary.
