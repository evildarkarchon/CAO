"""Public-contract tests for the Rust workspace GitHub Actions workflow."""

from __future__ import annotations

import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "rust-workspace.yml"


class RustWorkspaceWorkflowTests(unittest.TestCase):
    """Verify behavior exposed by the checked-in pull-request workflow."""

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
        acquisition_index = workflow.index("cargo fetch --locked")
        for offline_guard in (
            'CARGO_NET_OFFLINE: "true"',
            "HTTP_PROXY:",
            "HTTPS_PROXY:",
            "ALL_PROXY:",
        ):
            with self.subTest(offline_guard=offline_guard):
                self.assertLess(acquisition_index, workflow.index(offline_guard))
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
            "cargo test --package cao-domain --package cao-application --lib --frozen",
            commands,
        )
        self.assertNotIn("CAO_RUN_RELEASE_STAGING_TEST", workflow)
        self.assertNotIn("tools/build_workspace.py", workflow)


if __name__ == "__main__":
    unittest.main()
