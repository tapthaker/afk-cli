# AFK CLI

AFK keeps a remote terminal process alive across SSH disconnections.

```text
SSH connection -> afk attachment -> persistent session runner -> PTY -> shell
```

When SSH drops, the attachment ends but the runner and shell continue. Reconnect over SSH and attach to the same session.

```bash
SESSION_ID=0123456789abcdef0123456789abcdef
ssh -t host.example afk stream "$SESSION_ID"
# Connection is interrupted.
ssh -t host.example afk attach "$SESSION_ID"
```

## Project status

AFK CLI is an early Linux-first implementation undergoing security and lifecycle hardening. It is not ready for production use yet.

The repository produces a self-contained executable named **`afk`**. It does not require a hosted service, open a network port, or run a machine-wide daemon. One user-owned runner process exists for each persistent terminal session.

AFK CLI works through ordinary SSH PTY channels and does not depend on a particular SSH client or private integration.

The Rust executable implements the session lifecycle on Linux and preserves side-effect-free help and version paths.

## Commands

```bash
afk stream SESSION_ID [-- COMMAND [ARG...]]
afk attach SESSION_ID
afk sessions [--json]
afk stop SESSION_ID
```

With no command, `stream` starts the account's default interactive shell. A command after `--` is executed directly as argv without an intermediate shell. Each live `attach` first replays up to the last 256 KiB of raw terminal output. For 24 hours after observed process completion, a later `attach` can print that retained output and report the exit status.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Implementation plan](docs/IMPLEMENTATION_PLAN.md)
- [Linux runtime spike results](docs/SPIKE_RESULTS.md)
- [Acceptance tests](tests/acceptance/README.md)
- [Threat model](docs/THREAT_MODEL.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Core constraints

- SSH remains responsible for host verification, user authentication, encryption, and integrity.
- AFK exposes no TCP or UDP listener.
- AFK stores at most the last 256 KiB of terminal output after observed process completion; it does not store a separate input stream.
- The PTY is always drained so a disconnected client cannot block the shell.
- Live reattachment replays the bounded raw tail but does not reconstruct terminal screen state.
- Local IPC records and attachment queues are bounded.
- Reattachment never silently creates a replacement shell.
- No telemetry, hosted AFK dependency, or dependency on a particular SSH client.

## Open source

AFK CLI is dual-licensed under either:

- Apache License, Version 2.0; or
- MIT License.

Contributions are accepted under those same terms. See `LICENSE-APACHE` and `LICENSE-MIT`.
