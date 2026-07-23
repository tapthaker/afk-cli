# AFK CLI Implementation Plan

**Status:** In progress

This plan implements the small process-continuity design in [ARCHITECTURE.md](ARCHITECTURE.md) and [THREAT_MODEL.md](THREAT_MODEL.md).

## 1. Implementation shape

AFK remains one Cargo package and one executable. The runner and attach process are hidden modes of the same binary.

```text
src/
  main.rs          thin entry point
  lib.rs           dispatch and test seam
  cli.rs           argument parsing
  limits.rs        resource bounds
  identity.rs      session ID
  ipc.rs           bounded local records
  registry.rs      runtime files and lookup
  runner.rs        persistent PTY owner
  attach.rs        terminal forwarding
  platform/
    linux.rs       PTY, process, peer, and polling operations
```

Modules are added only when they carry behavior. Do not split the package into internal crates unless a real independent API appears.

## 2. Technical choices

### Linux musl first

Initial targets are:

```text
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
```

Both artifacts are static. Linux-specific operations stay under `platform` so they do not leak into CLI, IPC, or registry code.

### One synchronous runner loop

The runner is single-threaded and nonblocking. One state owner coordinates:

- PTY reads and writes;
- listener accept;
- the active attachment;
- resize;
- output queue limits;
- child exit and stop.

No async runtime or mutex is needed initially. Linux spikes confirmed one thread per AFK process using `poll`; see [SPIKE_RESULTS.md](SPIKE_RESULTS.md).

### Minimal local IPC

The owner-only Unix stream uses records with a five-byte header:

```text
kind: u8
payload_len: u32, big-endian
```

Payloads are capped at 64 KiB before allocation. The record kinds are attach, input, output, resize, stop, and exit. Socket closure without an exit record represents detach.

This is not a public protocol. It carries only live terminal bytes, dimensions, stop requests, and final process status.

### Minimal dependencies

Use the standard library where it remains clear. Expected dependencies are:

| Need | Candidate | Rule |
| --- | --- | --- |
| Unix and PTY operations | `rustix` | Enable only required features. |
| Signal registration | `signal-hook` | Atomic flags only; no signal thread. |
| CLI parsing | `lexopt` | Add only if the current parser becomes unclear. |
| Metadata JSON | `serde`, `serde_json` | Cap file size before parsing; never serialize terminal data. |

Do not add Tokio, TLS, terminal emulation, a general wire serializer, or a general logging framework.

### Unsafe and panic behavior

Keep unwinding enabled so terminal-mode guards can restore the invoking terminal. Unsafe Rust remains denied. The Linux spike confirmed a safe design: spawn fresh hidden runner and child-helper modes, perform `setsid` and controlling-terminal setup in those fresh processes, and avoid `fork`, `pre_exec`, and project-owned unsafe code.

## 3. Process startup

1. `afk stream` validates its required 128-bit session ID and optional argv after `--`.
2. The launcher creates a private startup socket pair.
3. It starts the current executable in hidden runner mode with that descriptor and detached standard streams.
4. The runner creates owner-only runtime files, binds its Unix socket, and safely opens a PTY master and slave.
5. It starts a fresh hidden child-helper with the slave as standard I/O; that helper creates its terminal session and executes the validated `$SHELL` default or exact supplied argv.
6. The runner reports `Ready` or a typed error.
7. The launcher becomes an attachment after the runner reports readiness.

The disposable Linux spike established this sequence on AArch64 and x86-64 musl. Production integration tests must preserve the same invariants and must not rely on `nohup` behavior.

## 4. Runner behavior

Each event-loop cycle performs bounded work:

1. read available PTY output first;
2. process child exit and stop state;
3. accept an attachment and replace any older one;
4. process bounded input and resize records;
5. flush the bounded output queue;
6. block only when no immediate work remains.

Every PTY output byte enters a 256 KiB in-memory tail ring. When detached, output is drained into that ring without being sent. On attach, a tail snapshot is placed in the 1 MiB attachment queue before new PTY output. When that queue reaches its limit, the attachment is dropped and PTY draining continues.

No terminal state is reconstructed. Live reattach receives the raw tail followed by new output in order; without an acknowledgement cursor, some previously seen bytes may be replayed. After observed process completion, the runner atomically persists the bounded tail for completed-session retrieval.

### Resize and signals

The attachment receives `SIGWINCH` when `sshd` changes the outer PTY. It reads that PTY with `TIOCGWINSZ` and sends rows and columns to the runner. The runner applies them to the inner PTY with `TIOCSWINSZ`; the kernel then signals the inner foreground process group.

No other interactive signal is proxied. Control characters remain input bytes, allowing the inner PTY to generate `SIGINT`, `SIGTSTP`, and `SIGQUIT`. Termination of the attachment closes only the attachment socket. `signal-hook` sets atomic flags; signal interruption and the bounded poll timeout return control to the same single-threaded loop without a signal thread.

## 5. Review-sized steps

### Step 0: scaffold — complete

- Cargo package and `afk` binary;
- Rust and Clippy lint policy;
- `--help` and `--version`;
- unit and CLI acceptance tests;
- Cargo Deny and pinned CI actions;
- static x86-64 and AArch64 musl builds.

### Step 1A: limits, identity, and IPC — complete

- Add constants for record, queue, output-tail, metadata, dimensions, path, and argv bounds.
- Add `SessionId([u8; 16])` lowercase hexadecimal display and strict parsing.
- Add the five-byte IPC header and validation for attach, input, output, resize, stop, and exit.
- Add round-trip, every-truncation, oversized-length, unknown-kind, and trailing-data tests.

Exit: malformed IPC cannot allocate above its limit or enter runner state.

### Step 1B: runtime registry — implemented, hostile-filesystem tests pending

- Resolve and validate the home-relative runtime root.
- Create it with mode 0700 and verify ownership.
- Add exclusive lock, owner-only socket path, bounded metadata, and atomic replacement.
- Reject symlinks, overlong socket paths, malformed IDs, and oversized metadata.
- Verify stale entries through the live socket rather than PID alone.
- Add 24-hour completed metadata and output tombstones with lazy expiry cleanup.
- Validate owner-only output files before reading and cap them at 256 KiB.

Exit: concurrent or hostile filesystem entries cannot redirect session control.

### Step 2A: process survival — implemented

- Add the spike-validated hidden-runner and child-helper startup sequence.
- Create a PTY and start the default shell or an explicit command without `sh -c`.
- Bind the owner-only Unix socket.
- Drain PTY output into a 256 KiB byte ring with or without an attachment.
- Snapshot and enqueue the raw tail before new output on each attachment.
- Emit a static marker when the replay has been truncated.
- Add a test-only lifecycle observation that never exposes terminal bytes.

Exit: killing the launcher leaves the single-threaded runner, PTY, child PID, cwd, and inherited synthetic environment value intact for both default-shell and explicit-command sessions.

### Step 2B: stream and attach — implemented, signal-path hardening pending

- Implement raw terminal mode with RAII restoration.
- Forward bounded input and output records.
- Forward initial dimensions and resize events.
- Replace an older attachment when a new one connects.
- Treat socket close and SSH loss as detach.

Exit: repeated attach and disconnect reaches the same shell without blocking it.

### Step 3: lifecycle commands — implemented, lifecycle hardening pending

- Implement `sessions`, `sessions --json`, completed output/status reporting, and `stop`.
- Persist the raw tail only after observed process completion and mark truncation in metadata.
- Implement best-effort stop by closing the PTY and applying bounded TERM/KILL escalation to the verified child session leader; do not add `/proc` scanning.
- Add slow-client, shell-exit, stale-cleanup, PID-reuse, and signal tests.

Exit: all intended lifecycle operations are explicit, bounded, and deterministic.

### Step 4: SSH and release hardening — in progress

- Add containerized OpenSSH create, abrupt TCP loss, reconnect, and stop tests.
- Execute both musl artifacts in clean target environments.
- Publish direct x86-64 and AArch64 Linux musl binaries plus Intel and Apple Silicon macOS binaries from version tags.
- Generate release checksums and build-provenance attestations before publishing the draft.
- Add SBOM, install, rollback, notarization, and reproducibility documentation.

Exit: a public prerelease demonstrates the same shell surviving real SSH transport loss.

## 6. Review discipline

- Introduce one dependency with the behavior that requires it.
- Keep filesystem, PTY, and IPC changes in separate review slices.
- Add malformed-input and disconnect tests with each affected boundary.
- Never print IPC or PTY payloads on failure.
- Keep public errors bounded and independent of supplied arguments.
- Update architecture and threat model when behavior changes.
- Record size and dependency deltas for both musl targets.

## 7. Measurements

Track from each release build:

- stripped and compressed executable size;
- direct and transitive dependency count;
- idle detached-runner RSS;
- runner-ready latency;
- open file descriptors while attached and detached;
- highest configured output queue and tail-ring allocation;
- completed-output file size and lazy-retention cleanup.

The current artifact checks remain under `tests/acceptance/`.

Initial lifecycle implementation measurement (Rust 1.85.0, stripped release profile):

```text
AArch64 musl   664,360 bytes; 346,177 bytes gzip -9
x86-64 musl    756,552 bytes; 365,283 bytes gzip -9
```

Direct Linux dependencies are `rustix`, `signal-hook`, `serde`, and `serde_json`; Cargo Deny verifies their transitive licenses, advisories, sources, and duplicate-version policy.
