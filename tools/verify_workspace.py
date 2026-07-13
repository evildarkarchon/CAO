"""Validate Tracetide's Rust workspace and release-root contract offline."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tomllib
from pathlib import Path
from typing import Any, Mapping, Sequence


EXPECTED_MEMBERS = (
  "crates/cao-domain",
  "crates/cao-application",
  "crates/cao-platform-windows",
  "crates/cao-backend-bsa",
  "crates/cao-backend-nif",
  "crates/cao-backend-texture",
  "crates/cao-backend-hkx",
  "crates/cao-hkx-protocol",
  "apps/cao-gui",
  "apps/cao-hkx-helper",
  "tools/cao-verification",
  "tools/cao-oracle-capture",
)
# Behavioral-oracle capture is restricted and must remain an explicit opt-in build.
EXPECTED_DEFAULT_MEMBERS = EXPECTED_MEMBERS[:-1]
EXPECTED_PACKAGE_NAMES = {
  path: path.rsplit("/", 1)[-1] for path in EXPECTED_MEMBERS
}
EXPECTED_LIBRARY_TARGETS = {
  path: {"name": EXPECTED_PACKAGE_NAMES[path].replace("-", "_"), "path": "src/lib.rs"}
  for path in (
    "crates/cao-domain",
    "crates/cao-application",
    "crates/cao-platform-windows",
    "crates/cao-backend-bsa",
    "crates/cao-backend-nif",
    "crates/cao-backend-texture",
    "crates/cao-backend-hkx",
    "crates/cao-hkx-protocol",
    "tools/cao-verification",
  )
}
EXPECTED_BINARY_TARGETS = {
  path: [{"name": EXPECTED_PACKAGE_NAMES[path], "path": "src/main.rs"}]
  for path in (
    "apps/cao-gui",
    "apps/cao-hkx-helper",
    "tools/cao-verification",
    "tools/cao-oracle-capture",
  )
}
EXPECTED_WORKSPACE_PATH_DEPENDENCIES = {
  "cao-domain": "crates/cao-domain",
  "cao-application": "crates/cao-application",
  "cao-platform-windows": "crates/cao-platform-windows",
  "cao-backend-bsa": "crates/cao-backend-bsa",
  "cao-backend-nif": "crates/cao-backend-nif",
  "cao-backend-texture": "crates/cao-backend-texture",
  "cao-backend-hkx": "crates/cao-backend-hkx",
  "cao-hkx-protocol": "crates/cao-hkx-protocol",
  "cao-verification": "tools/cao-verification",
}
EXPECTED_DEPENDENCIES = {
  "crates/cao-domain": set(),
  "crates/cao-application": {"cao-domain", "crossbeam-channel"},
  "crates/cao-platform-windows": {"cao-application"},
  "crates/cao-backend-bsa": {"ba2", "cao-application"},
  "crates/cao-backend-nif": {"cao-application"},
  "crates/cao-backend-texture": {"cao-application"},
  "crates/cao-backend-hkx": {"cao-application", "cao-hkx-protocol"},
  "crates/cao-hkx-protocol": set(),
  "apps/cao-gui": {
    "cao-application",
    "cao-backend-bsa",
    "cao-backend-hkx",
    "cao-backend-nif",
    "cao-backend-texture",
    "cao-platform-windows",
    "slint",
  },
  "apps/cao-hkx-helper": {
    "cao-hkx-protocol",
    "havok_classes",
    "serde_hkx",
  },
  "tools/cao-verification": {
    "cao-application",
    "cao-backend-bsa",
    "cao-backend-hkx",
    "cao-backend-nif",
    "cao-backend-texture",
    "cao-platform-windows",
  },
  "tools/cao-oracle-capture": {"cao-verification"},
}
EXPECTED_RELEASE_PACKAGES = ["cao-gui", "cao-hkx-helper"]
EXPECTED_STAGED_BINARIES = {
  "cao-gui": "tracetide.exe",
  "cao-hkx-helper": "bin/tracetide-hkx-helper.exe",
}
EXPECTED_UNSAFE_ALLOWLIST = [
  "cao-platform-windows",
  "cao-backend-nif",
  "cao-backend-texture",
]
WORKSPACE_INHERITED_PACKAGE_FIELDS = (
  "version",
  "edition",
  "rust-version",
  "license",
  "repository",
  "publish",
)
DISABLED_CARGO_AUTO_TARGET_FIELDS = (
  "autolib",
  "autobins",
  "autoexamples",
  "autotests",
  "autobenches",
)
EXPECTED_VENDOR_TREE_DIGESTS = {
  "vendor/ba2-3.0.1": (
    "8552a81b2741170eab22ffb6a152bfd16974ea12c8abcfc459835f46265869bd"
  ),
  "vendor/serde-hkx": (
    "ae57b1a5709cece9d9a9e9245db41c8e52cbd99949ecd228cf02c07241b95c8c"
  ),
}
EXPECTED_SKIA_SOURCE_LOCK_SHA256 = (
  "3b0b4cd008d87fd2ce605aa583c522a568435b2faff4df326b19501292e24976"
)


class ContractError(RuntimeError):
  """Report one workspace contract violation with an actionable message."""


def load_toml(path: Path) -> dict[str, Any]:
  """Load a required UTF-8 TOML document.

  Raises:
    ContractError: The file is missing or is not valid TOML.
  """
  try:
    return tomllib.loads(path.read_text(encoding="utf-8"))
  except (OSError, UnicodeError) as error:
    raise ContractError(f"cannot read required UTF-8 file {path}: {error}") from error
  except tomllib.TOMLDecodeError as error:
    raise ContractError(f"invalid TOML in {path}: {error}") from error


def load_json(path: Path) -> Any:
  """Load a required UTF-8 JSON document.

  Raises:
    ContractError: The file is missing, unreadable, or not valid JSON.
  """
  try:
    return json.loads(path.read_text(encoding="utf-8"))
  except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise ContractError(f"cannot load required JSON file {path}: {error}") from error


def require_equal(actual: object, expected: object, label: str) -> None:
  """Require exact equality so package or dependency drift fails closed.

  Raises:
    ContractError: The actual value differs from the frozen contract.
  """
  if actual != expected:
    raise ContractError(f"{label}: expected {expected!r}, found {actual!r}")


def source_tree_digest(root: Path) -> str:
  """Hash every path and byte in one vendored production source tree.

  Relative-path and file-size framing makes the digest independent of traversal
  order while preventing ambiguous path/content concatenations.

  Raises:
    ContractError: A source entry is a symlink or cannot be read.
  """
  digest = hashlib.sha256()
  paths = sorted(root.rglob("*"), key=lambda path: path.relative_to(root).as_posix())
  for path in paths:
    if path.is_symlink():
      raise ContractError(f"vendored source trees cannot contain symlinks: {path}")
    if not path.is_file():
      continue
    relative_path = path.relative_to(root).as_posix().encode("utf-8")
    try:
      content = path.read_bytes()
    except OSError as error:
      raise ContractError(f"cannot read vendored source file {path}: {error}") from error
    digest.update(len(relative_path).to_bytes(8, "big"))
    digest.update(relative_path)
    digest.update(len(content).to_bytes(8, "big"))
    digest.update(content)
  return digest.hexdigest()


def validate_toolchain(root: Path) -> None:
  """Validate the exact Rust release, components, and Windows target.

  Raises:
    ContractError: A toolchain or static-CRT setting is missing or changed.
  """
  toolchain = load_toml(root / "rust-toolchain.toml").get("toolchain", {})
  require_equal(toolchain.get("channel"), "1.97.0", "Rust toolchain channel")
  require_equal(toolchain.get("profile"), "minimal", "Rust toolchain profile")
  require_equal(
    toolchain.get("components"), ["clippy", "rustfmt"], "Rust components"
  )
  require_equal(
    toolchain.get("targets"),
    ["x86_64-pc-windows-msvc"],
    "Rust compilation targets",
  )

  cargo_config = load_toml(root / ".cargo/config.toml")
  require_equal(
    cargo_config.get("build", {}).get("target"),
    "x86_64-pc-windows-msvc",
    "Cargo default target",
  )
  require_equal(
    cargo_config.get("target", {})
    .get("x86_64-pc-windows-msvc", {})
    .get("rustflags"),
    ["-C", "target-feature=+crt-static"],
    "static CRT rustflags",
  )


def validate_dependency_pin(
  dependencies: Mapping[str, Any],
  name: str,
  expected: Mapping[str, Any],
) -> None:
  """Validate one direct dependency without accepting broad versions.

  Raises:
    ContractError: The dependency table is missing, broad, or otherwise changed.
  """
  dependency = dependencies.get(name)
  if not isinstance(dependency, dict):
    raise ContractError(f"workspace dependency {name!r} must use an explicit table")
  for field, value in expected.items():
    require_equal(dependency.get(field), value, f"{name} dependency field {field}")
  require_equal(
    set(dependency), set(expected), f"{name} dependency fields"
  )


def validate_workspace_manifest(root: Path) -> None:
  """Validate root membership, shared policy, source pins, and release metadata.

  Raises:
    ContractError: Any root workspace contract value has drifted.
  """
  manifest = load_toml(root / "Cargo.toml")
  require_equal(set(manifest), {"workspace"}, "root manifest top-level tables")
  workspace = manifest.get("workspace", {})
  require_equal(
    set(workspace),
    {
      "members",
      "default-members",
      "exclude",
      "resolver",
      "package",
      "dependencies",
      "metadata",
    },
    "workspace table fields",
  )
  require_equal(workspace.get("members"), list(EXPECTED_MEMBERS), "workspace members")
  require_equal(
    workspace.get("default-members"),
    list(EXPECTED_DEFAULT_MEMBERS),
    "workspace default members",
  )
  require_equal(
    workspace.get("exclude"),
    ["vendor/ba2-3.0.1", "vendor/serde-hkx"],
    "vendored workspace exclusions",
  )
  require_equal(workspace.get("resolver"), "3", "Cargo resolver")

  package_policy = workspace.get("package", {})
  require_equal(
    set(package_policy),
    {"version", "edition", "rust-version", "license", "repository", "publish"},
    "workspace package policy fields",
  )
  require_equal(package_policy.get("version"), "0.0.0", "workspace package version")
  require_equal(package_policy.get("edition"), "2024", "workspace edition")
  require_equal(
    package_policy.get("rust-version"), "1.97.0", "workspace rust-version"
  )
  require_equal(package_policy.get("publish"), False, "workspace publish policy")

  dependencies = workspace.get("dependencies", {})
  require_equal(
    set(dependencies),
    {
      *EXPECTED_WORKSPACE_PATH_DEPENDENCIES,
      "crossbeam-channel",
      "ba2",
      "slint",
      "serde_hkx",
      "havok_classes",
    },
    "workspace dependency declarations",
  )
  for dependency_name, dependency_path in EXPECTED_WORKSPACE_PATH_DEPENDENCIES.items():
    validate_dependency_pin(
      dependencies,
      dependency_name,
      {"version": "=0.0.0", "path": dependency_path},
    )
  validate_dependency_pin(
    dependencies,
    "crossbeam-channel",
    {"version": "=0.5.16", "default-features": False, "features": ["std"]},
  )
  validate_dependency_pin(
    dependencies,
    "ba2",
    {
      "version": "=3.0.1",
      "path": "vendor/ba2-3.0.1",
      "default-features": False,
      "features": ["zlib"],
    },
  )
  validate_dependency_pin(
    dependencies,
    "slint",
    {
      "version": "=1.17.1",
      "default-features": False,
      "features": [
        "std",
        "compat-1-2",
        "accessibility",
        "backend-winit",
        "renderer-skia",
        "renderer-software",
      ],
    },
  )
  validate_dependency_pin(
    dependencies,
    "serde_hkx",
    {
      "version": "=1.0.1",
      "path": "vendor/serde-hkx/serde_hkx",
      "default-features": False,
    },
  )
  validate_dependency_pin(
    dependencies,
    "havok_classes",
    {
      "version": "=1.0.1",
      "path": "vendor/serde-hkx/crates/havok_classes",
      "default-features": False,
    },
  )

  workspace_metadata = workspace.get("metadata", {})
  require_equal(set(workspace_metadata), {"cao"}, "workspace metadata tables")
  metadata = workspace_metadata.get("cao", {})
  require_equal(
    set(metadata),
    {
      "implementation-inputs",
      "release-packages",
      "staged-binaries",
      "unsafe-allowed",
      "native-sources",
      "vendored-sources",
    },
    "cao workspace metadata fields",
  )
  require_equal(
    metadata.get("implementation-inputs"),
    "verification/baseline/implementation-inputs.json",
    "implementation-input manifest",
  )
  require_equal(
    metadata.get("release-packages"),
    EXPECTED_RELEASE_PACKAGES,
    "release package roots",
  )
  require_equal(
    metadata.get("staged-binaries"),
    EXPECTED_STAGED_BINARIES,
    "staged binary allowlist",
  )
  require_equal(
    metadata.get("unsafe-allowed"),
    EXPECTED_UNSAFE_ALLOWLIST,
    "unsafe package allowlist",
  )
  require_equal(
    metadata.get("native-sources"),
    {
      "nifly": "965a1da1be7bff145b7b3435def5c04d6e8c8cce",
      "directxtex": "4feb3e11a020f35b796fc769a74216a555d4f5ef",
      "skia": "90af6f5c5a3ed8788b805fe6ca3fcdf28c2c2bc9",
    },
    "native source pins",
  )
  require_equal(
    metadata.get("vendored-sources"),
    {
      "ba2": "3.0.1+explicit-zlib-feature",
      "serde-hkx": "6c1bee56d42de7def991cf6fba025a9df7492d83",
    },
    "vendored source pins",
  )


def validate_package_manifests(root: Path) -> None:
  """Validate every first-party package and the exact allowed dependency DAG.

  Raises:
    ContractError: A package identity, policy, or dependency edge has drifted.
  """
  for relative_path in EXPECTED_MEMBERS:
    manifest_path = root / relative_path / "Cargo.toml"
    manifest = load_toml(manifest_path)
    package = manifest.get("package", {})
    package_name = EXPECTED_PACKAGE_NAMES[relative_path]
    expected_tables = {"package", "dependencies"}
    if relative_path in EXPECTED_LIBRARY_TARGETS:
      expected_tables.add("lib")
    if relative_path in EXPECTED_BINARY_TARGETS:
      expected_tables.add("bin")
    require_equal(
      set(manifest),
      expected_tables,
      f"{package_name} manifest top-level tables",
    )
    require_equal(
      set(package),
      {
        "name",
        "description",
        *WORKSPACE_INHERITED_PACKAGE_FIELDS,
        *DISABLED_CARGO_AUTO_TARGET_FIELDS,
        "build",
      },
      f"{package_name} package fields",
    )
    require_equal(package.get("name"), package_name, f"{relative_path} package name")
    for field in WORKSPACE_INHERITED_PACKAGE_FIELDS:
      require_equal(
        package.get(field),
        {"workspace": True},
        f"{package_name} inherited {field}",
      )
    for field in DISABLED_CARGO_AUTO_TARGET_FIELDS:
      require_equal(package.get(field), False, f"{package_name} package {field}")
    require_equal(package.get("build"), False, f"{package_name} build script policy")

    if relative_path in EXPECTED_LIBRARY_TARGETS:
      require_equal(
        manifest.get("lib"),
        EXPECTED_LIBRARY_TARGETS[relative_path],
        f"{package_name} library target",
      )
    if relative_path in EXPECTED_BINARY_TARGETS:
      require_equal(
        manifest.get("bin"),
        EXPECTED_BINARY_TARGETS[relative_path],
        f"{package_name} binary targets",
      )

    dependencies = manifest.get("dependencies", {})
    require_equal(
      set(dependencies),
      EXPECTED_DEPENDENCIES[relative_path],
      f"{package_name} direct dependencies",
    )
    for dependency_name, dependency in dependencies.items():
      require_equal(
        dependency,
        {"workspace": True},
        f"{package_name} dependency {dependency_name}",
      )
    require_equal(
      set(manifest).intersection({"dev-dependencies", "build-dependencies"}),
      set(),
      f"{package_name} non-production dependency sections",
    )

    package_root = root / relative_path
    implicit_target_sources = [package_root / "build.rs"]
    for directory_name in ("src/bin", "examples", "tests", "benches"):
      directory = package_root / directory_name
      if directory.is_dir():
        implicit_target_sources.extend(directory.rglob("*.rs"))
    unexpected_sources = [path for path in implicit_target_sources if path.is_file()]
    if unexpected_sources:
      raise ContractError(
        f"{package_name} has undeclared Rust target sources: "
        + ", ".join(str(path) for path in unexpected_sources)
      )


def validate_unsafe_boundaries(root: Path) -> None:
  """Require crate-level unsafe policy on every first-party Rust target.

  Raises:
    ContractError: A target is absent or lacks its required unsafe-code policy.
  """
  allowed = set(EXPECTED_UNSAFE_ALLOWLIST)
  for relative_path, package_name in EXPECTED_PACKAGE_NAMES.items():
    source_directory = root / relative_path / "src"
    source_files = [
      path for path in (source_directory / "lib.rs", source_directory / "main.rs")
      if path.exists()
    ]
    if not source_files:
      raise ContractError(f"{package_name} has no Rust library or binary target")
    expected_attribute = (
      "#![deny(unsafe_op_in_unsafe_fn)]"
      if package_name in allowed
      else "#![forbid(unsafe_code)]"
    )
    for source_file in source_files:
      try:
        source = source_file.read_text(encoding="utf-8")
      except (OSError, UnicodeError) as error:
        raise ContractError(
          f"cannot read required UTF-8 file {source_file}: {error}"
        ) from error
      if expected_attribute not in source.splitlines()[:3]:
        raise ContractError(
          f"{source_file} must declare {expected_attribute} near the crate root"
        )


def validate_vendored_sources(root: Path) -> None:
  """Validate the local source adaptations required by the production graph.

  Raises:
    ContractError: A vendored pin or required source patch is absent or changed.
  """
  ba2_manifest = load_toml(root / "vendor/ba2-3.0.1/Cargo.toml")
  require_equal(ba2_manifest.get("package", {}).get("version"), "3.0.1", "ba2 version")
  require_equal(
    ba2_manifest.get("features"),
    {"default": [], "zlib": ["flate2/zlib"]},
    "ba2 explicit zlib feature patch",
  )

  vendoring_record = root / "vendor/serde-hkx/VENDORING.md"
  try:
    record = vendoring_record.read_text(encoding="utf-8")
  except (OSError, UnicodeError) as error:
    raise ContractError(
      f"cannot read required UTF-8 file {vendoring_record}: {error}"
    ) from error
  commit = "6c1bee56d42de7def991cf6fba025a9df7492d83"
  if commit not in record:
    raise ContractError("serde-hkx vendoring record does not contain the pinned commit")

  for relative_path, expected_digest in EXPECTED_VENDOR_TREE_DIGESTS.items():
    require_equal(
      source_tree_digest(root / relative_path),
      expected_digest,
      f"{relative_path} authenticated source tree SHA-256",
    )


def validate_lockfile(root: Path) -> None:
  """Validate that the committed production lockfile retains every selected pin.

  Raises:
    ContractError: The lockfile is missing a first-party or selected dependency pin.
  """
  lockfile = load_toml(root / "Cargo.lock")
  packages = lockfile.get("package", [])
  identities = {(package.get("name"), package.get("version")) for package in packages}
  required = {
    ("ba2", "3.0.1"),
    ("crossbeam-channel", "0.5.16"),
    ("directxtex", "1.3.0"),
    ("lzzzz", "1.0.4"),
    ("skia-bindings", "0.99.0"),
    ("slint", "1.17.1"),
    ("serde_hkx", "1.0.1"),
  }
  missing = sorted(required - identities)
  if missing:
    raise ContractError(f"Cargo.lock is missing selected production pins: {missing}")

  first_party_names = set(EXPECTED_PACKAGE_NAMES.values())
  locked_first_party = {
    name for name, version in identities if name in first_party_names and version == "0.0.0"
  }
  require_equal(
    locked_first_party,
    first_party_names,
    "first-party packages represented in Cargo.lock",
  )

  skia_bindings = [
    package
    for package in packages
    if package.get("name") == "skia-bindings"
    and package.get("version") == "0.99.0"
  ]
  require_equal(len(skia_bindings), 1, "locked skia-bindings package count")
  require_equal(
    skia_bindings[0].get("checksum"),
    "3e2d1c3ebd697c0cbded0145e9204a38fa6b268446051b7196d0a096414ea7f3",
    "locked skia-bindings checksum",
  )


def validate_skia_source_lock(root: Path) -> None:
  """Validate the build-script source closure selected by locked skia-bindings.

  Raises:
    ContractError: The source lock bytes or their selected package identity drift.
  """
  path = root / "verification/build-inputs/skia-source-lock.json"
  try:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
  except OSError as error:
    raise ContractError(f"cannot read required file {path}: {error}") from error
  require_equal(digest, EXPECTED_SKIA_SOURCE_LOCK_SHA256, "Skia source lock SHA-256")

  source_lock = load_json(path)
  require_equal(source_lock.get("schema_version"), 1, "Skia source lock schema")
  require_equal(
    source_lock.get("selected_by"),
    {
      "package": "skia-bindings",
      "version": "0.99.0",
      "cargo_lock_checksum": (
        "3e2d1c3ebd697c0cbded0145e9204a38fa6b268446051b7196d0a096414ea7f3"
      ),
    },
    "Skia source lock selector",
  )
  require_equal(
    source_lock.get("root", {}).get("revision"),
    "90af6f5c5a3ed8788b805fe6ca3fcdf28c2c2bc9",
    "Skia root revision",
  )


def validate_release_script(root: Path) -> None:
  """Require the allowlist-driven release staging entry point.

  Raises:
    ContractError: The release staging entry point is missing.
  """
  script = root / "tools/stage_release.py"
  if not script.is_file():
    raise ContractError(f"missing required file: {script}")


def validate(root: Path) -> None:
  """Validate every machine-verifiable W1 workspace invariant.

  Raises:
    ContractError: Any workspace, source, lockfile, or staging invariant fails.
  """
  validate_toolchain(root)
  validate_workspace_manifest(root)
  validate_package_manifests(root)
  validate_unsafe_boundaries(root)
  validate_vendored_sources(root)
  validate_lockfile(root)
  validate_skia_source_lock(root)
  validate_release_script(root)


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
  """Parse command-line arguments for offline workspace validation."""
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument(
    "--root",
    type=Path,
    default=Path(__file__).resolve().parents[1],
    help="Repository root to validate",
  )
  return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
  """Run the workspace validator and return a process exit code."""
  options = parse_arguments(arguments if arguments is not None else sys.argv[1:])
  root = options.root.resolve()
  try:
    validate(root)
  except ContractError as error:
    print(f"workspace contract violation: {error}", file=sys.stderr)
    return 1
  print("Rust workspace contract verified")
  return 0


if __name__ == "__main__":
  raise SystemExit(main())
