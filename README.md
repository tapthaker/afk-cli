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

AFK CLI is in early implementation after architecture and security review. It is not ready for production use yet.

The repository produces a self-contained executable named **`afk`**. It will not require a hosted service, open a network port, or run a machine-wide daemon. Once session lifecycle is implemented, one user-owned runner process will exist for each persistent terminal session.

AFK CLI works through ordinary SSH PTY channels and does not depend on a particular SSH client or private integration.

The initial Rust executable currently provides side-effect-free help and version paths:

```bash
afk --help
afk --version
```

Session lifecycle is not implemented yet.

## Planned commands

```bash
afk stream SESSION_ID
afk attach SESSION_ID
afk sessions [--json]
afk stop SESSION_ID
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Implementation plan](docs/IMPLEMENTATION_PLAN.md)
- [Acceptance tests](tests/acceptance/README.md)
- [Threat model](docs/THREAT_MODEL.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Core constraints

- SSH remains responsible for host verification, user authentication, encryption, and integrity.
- AFK exposes no TCP or UDP listener.
- Terminal input and output are never stored on disk by AFK.
- The PTY is always drained so a disconnected client cannot block the shell.
- Output produced while detached is discarded rather than replayed.
- Local IPC records and attachment queues are bounded.
- Reattachment never silently creates a replacement shell.
- No telemetry, hosted AFK dependency, or dependency on a particular SSH client.

## Open source

AFK CLI is dual-licensed under either:

- Apache License, Version 2.0; or
- MIT License.

Contributions are accepted under those same terms. See `LICENSE-APACHE` and `LICENSE-MIT`.
