# AFK CLI Implementation Plan

**Status:** In progress

This plan turns the requirements in [ARCHITECTURE.md](ARCHITECTURE.md) and [THREAT_MODEL.md](THREAT_MODEL.md) into small, reviewable Rust increments. It does not replace either document or the future wire-protocol specification.

## 1. Goals

The implementation should be:

- one self-contained `afk` executable;
- statically linked for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`;
- small in binary size, idle memory, dependency count, and operational scope;
- single-threaded unless measurements demonstrate a need for concurrency;
- divided along protocol, filesystem, process, terminal, and user-interface trust boundaries;
- testable without SSH for most behavior, with OpenSSH reserved for end-to-end validation;
- understandable without private integration code or unpublished context.

Correctness, bounded memory, and process survival take priority over a smaller artifact.

## 2. Initial technical decisions

### One package and one executable

Start with one Cargo package containing a library and a thin binary:

```text
Cargo.toml
src/lib.rs
src/main.rs
```

`main.rs` only calls the library entry point and maps a typed result to an exit code. Internal modules provide review boundaries without introducing a workspace or multiple internal crates. A module should become a separate crate only when it needs an independently stable API, separate fuzz target, or platform implementation.

### Linux and musl first

The first supported hosts are Linux on x86-64 and AArch64. Linux-specific behavior is isolated behind `platform` so later Unix support does not spread conditional compilation through session logic.

The initial implementation must not depend on glibc, OpenSSL, a shell command wrapper, or a dynamically linked non-system library. Both musl artifacts are built from the same source and lockfile.

### Synchronous event loop

Use one nonblocking event loop per runner for the PTY, Unix listener, active attachment, signals, and timers. Do not introduce Tokio or another async runtime initially.

A single owner of runner state makes ordering explicit for:

- PTY draining;
- attach and takeover;
- replay and snapshot baselines;
- output sequence assignment;
- input deduplication;
- resize;
- child exit and cleanup.

No mutex should be needed in the runner hot path. Blocking filesystem work is limited to bounded startup and cleanup operations.

### Explicit bounded wire codec

Use the architecture's fixed frame header and an explicit binary payload codec. Integers use one documented byte order; byte strings and text have fixed-width lengths; decoders reject trailing fields unless a protocol version permits them.

The codec must:

- inspect and validate frame length before allocation;
- decode from bounded byte slices;
- borrow payload bytes where ownership is unnecessary;
- validate each message against a per-kind limit;
- reject arithmetic overflow, truncation, unknown required flags, and trailing data;
- contain no filesystem, PTY, logging, or command-execution behavior.

Do not use a general-purpose deserializer for wire messages in the first version. The format and golden vectors will be published in `docs/PROTOCOL.md` before compatibility is promised.

### Minimal dependency policy

Prefer the standard library. Initial dependency candidates are:

| Need | Candidate | Constraint |
| --- | --- | --- |
| Unix syscalls and types | `rustix` | Enable only required features; verify PTY and process APIs in the survival spike. |
| OS randomness | `getrandom` | Generate IDs directly; do not pull in a general RNG stack. |
| CLI parsing | `lexopt` | Keep parsing based on `OsString`; no shell interpolation. |
| Bounded metadata JSON | `serde` and `serde_json` | Registry files and `--json` only, never the wire protocol; read size is capped before parsing. |
| Terminal state | `vt100` or another reviewed engine | Deferred until the terminal-recovery phase and accepted only after conformance, license, size, and query-handling review. |

This is a candidate set, not permission to add all dependencies during scaffolding. Each dependency is introduced in the change that needs it with license, maintenance, transitive tree, binary-size, and attack-surface evidence.

Avoid `anyhow`, a general logging framework, an async runtime, and a TLS stack initially. Typed project errors and narrow diagnostics are preferable. This can be revisited with measurements and a design note.

### Panic and unsafe policy

Keep unwinding enabled initially so RAII guards can restore a human terminal after failure. Do not select `panic = "abort"` only to reduce binary size.

Use `#![deny(unsafe_code)]` at the crate boundary. If a required Linux PTY or process operation has no adequate safe API, place the smallest possible exception in a dedicated `platform::linux` module with:

- a module-level `allow(unsafe_code)`;
- documented preconditions and invariants for every operation;
- no protocol parsing or business state in the unsafe module;
- Linux integration tests and an independent review;
- sanitizer coverage where applicable.

Prefer owned file-descriptor types and RAII. Never pass an unvalidated raw descriptor across general application code.

## 3. Module boundaries

The starting source tree should grow toward this shape. Empty placeholder modules should not be created before they carry behavior.

```text
src/
  main.rs                 thin process entry point
  lib.rs                  dispatch and public test seam
  error.rs                typed top-level errors and exit mapping
  limits.rs               reviewed resource-limit constants and newtypes
  identity.rs             session/client IDs and epochs

  cli/
    mod.rs                argv parsing and command model
    output.rs             human and JSON-safe output

  protocol/
    mod.rs                public protocol types
    frame.rs              fixed header and frame limits
    encode.rs             explicit payload writer
    decode.rs             bounded payload reader
    state.rs              legal handshake/stream transitions

  registry/
    mod.rs                session lookup and lifecycle API
    path.rs               runtime-root and session-path validation
    metadata.rs           bounded safe metadata

  platform/
    mod.rs                platform-facing traits/types
    linux/
      pty.rs              PTY creation and terminal control
      process.rs          session/process-group/signal operations
      poll.rs             nonblocking readiness and timers
      peer.rs             Unix peer credentials

  session/
    launcher.rs           checked runner startup and readiness channel
    runner.rs             event-loop coordinator
    lifecycle.rs          child exit, stop, retention, and cleanup
    lease.rs              one-controller attachment policy
    replay.rs             byte-bounded output sequence ring
    input.rs              bounded input deduplication state

  attach/
    human.rs              local raw-terminal adapter and detach escape
    bridge.rs             machine stdio adapter

  terminal/
    mod.rs                bounded terminal-engine interface
    snapshot.rs           versioned ANSI snapshot generation
```

### Dependency direction

Dependencies point inward toward small, pure modules:

```text
main -> cli -> session/attach
session -> protocol + registry + terminal + platform
attach  -> protocol + platform
registry -> identity + limits + platform
protocol -> identity + limits
terminal -> limits
platform -> standard library / reviewed syscall crate
```

Additional rules:

- `protocol` does not perform I/O or know about CLI commands.
- `platform` does not know protocol message types.
- `registry` never stores terminal bytes, input, environment values, or credentials.
- `cli` does not directly manipulate PTYs, sockets, or runtime files.
- `runner` is the only module that coordinates ordering across PTY, lease, replay, input, and terminal state.
- resource limits live in `limits` and are enforced again at trust-boundary entry points;
- important scalar values use newtypes rather than interchangeable integers or strings.

## 4. Process and I/O plan

### Checked runner startup

1. The launcher generates the session ID and creates a private Unix startup socket pair.
2. It starts the current executable in the hidden runner mode with one end of that socket as a known inherited standard descriptor and all other standard streams detached from SSH.
3. The runner validates the startup descriptor, creates a new Unix session, resets inherited state, applies umask `077`, and closes unrelated descriptors.
4. It validates the runtime root, takes the exclusive session lock, binds the owner-only control socket, creates the PTY, and starts the child by argv.
5. It atomically writes safe metadata and reports either `Ready` or a bounded typed error over the startup socket.
6. The launcher reports success only after `Ready`; an uncertain create can be retried with the same session ID.

The process-survival spike must prove the exact PTY, controlling-terminal, process-group, descriptor-inheritance, and `setsid` sequence. Those details must not be inferred from `nohup` behavior.

### Runner loop

Each loop iteration follows a fixed priority policy that is covered by state-machine tests:

1. read PTY output first, up to a per-tick work budget, and remain nonblocking while more output is ready;
2. process child/signal lifecycle events;
3. accept and negotiate bounded attachment work;
4. process bounded input, acknowledgement, and resize frames;
5. flush bounded client output;
6. process timers and cleanup;
7. block for readiness only when no immediate work remains.

Per-tick work limits prevent a continuously readable PTY or client from starving lifecycle events. A full client queue detaches that client and never disables PTY reads.

### Human and machine adapters

Human attachment and `ssh-bridge` are adapters over the same runner protocol.

- The human adapter owns a terminal-mode RAII guard, stdin/stdout, resize signals, and `~.` handling.
- The machine bridge owns framed stdin/stdout and emits no non-protocol stdout.
- Neither adapter owns session lifecycle state.
- Disconnecting either adapter drops only its lease and transport resources.

## 5. Build profile and size controls

Begin with a release profile that improves size without changing panic behavior:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

Choose `opt-level = "s"`, fat LTO, or other tuning only after comparing startup time, throughput, and artifact size. Keep debug symbols in a separate release artifact if stripping is enabled.

CI verifies each release candidate with equivalent checks to:

```bash
cargo build --release --locked --target x86_64-unknown-linux-musl
cargo build --release --locked --target aarch64-unknown-linux-musl
file target/x86_64-unknown-linux-musl/release/afk
python3 tests/acceptance/check_static_elf.py --json \
  target/x86_64-unknown-linux-musl/release/afk
python3 tests/acceptance/check_static_elf.py --json \
  target/aarch64-unknown-linux-musl/release/afk
```

The checker fails if a musl artifact contains a `PT_INTERP` segment or any `DT_NEEDED` dynamic-library entry. Every dependency change records compressed binary-size and idle-RSS deltas. Size budgets are measured in CI rather than enforced by intuition.

## 6. Review-sized implementation sequence

The executable criteria and delivery tiers are tracked in the [acceptance test catalog](../tests/acceptance/README.md).

Every step ends in a working, tested repository. Protocol and process-lifecycle changes include malformed-input and disconnect-path tests in the same change.

### Step 0: decisions and scaffolding

- Add Cargo package, thin `main.rs`, `lib.rs`, lint policy, and release profile.
- Pin a Rust toolchain and document the initial MSRV policy.
- Add formatting, clippy, tests, `cargo deny`, and x86-64 musl CI.
- Record ADRs for the event loop, wire encoding, runtime path, and runner startup strategy.
- Add an `afk --version` smoke path without starting a session.

Exit gate: clean checkout produces a static x86-64 musl executable and dependency/license inventory.

### Step 1: bounded foundations

- Add limits and validated newtypes for IDs, dimensions, names, paths, and sequence numbers.
- Implement fixed frame header and the minimum startup/handshake messages.
- Add golden vectors, round trips, truncation/overflow tests, and decoder fuzz target.
- Implement bounded metadata and runtime-path validation with ownership, mode, and symlink tests.

Exit gate: pure protocol and registry tests pass without creating a PTY.

### Step 2: process-survival vertical slice

- Implement checked launcher/runner startup.
- Create PTY and login shell, bind an owner-only Unix socket, and forward bounded raw bytes.
- Keep draining PTY output after the launcher and attachment disappear.
- Implement minimal cleanup and a test-only inspection path.
- Measure ready time, idle RSS, and binary size.

Exit gate: killing the launcher and transport leaves the same shell PID and cwd alive; reconnect reaches that shell.

### Step 3: hardened human lifecycle

- Implement `stream`, `attach`, `detach`, `sessions`, `stop`, and the first `doctor` probes.
- Add raw-terminal restoration, resize, detach escape, active lease, takeover, and process-group stop.
- Complete registry stale-entry and PID-reuse defenses.
- Test high output, slow clients, signals, pipelines, child trees, and abrupt disconnects.

Exit gate: repeated local detach/attach and deliberate stop are deterministic and bounded.

### Step 4: public machine protocol

- Publish `docs/PROTOCOL.md` with limits, state diagrams, errors, and golden vectors.
- Implement `ssh-bridge` negotiation and machine-only output rules.
- Add protocol compatibility fixtures independent of any particular client.
- Add input sequence acknowledgement/deduplication and output replay.
- Add OpenSSH E2E that destroys TCP without graceful channel close.

Exit gate: an independent fixture can create, disconnect, reconnect, and resume through the published protocol.

### Step 5: terminal recovery

- Evaluate and record the terminal-engine decision.
- Add bounded terminal parsing, query responses, replay-gap detection, and ANSI snapshot fallback.
- Add alternate-screen, resize, malformed-escape, and query-duplication tests.
- Re-measure memory and artifact budgets with configured scrollback.

Exit gate: warm and cold reconnect recover usable shell and full-screen state without blocking the child.

### Step 6: release hardening

- Add AArch64 musl execution tests, not only cross-compilation.
- Add artifact checks, SBOM, provenance, checksums, rollback, and reproducibility notes.
- Complete hostile host-policy diagnostics and release compatibility tests.
- Reconcile implementation, protocol specification, architecture, and threat model.

Exit gate: the public prerelease meets the architecture's security, continuity, and artifact requirements.

## 7. Review discipline

Keep changes independently reviewable:

- introduce a dependency separately from unrelated behavior;
- keep protocol schema changes separate from runner-state changes where possible;
- include tests beside the invariant they establish;
- avoid generated code in protocol and lifecycle modules;
- use typed errors with stable codes at boundaries and private detail internally;
- document state transitions in code and test illegal transitions;
- do not add fallback behavior that hides a failed invariant;
- update architecture, threat model, protocol, and public CLI docs in the same change as affected behavior.

A pull request that changes framing, unsafe code, descriptor inheritance, process groups, runtime paths, or terminal parsing receives focused security review. Large phases are delivered as multiple vertical slices rather than one phase-sized pull request.

## 8. Measurements recorded from the first executable

Track these values over time for both target architectures where practical:

- stripped and compressed executable size;
- direct and transitive runtime dependency count;
- idle runner RSS at 80x24 and configured scrollback;
- runner ready latency;
- throughput while PTY output is continuously drained;
- maximum observed attachment queue and replay allocation;
- startup and steady-state open file-descriptor count.

Measurements use a checked-in benchmark method and representative host description. Regressions beyond an agreed tolerance require explanation rather than silent budget changes.
