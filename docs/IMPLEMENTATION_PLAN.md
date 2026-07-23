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

No async runtime or mutex is needed initially.

### Minimal local IPC

The owner-only Unix stream uses records with a five-byte header:

```text
kind: u8
payload_len: u32, big-endian
```

Payloads are capped at 64 KiB before allocation. The format carries only attach, input, output, resize, stop, exit, and bounded error messages.

This is not a public protocol. It carries only live terminal and lifecycle events.

### Minimal dependencies

Use the standard library where it remains clear. Expected dependencies are:

| Need | Candidate | Rule |
| --- | --- | --- |
| OS randomness | `getrandom` | Session IDs only. |
| Unix and PTY operations | `rustix` | Enable only required features. |
| CLI parsing | `lexopt` | Add only if the current parser becomes unclear. |
| Metadata JSON | `serde`, `serde_json` | Cap file size before parsing; never serialize terminal data. |

Do not add Tokio, TLS, terminal emulation, a general wire serializer, or a general logging framework.

### Unsafe and panic behavior

Keep unwinding enabled so terminal-mode guards can restore the invoking terminal. Unsafe Rust remains denied. If a safe PTY API is insufficient, isolate the minimum exception under `platform::linux` with documented invariants and dedicated tests.

## 3. Process startup

1. `afk stream` chooses or validates a 128-bit session ID.
2. The launcher creates a private startup socket pair.
3. It starts the current executable in hidden runner mode with that descriptor and detached standard streams.
4. The runner calls checked session/process setup, creates owner-only runtime files, binds its Unix socket, creates a PTY, and starts the login shell.
5. The runner reports `Ready` or a typed error.
6. The launcher either exits for `--detach` or becomes an attachment.

The survival test must establish the exact `setsid`, controlling-terminal, process-group, and descriptor sequence. Do not rely on `nohup` behavior.

## 4. Runner behavior

Each event-loop cycle performs bounded work:

1. read available PTY output first;
2. process child exit and stop state;
3. accept an attachment and replace any older one;
4. process bounded input and resize records;
5. flush the bounded output queue;
6. block only when no immediate work remains.

When detached, PTY output is read and discarded. When the active queue reaches 1 MiB, the attachment is dropped and PTY draining continues.

No terminal state is reconstructed. Reattach begins with output produced after the new connection and an applied resize.

## 5. Review-sized steps

### Step 0: scaffold — complete

- Cargo package and `afk` binary;
- Rust and Clippy lint policy;
- `--help` and `--version`;
- unit and CLI acceptance tests;
- Cargo Deny and pinned CI actions;
- static x86-64 and AArch64 musl builds.

### Step 1A: limits, identity, and IPC

- Add constants for record, queue, metadata, dimensions, and path bounds.
- Add `SessionId([u8; 16])` generation, lowercase hexadecimal display, and strict parsing.
- Add the five-byte IPC header and record-kind validation.
- Add round-trip, every-truncation, oversized-length, unknown-kind, and trailing-data tests.

Exit: malformed IPC cannot allocate above its limit or enter runner state.

### Step 1B: runtime registry

- Resolve and validate the home-relative runtime root.
- Create it with mode 0700 and verify ownership.
- Add exclusive lock, owner-only socket path, bounded metadata, and atomic replacement.
- Reject symlinks, overlong socket paths, malformed IDs, and oversized metadata.
- Verify stale entries through the live socket rather than PID alone.

Exit: concurrent or hostile filesystem entries cannot redirect session control.

### Step 2A: process-survival spike

- Add checked hidden-runner startup and readiness response.
- Create a PTY and login shell.
- Bind the owner-only Unix socket.
- Drain PTY output with no attachment.
- Add a test-only lifecycle observation that never exposes terminal bytes.

Exit: killing the launcher leaves the runner, PTY, shell PID, and cwd alive.

### Step 2B: stream and attach

- Implement raw terminal mode with RAII restoration.
- Forward bounded input and output records.
- Forward initial dimensions and resize events.
- Replace an older attachment when a new one connects.
- Treat socket close and SSH loss as detach.

Exit: repeated attach and disconnect reaches the same shell without blocking it.

### Step 3: lifecycle commands

- Implement `sessions`, `sessions --json`, `stream --detach`, and `stop`.
- Add verified process-group signaling and bounded TERM/KILL escalation.
- Add slow-client, shell-exit, stale-cleanup, PID-reuse, and signal tests.

Exit: all intended lifecycle operations are explicit, bounded, and deterministic.

### Step 4: SSH and release hardening

- Add containerized OpenSSH create, abrupt TCP loss, reconnect, and stop tests.
- Execute both musl artifacts in clean target environments.
- Add SBOM, checksums, provenance, install, rollback, and reproducibility documentation.

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
- highest configured output queue allocation.

The current artifact checks remain under `tests/acceptance/`.
