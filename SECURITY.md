# Security Policy

## Supported versions

AFK CLI has not published a stable release. Security fixes currently apply to the latest development branch.

A supported-version table and backport policy will be added before the first stable release.

## Reporting a vulnerability

Please do not open a public GitHub issue for a suspected vulnerability.

Use GitHub's private vulnerability reporting for this repository:

1. Open the repository's **Security** tab.
2. Choose **Report a vulnerability**.
3. Include affected revision/version, platform, reproduction steps, impact, and any suggested mitigation.

If private vulnerability reporting is temporarily unavailable, contact the repository owner through a private channel and request a secure reporting path. Do not include exploit details in a public issue.

## Response goals

Maintainers aim to:

- acknowledge a report within 3 business days;
- provide an initial assessment within 7 business days;
- coordinate disclosure and remediation with the reporter;
- credit reporters who request attribution.

These are goals, not guarantees.

## Scope

Security-sensitive areas include:

- bounded local IPC decoding;
- runtime path and Unix socket handling;
- PTY/process-group lifecycle;
- detach/reattach ordering and output draining;
- terminal-mode restoration and resize forwarding;
- artifact installation, verification, and rollback;
- sensitive-data logging.

See `docs/THREAT_MODEL.md` for current assumptions and exclusions.
