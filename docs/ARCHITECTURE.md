# AFK CLI Architecture

**Status:** Draft

**Repository:** `afk-cli`

**Installed binary:** `afk`

## 1. Purpose

AFK CLI keeps one user-owned terminal process alive when its SSH connection disappears.

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
login shell and child processes
```

When SSH disconnects, the attach process and its socket connection end. The runner keeps the PTY and shell alive. A later SSH connection starts another attach process and connects to the same runner.

AFK does not implement SSH. A normal SSH PTY channel carries terminal stdin and stdout to the remote `afk` process.

AFK is intentionally smaller than a terminal multiplexer:

- no windows or panes;
- no hosted service or account;
- no TCP or UDP listener;
- no machine-wide daemon;
- no terminal emulator or screen reconstruction;
- no terminal recording;
- no public wire protocol;
- no survival across host reboot.

The initial promise is process continuity, not perfect reconstruction of terminal output missed while disconnected.

## 2. Design rules

1. The runner, not the SSH attachment, owns the PTY and child process.
2. Losing an attachment never stops the session.
3. The runner always drains PTY output, even with no attachment.
4. Output produced while detached is discarded rather than stored without bound.
5. Runtime sockets and metadata are accessible only to the owning Unix user.
6. Internal IPC records, buffers, names, paths, and terminal dimensions are bounded.
7. Session identifiers are caller-supplied and strictly validated before use in a path.
8. `attach` never creates a replacement shell.
9. `stop` is explicit and distinct from disconnecting.
10. Terminal bytes, input, environment values, and credentials are never logged.
11. User-provided values are never interpolated into shell command strings.
12. Unsafe Rust is denied by default and isolated if a PTY operation requires it.

## 3. Commands

The initial command surface is:

```text
afk --help
afk --version
afk stream SESSION_ID
afk attach SESSION_ID
afk sessions [--json]
afk stop SESSION_ID
```

### `afk stream`

`stream` requires a session ID, creates a runner, starts the account's login shell in a PTY, and attaches immediately.

- The ID must contain exactly 32 lowercase hexadecimal characters.
- The caller chooses the ID before starting the SSH command.
- A caller-known ID allows a safe retry after an uncertain SSH disconnect.
- If the same ID already has a live runner, `stream` attaches to it instead of creating another shell.
- No detached creation, arbitrary command, or working-directory option is included initially.

### `afk attach`

`attach` connects the invoking terminal to an existing runner.

- The session must already exist.
- A new attachment replaces an older one so a stale SSH connection cannot block recovery.
- The local terminal enters raw mode and is restored on exit and handled signals.
- Initial dimensions and later resize events are forwarded to the runner.
- Closing SSH, stdin, or the Unix socket detaches without stopping the shell.
- No failed attach path creates a new session.

### `afk sessions`

`sessions` lists live sessions using safe metadata:

- session ID;
- runner and child PID;
- start time;
- attached or detached state.

`--json` provides bounded machine-readable output. Output excludes command input, terminal bytes, environment values, and credentials.

### `afk stop`

`stop` asks the verified runner to terminate its PTY process group. The runner sends `SIGTERM`, waits for a bounded grace period, then uses `SIGKILL` if required.

AFK never signals a PID from stale metadata without first connecting to and verifying the runner.

## 4. Process model

```text
sshd
  |
  +-- afk stream/attach             SSH lifetime
        |
        | Unix socket
        v
      afk __runner                  session lifetime
        |
        +-- PTY master
              |
              +-- login shell
                    |
                    +-- foreground process tree
```

There is one runner per session and no global daemon.

### Checked startup

The launcher creates a private startup socket pair and starts the current executable in a hidden runner mode. The runner:

1. validates the inherited startup descriptor;
2. creates a new Unix session and detaches from the SSH process group;
3. applies umask `077` and closes unrelated descriptors;
4. creates and validates its runtime files;
5. binds an owner-only Unix socket;
6. creates the PTY and starts the login shell by argv;
7. writes bounded safe metadata atomically;
8. reports `Ready` or a typed error through the startup socket;
9. enters the runner loop.

The launcher reports success only after `Ready`. Every system-call result is checked. `nohup` alone is not sufficient.

Host cgroup or login policy may still kill detached processes. AFK reports this limitation honestly; a later diagnostic command may automate host-policy checks.

## 5. Internal IPC

The Unix socket record format is private implementation plumbing between `afk` processes on the same host. It is not carried directly over SSH and is not an integration API.

Each record has a small fixed header:

```text
kind          u8
payload_len   u32, big-endian
payload       payload_len bytes
```

The decoder rejects a payload length above 64 KiB before allocation. Initial record kinds are:

- `Attach`: initial rows and columns;
- `Input`: terminal input bytes;
- `Output`: PTY output bytes;
- `Resize`: rows and columns;
- `Stop`: deliberate session termination;
- `Exit`: child exit status;
- `Error`: bounded stable internal error code.

The format carries only live terminal and lifecycle events. It does not attempt reliable delivery or reconstruct missed output.

Before version 1.0, users should stop existing sessions before upgrading.

### Forwarding behavior

The runner uses one nonblocking event loop for the PTY, listener, and single active attachment. Accepting a new attachment closes the previous attachment socket.

- PTY reads never wait for a client write.
- Input and output records are at most 64 KiB.
- The active attachment output queue is capped initially at 1 MiB.
- A slow attachment is disconnected when its queue fills.
- With no attachment, PTY output is read and discarded.
- Reattachment starts with new output; missed output is not replayed.
- A resize is applied to the runner-owned PTY and normally prompts full-screen applications to redraw.

Input that was in flight when SSH failed has the same uncertainty as ordinary SSH. AFK does not attempt transactional input delivery.

Initial fixed bounds are:

```text
IPC payload                 64 KiB
attachment output queue      1 MiB
metadata file               64 KiB
terminal rows/columns        1..=4096
sessions returned per list   1024
PTY bytes processed per tick 256 KiB
stop grace period             5 seconds
```

These values may change after measurement, but they never become unbounded.

## 6. Runtime files

Default runtime root:

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
- no symlink traversal;
- exclusive creation;
- atomic metadata replacement;
- Unix socket path-length validation;
- bounded metadata read before JSON parsing;
- stale cleanup based on a socket handshake, not PID alone.

The home-relative root avoids relying only on `$XDG_RUNTIME_DIR`, which may disappear when the final login ends.

Metadata contains lifecycle information only. Terminal input and output remain memory-only and are not retained after being forwarded or discarded.

## 7. Session lifecycle

A session ends when:

- the shell exits;
- the user runs `afk stop`;
- the host or administrator kills it;
- the runner or PTY fails unrecoverably.

There is no detached-session timeout. When the shell exits, the runner sends an exit record to an active attachment, removes its runtime files, and exits. Completed sessions are not retained initially.

Disconnecting, terminal EOF, and `stop` are separate state transitions and have separate tests.

## 8. Security boundary

SSH remains responsible for host verification, user authentication, encryption, and transport integrity. AFK adds no network listener or remote credential.

The local controls are:

- owner-only runtime directory and socket;
- peer credential checks where available;
- strict session-ID parsing;
- bounded IPC records and queues;
- symlink-safe runtime operations;
- argv-based shell startup;
- verified runner control before signaling;
- no terminal data in logs or metadata.

AFK does not isolate mutually untrusted people sharing one Unix UID. See [THREAT_MODEL.md](THREAT_MODEL.md) for the complete scoped analysis.

## 9. Rust structure

The repository remains one Cargo package and one executable. Modules are introduced only when they contain behavior:

```text
src/
  main.rs          thin process entry point
  lib.rs           application dispatch and test seam
  cli.rs           argument parsing
  identity.rs      session IDs
  limits.rs        reviewed bounds
  ipc.rs           local record encoding and decoding
  registry.rs      runtime files and session lookup
  runner.rs        PTY and attachment event loop
  attach.rs        terminal and Unix-socket forwarding
  platform/linux.rs
                   PTY, process-group, peer, and polling operations
```

Dependency direction stays simple:

```text
cli -> application operations
runner/attach -> ipc + registry + platform
ipc -> limits
registry -> identity + limits + platform
platform -> standard library and reviewed syscall crate
```

No async runtime is planned initially. The runner is single-threaded so PTY, attachment, resize, and exit ordering remain explicit.

## 10. Dependencies and build

Prefer the standard library. Expected small dependencies are introduced only with the feature that needs them:

- `rustix` for reviewed Unix and PTY operations;
- `lexopt` if hand-written parsing stops being clear;
- `serde` and `serde_json` for bounded metadata and `sessions --json`.

AFK does not need Tokio, TLS, a terminal-emulation crate, a general logging framework, or a general wire serializer.

Initial release targets are:

```text
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
```

Release artifacts must be static and contain no `PT_INTERP` or `DT_NEEDED` entries. Current engineering budgets remain:

- compressed artifact below 15 MiB;
- idle runner RSS below 25 MiB;
- runner ready in under 250 ms on a representative host.

These are measured budgets, not reasons to weaken correctness.

## 11. Testing

### Unit tests

- CLI parsing and bounded diagnostics;
- session-ID parsing and formatting;
- every IPC record round trip;
- truncated, oversized, and unknown IPC records;
- runtime path and metadata validation;
- runner state transitions and queue limits.

### Local integration tests

- shell PID and cwd survive attachment loss;
- runner survives launcher exit;
- detached output cannot block the child;
- reconnect reaches the same shell;
- resize reaches the PTY;
- a slow attachment is dropped while the shell survives;
- `stop` handles shell process groups and descendants;
- runtime files are cleaned after exit;
- symlink and stale-PID cases are rejected.

### OpenSSH end-to-end tests

- create through an SSH PTY channel;
- destroy TCP without graceful close;
- verify runner and shell survive;
- reconnect and attach to the same session;
- verify shell PID, cwd, and a synthetic variable remain;
- stop and verify cleanup.

### Artifact tests

- both musl architectures;
- no dynamic loader or shared-library dependencies;
- size budgets;
- dependency licenses and advisories;
- clean-image execution.

The concrete criteria are maintained in the [acceptance test catalog](../tests/acceptance/README.md).

## 12. Delivery plan

### Step 0: scaffold — complete

- Rust package, thin binary, lints, tests, CI, Cargo Deny, and static musl builds;
- side-effect-free `--help` and `--version`.

### Step 1: local session foundation

- bounded session ID;
- secure runtime root and registry;
- small internal IPC codec;
- malformed-input and path tests.

### Step 2: process survival

- checked runner startup;
- PTY and login shell;
- always-drained output;
- basic `stream` and `attach`;
- prove disconnect/reconnect preserves the process.

### Step 3: lifecycle and hardening

- `sessions`, `--json`, and `stop`;
- resize, slow-client handling, stale cleanup, and signal tests;
- OpenSSH abrupt-disconnect tests.

### Step 4: release readiness

- both musl targets executed in clean fixtures;
- artifact checks, SBOM, checksums, provenance, and install documentation;
- architecture and threat model reconciled with implementation.

## 13. Deferred scope

- terminal output replay;
- terminal screen snapshots or emulation;
- exactly-once input delivery;
- multiple simultaneous attachments;
- attachment takeover;
- compatibility with old runners after upgrade;
- custom child commands and working directories;
- windows, panes, and scrollback;
- host reboot persistence;
- machine-wide service;
- TCP/UDP listener;
- hosted relay or discovery;
- file transfer and port forwarding;
- telemetry and terminal recording;
- non-Linux hosts.
