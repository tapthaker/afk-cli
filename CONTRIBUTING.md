# Contributing to AFK CLI

AFK CLI is security-sensitive infrastructure: it owns a remote PTY, parses untrusted terminal output, and accepts framed control traffic. Correctness, bounded resource use, and reviewability take priority over feature speed.

## Current phase

The project is currently reviewing architecture and threat model before implementation. Design feedback is welcome through focused issues or pull requests.

## Contribution workflow

1. Open an issue for protocol, security, process-lifecycle, dependency, or CLI-contract changes.
2. Explain the user problem and failure modes before proposing code.
3. Keep pull requests narrow.
4. Add tests for success, disconnect, malformed input, and resource-limit behavior as applicable.
5. Update architecture, protocol, threat model, or changelog when behavior changes.
6. Obtain at least one review independent from the author.

## Required checks

Once the Rust package is present, changes are expected to pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo deny check
```

Protocol parsers and terminal-facing code may also require fuzz, sanitizer, integration, cross-compilation, and OpenSSH E2E checks.

Do not weaken a check to merge a change. Fix the issue or document and review a narrowly scoped exception.

## Design review requirements

A written design note or ADR is required for changes to:

- wire framing or stable error codes;
- protocol negotiation;
- input/output acknowledgement semantics;
- session identity or takeover;
- runtime filesystem layout;
- process groups, daemonization, or signal behavior;
- terminal engine or snapshot format;
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
- Never log terminal bytes, input frames, environment values, or secrets.
- Enforce input limits before allocating.
- Do not introduce shell interpolation for protocol values.
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
