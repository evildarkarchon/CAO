"""Public-boundary tests for the offline evidence baseline validator."""

from __future__ import annotations

import json
import hashlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
VALIDATOR = REPOSITORY_ROOT / "tools" / "verify_baseline.py"


class VerifyBaselineTests(unittest.TestCase):
  """Exercise baseline validation only through its command-line interface."""

  def run_validator(self, root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    """Run the public validator against a chosen repository-shaped root."""
    return subprocess.run(
      [sys.executable, str(VALIDATOR), "--root", str(root), *arguments],
      cwd=REPOSITORY_ROOT,
      capture_output=True,
      text=True,
      check=False,
    )

  def copy_validation_root(self, destination: Path) -> None:
    """Copy only governed baseline inputs into an isolated mutation sandbox."""
    shutil.copytree(REPOSITORY_ROOT / "verification", destination / "verification")

  def write_artifact(
    self, directory: Path, relative_path: str, content: bytes
  ) -> dict[str, str | int]:
    """Write an evidence artifact and return its independently computed identity."""
    path = directory / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return {
      "path": relative_path,
      "sha256": hashlib.sha256(content).hexdigest(),
      "size_bytes": len(content),
    }

  def fixture_content_sha256(self, inputs: list[dict[str, object]]) -> str:
    """Compute the fixture inventory identity used by the public contract."""
    digest = hashlib.sha256()
    for fixture_input in sorted(inputs, key=lambda item: str(item["path"])):
      digest.update(str(fixture_input["path"]).encode("utf-8"))
      digest.update(b"\0")
      digest.update(str(fixture_input["size_bytes"]).encode("ascii"))
      digest.update(b"\0")
      digest.update(bytes.fromhex(str(fixture_input["sha256"])))
    return digest.hexdigest()

  def write_linked_fixture_and_evidence(self, root: Path) -> tuple[Path, Path]:
    """Add one schema-valid linked fixture, evidence bundle, and matrix cell."""
    fixture_path = root / "verification/fixtures/setup/fixture.json"
    evidence_path = root / "verification/evidence/setup/evidence.json"
    fixture_path.parent.mkdir(parents=True)
    evidence_path.parent.mkdir(parents=True)
    evidence_directory = evidence_path.parent
    bundle = self.write_artifact(
      evidence_directory,
      "bundle/capture.json",
      b'{"capture":"reviewed oracle observations"}\n',
    )
    fixture_input = self.write_artifact(
      fixture_path.parent,
      "input/profile.ini",
      b"[Profile]\nname=SSE test\n",
    )
    fixture_input.update({
      "origin": "deterministic test recipe",
      "license_class": "CC0-1.0",
      "redistributable": True,
    })
    fixture_content_sha256 = self.fixture_content_sha256([fixture_input])
    evidence = {
      "schema_version": 1,
      "baseline_revision": 1,
      "id": "EVD-SETUP",
      "revision": 1,
      "kind": "oracle-capture",
      "fixture_ref": {
        "id": "FIX-SETUP",
        "revision": 1,
        "sha256": fixture_content_sha256,
      },
      "oracle": {
        "input_id": "behavioral-oracle",
        "archive_sha256": "b25cf0c0c97160b602dd47c252af2ede735c27abe565c9ab84d272306308abb6",
      },
      "environment": {
        "vm_image": "windows-10-22h2-clean-v1",
        "gpu_lane": "software-fallback",
        "profile_tree_sha256": "4" * 64,
        "selected_profile": "SSE",
        "ui_options": {"dry_run": False},
        "fixture_sha256": fixture_content_sha256,
        "working_directory": "C:/oracle/case",
      },
      "observations": {
        "raw_inis": [self.write_artifact(evidence_directory, "raw/settings.ini", b"[General]\n")],
        "html_logs": [self.write_artifact(evidence_directory, "raw/log.html", b"<html></html>\n")],
        "initial_ui_state": [self.write_artifact(evidence_directory, "raw/initial-ui.json", b"{}\n")],
        "post_profile_ui_state": [self.write_artifact(evidence_directory, "raw/profile-ui.json", b"{}\n")],
        "warnings_and_validation": [],
        "progress_and_log_sequence": [self.write_artifact(evidence_directory, "raw/events.json", b"[]\n")],
        "screenshots_and_final_state": [self.write_artifact(evidence_directory, "raw/final.png", b"test-png")],
        "process_tree_and_helper_invocations": [self.write_artifact(evidence_directory, "raw/processes.json", b"[]\n")],
        "cancellation_point": None,
      },
      "filesystem": {
        "before_manifest": self.write_artifact(evidence_directory, "normalized/before.json", b"{}\n"),
        "after_manifest": self.write_artifact(evidence_directory, "normalized/after.json", b"{}\n"),
      },
      "normalization_evidence": [self.write_artifact(evidence_directory, "raw/filesystem.json", b"{}\n")],
      "captured_on": "2026-07-12",
      "reviewed_on": "2026-07-12",
      "reviewed_by": "maintainer",
      "bundle": bundle,
    }
    evidence_path.write_text(json.dumps(evidence), encoding="utf-8")
    evidence_manifest_sha256 = hashlib.sha256(evidence_path.read_bytes()).hexdigest()

    fixture = {
      "schema_version": 1,
      "baseline_revision": 1,
      "id": "FIX-SETUP",
      "revision": 1,
      "content_sha256": fixture_content_sha256,
      "matrix_cells": [{"id": "CELL-SSE-SETUP", "revision": 1}],
      "inputs": [fixture_input],
      "provenance": {
        "mode": "deterministic-recipe",
        "instructions": "Generate the UTF-8 profile fixture from reviewed literals.",
      },
      "required_engines": [{
        "input_id": "behavioral-oracle",
        "version": "5.3.15",
        "commit": None,
      }],
      "comparison_profile": {
        "tier": "interaction",
        "fallback_allowed": False,
        "assertions": ["setup state"],
        "tolerances": [],
      },
      "oracle_evidence": {
        "id": "EVD-SETUP",
        "revision": 1,
        "manifest_sha256": evidence_manifest_sha256,
        "bundle_sha256": bundle["sha256"],
      },
      "discrepancy_refs": [],
      "temporary_opt_in": {
        "enabled": False,
        "reason": None,
        "removal_condition": None,
      },
    }
    fixture_path.write_text(json.dumps(fixture), encoding="utf-8")
    fixture_manifest_sha256 = hashlib.sha256(fixture_path.read_bytes()).hexdigest()

    matrix_path = root / "verification/parity/coverage-matrix.json"
    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    matrix["cells"] = [{
      "id": "CELL-SSE-SETUP",
      "revision": 1,
      "dimensions": {
        "game": "SSE",
        "profile": "SSE",
        "operation": "setup",
        "option_interaction": "dry-run-disabled",
        "input_class": "valid-profile",
        "control_path": "setup",
        "outcome": "Succeeded",
      },
      "resolution": {
        "status": "covered",
        "fixture_refs": [{
          "id": "FIX-SETUP",
          "revision": 1,
          "manifest_sha256": fixture_manifest_sha256,
        }],
        "discrepancy_refs": [],
        "evidence_refs": [{
          "id": "EVD-SETUP",
          "revision": 1,
          "manifest_sha256": evidence_manifest_sha256,
        }],
        "reason": None,
        "removal_condition": None,
      },
    }]
    matrix_path.write_text(json.dumps(matrix), encoding="utf-8")
    return fixture_path, evidence_path

  def write_discrepancy(
    self,
    root: Path,
    *,
    status: str = "Proposed",
    oracle_evidence_refs: list[str] | None = None,
  ) -> Path:
    """Write a schema-valid discrepancy with caller-selected evidence links."""
    register_path = root / "verification/discrepancies/register.json"
    register = json.loads(register_path.read_text(encoding="utf-8"))
    register["entries"] = [{
      "id": "DISC-0001",
      "revision": 1,
      "status": status,
      "classification": "safer replacement behavior",
      "rationale": "The replacement prevents a destructive oracle behavior.",
      "oracle_evidence_refs": oracle_evidence_refs or ["EVD-SETUP@1"],
      "affected": {
        "games": ["SSE"],
        "profiles": ["SSE"],
        "options": ["cleanup"],
        "inputs": ["dummy plugin"],
        "operations": ["delete"],
        "outputs": ["plugin"],
        "workflows": ["processing run"],
      },
      "risk": {
        "compatibility": "low",
        "data_integrity": "reduced deletion risk",
        "migration": "none",
      },
      "replacement_behavior": "Delete only an exact authenticated dummy plugin.",
      "automated_regression_evidence": [],
      "manual_evidence": [],
      "approval": {
        "maintainer": None,
        "decided_on": None,
        "superseded_by": None,
      },
    }]
    register_path.write_text(json.dumps(register), encoding="utf-8")
    return register_path

  def test_checked_in_baseline_is_valid(self) -> None:
    """The committed baseline validates without network or restricted payloads."""
    result = self.run_validator(REPOSITORY_ROOT)

    self.assertEqual(0, result.returncode, result.stdout + result.stderr)

  def test_unreviewed_schema_revision_is_rejected(self) -> None:
    """A record and schema cannot silently advance beyond validator support."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      document_path = root / "verification/baseline/implementation-inputs.json"
      schema_path = root / "verification/schemas/implementation-inputs.schema.json"

      document = json.loads(document_path.read_text(encoding="utf-8"))
      document["schema_version"] = 2
      document_path.write_text(json.dumps(document), encoding="utf-8")

      schema = json.loads(schema_path.read_text(encoding="utf-8"))
      schema["properties"]["schema_version"]["const"] = 2
      schema_path.write_text(json.dumps(schema), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("stale schema revision", result.stderr)

  def test_stale_fixture_evidence_revision_is_rejected(self) -> None:
    """Fixture evidence references must resolve to an exact current revision."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      fixture_path, _ = self.write_linked_fixture_and_evidence(root)
      fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
      fixture["oracle_evidence"]["revision"] = 2
      fixture_path.write_text(json.dumps(fixture), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("stale oracle evidence reference EVD-SETUP@2", result.stderr)

  def test_fixture_rejects_replay_as_oracle_evidence(self) -> None:
    """A fixture's oracle evidence link cannot resolve to replacement replay."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      _, evidence_path = self.write_linked_fixture_and_evidence(root)
      evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
      evidence["kind"] = "replacement-replay"
      evidence_path.write_text(json.dumps(evidence), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("EVD-SETUP@1 is not oracle-capture evidence", result.stderr)

  def test_stale_fixture_engine_version_is_rejected(self) -> None:
    """Fixture engine identities must match the current pinned input manifest."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      fixture_path, _ = self.write_linked_fixture_and_evidence(root)
      fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
      fixture["required_engines"][0]["version"] = "5.3.14"
      fixture_path.write_text(json.dumps(fixture), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("stale engine identity behavioral-oracle", result.stderr)

  def test_stale_fixture_matrix_cell_revision_is_rejected(self) -> None:
    """Fixture matrix links must resolve to the exact governed cell revision."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      fixture_path, _ = self.write_linked_fixture_and_evidence(root)
      fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
      fixture["matrix_cells"][0]["revision"] = 2
      fixture_path.write_text(json.dumps(fixture), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("stale matrix cell reference CELL-SSE-SETUP@2", result.stderr)

  def test_blocked_matrix_cannot_pass_the_parity_gate(self) -> None:
    """Covered cells do not override the matrix's explicit blocked state."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      self.write_linked_fixture_and_evidence(root)

      result = self.run_validator(root, "--parity-gate")

      self.assertNotEqual(0, result.returncode)
      self.assertIn("closure_state is blocked", result.stderr)

  def test_every_governed_schema_rejects_a_missing_required_field(self) -> None:
    """All six versioned contracts reject incomplete records at the CLI seam."""
    cases = [
      ("verification/baseline/implementation-inputs.json", "scope"),
      ("verification/baseline/compliance-policy.json", "authority"),
      ("verification/discrepancies/register.json", "register_revision"),
      ("verification/parity/coverage-matrix.json", "matrix_revision"),
      ("verification/fixtures/setup/fixture.json", "comparison_profile"),
      ("verification/evidence/setup/evidence.json", "environment"),
    ]
    for relative_path, required_field in cases:
      with self.subTest(document=relative_path, field=required_field):
        with tempfile.TemporaryDirectory() as temporary_directory:
          root = Path(temporary_directory)
          self.copy_validation_root(root)
          self.write_linked_fixture_and_evidence(root)
          document_path = root / relative_path
          document = json.loads(document_path.read_text(encoding="utf-8"))
          del document[required_field]
          document_path.write_text(json.dumps(document), encoding="utf-8")

          result = self.run_validator(root)

          self.assertNotEqual(0, result.returncode)
          self.assertIn("required property is missing", result.stderr)

  def test_compliance_policy_drift_is_rejected(self) -> None:
    """Licensing, static CRT, and Windows floor values remain normative."""
    cases = [
      (("licensing", "combined_work"), "MPL-2.0"),
      (("licensing", "slint_choice"), "Royalty-free"),
      (("native_runtime", "msvc_flag"), "/MD"),
      (("windows", "minimum_release"), "Windows 11 24H2"),
    ]
    for path_parts, replacement in cases:
      with self.subTest(policy=".".join(path_parts)):
        with tempfile.TemporaryDirectory() as temporary_directory:
          root = Path(temporary_directory)
          self.copy_validation_root(root)
          policy_path = root / "verification/baseline/compliance-policy.json"
          policy = json.loads(policy_path.read_text(encoding="utf-8"))
          policy[path_parts[0]][path_parts[1]] = replacement
          policy_path.write_text(json.dumps(policy), encoding="utf-8")

          result = self.run_validator(root)

          self.assertNotEqual(0, result.returncode)
          self.assertIn("approved value", result.stderr)

  def test_restricted_oracle_payload_hash_is_verified_when_supplied(self) -> None:
    """A local oracle kit payload must match the committed size and SHA-256."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      fake_oracle = Path(temporary_directory) / "oracle.7z"
      fake_oracle.write_bytes(b"not the authenticated behavioral oracle")

      result = self.run_validator(
        REPOSITORY_ROOT,
        "--verify-input",
        f"behavioral-oracle={fake_oracle}",
      )

      self.assertNotEqual(0, result.returncode)
      self.assertIn("SHA-256 mismatch", result.stderr)

  def test_toolchain_version_output_is_verified_when_supplied(self) -> None:
    """Version-pinned tools are executed and compared with expected output."""
    result = self.run_validator(
      REPOSITORY_ROOT,
      "--verify-input",
      f"rust-toolchain={sys.executable}",
    )

    self.assertNotEqual(0, result.returncode)
    self.assertIn("version output mismatch", result.stderr)

  def test_fixture_cannot_link_a_proposed_discrepancy(self) -> None:
    """Only an exact Approved discrepancy revision can satisfy a fixture."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      fixture_path, _ = self.write_linked_fixture_and_evidence(root)
      fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
      fixture["discrepancy_refs"] = ["DISC-0001@1"]
      fixture_path.write_text(json.dumps(fixture), encoding="utf-8")

      self.write_discrepancy(root)

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("DISC-0001@1 is not Approved", result.stderr)

  def test_stale_discrepancy_evidence_revision_is_rejected(self) -> None:
    """Every discrepancy evidence link must resolve to an exact revision."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      self.write_linked_fixture_and_evidence(root)
      self.write_discrepancy(root, oracle_evidence_refs=["EVD-SETUP@2"])

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("stale evidence reference EVD-SETUP@2", result.stderr)

  def test_discrepancy_rejects_oracle_capture_as_regression_evidence(self) -> None:
    """Automated discrepancy regressions require replacement replay evidence."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      self.write_linked_fixture_and_evidence(root)
      register_path = self.write_discrepancy(root)
      register = json.loads(register_path.read_text(encoding="utf-8"))
      register["entries"][0]["automated_regression_evidence"] = ["EVD-SETUP@1"]
      register_path.write_text(json.dumps(register), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn(
        "EVD-SETUP@1 is not replacement-replay evidence",
        result.stderr,
      )

  def test_evidence_artifact_tampering_is_rejected(self) -> None:
    """Evidence hashes are recomputed from artifact bytes during validation."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      _, evidence_path = self.write_linked_fixture_and_evidence(root)
      artifact_path = evidence_path.parent / "raw/settings.ini"
      artifact_path.write_bytes(b"tampered after review")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("artifact SHA-256 mismatch", result.stderr)

  def test_fixture_payload_tampering_is_rejected(self) -> None:
    """Redistributable fixture payload hashes are recomputed from disk."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      fixture_path, _ = self.write_linked_fixture_and_evidence(root)
      input_path = fixture_path.parent / "input/profile.ini"
      input_path.write_bytes(b"tampered fixture payload")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("artifact SHA-256 mismatch", result.stderr)

  def test_pinned_dependency_feature_drift_is_rejected(self) -> None:
    """Feature, source, and license fields cannot change without baseline review."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      manifest_path = root / "verification/baseline/implementation-inputs.json"
      manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
      slint = next(entry for entry in manifest["inputs"] if entry["id"] == "slint")
      slint["required_features"].remove("accessibility")
      manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("implementation input contract changed", result.stderr)

  def test_evidence_manifest_tampering_is_rejected(self) -> None:
    """Versioned manifest references are checked against the manifest bytes."""
    with tempfile.TemporaryDirectory() as temporary_directory:
      root = Path(temporary_directory)
      self.copy_validation_root(root)
      _, evidence_path = self.write_linked_fixture_and_evidence(root)
      evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
      evidence["reviewed_by"] = "unreviewed replacement"
      evidence_path.write_text(json.dumps(evidence), encoding="utf-8")

      result = self.run_validator(root)

      self.assertNotEqual(0, result.returncode)
      self.assertIn("evidence manifest SHA-256 mismatch", result.stderr)


if __name__ == "__main__":
  unittest.main()
