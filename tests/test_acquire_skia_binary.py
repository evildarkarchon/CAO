"""Public-contract tests for authenticated rust-skia binary acquisition."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
ACQUIRER = REPOSITORY_ROOT / "tools" / "acquire_skia_binary.py"
SPEC = importlib.util.spec_from_file_location("acquire_skia_binary", ACQUIRER)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load Skia acquisition tool")
ACQUIRE_SKIA_BINARY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ACQUIRE_SKIA_BINARY)


class AcquireSkiaBinaryTests(unittest.TestCase):
    """Exercise cache placement, identity, and transfer bounds."""

    def run_acquirer(
        self, *arguments: str, environment: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        """Run the public acquisition CLI with captured output.

        Raises:
          OSError: The Python interpreter cannot launch the tool.
        """
        return subprocess.run(
            [sys.executable, str(ACQUIRER), *arguments],
            cwd=REPOSITORY_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_cache_root_inside_repository_is_rejected(self) -> None:
        """Repository contents cannot be used as a machine-local binary cache."""
        result = self.run_acquirer(
            "--cache-root", str(REPOSITORY_ROOT / "target/cache"), "--offline"
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("overlaps the repository", result.stderr)

    def test_cache_root_overlapping_cargo_home_is_rejected(self) -> None:
        """Cargo home and the authenticated native cache must remain isolated."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            cargo_home = Path(temporary_directory) / "cargo-home"
            environment = os.environ.copy()
            environment["CARGO_HOME"] = str(cargo_home)

            result = self.run_acquirer(
                "--cache-root",
                str(cargo_home / "cao-cache"),
                "--offline",
                environment=environment,
            )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("overlaps Cargo home", result.stderr)

    def test_cache_identity_is_derived_from_the_reviewed_lock(self) -> None:
        """CI can derive its exact cache key without duplicating lock fields."""
        result = self.run_acquirer("--print-cache-key")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.strip(),
            "0.99.0-9e6c3d1da63ae202bff9938329ccaf81afc24acb4193aec15d6f0aac72a5960f",
        )

    def test_download_rejects_more_than_the_locked_byte_count(self) -> None:
        """A response cannot consume disk beyond the authenticated archive size."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source = root / "source.bin"
            destination = root / "cache/archive.bin"
            source.write_bytes(b"12345")

            with self.assertRaises(ACQUIRE_SKIA_BINARY.AcquisitionError):
                ACQUIRE_SKIA_BINARY.download_archive(
                    destination, source.as_uri(), expected_size=4
                )

            self.assertFalse(destination.exists())
            self.assertEqual(list(destination.parent.glob("*.tmp")), [])


if __name__ == "__main__":
    unittest.main()
