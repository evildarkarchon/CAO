"""Public-contract tests for the Rust workspace GitHub Actions workflow."""

from __future__ import annotations

import subprocess
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "rust-workspace.yml"


class RustWorkspaceWorkflowTests(unittest.TestCase):
    """Verify behavior exposed by the checked-in pull-request workflow."""

    def test_byte_authenticated_resources_are_checked_out_with_lf_bytes(self) -> None:
        """Byte-authenticated setup inputs must be stable under Windows checkout."""
        artifact_paths = (
            "verification/tracers/setup/fixture.json",
            "verification/tracers/setup/evidence.json",
            "resources/profiles/SSE/startup.state",
            "resources/profiles/built-ins.state",
        )
        result = subprocess.run(
            ["git", "check-attr", "eol", "--", *artifact_paths],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        for artifact_path in artifact_paths:
            with self.subTest(artifact_path=artifact_path):
                self.assertIn(f"{artifact_path}: eol: lf", result.stdout)

    def test_authenticated_source_inputs_are_checked_out_with_lf_bytes(self) -> None:
        """Authenticated source inputs must have checkout-independent bytes."""
        source_paths = (
            "vendor/ba2-3.0.1/src/lib.rs",
            "vendor/ba2-3.0.1/Cargo.toml.orig",
            "vendor/serde-hkx/serde_hkx/src/lib.rs",
            "verification/build-inputs/skia-source-lock.json",
            "verification/build-inputs/skia-binary-lock.json",
        )
        result = subprocess.run(
            ["git", "check-attr", "eol", "--", *source_paths],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        for source_path in source_paths:
            with self.subTest(source_path=source_path):
                self.assertIn(f"{source_path}: eol: lf", result.stdout)

        archived_manifest = source_paths[1]
        tracked_result = subprocess.run(
            ["git", "ls-files", "--error-unmatch", "--", archived_manifest],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            tracked_result.returncode,
            0,
            f"authenticated archive input is not tracked: {archived_manifest}",
        )

    def test_pull_request_validation_uses_a_hosted_acquire_then_offline_contract(
        self,
    ) -> None:
        """PR validation must be schedulable without the private release cache."""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        commands = {
            line.strip().removeprefix("run: ") for line in workflow.splitlines()
        }

        self.assertIn("runs-on: windows-2025", workflow)
        self.assertNotIn("self-hosted", workflow)
        cargo_acquisition_index = workflow.index("cargo fetch --locked")
        skia_acquisition_index = workflow.index("python tools/acquire_skia_binary.py")
        for offline_guard in (
            'CARGO_NET_OFFLINE: "true"',
            "HTTP_PROXY:",
            "HTTPS_PROXY:",
            "ALL_PROXY:",
        ):
            with self.subTest(offline_guard=offline_guard):
                self.assertLess(cargo_acquisition_index, workflow.index(offline_guard))
                self.assertLess(skia_acquisition_index, workflow.index(offline_guard))
        self.assertIn(
            "rustup toolchain install 1.97.0 --profile minimal --component clippy "
            "--component rustfmt --target x86_64-pc-windows-msvc",
            commands,
        )
        self.assertIn("rustup default 1.97.0", commands)
        self.assertIn("python tools/verify_baseline.py", commands)
        self.assertIn(
            'python tools/verify_baseline.py --verify-input "rust-toolchain=$rustc"',
            commands,
        )
        self.assertIn("python tools/verify_workspace.py", commands)
        self.assertIn(
            "cargo test --workspace --all-targets --frozen",
            commands,
        )
        self.assertIn(
            "uses: actions/cache@27d5ce7f107fe9357f9df03efb73ab90386fccae",
            workflow,
        )
        self.assertIn("python tools/acquire_skia_binary.py --print-cache-key", workflow)
        self.assertIn(
            "key: rust-skia-${{ runner.os }}-${{ steps.skia-lock.outputs.identity }}",
            workflow,
        )
        self.assertNotIn("key: rust-skia-${{ runner.os }}-0.99.0-", workflow)
        self.assertIn("SKIA_BINARIES_URL=$template", workflow)
        self.assertNotIn("restore-keys:", workflow)
        self.assertNotIn("CAO_RUN_RELEASE_STAGING_TEST", workflow)
        self.assertNotIn("tools/build_workspace.py", workflow)


if __name__ == "__main__":
    unittest.main()
