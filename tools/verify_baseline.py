"""Validate Tracetide's evidence, source, and compliance baseline offline."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


BASELINE_REVISION = 1
SCHEMA_REVISION = 1
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
# Requiring a validator change makes every source/feature/license drift explicit in review.
EXPECTED_INPUT_MANIFEST_SHA256 = "7b3de50d0710f107e62e1d98776d1978d2e58637ec42bee25d7b607905d7a349"

DOCUMENT_SCHEMAS = {
  "verification/baseline/implementation-inputs.json": (
    "verification/schemas/implementation-inputs.schema.json"
  ),
  "verification/baseline/compliance-policy.json": (
    "verification/schemas/compliance-policy.schema.json"
  ),
  "verification/discrepancies/register.json": (
    "verification/schemas/discrepancy-register.schema.json"
  ),
  "verification/parity/coverage-matrix.json": (
    "verification/schemas/parity-coverage-matrix.schema.json"
  ),
}

COLLECTION_SCHEMAS = {
  "verification/fixtures": (
    "fixture.json",
    "verification/schemas/fixture.schema.json",
  ),
  "verification/evidence": (
    "evidence.json",
    "verification/schemas/evidence.schema.json",
  ),
}

EXPECTED_INPUT_IDENTITIES = {
  "behavioral-oracle": {
    "version": "5.3.15",
    "filename": "Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z",
    "verification.sha256": "b25cf0c0c97160b602dd47c252af2ede735c27abe565c9ab84d272306308abb6",
    "verification.size_bytes": 10410192,
    "redistribution": "restricted-local-only",
    "license_expression": None,
    "selected_license": None,
  },
  "rust-toolchain": {
    "version": "1.97.0",
    "license_expression": "Apache-2.0 OR MIT",
    "selected_license": None,
  },
  "crossbeam-channel": {
    "version": "0.5.16",
    "license_expression": "MIT OR Apache-2.0",
    "selected_license": "Apache-2.0",
    "default_features": False,
    "verification.sha256": "d85363c37faeca707aef026efa9f3b34d077bce547e48f770770625c6013679e",
  },
  "ba2": {
    "version": "3.0.1",
    "license_expression": "0BSD",
    "selected_license": "0BSD",
    "verification.sha256": "7fdb05c5c954898b463887df1145016492deee06e9a778f8af491c7cde14c210",
  },
  "nifly": {
    "commit": "965a1da1be7bff145b7b3435def5c04d6e8c8cce",
    "license_expression": "GPL-3.0-only",
    "selected_license": "GPL-3.0-only",
    "verification.sha256": "fcc737f7362e9326b4585210b5173aa2d1b4caa6395cddf2ad5a99cb625b55c3",
  },
  "directxtex": {
    "commit": "4feb3e11a020f35b796fc769a74216a555d4f5ef",
    "license_expression": "MIT",
    "selected_license": "MIT",
    "verification.sha256": "9488bdc5c292534caed070276d426bbf89a021d4e6764a56fcc4a41b2198758e",
  },
  "serde-hkx": {
    "version": "1.0.1",
    "commit": "6c1bee56d42de7def991cf6fba025a9df7492d83",
    "license_expression": "MIT OR Apache-2.0",
    "selected_license": "Apache-2.0",
    "default_features": False,
    "verification.sha256": "34f5cb574f8bc5a67354aa3d5c184abfa79e41007cd7955786d47ed950dcc430",
  },
  "slint": {
    "version": "1.17.1",
    "license_expression": "GPL-3.0-only",
    "selected_license": "GPL-3.0-only",
    "default_features": False,
    "verification.sha256": "ce12038f57f8f3a423564b0c9301a2ab3fda4246e4cbadd6b97acf9124bdc315",
  },
  "ba2-directxtex": {
    "version": "1.3.0",
    "commit": "8e44d15166acb5c5d03864ac58e593da8f8e555b",
    "license_expression": "MIT",
    "verification.sha256": "0f0cea4cfe7ef3eeeae77cbbe6c8eddb7e518b1af877ca788b7a9d4fde45e658",
    "verification.size_bytes": 797360,
  },
  "ba2-lzzzz": {
    "version": "1.0.4",
    "commit": "226150ad7c8dda5cd782f5580040c74c1c9b2842",
    "license_expression": "MIT AND BSD-2-Clause",
    "verification.sha256": "8014d1362004776e6a91e4c15a3aa7830d1b6650a075b51a9969ebb6d6af13bc",
    "verification.size_bytes": 128643,
  },
  "msvc-build-tools": {
    "version": "18.7.3+11925.98",
    "verification.sha256": "07b09afd416dc05c781f171c881c23e42907eeb8d812fa1d2993dffb9323c869",
    "verification.size_bytes": 5693576,
  },
  "msvc-toolchain": {
    "version": "14.51.36231",
    "verification.sha256": "38c32e72648a8c37c9fbe5d96f17a92d5bcc1f77367b3e89e042fc85f15bb8f1",
    "verification.size_bytes": 2572193143,
    "verification.file_count": 1599,
  },
  "windows-sdk": {
    "version": "10.0.26100.8249",
    "verification.sha256": "239d1505a6e739ba0a64712008198136186013d4b68c86389afe96112076b9dd",
    "verification.size_bytes": 1159639040,
  },
  "cmake": {
    "version": "4.4.0",
    "verification.sha256": "156d70eb7625a7b469444df7d0861d2af8d5d0a437fce32c350372b08f5620e8",
    "verification.size_bytes": 54388920,
  },
  "ninja": {
    "version": "1.13.2",
    "verification.sha256": "07fc8261b42b20e71d1720b39068c2e14ffcee6396b76fb7a795fb460b78dc65",
    "verification.size_bytes": 291570,
  },
  "cargo-deny": {
    "version": "0.20.2",
    "verification.sha256": "e528dfcbe739af7ce37a77d3d6df1b29dd6887b1c701d888820c0f16b864f737",
    "verification.size_bytes": 217654,
  },
  "cargo-about": {
    "version": "0.9.1",
    "verification.sha256": "4d62bfc04a579b87777727b0f5f389f72bdeb1c6cc8fbcf1c0e0c736c9b5b7a4",
    "verification.size_bytes": 96994,
  },
  "cargo-cyclonedx": {
    "version": "0.5.9",
    "verification.sha256": "5d162f67705f0f5038759d73bf546a083bf30e8677c2e944b416bca48d9d69a8",
    "verification.size_bytes": 47272,
  },
  "python-zip-tool": {
    "version": "3.14.5",
    "verification.sha256": "ba6bd811c4eedb19195cf275770ef127e893d63701e24152606e2cb76f6d876a",
    "verification.size_bytes": 12048447,
  },
}

EXPECTED_COMPLIANCE = {
  "licensing.combined_work": "GPL-3.0-only",
  "licensing.slint_choice": "GPL-3.0-only",
  "licensing.existing_source_files": "preserve-MPL-2.0-coverage-and-notices",
  "licensing.corresponding_source": "exact-source-alongside-every-binary",
  "native_runtime.scope": "tracetide-production-graph",
  "native_runtime.msvc_runtime": "MultiThreaded",
  "native_runtime.msvc_flag": "/MT",
  "native_runtime.rust_target_feature": "+crt-static",
  "native_runtime.debug_crt_allowed": False,
  "windows.minimum_release": "Windows 10 22H2",
  "windows.minimum_architecture": "x86_64",
  "windows.minimum_support_status": "out-of-support",
  "windows.primary_release": "Windows 11 x64",
  "windows.cpu_baseline": "x86-64",
  "windows.avx_required": False,
  "windows.avx2_required": False,
}


@dataclass(frozen=True)
class ValidationError:
  """Describe one validation failure with its source and JSON pointer."""

  source: Path
  pointer: str
  message: str

  def render(self, root: Path) -> str:
    """Render a stable repository-relative diagnostic."""
    try:
      source = self.source.relative_to(root)
    except ValueError:
      source = self.source
    return f"{source}:{self.pointer}: {self.message}"


def load_json(path: Path) -> Any:
  """Load one UTF-8 JSON document and preserve parse failures for the caller."""
  with path.open("r", encoding="utf-8") as handle:
    return json.load(handle)


def matches_type(value: Any, expected: str) -> bool:
  """Return whether a Python value matches one supported JSON Schema type."""
  if expected == "object":
    return isinstance(value, dict)
  if expected == "array":
    return isinstance(value, list)
  if expected == "string":
    return isinstance(value, str)
  if expected == "integer":
    return isinstance(value, int) and not isinstance(value, bool)
  if expected == "number":
    return isinstance(value, (int, float)) and not isinstance(value, bool)
  if expected == "boolean":
    return isinstance(value, bool)
  if expected == "null":
    return value is None
  return False


def escape_pointer(part: str) -> str:
  """Escape one RFC 6901 JSON Pointer segment."""
  return part.replace("~", "~0").replace("/", "~1")


def resolve_local_reference(
  root_schema: Mapping[str, Any], reference: str
) -> Mapping[str, Any] | None:
  """Resolve one local JSON Schema reference from the document root."""
  if not reference.startswith("#/"):
    return None
  current: Any = root_schema
  for encoded_part in reference[2:].split("/"):
    part = encoded_part.replace("~1", "/").replace("~0", "~")
    if not isinstance(current, dict) or part not in current:
      return None
    current = current[part]
  return current if isinstance(current, dict) else None


def validate_instance(
  value: Any,
  schema: Mapping[str, Any],
  source: Path,
  pointer: str = "",
  root_schema: Mapping[str, Any] | None = None,
) -> list[ValidationError]:
  """Validate the deterministic JSON Schema subset used by baseline contracts."""
  if root_schema is None:
    root_schema = schema
  reference = schema.get("$ref")
  if reference is not None:
    resolved = resolve_local_reference(root_schema, reference)
    if resolved is None:
      return [
        ValidationError(source, pointer or "/", f"unresolved schema reference {reference}")
      ]
    return validate_instance(value, resolved, source, pointer, root_schema)

  errors: list[ValidationError] = []
  expected_types = schema.get("type")
  if expected_types is not None:
    if isinstance(expected_types, str):
      expected_types = [expected_types]
    if not any(matches_type(value, item) for item in expected_types):
      actual = "null" if value is None else type(value).__name__
      return [
        ValidationError(
          source,
          pointer or "/",
          f"expected type {expected_types}, found {actual}",
        )
      ]

  if "const" in schema and value != schema["const"]:
    errors.append(
      ValidationError(source, pointer or "/", f"must equal {schema['const']!r}")
    )
  if "enum" in schema and value not in schema["enum"]:
    errors.append(
      ValidationError(source, pointer or "/", f"must be one of {schema['enum']!r}")
    )

  if isinstance(value, dict):
    required = schema.get("required", [])
    for name in required:
      if name not in value:
        errors.append(
          ValidationError(
            source,
            f"{pointer}/{escape_pointer(name)}",
            "required property is missing",
          )
        )

    properties = schema.get("properties", {})
    for name, child in value.items():
      child_pointer = f"{pointer}/{escape_pointer(name)}"
      child_schema = properties.get(name)
      if child_schema is not None:
        errors.extend(
          validate_instance(child, child_schema, source, child_pointer, root_schema)
        )
      elif schema.get("additionalProperties") is False:
        errors.append(
          ValidationError(source, child_pointer, "additional property is not allowed")
        )

    minimum_properties = schema.get("minProperties")
    if minimum_properties is not None and len(value) < minimum_properties:
      errors.append(
        ValidationError(
          source,
          pointer or "/",
          f"must contain at least {minimum_properties} properties",
        )
      )

  if isinstance(value, list):
    minimum_items = schema.get("minItems")
    if minimum_items is not None and len(value) < minimum_items:
      errors.append(
        ValidationError(
          source,
          pointer or "/",
          f"must contain at least {minimum_items} items",
        )
      )
    if schema.get("uniqueItems"):
      serialized = [json.dumps(item, sort_keys=True) for item in value]
      if len(serialized) != len(set(serialized)):
        errors.append(
          ValidationError(source, pointer or "/", "items must be unique")
        )
    item_schema = schema.get("items")
    if item_schema is not None:
      for index, item in enumerate(value):
        errors.extend(
          validate_instance(item, item_schema, source, f"{pointer}/{index}", root_schema)
        )

  if isinstance(value, str):
    minimum_length = schema.get("minLength")
    if minimum_length is not None and len(value) < minimum_length:
      errors.append(
        ValidationError(
          source,
          pointer or "/",
          f"must contain at least {minimum_length} characters",
        )
      )
    pattern = schema.get("pattern")
    if pattern is not None and re.fullmatch(pattern, value) is None:
      errors.append(
        ValidationError(source, pointer or "/", f"must match /{pattern}/")
      )
    if schema.get("format") == "date":
      try:
        date.fromisoformat(value)
      except ValueError:
        errors.append(
          ValidationError(source, pointer or "/", "must be an ISO 8601 date")
        )

  if isinstance(value, (int, float)) and not isinstance(value, bool):
    minimum = schema.get("minimum")
    if minimum is not None and value < minimum:
      errors.append(
        ValidationError(source, pointer or "/", f"must be at least {minimum}")
      )

  return errors


def nested_value(document: Mapping[str, Any], dotted_path: str) -> Any:
  """Read a required nested policy value using a stable dotted path."""
  current: Any = document
  for part in dotted_path.split("."):
    if not isinstance(current, dict) or part not in current:
      return None
    current = current[part]
  return current


def canonical_document_sha256(document: Mapping[str, Any]) -> str:
  """Hash a JSON document independently of whitespace and property ordering."""
  canonical = json.dumps(
    document,
    sort_keys=True,
    separators=(",", ":"),
    ensure_ascii=False,
  ).encode("utf-8")
  return hashlib.sha256(canonical).hexdigest()


def validate_input_semantics(
  document: Mapping[str, Any], source: Path
) -> list[ValidationError]:
  """Reject missing, stale, or weakened identities selected by the blueprint."""
  errors: list[ValidationError] = []
  actual_manifest_hash = canonical_document_sha256(document)
  if actual_manifest_hash != EXPECTED_INPUT_MANIFEST_SHA256:
    errors.append(
      ValidationError(
        source,
        "/",
        "implementation input contract changed without a reviewed validator update",
      )
    )
  entries = document.get("inputs", [])
  by_id = {entry.get("id"): entry for entry in entries if isinstance(entry, dict)}

  missing = sorted(set(EXPECTED_INPUT_IDENTITIES) - set(by_id))
  extra = sorted(set(by_id) - set(EXPECTED_INPUT_IDENTITIES))
  if missing:
    errors.append(ValidationError(source, "/inputs", f"missing pinned inputs: {missing}"))
  if extra:
    errors.append(
      ValidationError(source, "/inputs", f"unreviewed pinned inputs: {extra}")
    )

  for input_id, expected_fields in EXPECTED_INPUT_IDENTITIES.items():
    entry = by_id.get(input_id)
    if entry is None:
      continue
    for field, expected in expected_fields.items():
      actual = nested_value(entry, field)
      if actual != expected:
        errors.append(
          ValidationError(
            source,
            f"/inputs/{escape_pointer(input_id)}/{field.replace('.', '/')}",
            f"must remain pinned to {expected!r}; found {actual!r}",
          )
        )

    verification = entry.get("verification", {})
    method = verification.get("method")
    if method in {"sha256", "tree-sha256"}:
      digest = verification.get("sha256", "")
      if not HEX_SHA256.fullmatch(digest):
        errors.append(
          ValidationError(
            source,
            f"/inputs/{escape_pointer(input_id)}/verification/sha256",
            "must be a lowercase SHA-256 digest",
          )
        )
    elif method == "version-output":
      if not verification.get("expected"):
        errors.append(
          ValidationError(
            source,
            f"/inputs/{escape_pointer(input_id)}/verification/expected",
            "version verification requires expected output",
          )
        )
    else:
      errors.append(
        ValidationError(
          source,
          f"/inputs/{escape_pointer(input_id)}/verification/method",
          "must use sha256, tree-sha256, or version-output verification",
        )
      )

  return errors


def validate_compliance_semantics(
  document: Mapping[str, Any], source: Path
) -> list[ValidationError]:
  """Enforce the approved licensing, CRT, and Windows support decisions."""
  errors: list[ValidationError] = []
  for dotted_path, expected in EXPECTED_COMPLIANCE.items():
    actual = nested_value(document, dotted_path)
    if actual != expected:
      pointer = "/" + dotted_path.replace(".", "/")
      errors.append(
        ValidationError(
          source,
          pointer,
          f"must equal approved value {expected!r}; found {actual!r}",
        )
      )
  return errors


def validate_discrepancy_semantics(
  document: Mapping[str, Any], source: Path
) -> list[ValidationError]:
  """Enforce status-specific discrepancy approval and supersession records."""
  errors: list[ValidationError] = []
  for index, entry in enumerate(document.get("entries", [])):
    status = entry.get("status")
    approval = entry.get("approval", {})
    base = f"/entries/{index}"
    if status in {"Approved", "Rejected", "Superseded"}:
      if not approval.get("maintainer") or not approval.get("decided_on"):
        errors.append(
          ValidationError(
            source,
            f"{base}/approval",
            f"{status} discrepancies require a dated maintainer decision",
          )
        )
    if status == "Approved" and not entry.get("automated_regression_evidence"):
      errors.append(
        ValidationError(
          source,
          f"{base}/automated_regression_evidence",
          "Approved discrepancies require automated regression evidence",
        )
      )
    if status == "Superseded" and not approval.get("superseded_by"):
      errors.append(
        ValidationError(
          source,
          f"{base}/approval/superseded_by",
          "Superseded discrepancies require a replacement ID",
        )
      )
  return errors


def validate_matrix_semantics(
  document: Mapping[str, Any], source: Path, parity_gate: bool
) -> list[ValidationError]:
  """Reject unresolved or temporarily skipped coverage when closing parity."""
  errors: list[ValidationError] = []
  cells = document.get("cells", [])
  if parity_gate and document.get("closure_state") != "closed":
    errors.append(
      ValidationError(
        source,
        "/closure_state",
        f"closure_state is {document.get('closure_state')}; parity gate requires closed",
      )
    )
  if parity_gate and not cells:
    errors.append(
      ValidationError(source, "/cells", "parity gate requires declared coverage cells")
    )
  for index, cell in enumerate(cells):
    resolution = cell.get("resolution", {})
    status = resolution.get("status")
    base = f"/cells/{index}/resolution"
    if status == "covered" and not (
      resolution.get("fixture_refs") or resolution.get("discrepancy_refs")
    ):
      errors.append(
        ValidationError(
          source,
          base,
          "covered cells require a fixture or approved discrepancy reference",
        )
      )
    if status == "not-applicable" and not resolution.get("evidence_refs"):
      errors.append(
        ValidationError(
          source,
          f"{base}/evidence_refs",
          "Not Applicable requires evidence",
        )
      )
    if status == "temporary-opt-in" and (
      not resolution.get("reason") or not resolution.get("removal_condition")
    ):
      errors.append(
        ValidationError(
          source,
          base,
          "temporary opt-in requires a reason and named removal condition",
        )
      )
    if parity_gate and status in {"blocked", "temporary-opt-in", "unknown"}:
      errors.append(
        ValidationError(source, f"{base}/status", f"{status} blocks parity closure")
      )
  return errors


def validate_fixture_semantics(
  document: Mapping[str, Any], source: Path
) -> list[ValidationError]:
  """Enforce provenance and non-fallback comparison rules for one fixture."""
  errors: list[ValidationError] = []
  comparison = document.get("comparison_profile", {})
  if comparison.get("fallback_allowed") is not False:
    errors.append(
      ValidationError(
        source,
        "/comparison_profile/fallback_allowed",
        "comparison profiles must never weaken themselves automatically",
      )
    )
  opt_in = document.get("temporary_opt_in", {})
  if opt_in.get("enabled") and (
    not opt_in.get("reason") or not opt_in.get("removal_condition")
  ):
    errors.append(
      ValidationError(
        source,
        "/temporary_opt_in",
        "temporary opt-in requires a reason and named removal condition",
      )
    )
  inputs = document.get("inputs", [])
  for index, fixture_input in enumerate(inputs):
    if fixture_input.get("redistributable"):
      errors.extend(
        validate_local_artifact_reference(
          source,
          f"/inputs/{index}",
          fixture_input,
        )
      )
  declared_content_hash = fixture_content_sha256(inputs)
  if document.get("content_sha256") != declared_content_hash:
    errors.append(
      ValidationError(
        source,
        "/content_sha256",
        f"fixture content SHA-256 mismatch; expected {declared_content_hash}",
      )
    )
  return errors


def fixture_content_sha256(inputs: Sequence[Mapping[str, Any]]) -> str:
  """Hash a fixture inventory by ordinal path, size, and verified input digest."""
  digest = hashlib.sha256()
  for fixture_input in sorted(inputs, key=lambda item: item.get("path", "")):
    digest.update(fixture_input.get("path", "").encode("utf-8"))
    digest.update(b"\0")
    digest.update(str(fixture_input.get("size_bytes", "")).encode("ascii"))
    digest.update(b"\0")
    input_hash = fixture_input.get("sha256", "")
    if HEX_SHA256.fullmatch(input_hash):
      digest.update(bytes.fromhex(input_hash))
  return digest.hexdigest()


def versioned_reference(identifier: Any, revision: Any) -> str | None:
  """Build a canonical ID@revision reference when both fields are well formed."""
  if not isinstance(identifier, str) or not isinstance(revision, int):
    return None
  return f"{identifier}@{revision}"


def index_versioned_documents(
  documents: Iterable[tuple[Path, Mapping[str, Any]]],
) -> tuple[dict[str, tuple[Path, Mapping[str, Any]]], list[ValidationError]]:
  """Index immutable revisions and report duplicate identities deterministically."""
  index: dict[str, tuple[Path, Mapping[str, Any]]] = {}
  errors: list[ValidationError] = []
  for path, document in documents:
    reference = versioned_reference(document.get("id"), document.get("revision"))
    if reference is None:
      continue
    if reference in index:
      errors.append(
        ValidationError(path, "/id", f"duplicate versioned identity {reference}")
      )
    else:
      index[reference] = (path, document)
  return index, errors


def evidence_artifact_references(
  document: Mapping[str, Any],
) -> Iterable[tuple[str, Mapping[str, Any]]]:
  """Yield every hash-addressed artifact governed by one evidence manifest."""
  bundle = document.get("bundle")
  if isinstance(bundle, dict):
    yield "/bundle", bundle
  observations = document.get("observations", {})
  for name in (
    "raw_inis",
    "html_logs",
    "initial_ui_state",
    "post_profile_ui_state",
    "warnings_and_validation",
    "progress_and_log_sequence",
    "screenshots_and_final_state",
    "process_tree_and_helper_invocations",
  ):
    for index, artifact in enumerate(observations.get(name, [])):
      if isinstance(artifact, dict):
        yield f"/observations/{name}/{index}", artifact
  filesystem = document.get("filesystem", {})
  for name in ("before_manifest", "after_manifest"):
    artifact = filesystem.get(name)
    if isinstance(artifact, dict):
      yield f"/filesystem/{name}", artifact
  for index, artifact in enumerate(document.get("normalization_evidence", [])):
    if isinstance(artifact, dict):
      yield f"/normalization_evidence/{index}", artifact


def validate_local_artifact_reference(
  manifest_path: Path,
  pointer: str,
  artifact: Mapping[str, Any],
) -> list[ValidationError]:
  """Verify one contained local artifact's path, size, and SHA-256."""
  raw_path = artifact.get("path")
  if not isinstance(raw_path, str):
    return []
  relative_path = Path(raw_path)
  if relative_path.is_absolute() or ".." in relative_path.parts:
    return [
      ValidationError(
        manifest_path,
        f"{pointer}/path",
        "artifact path must remain relative to its manifest directory",
      )
    ]
  evidence_root = manifest_path.parent.resolve()
  artifact_path = (evidence_root / relative_path).resolve()
  if not artifact_path.is_relative_to(evidence_root):
    return [
      ValidationError(
        manifest_path,
        f"{pointer}/path",
        "artifact path escapes its manifest directory",
      )
    ]
  if not artifact_path.is_file():
    return [ValidationError(artifact_path, "/", "local artifact does not exist")]

  normalization = artifact.get("normalization")
  if normalization == "lf-text":
    canonical_bytes = artifact_path.read_bytes().replace(b"\r\n", b"\n")
    actual_hash = hashlib.sha256(canonical_bytes).hexdigest()
    actual_size = len(canonical_bytes)
  else:
    actual_hash, actual_size = sha256_file(artifact_path)
  errors: list[ValidationError] = []
  if artifact.get("sha256") != actual_hash:
    errors.append(
      ValidationError(
        manifest_path,
        f"{pointer}/sha256",
        f"artifact SHA-256 mismatch; found {actual_hash}",
      )
    )
  if artifact.get("size_bytes") != actual_size:
    errors.append(
      ValidationError(
        manifest_path,
        f"{pointer}/size_bytes",
        f"artifact size mismatch; found {actual_size}",
      )
    )
  return errors


def validate_cross_references(
  root: Path,
  loaded: Mapping[Path, Any],
) -> list[ValidationError]:
  """Resolve exact fixture, evidence, matrix, and discrepancy revisions."""
  errors: list[ValidationError] = []
  fixture_root = root / "verification/fixtures"
  evidence_root = root / "verification/evidence"
  fixtures = [
    (path, document)
    for path, document in loaded.items()
    if path.is_relative_to(fixture_root) and isinstance(document, dict)
  ]
  evidence = [
    (path, document)
    for path, document in loaded.items()
    if path.is_relative_to(evidence_root) and isinstance(document, dict)
  ]
  fixture_index, fixture_errors = index_versioned_documents(fixtures)
  evidence_index, evidence_errors = index_versioned_documents(evidence)
  errors.extend(fixture_errors)
  errors.extend(evidence_errors)

  input_path = root / "verification/baseline/implementation-inputs.json"
  input_document = loaded.get(input_path, {})
  input_index = {
    entry.get("id"): entry
    for entry in input_document.get("inputs", [])
    if isinstance(entry, dict)
  }

  register_path = root / "verification/discrepancies/register.json"
  register = loaded.get(register_path, {})
  discrepancies = {
    versioned_reference(entry.get("id"), entry.get("revision")): entry
    for entry in register.get("entries", [])
    if isinstance(entry, dict)
  }
  matrix_path = root / "verification/parity/coverage-matrix.json"
  matrix = loaded.get(matrix_path, {})
  matrix_cells = {
    versioned_reference(cell.get("id"), cell.get("revision")): cell
    for cell in matrix.get("cells", [])
    if isinstance(cell, dict)
  }
  # Exact IDs are insufficient if capture and replay evidence can trade roles.
  evidence_role_by_field = {
    "oracle_evidence_refs": "oracle-capture",
    "automated_regression_evidence": "replacement-replay",
  }

  for entry_index, entry in enumerate(register.get("entries", [])):
    for field in (
      "oracle_evidence_refs",
      "automated_regression_evidence",
      "manual_evidence",
    ):
      for reference_index, reference in enumerate(entry.get(field, [])):
        evidence_record = evidence_index.get(reference)
        if evidence_record is None:
          errors.append(
            ValidationError(
              register_path,
              f"/entries/{entry_index}/{field}/{reference_index}",
              f"stale evidence reference {reference}",
            )
          )
        elif (
          field in evidence_role_by_field
          and evidence_record[1].get("kind") != evidence_role_by_field[field]
        ):
          expected_role = evidence_role_by_field[field]
          errors.append(
            ValidationError(
              register_path,
              f"/entries/{entry_index}/{field}/{reference_index}",
              f"{reference} is not {expected_role} evidence",
            )
          )

  for path, fixture in fixtures:
    for cell_index, cell_reference in enumerate(fixture.get("matrix_cells", [])):
      reference = versioned_reference(
        cell_reference.get("id"), cell_reference.get("revision")
      )
      if reference not in matrix_cells:
        errors.append(
          ValidationError(
            path,
            f"/matrix_cells/{cell_index}",
            f"stale matrix cell reference {reference}",
          )
        )
    for engine_index, engine in enumerate(fixture.get("required_engines", [])):
      input_id = engine.get("input_id")
      pinned = input_index.get(input_id)
      if pinned is None:
        errors.append(
          ValidationError(
            path,
            f"/required_engines/{engine_index}/input_id",
            f"unknown pinned input {input_id!r}",
          )
        )
        continue
      if (
        engine.get("version") != pinned.get("version")
        or engine.get("commit") != pinned.get("commit")
      ):
        errors.append(
          ValidationError(
            path,
            f"/required_engines/{engine_index}",
            f"stale engine identity {input_id}",
          )
        )

    reference_data = fixture.get("oracle_evidence", {})
    reference = versioned_reference(
      reference_data.get("id"), reference_data.get("revision")
    )
    if reference not in evidence_index:
      errors.append(
        ValidationError(
          path,
          "/oracle_evidence",
          f"stale oracle evidence reference {reference}",
        )
      )
    else:
      evidence_path, evidence_document = evidence_index[reference]
      evidence_manifest_hash, _ = sha256_file(evidence_path)
      if reference_data.get("manifest_sha256") != evidence_manifest_hash:
        errors.append(
          ValidationError(
            path,
            "/oracle_evidence/manifest_sha256",
            f"does not match {reference} manifest SHA-256",
          )
        )
      if reference_data.get("bundle_sha256") != evidence_document.get("bundle", {}).get("sha256"):
        errors.append(
          ValidationError(
            path,
            "/oracle_evidence/bundle_sha256",
            f"does not match {reference} bundle SHA-256",
          )
        )
      if evidence_document.get("kind") != "oracle-capture":
        errors.append(
          ValidationError(
            path,
            "/oracle_evidence",
            f"{reference} is not oracle-capture evidence",
          )
        )

    for index, reference in enumerate(fixture.get("discrepancy_refs", [])):
      discrepancy = discrepancies.get(reference)
      if discrepancy is None:
        message = f"stale discrepancy reference {reference}"
      elif discrepancy.get("status") != "Approved":
        message = f"discrepancy reference {reference} is not Approved"
      else:
        continue
      errors.append(
        ValidationError(path, f"/discrepancy_refs/{index}", message)
      )

  for path, evidence_document in evidence:
    reference_data = evidence_document.get("fixture_ref", {})
    reference = versioned_reference(
      reference_data.get("id"), reference_data.get("revision")
    )
    fixture_record = fixture_index.get(reference)
    if fixture_record is None:
      errors.append(
        ValidationError(path, "/fixture_ref", f"stale fixture reference {reference}")
      )
    else:
      _, fixture_document = fixture_record
      fixture_content_hash = fixture_document.get("content_sha256")
      if reference_data.get("sha256") != fixture_content_hash:
        errors.append(
          ValidationError(
            path,
            "/fixture_ref/sha256",
            f"fixture content SHA-256 mismatch; expected {fixture_content_hash}",
          )
        )
    if reference_data.get("sha256") != evidence_document.get("environment", {}).get("fixture_sha256"):
      errors.append(
        ValidationError(
          path,
          "/fixture_ref/sha256",
          "must match the fixture SHA-256 recorded in the run tuple",
        )
      )
    for pointer, artifact in evidence_artifact_references(evidence_document):
      errors.extend(validate_local_artifact_reference(path, pointer, artifact))

  for cell_index, cell in enumerate(matrix.get("cells", [])):
    resolution = cell.get("resolution", {})
    for reference_index, reference_data in enumerate(resolution.get("fixture_refs", [])):
      reference = versioned_reference(
        reference_data.get("id"), reference_data.get("revision")
      )
      fixture_record = fixture_index.get(reference)
      if fixture_record is None:
        errors.append(
          ValidationError(
            matrix_path,
            f"/cells/{cell_index}/resolution/fixture_refs/{reference_index}",
            f"stale fixture reference {reference}",
          )
        )
      else:
        fixture_path, _ = fixture_record
        actual_hash, _ = sha256_file(fixture_path)
        if reference_data.get("manifest_sha256") != actual_hash:
          errors.append(
            ValidationError(
              matrix_path,
              f"/cells/{cell_index}/resolution/fixture_refs/{reference_index}/manifest_sha256",
              f"fixture manifest SHA-256 mismatch; found {actual_hash}",
            )
          )
    for reference_index, reference_data in enumerate(resolution.get("evidence_refs", [])):
      reference = versioned_reference(
        reference_data.get("id"), reference_data.get("revision")
      )
      evidence_record = evidence_index.get(reference)
      if evidence_record is None:
        errors.append(
          ValidationError(
            matrix_path,
            f"/cells/{cell_index}/resolution/evidence_refs/{reference_index}",
            f"stale evidence reference {reference}",
          )
        )
      else:
        evidence_path, _ = evidence_record
        actual_hash, _ = sha256_file(evidence_path)
        if reference_data.get("manifest_sha256") != actual_hash:
          errors.append(
            ValidationError(
              matrix_path,
              f"/cells/{cell_index}/resolution/evidence_refs/{reference_index}/manifest_sha256",
              f"evidence manifest SHA-256 mismatch; found {actual_hash}",
            )
          )
    for reference_index, reference in enumerate(resolution.get("discrepancy_refs", [])):
      discrepancy = discrepancies.get(reference)
      if discrepancy is None:
        message = f"stale discrepancy reference {reference}"
      elif discrepancy.get("status") != "Approved":
        message = f"discrepancy reference {reference} is not Approved"
      else:
        continue
      errors.append(
        ValidationError(
          matrix_path,
          f"/cells/{cell_index}/resolution/discrepancy_refs/{reference_index}",
          message,
        )
      )

  return errors


def parse_verification_arguments(values: Sequence[str]) -> dict[str, Path]:
  """Parse repeated INPUT_ID=PATH requests.

  Raises:
    ValueError: If a request omits its input ID, separator, or path.
  """
  result: dict[str, Path] = {}
  for value in values:
    input_id, separator, raw_path = value.partition("=")
    if not separator or not input_id or not raw_path:
      raise ValueError(f"invalid --verify-input {value!r}; expected INPUT_ID=PATH")
    result[input_id] = Path(raw_path)
  return result


def sha256_file(path: Path) -> tuple[str, int]:
  """Hash a file incrementally so large restricted inputs need not enter memory.

  Returns:
    A ``(hex_digest, size_bytes)`` tuple for the file's exact contents.
  """
  digest = hashlib.sha256()
  size = 0
  with path.open("rb") as handle:
    for chunk in iter(lambda: handle.read(1024 * 1024), b""):
      digest.update(chunk)
      size += len(chunk)
  return digest.hexdigest(), size


def sha256_tree(path: Path) -> tuple[str, int, int]:
  """Hash a tool tree by ordinal relative path, size, and each file digest.

  Returns:
    A ``(hex_digest, file_count, total_size_bytes)`` tuple for the tree.
  """
  digest = hashlib.sha256()
  file_count = 0
  total_size = 0
  files = sorted(
    (candidate for candidate in path.rglob("*") if candidate.is_file()),
    key=lambda candidate: candidate.relative_to(path).as_posix(),
  )
  for candidate in files:
    relative_path = candidate.relative_to(path).as_posix()
    file_hash, file_size = sha256_file(candidate)
    digest.update(relative_path.encode("utf-8"))
    digest.update(b"\0")
    digest.update(str(file_size).encode("ascii"))
    digest.update(b"\0")
    digest.update(bytes.fromhex(file_hash))
    file_count += 1
    total_size += file_size
  return digest.hexdigest(), file_count, total_size


def verify_acquired_inputs(
  requests: Mapping[str, Path],
  input_document: Mapping[str, Any],
  source: Path,
) -> list[ValidationError]:
  """Verify user-supplied source/oracle payloads against committed identities."""
  errors: list[ValidationError] = []
  entries = {entry["id"]: entry for entry in input_document.get("inputs", [])}
  for input_id, path in requests.items():
    entry = entries.get(input_id)
    if entry is None:
      errors.append(
        ValidationError(source, "/inputs", f"unknown input ID {input_id!r}")
      )
      continue
    verification = entry.get("verification", {})
    method = verification.get("method")
    if method == "tree-sha256":
      if not path.is_dir():
        errors.append(ValidationError(path, "/", "input directory does not exist"))
        continue
      actual_hash, actual_count, actual_size = sha256_tree(path)
      expected_hash = verification.get("sha256")
      expected_count = verification.get("file_count")
      expected_size = verification.get("size_bytes")
      if actual_hash != expected_hash:
        errors.append(
          ValidationError(
            path,
            "/",
            f"tree SHA-256 mismatch: expected {expected_hash}, found {actual_hash}",
          )
        )
      if actual_count != expected_count:
        errors.append(
          ValidationError(
            path,
            "/",
            f"tree file-count mismatch: expected {expected_count}, found {actual_count}",
          )
        )
      if actual_size != expected_size:
        errors.append(
          ValidationError(
            path,
            "/",
            f"tree size mismatch: expected {expected_size}, found {actual_size}",
          )
        )
      continue
    if not path.is_file():
      errors.append(ValidationError(path, "/", "input file does not exist"))
      continue
    if method == "version-output":
      # The timeout keeps a substituted or broken executable from hanging validation.
      try:
        process = subprocess.run(
          [str(path), *verification.get("arguments", [])],
          capture_output=True,
          text=True,
          check=False,
          timeout=10,
        )
      except (OSError, subprocess.TimeoutExpired) as error:
        errors.append(ValidationError(path, "/", f"version command failed: {error}"))
        continue
      actual_output = (process.stdout + process.stderr).strip()
      expected_output = verification.get("expected", "")
      allowed_exit_codes = verification.get("allowed_exit_codes", [0])
      if (
        process.returncode not in allowed_exit_codes
        or expected_output not in actual_output
      ):
        errors.append(
          ValidationError(
            path,
            "/",
            f"version output mismatch: expected {expected_output!r}, found {actual_output!r}",
          )
        )
      continue
    if method != "sha256":
      errors.append(
        ValidationError(
          source,
          f"/inputs/{escape_pointer(input_id)}/verification/method",
          "unsupported verification method",
        )
      )
      continue
    actual_hash, actual_size = sha256_file(path)
    expected_hash = verification["sha256"]
    expected_size = verification.get("size_bytes")
    if actual_hash != expected_hash:
      errors.append(
        ValidationError(
          path,
          "/",
          f"SHA-256 mismatch: expected {expected_hash}, found {actual_hash}",
        )
      )
    if expected_size is not None and actual_size != expected_size:
      errors.append(
        ValidationError(
          path,
          "/",
          f"size mismatch: expected {expected_size}, found {actual_size}",
        )
      )
  return errors


def discover_documents(root: Path) -> Iterable[tuple[Path, Path]]:
  """Yield every governed baseline document with its versioned schema."""
  for document, schema in DOCUMENT_SCHEMAS.items():
    yield root / document, root / schema
  for collection, (pattern, schema) in COLLECTION_SCHEMAS.items():
    directory = root / collection
    if directory.is_dir():
      for document in sorted(directory.rglob(pattern)):
        yield document, root / schema


def validate_repository(
  root: Path,
  verification_requests: Mapping[str, Path],
  parity_gate: bool,
) -> list[ValidationError]:
  """Validate schemas, governed documents, semantic policy, and requested hashes."""
  errors: list[ValidationError] = []
  loaded: dict[Path, Any] = {}

  for document_path, schema_path in discover_documents(root):
    try:
      document = load_json(document_path)
    except (OSError, json.JSONDecodeError) as error:
      errors.append(ValidationError(document_path, "/", str(error)))
      continue
    try:
      schema = load_json(schema_path)
    except (OSError, json.JSONDecodeError) as error:
      errors.append(ValidationError(schema_path, "/", str(error)))
      continue
    loaded[document_path] = document
    errors.extend(validate_instance(document, schema, document_path))

    baseline_revision = document.get("baseline_revision") if isinstance(document, dict) else None
    if baseline_revision != BASELINE_REVISION:
      errors.append(
        ValidationError(
          document_path,
          "/baseline_revision",
          f"stale baseline revision; expected {BASELINE_REVISION}",
        )
      )
    schema_revision = document.get("schema_version") if isinstance(document, dict) else None
    if schema_revision != SCHEMA_REVISION:
      errors.append(
        ValidationError(
          document_path,
          "/schema_version",
          f"stale schema revision; expected {SCHEMA_REVISION}",
        )
      )

  input_path = root / "verification/baseline/implementation-inputs.json"
  policy_path = root / "verification/baseline/compliance-policy.json"
  register_path = root / "verification/discrepancies/register.json"
  matrix_path = root / "verification/parity/coverage-matrix.json"

  if input_path in loaded:
    errors.extend(validate_input_semantics(loaded[input_path], input_path))
    for input_index, entry in enumerate(loaded[input_path].get("inputs", [])):
      for patch_index, patch in enumerate(entry.get("patches", [])):
        errors.extend(
          validate_local_artifact_reference(
            input_path,
            f"/inputs/{input_index}/patches/{patch_index}",
            patch,
          )
        )
    errors.extend(
      verify_acquired_inputs(verification_requests, loaded[input_path], input_path)
    )
  if policy_path in loaded:
    errors.extend(validate_compliance_semantics(loaded[policy_path], policy_path))
  if register_path in loaded:
    errors.extend(validate_discrepancy_semantics(loaded[register_path], register_path))
  if matrix_path in loaded:
    errors.extend(validate_matrix_semantics(loaded[matrix_path], matrix_path, parity_gate))

  fixture_root = root / "verification/fixtures"
  for path, document in loaded.items():
    if path.is_relative_to(fixture_root):
      errors.extend(validate_fixture_semantics(document, path))

  errors.extend(validate_cross_references(root, loaded))

  return errors


def build_parser() -> argparse.ArgumentParser:
  """Build the stable command-line interface for local and CI validation."""
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument(
    "--root",
    type=Path,
    default=Path(__file__).resolve().parents[1],
    help="repository root (defaults to the validator's parent repository)",
  )
  parser.add_argument(
    "--verify-input",
    action="append",
    default=[],
    metavar="INPUT_ID=PATH",
    help="verify one acquired payload or versioned tool against the manifest",
  )
  parser.add_argument(
    "--parity-gate",
    action="store_true",
    help="also reject unresolved, missing, or temporary parity coverage",
  )
  return parser


def main(argv: Sequence[str] | None = None) -> int:
  """Run validation and return a process-friendly success or failure status."""
  parser = build_parser()
  arguments = parser.parse_args(argv)
  root = arguments.root.resolve()
  try:
    requests = parse_verification_arguments(arguments.verify_input)
  except ValueError as error:
    parser.error(str(error))

  errors = validate_repository(root, requests, arguments.parity_gate)
  if errors:
    for error in errors:
      print(error.render(root), file=sys.stderr)
    print(f"baseline validation failed with {len(errors)} error(s)", file=sys.stderr)
    return 1

  print("baseline validation passed")
  return 0


if __name__ == "__main__":
  raise SystemExit(main())
