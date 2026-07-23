# Acceptance Tests

Acceptance tests prove AFK's narrow promise: the same remote shell remains alive across SSH attachment loss and can be reached again.

The current package implements CLI-001 and CLI-002 plus the release-artifact dependency checker. Product tests are added with the implementation slice that makes each criterion meaningful; skipped placeholders are not accepted.

## Harness rules

- Give every test an isolated temporary `HOME` and runtime root.
- Use synthetic commands, paths, and sentinel values.
- Apply explicit operation and whole-test timeouts.
- Track and clean up every spawned PID and runtime path.
- Treat abrupt descriptor close and killed transport as normal inputs.
- Do not print PTY or IPC payloads on failure.
- Assert PIDs, states, lengths, and exit reasons instead.

## Acceptance criteria

### CLI

| ID | Criterion | First step |
| --- | --- | --- |
| CLI-001 | `afk --version` succeeds without creating runtime files. | Step 0 |
| CLI-002 | Invalid arguments return exit code 2 and bounded stderr without echoing the argument. | Step 0 |
| CLI-003 | `stream` requires exactly one valid session ID, accepts optional argv after `--`, and rejects `--detach`. | Step 1A/2A |
| CLI-004 | Explicit command arguments, including shell metacharacters, are passed literally without `sh -c`. | Step 2A |
| CLI-005 | `attach` and `stop` reject malformed or missing session IDs. | Step 1A/3 |
| CLI-006 | A failed `attach` never creates a process or runtime entry. | Step 2B |
| CLI-007 | `stream` returns `SessionExists` for a live or retained completed ID. | Step 2A/3 |
| CLI-008 | Attaching to completed metadata prints only the completion summary and returns the recorded status. | Step 3 |

### Local IPC and runtime files

| ID | Criterion | First step |
| --- | --- | --- |
| IPC-001 | `Attach`, `Input`, `Output`, `Resize`, `Stop`, and `Exit` round-trip through the bounded codec. | Step 1A |
| IPC-002 | Every truncated header and payload is rejected. | Step 1A |
| IPC-003 | Payload lengths above 64 KiB are rejected before allocation. | Step 1A |
| IPC-004 | Unknown kinds, wrong fixed payload lengths, and invalid state transitions are rejected. | Step 1A |
| FS-001 | Runtime root and session files have expected ownership and restrictive modes. | Step 1B |
| FS-002 | Symlinked root, lock, metadata, and socket entries are rejected without modifying their targets. | Step 1B |
| FS-003 | Concurrent create requests for one ID produce one runner. | Step 1B/2A |
| FS-004 | A stale PID cannot authorize cleanup or signaling. | Step 1B/3 |
| FS-005 | Metadata stays within its limit and excludes terminal, argument, and environment contents. | Step 1B/3 |
| FS-006 | Completed metadata records finish time and exit code or signal, then expires lazily after 24 hours. | Step 1B/3 |

### Process continuity

| ID | Criterion | First step |
| --- | --- | --- |
| PROC-001 | Launcher exit leaves the single-threaded runner, PTY, and default shell or explicit command alive. | Step 2A |
| PROC-002 | Closing an attachment does not change the shell PID or cwd. | Step 2B |
| PROC-003 | Continuous output while detached does not block the child. | Step 2A |
| PROC-004 | Reattach reaches the same default-shell PID and synthetic shell variable. | Step 2B |
| PROC-005 | Reattach reaches the same PID, cwd, and inherited synthetic environment value for an explicitly supplied long-running command. | Step 2B |
| PROC-006 | Initial dimensions and later outer-PTY `SIGWINCH` updates reach the inner PTY. | Step 2B |
| PROC-007 | Forwarded `Ctrl-C` is interpreted by the inner PTY and reaches its foreground process as `SIGINT`. | Step 2B |
| PROC-008 | `SIGHUP` or `SIGTERM` ending an attachment does not signal the session process. | Step 2B |
| PROC-009 | A 1 MiB full output queue drops the slow attachment while the child continues. | Step 2B |
| PROC-010 | A new attachment replaces a stale attachment and becomes the only input owner. | Step 2B |
| PROC-011 | `stop` closes the PTY and terminates the verified child session leader without signaling an unrelated process. | Step 3 |
| PROC-012 | A signal-ignoring or `setsid` descendant is outside the best-effort stop guarantee and is cleaned up by the fixture. | Step 3 |
| PROC-013 | Process exit sends one `Exit` record, persists completion metadata, and removes the socket and lock. | Step 3 |

### OpenSSH disconnect

| ID | Criterion | First step |
| --- | --- | --- |
| SSH-001 | Destroying TCP without a graceful channel close leaves the runner and shell alive. | Step 4 |
| SSH-002 | A new SSH PTY channel attaches to the same session and shell PID. | Step 4 |
| SSH-003 | Shell cwd and a synthetic variable survive the disconnect. | Step 4 |
| SSH-004 | Detached high output does not freeze the shell before reconnect. | Step 4 |
| SSH-005 | `stop` over a later SSH connection terminates the session and cleans runtime files. | Step 4 |

AFK does not promise replay of output produced while detached. Tests must not expect screen reconstruction, output sequence acknowledgements, or exactly-once input delivery.

### Release artifacts

| ID | Criterion | First step |
| --- | --- | --- |
| ART-001 | The x86-64 artifact is an ELF file for the expected architecture. | Step 0 |
| ART-002 | The AArch64 artifact is an ELF file for the expected architecture. | Step 0 |
| ART-003 | Both musl artifacts have no `PT_INTERP` header and no `DT_NEEDED` entries. | Step 0 |
| ART-004 | Dependency inspection emits machine-readable output with an empty `needed` list. | Step 0 |
| ART-005 | Compressed artifacts remain below 15 MiB. | Step 0 onward |
| ART-006 | Clean target environments execute each artifact without installed runtime libraries. | Step 4 |
| ART-007 | Dependency advisories, licenses, bans, and sources pass policy. | Step 0 onward |

For Linux musl, static means no dynamic loader and no dynamically loaded system or third-party libraries. A Cargo dependency tree does not prove what the linker placed in the executable.

`check_static_elf.py` inspects ELF program and dynamic headers with `readelf`. It works for cross-architecture artifacts and reports the interpreter and shared libraries on failure.

Run its self-tests:

```bash
python3 -m unittest discover -s tests/acceptance -p 'test_*.py'
```

Inspect built artifacts:

```bash
python3 tests/acceptance/check_static_elf.py --json \
  target/x86_64-unknown-linux-musl/release/afk

python3 tests/acceptance/check_static_elf.py --json \
  target/aarch64-unknown-linux-musl/release/afk
```

Set `READELF=llvm-readelf` or pass `--readelf` when GNU `readelf` is unavailable.

## Execution tiers

### Pull requests

- formatting, Clippy, Rust unit and integration tests;
- acceptance-checker self-tests;
- Cargo Deny;
- both musl builds, architecture checks, dynamic-dependency checks, and size budgets;
- malformed-input and disconnect tests for changed boundaries.

### Scheduled

- IPC fuzzing;
- high-output and slow-client stress;
- OpenSSH abrupt-TCP tests;
- sanitizer runs for any reviewed unsafe module.

### Release

- every implemented acceptance criterion;
- clean execution of x86-64 and AArch64 artifacts;
- artifact size, RSS, startup, SBOM, checksum, provenance, install, and rollback checks.

Every failed test retains only bounded lifecycle diagnostics and never terminal input or output.
