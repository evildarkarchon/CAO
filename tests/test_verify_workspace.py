"""Public-boundary tests for the Rust workspace contract validator."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
VALIDATOR = REPOSITORY_ROOT / "tools" / "verify_workspace.py"
STAGER = REPOSITORY_ROOT / "tools" / "stage_release.py"


class VerifyWorkspaceTests(unittest.TestCase):
  """Exercise workspace validation only through its command-line interface."""

  def run_validator(self, root: Path) -> subprocess.CompletedProcess[str]:
    """Run the public workspace validator against a repository-shaped root.

    Raises:
      OSError: The Python interpreter cannot launch the validator process.
    """
    return subprocess.run(
      [sys.executable, str(VALIDATOR), "--root", str(root)],
      cwd=REPOSITORY_ROOT,
      capture_output=True,
      text=True,
      check=False,
    )

  def copy_contract_root(self, destination: Path) -> None:
    """Copy only the files governed by workspace contract validation.

    Raises:
      OSError: A required contract file cannot be copied into the sandbox.
    """
    destination.mkdir()
    for file_name in (
      "Cargo.lock",
      "Cargo.toml",
      "rust-toolchain.toml",
    ):
      shutil.copy2(REPOSITORY_ROOT / file_name, destination / file_name)
    shutil.copytree(REPOSITORY_ROOT / ".cargo", destination / ".cargo")
    shutil.copytree(REPOSITORY_ROOT / "apps", destination / "apps")
    shutil.copytree(REPOSITORY_ROOT / "crates", destination / "crates")
    shutil.copytree(
      REPOSITORY_ROOT / "tools/cao-verification",
      destination / "tools/cao-verification",
    )
    shutil.copytree(
      REPOSITORY_ROOT / "tools/cao-oracle-capture",
      destination / "tools/cao-oracle-capture",
    )
    shutil.copy2(
      REPOSITORY_ROOT / "tools/stage_release.py",
      destination / "tools/stage_release.py",
    )
    (destination / "verification/build-inputs").mkdir(parents=True)
    shutil.copy2(
      REPOSITORY_ROOT / "verification/build-inputs/skia-source-lock.json",
      destination / "verification/build-inputs/skia-source-lock.json",
    )
    shutil.copytree(
      REPOSITORY_ROOT / "vendor/ba2-3.0.1",
      destination / "vendor/ba2-3.0.1",
    )
    shutil.copytree(
      REPOSITORY_ROOT / "vendor/serde-hkx",
      destination / "vendor/serde-hkx",
    )

  def test_committed_workspace_contract_passes(self) -> None:
    """Accept the exact production, verification, and oracle package graph."""
    result = self.run_validator(REPOSITORY_ROOT)

    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
    self.assertIn("Rust workspace contract verified", result.stdout)

  def test_internal_workspace_dependency_must_remain_exact(self) -> None:
    """Reject a broad first-party version even when its path remains unchanged."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      manifest = root / "Cargo.toml"
      content = manifest.read_text(encoding="utf-8")
      content = content.replace(
        'cao-domain = { version = "=0.0.0",',
        'cao-domain = { version = "0.0.0",',
        1,
      )
      manifest.write_text(content, encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("cao-domain dependency field version", result.stderr)

  def test_root_dependency_patch_must_be_rejected(self) -> None:
    """Reject a root Cargo patch that redirects a locked production package."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      manifest = root / "Cargo.toml"
      content = manifest.read_text(encoding="utf-8")
      manifest.write_text(
        content
        + '\n[patch.crates-io]\ncrossbeam-channel = { path = "unreviewed" }\n',
        encoding="utf-8",
      )

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("root manifest top-level tables", result.stderr)

  def test_skia_source_lock_must_remain_byte_exact(self) -> None:
    """Reject a native source closure that differs from the reviewed lock."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      source_lock = root / "verification/build-inputs/skia-source-lock.json"
      content = source_lock.read_text(encoding="utf-8")
      source_lock.write_text(
        content.replace("m150-0.98.1", "m150-unreviewed", 1),
        encoding="utf-8",
      )

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("Skia source lock SHA-256", result.stderr)

  def test_target_specific_dependency_must_be_rejected(self) -> None:
    """Reject dependency edges hidden behind a Windows target table."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      manifest = root / "crates/cao-domain/Cargo.toml"
      content = manifest.read_text(encoding="utf-8")
      manifest.write_text(
        content + '\n[target."cfg(windows)".dependencies]\nserde = "1"\n',
        encoding="utf-8",
      )

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("manifest top-level tables", result.stderr)

  def test_implicit_binary_target_must_be_rejected(self) -> None:
    """Reject a source file that could become an undeclared Cargo target."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      rogue_binary = root / "crates/cao-domain/src/bin/rogue.rs"
      rogue_binary.parent.mkdir()
      rogue_binary.write_text("fn main() {}\n", encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("undeclared Rust target sources", result.stderr)

  def test_vendored_source_tree_must_remain_byte_exact(self) -> None:
    """Reject modified Rust bytes in an authenticated path dependency."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory) / "repository"
      self.copy_contract_root(root)
      source = root / "vendor/ba2-3.0.1/src/lib.rs"
      content = source.read_text(encoding="utf-8")
      source.write_text(content + "\n// Unreviewed source mutation.\n", encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(result.returncode, 0)
      self.assertIn("authenticated source tree SHA-256", result.stderr)

  @unittest.skipUnless(
    os.environ.get("CAO_RUN_RELEASE_STAGING_TEST") == "1",
    "set CAO_RUN_RELEASE_STAGING_TEST=1 with the offline Skia inputs",
  )
  def test_release_staging_contains_only_gui_and_helper(self) -> None:
    """Build offline and stage only the two production composition roots."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      output = Path(temporary_directory) / "staged"
      result = subprocess.run(
        [sys.executable, str(STAGER), "--output", str(output)],
        cwd=REPOSITORY_ROOT,
        capture_output=True,
        text=True,
        check=False,
      )

      self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
      staged_files = {
        path.relative_to(output).as_posix()
        for path in output.rglob("*")
        if path.is_file()
      }
      self.assertEqual(
        staged_files,
        {"tracetide.exe", "bin/tracetide-hkx-helper.exe"},
      )


if __name__ == "__main__":
  unittest.main()
