# Contributing to AFK CLI

AFK CLI is security-sensitive infrastructure: it owns a remote PTY, forwards untrusted terminal bytes, and accepts bounded local IPC. Correctness, bounded resource use, and reviewability take priority over feature speed.

## Current phase

The project is in early implementation following its initial architecture and threat-model review. Design feedback is welcome through focused issues or pull requests. Proposals must describe behavior through the public CLI and documented process model rather than assumptions about a particular client integration.

## Contribution workflow

1. Open an issue for IPC, security, process-lifecycle, dependency, or CLI-contract changes.
2. Explain the user problem and failure modes before proposing code.
3. Keep pull requests narrow.
4. Add tests for success, disconnect, malformed input, and resource-limit behavior as applicable.
5. Update architecture, threat model, or changelog when behavior changes.
6. Obtain at least one review independent from the author.

## Required checks

Acceptance-tooling changes are expected to pass:

```bash
python3 -m unittest discover -s tests/acceptance -p 'test_*.py'
```

Once the Rust package is present, changes are expected to pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo deny check
```

IPC parsers, PTY code, and terminal-facing code may also require fuzz, sanitizer, integration, cross-compilation, and OpenSSH E2E checks.

## Rust toolchain

The repository pins Rust 1.85.0 and declares 1.85 as its initial minimum supported Rust version (MSRV). Toolchain or MSRV changes must be focused, documented, and validated against both Linux musl and both macOS release targets. The package uses Rust 2024 edition.

Do not weaken a check to merge a change. Fix the issue or document and review a narrowly scoped exception.

## Design review requirements

A written design note or ADR is required for changes to:

- local IPC framing, limits, or stable error codes;
- session identity or runtime filesystem layout;
- process groups, daemonization, PTY setup, or signal behavior;
- attachment ownership or terminal-mode handling;
- installation, updates, or artifact verification;
- privacy or logging;
- dependency/license policy;
- unsafe code.

The review must state:

- invariants;
- trust boundaries;
- limits;
- backward compatibility;
- failure behavior;
- tests and rollback plan.

## Dependency policy

AFK CLI aims to remain permissively licensed and easy to redistribute as one binary.

New dependencies must be:

- actively maintained or sufficiently small to audit;
- compatible with MIT OR Apache-2.0 distribution;
- justified against binary size and attack surface;
- pinned through `Cargo.lock`;
- included in license/SBOM checks.

Copyleft, source-available, non-commercial, or unclear licenses require explicit project-owner and legal review and normally will not be accepted.

## Security and privacy

- Never include real terminal recordings, credentials, private hostnames, or production paths in tests.
- Use synthetic sentinel values.
- Never place terminal bytes in diagnostics or metadata; the bounded completed-output store is the only persistence exception.
- Never persist a separate terminal-input stream, environment values, or secrets intentionally.
- Enforce input limits before allocating.
- Do not introduce shell interpolation for session IDs or IPC values.
- See `docs/THREAT_MODEL.md` and `SECURITY.md`.

Report vulnerabilities privately rather than through a public issue.

## Unsafe Rust

Unsafe Rust is denied by default. A proposed exception needs documented invariants, a safe-alternative analysis, dedicated tests, and independent review.

## Commit and pull-request quality

- Use concise imperative commit messages.
- Explain why, not only what.
- Avoid unrelated formatting or refactoring.
- Keep generated files clearly identified.
- Update public documentation in the same change as public behavior.

## License

By contributing, you agree that your contribution is licensed under either Apache-2.0 or MIT, at the recipient's option.
