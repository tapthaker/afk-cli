# AFK CLI

**Keep a remote shell running when your SSH connection does not.**

AFK CLI is a small remote-host companion for persistent SSH sessions. Start a shell or command through `afk`, lose Wi-Fi, close your laptop, or let a mobile connection drop, then reconnect and attach to the same process.

It was created primarily for the **AFK mobile app**. The app connects directly to your server over SSH and is designed to invoke AFK CLI on that server to preserve the remote process between connections. There is no AFK relay in the middle, and ordinary SSH use does not require an AFK account or hosted service.

AFK CLI is not tied to the AFK app. Any SSH client that can run a remote command can use it:

```bash
SESSION_ID="$(openssl rand -hex 16)"

ssh -t host.example afk stream "$SESSION_ID"
# The SSH connection is interrupted, but the remote shell keeps running.
ssh -t host.example afk attach "$SESSION_ID"
```

> [!WARNING]
> AFK CLI is an early Linux and macOS implementation undergoing security and lifecycle hardening. It is not ready for production use yet.

## Why AFK CLI exists

A normal SSH terminal owns a remote PTY. When that SSH connection disappears, the PTY usually closes and the shell—and anything running inside it—may receive a hangup and exit.

That is especially frustrating on a phone, where switching networks, backgrounding the app, or briefly losing connectivity is routine. Reconnecting SSH is easy; recovering the exact remote process is the missing piece.

AFK CLI provides that piece without replacing SSH:

- **SSH still handles trust, authentication, encryption, and transport.**
- **AFK keeps the remote PTY and process alive.**
- **A later SSH connection attaches to that same process.**

Use AFK CLI when you want process continuity without adopting a full terminal multiplexer or a second network protocol.

## How it works

```text
AFK app or another SSH client
            |
            | ordinary verified SSH
            v
      short-lived afk attachment
            |
            | owner-only Unix socket
            v
      per-session afk runner
            |
            | PTY
            v
        shell or command
```

`afk stream` creates one runner, one PTY, and one shell or command for a session ID. The attachment belongs to the current SSH connection; the runner does not.

When SSH disconnects:

1. the attachment ends;
2. the runner continues draining the PTY so the process cannot block on terminal output;
3. up to 256 KiB of recent raw output remains available in memory;
4. a new `afk attach` reconnects to the runner and receives that tail before live output.

When the process finishes, AFK stores only the final bounded output tail and safe completion metadata for 24 hours. It does not persist a separate input log, argv, environment values, or a reconstructed terminal screen.

There is no shared AFK server and no machine-wide daemon. Each live session has its own user-owned runner, and that runner exits when the session completes.

## Why use it

AFK CLI is a good fit when you want:

- simple resume behavior in the AFK app or another SSH client;
- an SSH-only solution with no TCP or UDP listener of its own;
- one focused PTY session instead of windows, panes, or a multiplexer UI;
- a small standalone remote binary;
- bounded replay rather than an unbounded terminal recording;
- no hosted backend, account, telemetry, or client lock-in.

The release artifacts are designed for easy per-user installation:

- Linux releases are statically linked musl executables;
- macOS releases link only Apple-provided system libraries;
- no AFK-specific shared libraries, package runtime, service unit, or privileged installation is required.

Put the matching `afk` binary somewhere on the remote host's `PATH`, such as `~/.local/bin/afk`:

```bash
mkdir -p "$HOME/.local/bin"
install -m 0755 ./afk-linux-x86_64-musl "$HOME/.local/bin/afk"
afk --version
```

Release checksums, provenance attestations, supported asset names, and verification steps are documented in [docs/RELEASE.md](docs/RELEASE.md).

## Commands

```bash
afk stream SESSION_ID [-- COMMAND [ARG...]]
afk attach SESSION_ID
afk sessions [--json]
afk stop SESSION_ID
```

Session IDs are exactly 32 lowercase hexadecimal characters. The client chooses the ID before creating a session; the AFK app is intended to manage this automatically.

### Start and attach

```bash
SESSION_ID="$(openssl rand -hex 16)"
ssh -t host.example afk stream "$SESSION_ID"
```

With no command, `stream` starts the account's default shell. To run a specific command, place its argv after `--`:

```bash
ssh -t host.example afk stream "$SESSION_ID" -- htop
```

`stream` never replaces an existing live or recently completed session with the same ID.

### Reattach

```bash
ssh -t host.example afk attach "$SESSION_ID"
```

`attach` never creates a replacement shell. If the session is live, it replays the bounded raw tail and continues with live terminal I/O. If the process has completed recently, it prints the retained tail and reports the recorded exit status.

### List and stop sessions

```bash
ssh host.example afk sessions
ssh host.example afk sessions --json
ssh host.example afk stop "$SESSION_ID"
```

## How is this different from tmux, screen, shpool, cmux, or Mosh?

AFK CLI is intentionally narrower than most terminal-session tools.

| Tool | Primary job | How AFK CLI differs |
| --- | --- | --- |
| **tmux / GNU Screen** | Server-side terminal multiplexing with windows, panes, keybindings, and a terminal screen model | AFK owns one PTY process per session. It has no panes, windows, prefix keys, or screen reconstruction. Use tmux or Screen when you want a full remote workspace; use AFK when you only need a process to survive SSH loss. |
| **shpool** | Persistent shells that can be detached and reattached | This is the closest comparison. AFK avoids a shared pool daemon: every session has an independent runner. AFK is also packaged around direct standalone binaries and bounded raw-tail replay for SSH clients. |
| **cmux and local terminal workspace tools** | Organizing local terminals, tabs, panes, and workflows | They improve the client-side workspace but do not by themselves keep a process alive on a remote host after SSH disappears. They can be used together with AFK CLI. |
| **Mosh** | A roaming remote-terminal protocol with UDP transport and predictive local echo | AFK does not replace SSH or open UDP ports. Reconnection establishes ordinary SSH again and then reattaches. AFK is simpler for SSH-only hosts, but it does not provide Mosh-style roaming or predictive echo. |

AFK CLI may also complement these tools. For example, AFK can preserve a command that happens to be `tmux`, but if tmux already solves your workflow, you may not need both.

## What AFK CLI deliberately does not do

AFK CLI is not a terminal emulator or a general session manager. It does not provide:

- windows, panes, or rendered scrollback;
- exact terminal-screen reconstruction;
- a TCP or UDP listener;
- network roaming or predictive local echo;
- survival across a host reboot;
- a machine-wide service;
- protection from host policy that kills all user processes after logout.

Live reattachment replays raw PTY bytes, so some output may be duplicated and a full-screen application may not redraw exactly as it looked before disconnection. AFK preserves the process, not a pixel-perfect terminal snapshot.

## Security and privacy boundaries

- SSH owns host verification, authentication, encryption, and integrity.
- Runtime directories, sockets, metadata, and retained output are owner-only.
- Local IPC records, queues, paths, terminal dimensions, and retained output are explicitly bounded.
- A disconnected or slow attachment cannot stop AFK from draining the PTY.
- Terminal bytes never enter diagnostics or metadata.
- Only the final 256 KiB output tail is written after observed process completion and retained for 24 hours.
- AFK has no telemetry and does not require a hosted AFK service.

See the [architecture](docs/ARCHITECTURE.md) and [threat model](docs/THREAT_MODEL.md) for the complete design and limitations.

## Releases and supported hosts

Version tags publish direct binaries for:

```text
afk-linux-x86_64-musl
afk-linux-aarch64-musl
afk-macos-x86_64
afk-macos-aarch64
```

All four artifacts implement the same session lifecycle. Releases include SHA-256 checksums, an SPDX SBOM, and GitHub build-provenance attestations.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Threat model](docs/THREAT_MODEL.md)
- [Release process and artifact verification](docs/RELEASE.md)
- [Implementation plan](docs/IMPLEMENTATION_PLAN.md)
- [Runtime spike and platform validation results](docs/SPIKE_RESULTS.md)
- [Acceptance tests](tests/acceptance/README.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Open source

AFK CLI is dual-licensed under either:

- Apache License, Version 2.0; or
- MIT License.

Contributions are accepted under those same terms. See `LICENSE-APACHE` and `LICENSE-MIT`.
