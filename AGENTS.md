# AGENTS.md

Shared coding-agent guidance for AFK CLI.

## Product boundary

AFK CLI persists one user-owned PTY process across SSH disconnections.

- Binary name: `afk`
- Repository and Cargo package name: `afk-cli`
- No hosted backend or account
- No dependency on a particular SSH client or private integration
- No public wire protocol or terminal emulation
- No TCP/UDP listener
- No machine-wide daemon
- No telemetry
- No terminal input/output persisted to disk by default
- SSH owns host verification, authentication, encryption, and integrity

## Engineering rules

1. Read `docs/ARCHITECTURE.md` and `docs/THREAT_MODEL.md` before implementation.
2. Keep local IPC, queues, paths, and terminal dimensions explicitly bounded.
3. Never log terminal bytes, input, environment values, or credentials.
4. Do not interpolate session IDs or IPC values into shell commands.
5. Deny unsafe Rust by default; exceptions require documented invariants and review.
6. Keep dependencies permissively licensed and justify their size/attack surface.
7. Add malformed-input and disconnect-path tests with every IPC or lifecycle change.
8. Update public documentation with public behavior.
9. Stage files explicitly; never use `git add .` or `git add -A`.
10. Commit completed and validated code or documentation work with a concise message.

## Validation

Current acceptance-tooling checks:

```bash
python3 -m unittest discover -s tests/acceptance -p 'test_*.py'
```

Once the Rust package exists:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo deny check
```

Additional IPC fuzz, OpenSSH E2E, and artifact checks are required where applicable.
