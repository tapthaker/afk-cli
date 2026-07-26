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

The Linux binaries are static musl ELF executables and are checked for the absence of `PT_INTERP` and `DT_NEEDED`. The macOS binaries are signed with a Developer ID Application certificate, use the hardened runtime and a trusted timestamp, and are submitted to Apple's notary service. The release fails unless Apple returns an `Accepted` result and Gatekeeper accepts each signed binary.

All four binaries implement AFK's session lifecycle. Linux uses static musl artifacts; macOS uses native Mach-O artifacts for Intel and Apple Silicon. Apple does not support stapling a notarization ticket to a standalone command-line executable, so Gatekeeper retrieves the ticket by the signed binary's hash when it assesses a downloaded asset.

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
5. Developer ID signs, notarizes, and Gatekeeper-assesses both macOS binaries;
6. creates GitHub build-provenance attestations for each binary;
7. uploads each binary directly, without wrapping it in a zip or tar archive;
8. downloads and verifies the complete asset set;
9. generates a direct SPDX JSON SBOM;
10. uploads `SHA256SUMS` and `SBOM.spdx.json`, then publishes the draft.

A failed build leaves the release as a draft. Re-running the workflow for the same tag reuses that draft and replaces binary assets through the GitHub release API. A workflow run refuses to modify an already published release.

The manual workflow trigger accepts an existing version tag and follows the same checks. It does not create or move tags.

## macOS signing configuration

The repository must have these Actions secrets before a release is run:

- `APPLE_DEVELOPER_ID_P12_BASE64`: base64-encoded PKCS #12 export containing one Developer ID Application certificate and private key;
- `APPLE_DEVELOPER_ID_P12_PASSWORD`: password protecting that PKCS #12 export;
- `APPLE_NOTARY_KEY_P8_BASE64`: base64-encoded App Store Connect API private key;
- `APPLE_NOTARY_KEY_ID`: API key ID;
- `APPLE_NOTARY_ISSUER_ID`: API issuer ID.

Store only the Developer ID Application identity in the PKCS #12 file. The workflow imports it into an ephemeral keychain, requires exactly one matching identity, and deletes the keychain and decoded credentials after signing. The API key should be dedicated to release automation and granted only the access needed to submit software for notarization.

To encode files without line wrapping on macOS:

```bash
base64 -i DeveloperIDApplication.p12 | tr -d '\n'
base64 -i AuthKey_KEYID.p8 | tr -d '\n'
```

Configure these values as repository Actions secrets; never commit signing credentials. A missing credential, rejected notarization, or failed Gatekeeper assessment leaves the GitHub release in draft state.

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
