# AFK CLI

AFK keeps a remote terminal process alive across SSH disconnections.

```text
SSH connection -> afk attachment -> persistent session runner -> PTY -> shell
```

When SSH drops, the attachment ends but the runner and shell continue. Reconnect over SSH and attach to the same session.

```bash
ssh -t host.example afk stream
# Connection is interrupted.
ssh -t host.example afk attach <session-id>
```

## Project status

AFK CLI is in architecture and security review. It is not ready for production use yet.

The repository will produce a self-contained executable named **`afk`**. It will not require a hosted service, open a network port, or run a machine-wide daemon. One user-owned runner process exists for each persistent terminal session.

AFK CLI is client-agnostic. Its human CLI and public wire protocol are intended to be complete integration surfaces; no particular SSH client or unpublished integration behavior is required.

## Planned commands

```bash
afk stream [--detach] [--name NAME] [--cwd PATH] [-- COMMAND...]
afk attach SESSION [--takeover]
afk detach SESSION
afk sessions [--json]
afk stop SESSION
afk doctor [--json]
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Implementation plan](docs/IMPLEMENTATION_PLAN.md)
- [Threat model](docs/THREAT_MODEL.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

A standalone wire-protocol specification will be added before implementation is considered stable.

## Core constraints

- SSH remains responsible for host verification, user authentication, encryption, and integrity.
- AFK exposes no TCP or UDP listener.
- Terminal input and output are not stored on disk by default.
- The PTY is always drained so a disconnected client cannot block the shell.
- Replay, snapshots, queues, and protocol frames are bounded.
- Reattachment never silently creates a replacement shell.
- No telemetry, hosted AFK dependency, or dependency on a particular SSH client.

## Open source

AFK CLI is dual-licensed under either:

- Apache License, Version 2.0; or
- MIT License.

Contributions are accepted under those same terms. See `LICENSE-APACHE` and `LICENSE-MIT`.
