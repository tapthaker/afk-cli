import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_targets_and_direct_assets_are_explicit(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        for target in (
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
        ):
            self.assertIn(target, workflow)

        for asset in (
            "afk-linux-x86_64-musl",
            "afk-linux-aarch64-musl",
            "afk-macos-x86_64",
            "afk-macos-aarch64",
        ):
            self.assertIn(asset, workflow)

        self.assertIn('gh release upload "$RELEASE_TAG" "dist/${{ matrix.asset }}"', workflow)
        self.assertNotIn("actions/upload-artifact", workflow)
        self.assertNotIn('gh release upload "$RELEASE_TAG" "$archive_path"', workflow)
        self.assertNotIn(".tar.gz", workflow)

    def test_release_is_drafted_verified_and_attested_before_publish(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("--draft", workflow)
        self.assertIn("--verify-tag", workflow)
        self.assertIn("actions/attest-build-provenance@", workflow)
        self.assertIn("SHA256SUMS", workflow)
        self.assertIn("SBOM.spdx.json", workflow)
        self.assertIn("upload-artifact: false", workflow)
        self.assertIn('--draft=false', workflow)

    def test_macos_assets_are_developer_id_signed_and_notarized(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        for secret in (
            "APPLE_DEVELOPER_ID_P12_BASE64",
            "APPLE_DEVELOPER_ID_P12_PASSWORD",
            "APPLE_NOTARY_KEY_P8_BASE64",
            "APPLE_NOTARY_KEY_ID",
            "APPLE_NOTARY_ISSUER_ID",
        ):
            self.assertIn(f"secrets.{secret}", workflow)

        self.assertIn("Developer ID Application:", workflow)
        self.assertIn("--options runtime", workflow)
        self.assertIn("--timestamp", workflow)
        self.assertIn("notarytool submit", workflow)
        self.assertIn("--wait", workflow)
        self.assertIn('result.get("status") != "Accepted"', workflow)
        self.assertIn("spctl --assess --type execute", workflow)
        self.assertNotIn("--sign -", workflow)
        self.assertNotIn("--timestamp=none", workflow)


if __name__ == "__main__":
    unittest.main()
