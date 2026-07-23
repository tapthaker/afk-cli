# Release Process

AFK publishes versioned GitHub Releases from tags matching the Cargo package version.

## Release assets

Each release contains four direct, uncompressed binary assets:

```text
afk-linux-x86_64-musl
afk-linux-aarch64-musl
afk-macos-x86_64
afk-macos-aarch64
```

`SHA256SUMS` covers those four binaries, and `SBOM.spdx.json` inventories the tagged source dependencies. GitHub may also display its automatically generated source-code archives; they are not AFK binary artifacts.

The Linux binaries are static musl ELF executables and are checked for the absence of `PT_INTERP` and `DT_NEEDED`. The macOS binaries are ad-hoc signed but are not Developer ID signed or notarized.

AFK's session lifecycle is currently Linux-only. The macOS binaries provide the side-effect-free help and version commands and report that session commands require Linux. They are published now so packaging and future platform support use stable asset names.

## Creating a release

1. Update `package.version` in `Cargo.toml` and regenerate `Cargo.lock` if needed.
2. Complete the normal validation and merge the release commit.
3. Create a tag whose name is exactly `v` followed by the package version.
4. Push the tag.

Example:

```bash
git tag -s v0.1.0 -m "AFK CLI v0.1.0"
git push origin v0.1.0
```

`.github/workflows/release.yml` then:

1. verifies the tag and package version;
2. runs formatting, lint, tests, acceptance tooling, and Cargo Deny;
3. creates a draft GitHub Release;
4. builds and verifies all four target binaries;
5. creates GitHub build-provenance attestations for each binary;
6. uploads each binary directly, without wrapping it in a zip or tar archive;
7. downloads and verifies the complete asset set;
8. generates a direct SPDX JSON SBOM;
9. uploads `SHA256SUMS` and `SBOM.spdx.json`, then publishes the draft.

A failed build leaves the release as a draft. Re-running the workflow for the same tag reuses that draft and replaces binary assets through the GitHub release API. A workflow run refuses to modify an already published release.

The manual workflow trigger accepts an existing version tag and follows the same checks. It does not create or move tags.

## Verifying a download

Linux example:

```bash
asset=afk-linux-x86_64-musl
curl -LO "https://github.com/tapthaker/afk-cli/releases/download/v0.1.0/$asset"
curl -LO "https://github.com/tapthaker/afk-cli/releases/download/v0.1.0/SHA256SUMS"
grep "  $asset$" SHA256SUMS | sha256sum --check
gh attestation verify "$asset" --repo tapthaker/afk-cli
chmod 0755 "$asset"
```

The executable bit is not represented by an HTTP release asset, so it must be set after downloading.
