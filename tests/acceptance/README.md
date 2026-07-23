# Acceptance Tests

Acceptance tests exercise AFK CLI at process, protocol, SSH, and release-artifact boundaries. Unit tests remain responsible for individual parsers and state transitions.

The Rust package does not exist yet, so only the artifact dependency checker and its self-tests are currently executable. Product tests are added with the implementation slice that makes each criterion meaningful; they must not be committed as skipped placeholders.

## Test harness rules

- Give every test an isolated temporary `HOME` and AFK runtime root.
- Use synthetic commands and sentinel values; never capture real terminal data or credentials.
- Apply explicit per-operation and whole-test timeouts.
- Track every spawned PID and remove every runtime path during teardown, including failed tests.
- Treat abrupt descriptor close and killed transport as normal test inputs.
- Assert bounded outcomes rather than relying on sleeps alone.
- Keep protocol clients in the fixture independent from production client code.
- Do not print PTY bytes when a test fails; report sequence numbers, lengths, and stable states only.

Shared Rust fixture code will live under `tests/support/`. Linux-only tests should report an explicit unsupported platform rather than silently passing.

## Acceptance matrix

### CLI and startup

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| CLI-001 | `afk --version` exits successfully without creating runtime files. | Cargo integration | Step 0 |
| CLI-002 | Invalid arguments return a documented nonzero exit code and bounded stderr. | Cargo integration | Step 1 |
| START-001 | The launcher reports success only after the runner socket, PTY, and child are ready. | Linux process integration | Step 2 |
| START-002 | A startup failure returns a typed error and leaves no live runner or stale socket. | Linux process integration | Step 2 |
| START-003 | Retrying an uncertain create with the same session ID reaches one shell, not two. | Linux process integration | Step 3 |

### Process continuity and lifecycle

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| PROC-001 | Closing the launcher and attachment descriptors does not change the shell PID, cwd, or nonce. | Linux process integration | Step 2 |
| PROC-002 | Continuous PTY output while detached does not block the child. | Linux process integration | Step 2 |
| PROC-003 | A full attachment queue detaches the slow client while the child keeps producing output. | Linux process integration | Step 3 |
| PROC-004 | `detach` keeps the process group alive; `stop` terminates it after bounded TERM/KILL handling. | Linux process integration | Step 3 |
| PROC-005 | Stop handles pipelines and descendants without signaling an unrelated process group. | Linux process integration | Step 3 |
| PROC-006 | Completed retention expires and removes socket, lock, and metadata without persisting terminal bytes. | Linux process integration | Step 3 |

### Runtime filesystem

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| FS-001 | Runtime root and per-session files have expected ownership and restrictive modes. | Linux filesystem integration | Step 1 |
| FS-002 | Symlinked roots, metadata, locks, and sockets are rejected without modifying their targets. | Linux filesystem integration | Step 1 |
| FS-003 | A stale PID alone cannot authorize cleanup or signaling. | Linux filesystem/process integration | Step 3 |
| FS-004 | Concurrent create requests for one ID produce exactly one runner. | Linux process integration | Step 3 |
| FS-005 | Metadata remains within its byte limit and excludes input, output, argv, and environment values. | Linux filesystem integration | Step 3 |

### Protocol and reconnect

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| WIRE-001 | An independent fixture negotiates every supported protocol overlap and rejects no-overlap. | Protocol integration | Step 4 |
| WIRE-002 | Truncated, oversized, overflowing, and invalid-state frames close safely with bounded allocation. | Protocol integration/fuzz | Step 1 |
| WIRE-003 | Reconnect replays each retained output sequence once and in order before live output. | Protocol/process integration | Step 4 |
| WIRE-004 | A replay gap or epoch mismatch selects a snapshot rather than incomplete replay. | Protocol/terminal integration | Step 5 |
| WIRE-005 | Retried unacknowledged input is written to the PTY once; duplicate sequence numbers are only acknowledged. | Protocol/process integration | Step 4 |
| WIRE-006 | Takeover increments lease generation and prevents the replaced attachment from sending input. | Protocol/process integration | Step 3 |
| WIRE-007 | Machine stdout contains frames only, and malformed input cannot inject diagnostics into stdout. | Bridge integration | Step 4 |

### Terminal recovery

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| TERM-001 | Cold attach restores a usable primary-screen baseline before live output. | Terminal integration | Step 5 |
| TERM-002 | Alternate-screen applications recover cursor, attributes, modes, and screen contents within configured bounds. | Terminal integration | Step 5 |
| TERM-003 | Resize establishes one ordered snapshot baseline and does not lose subsequent PTY output. | Terminal integration | Step 5 |
| TERM-004 | Runner-generated query replies continue while detached and are not duplicated after attach. | Terminal/process integration | Step 5 |
| TERM-005 | Malformed or oversized escape strings cannot exceed parser, title, scrollback, or snapshot limits. | Terminal integration/fuzz | Step 5 |

### OpenSSH disconnect

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| SSH-001 | Destroying TCP without a graceful channel close leaves the runner and shell alive. | Containerized OpenSSH E2E | Step 4 |
| SSH-002 | A new TCP connection attaches to the same session ID and epoch, PID, cwd, and nonce. | Containerized OpenSSH E2E | Step 4 |
| SSH-003 | TCP destruction during high output, resize, and alternate-screen activity remains recoverable. | Containerized OpenSSH E2E | Step 5 |
| SSH-004 | A host policy that kills login processes is detected and reported rather than treated as continuity. | Container/system policy E2E | Step 6 |

### Release artifacts and dynamic-library dependencies

| ID | Criterion | Layer | First required step |
| --- | --- | --- | --- |
| ART-001 | The final x86-64 artifact is an ELF file for the expected architecture. | Artifact acceptance | Step 0 |
| ART-002 | The final AArch64 artifact is an ELF file for the expected architecture. | Artifact acceptance | Step 6 |
| ART-003 | Each Linux musl artifact has no `PT_INTERP` program header and no `DT_NEEDED` entries. | Artifact acceptance | Step 0/6 |
| ART-004 | Dynamic dependency inventory is emitted in machine-readable form, including an empty `needed` list for static artifacts. | Artifact acceptance | Step 0 |
| ART-005 | The unpacked and compressed executable remain within documented size budgets. | Artifact acceptance | Step 0 onward |
| ART-006 | A clean minimal image can execute the artifact without installing runtime libraries. | Container/native execution | Step 0/6 |
| ART-007 | The release artifact passes version/protocol self-check and matches its manifest checksum. | Artifact acceptance | Step 6 |
| ART-008 | SBOM and license inventory correspond to the locked source dependency graph. | Release acceptance | Step 6 |

For Linux musl, “static” means no dynamic loader and no dynamically loaded dependencies, including system libraries. `cargo tree` and an SBOM describe source dependencies but do not prove what the linker placed in the executable.

`check_static_elf.py` uses `readelf` program and dynamic headers instead of `ldd`. This works for cross-architecture artifacts, avoids depending on a target loader, and provides the exact interpreter and `DT_NEEDED` inventory on failure.

Run the checker self-tests:

```bash
python3 -m unittest discover -s tests/acceptance -p 'test_*.py'
```

Inspect a built artifact:

```bash
python3 tests/acceptance/check_static_elf.py \
  target/x86_64-unknown-linux-musl/release/afk

python3 tests/acceptance/check_static_elf.py --json \
  target/aarch64-unknown-linux-musl/release/afk
```

Set `READELF=llvm-readelf` or pass `--readelf` when GNU `readelf` is unavailable.

The checker has negative fixtures for both `PT_INTERP` and multiple `DT_NEEDED` entries. CI must run those self-tests before trusting the artifact result.

## Execution tiers

### Pull requests

- Rust unit and Cargo integration tests;
- artifact-checker self-tests;
- x86-64 musl build and ART-001, ART-003, ART-004, and ART-005;
- relevant malformed-input and disconnect tests for changed boundaries.

### Scheduled

- fuzz targets;
- high-output and slow-client stress cases;
- OpenSSH abrupt-TCP tests;
- terminal conformance suite;
- sanitizer runs for any reviewed unsafe module.

### Release

- all acceptance IDs required by the implemented phase;
- x86-64 and AArch64 static dependency inspection;
- native or emulated execution in clean minimal images;
- artifact size, RSS, startup, SBOM, checksum, provenance, install, and rollback checks.

Every failed acceptance test must retain bounded lifecycle diagnostics without retaining terminal input or output.
