# AFK CLI Threat Model

**Status:** Draft

This document defines the initial security boundary for AFK CLI. It must be updated when protocol or process-lifecycle behavior changes.

## Assets

AFK protects:

- integrity and confidentiality of terminal input/output while transported by SSH;
- continuity of the intended PTY and shell;
- isolation from other Unix users;
- integrity of session metadata and control messages;
- host resources against unbounded protocol or terminal input;
- release artifact integrity.

AFK does not persist terminal content to protect it after runner exit. Shell history and application-specific files remain governed by those applications.

## Trust boundaries

### Trusted

- the authenticated remote Unix account;
- the SSH server and transport after the client verifies the host key;
- the installed `afk` executable and its reviewed dependencies;
- operating-system PTY and Unix socket primitives.

### Untrusted

- every protocol byte received from an attachment;
- PTY output and escape sequences produced by applications;
- filesystem entries found in the runtime directory before validation;
- stale metadata and PIDs;
- terminal dimensions, names, cwd, argv, and environment-related requests;
- downloaded release artifacts before verification;
- timing and ordering around disconnect/reconnect.

### Outside the security claim

- root or host administrator compromise;
- compromise of the authenticated Unix account;
- mutually untrusted people sharing one Unix UID;
- malicious SSH server after a user explicitly trusts its key;
- host reboot persistence;
- kernel compromise;
- denial of service by the session's own child process within the user's permissions.

## Primary threats and controls

### Unauthorized local attachment

Threat: another local user connects to a session socket.

Controls:

- mode-0700 runtime directory;
- mode-0600 sockets;
- owner verification and symlink rejection;
- peer credential checks where available;
- random session IDs;
- no public listener.

### Runtime path replacement

Threat: attacker replaces metadata, lock, or socket paths through symlinks or races.

Controls:

- descriptor-relative filesystem operations where possible;
- no-follow/openat-style primitives;
- expected ownership and mode checks;
- exclusive creation;
- atomic rename for metadata;
- live socket handshake with session ID and epoch.

### PID reuse

Threat: stale metadata points at an unrelated process that reused a PID.

Controls:

- never use PID as identity;
- require matching socket handshake and session epoch;
- record/check platform process start identity where available;
- signal only through a verified live runner control path.

### Oversized or malformed frames

Threat: allocation exhaustion, integer overflow, parser confusion, or state-machine bypass.

Controls:

- fixed header and checked arithmetic;
- enforce length before allocation;
- explicit per-kind and aggregate limits;
- reject invalid state transitions;
- fuzz frame and payload decoders;
- bounded error responses.

### Slow or disconnected attachment

Threat: client backpressure blocks PTY draining and freezes the shell.

Controls:

- bounded per-attachment queue;
- PTY draining independent from client writes;
- disconnect `ClientTooSlow` attachments;
- bounded replay/snapshot recovery.

### Duplicate input after reconnect

Threat: uncertain delivery causes a command or keystroke sequence to be sent twice.

Controls:

- stable client ID;
- monotonic input sequence;
- runner-side deduplication and acknowledgement;
- client retries only unacknowledged frames.

This prevents transport-level duplicates but cannot make arbitrary shell activity transactional.

### Output gap or reordering

Threat: client renders live output before its recovered baseline or silently misses bytes.

Controls:

- monotonic output sequence;
- contiguous acknowledgements;
- replay only when complete range remains buffered;
- atomic snapshot baseline sequence;
- live output strictly after baseline;
- epoch invalidates stale sequence state.

### Terminal escape-sequence abuse

Threat: malicious child output exploits parser bugs, creates unbounded state, or causes unsafe terminal behavior.

Controls:

- maintained permissively licensed parser;
- bounds on OSC/DCS/APC and snapshot state;
- terminal parser fuzzing;
- raw bytes never used as logs or shell commands;
- mobile/human terminal receives only protocol-authorized output and snapshots.

Terminal emulators inherently process untrusted escape sequences; parser quality is security-critical.

### Query response duplication

Threat: both runner and attached terminal answer a terminal query, corrupting application input.

Controls:

- negotiated `runner_answers_terminal_queries` capability;
- runner is authoritative terminal endpoint;
- clients suppress local query responses in that mode;
- ordering tests around attach and snapshot.

### Shell injection

Threat: session name, cwd, argv, or protocol value enters a shell command string.

Controls:

- machine SSH command is fixed;
- create fields arrive through binary framing;
- child starts through argv APIs;
- no `sh -c` for user-provided values;
- cwd validated as a path;
- bounded argv count and element length.

### Wrong-session fallback

Threat: failed attach silently starts a new shell and user executes commands believing the old process resumed.

Controls:

- attach and create are distinct typed operations;
- no automatic attach-to-create fallback;
- epoch and process metadata shown after attach;
- explicit user action required for a replacement session.

### Attachment takeover abuse

Threat: stale or unrelated client steals control.

Controls:

- one controlling lease;
- explicit takeover policy;
- lease generation invalidates old client;
- same-client reconnect identity used only as a usability signal, not a replacement for Unix/SSH authentication;
- human CLI requires `--takeover` by default.

### Process-group escape

Threat: stop kills unrelated processes or fails to terminate the intended tree.

Controls:

- runner creates and tracks PTY process group;
- verify group identity before signals;
- signal group rather than arbitrary PIDs;
- bounded TERM-to-KILL escalation;
- integration tests with pipelines and descendants.

### Runner killed with SSH login

Threat: host policy kills detached processes, violating continuity expectations.

Controls:

- detach before attachment;
- checked `setsid`/daemonization;
- `doctor` survival probe;
- detect known login/cgroup policies;
- report unsupported rather than claim continuity;
- optional user-service strategy only after separate review.

### Malicious release artifact

Threat: installer executes tampered binary.

Controls:

- HTTPS transport is not sufficient by itself;
- pinned release manifest and SHA-256;
- signed provenance/attestation;
- public CI builds;
- SBOM and license inventory;
- atomic activation and rollback;
- machine-readable self-check before use.

### Sensitive logs

Threat: terminal input/output or environment secrets enter diagnostics.

Controls:

- no telemetry;
- no terminal bytes in logs;
- stable metadata-only errors;
- bounded owner-only local diagnostics;
- tests with sentinel secrets;
- review all debug formatting for protocol payloads.

## Resource limits

All limits are part of the security contract and must have tests:

- frame payload;
- aggregate snapshot;
- replay bytes and chunk count;
- attachment queue;
- number of remembered client input sequences;
- session name, path, argv count, and argv element length;
- terminal rows, columns, scrollback, title, OSC/DCS payload;
- completed-session retention;
- stop grace period;
- metadata and diagnostic file size.

## Unsafe code

Unsafe Rust is denied by default. Any exception requires:

- a dedicated module;
- documented preconditions and invariants;
- explanation of why a safe crate/API is insufficient;
- platform-specific tests;
- independent review;
- fuzz or sanitizer coverage where applicable.

PTY/process operations may require low-level Unix APIs, but that does not justify broad unsafe scope.

## Security review gates

Before a prerelease:

- frame decoder fuzz target runs in CI or scheduled CI;
- runtime path race/symlink tests exist;
- SSH disconnect E2E proves shell survival;
- slow-client test proves bounded memory and live shell;
- sentinel-secret tests cover logs and metadata;
- dependency audit and license check pass;
- terminal parser choice receives explicit security review;
- release checksums and provenance are verifiable.

Before protocol stability:

- protocol specification is complete;
- compatibility and downgrade behavior are tested;
- independent reviewer signs off on state machine and limits;
- threat model is reconciled against implementation.

## Reporting

Do not open a public issue for a suspected vulnerability before maintainers have had a reasonable opportunity to respond. Follow the repository's `SECURITY.md` process.
