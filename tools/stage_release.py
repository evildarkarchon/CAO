"""Build and stage only Tracetide's two production composition roots."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Sequence

from verify_workspace import ContractError, validate as validate_workspace


TARGET = "x86_64-pc-windows-msvc"
# Keep this independent from Cargo metadata so changing metadata alone cannot widen staging.
RELEASE_BINARIES = {
  "cao-gui": ("cao-gui.exe", Path("tracetide.exe")),
  "cao-hkx-helper": (
    "cao-hkx-helper.exe",
    Path("bin/tracetide-hkx-helper.exe"),
  ),
}


class OfflineInputError(RuntimeError):
  """Report a missing or incompatible pre-acquired native build input."""


def verify_git_source(path: Path, expected_revision: str) -> None:
  """Require one clean source repository at its exact locked revision.

  Raises:
    OfflineInputError: The path is not a clean checkout of the locked revision.
    OSError: Git cannot be launched.
  """
  git = os.environ.get("GIT", "git")
  revision = subprocess.run(
    [git, "-C", str(path), "rev-parse", "HEAD"],
    capture_output=True,
    text=True,
    check=False,
  )
  if revision.returncode != 0 or revision.stdout.strip() != expected_revision:
    raise OfflineInputError(
      f"source repository must be at {expected_revision}: {path}"
    )
  status = subprocess.run(
    [git, "-C", str(path), "status", "--porcelain", "--untracked-files=all"],
    capture_output=True,
    text=True,
    check=False,
  )
  if status.returncode != 0 or status.stdout.strip():
    raise OfflineInputError(f"source repository must be clean: {path}")


def load_skia_source_lock(root: Path) -> dict[str, object]:
  """Load the source lock already authenticated by workspace validation.

  Raises:
    OfflineInputError: The authenticated source lock cannot be decoded.
  """
  path = root / "verification/build-inputs/skia-source-lock.json"
  try:
    source_lock = json.loads(path.read_text(encoding="utf-8"))
  except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise OfflineInputError(f"cannot load Skia source lock: {error}") from error
  if not isinstance(source_lock, dict):
    raise OfflineInputError("Skia source lock must be a JSON object")
  return source_lock


def offline_build_environment(root: Path) -> dict[str, str]:
  """Validate and return the environment that prevents Skia network fallback.

  Raises:
    OfflineInputError: Skia sources or pinned Ninja 1.13.2 are unavailable.
    OSError: The configured Ninja executable cannot be launched.
  """
  environment = os.environ.copy()
  source_value = environment.get("SKIA_SOURCE_DIR")
  ninja_value = environment.get("SKIA_NINJA_COMMAND")
  if source_value is None or ninja_value is None:
    raise OfflineInputError(
      "set SKIA_SOURCE_DIR and SKIA_NINJA_COMMAND to the pre-acquired "
      "Skia source closure and pinned Ninja executable"
    )

  source_directory = Path(source_value).resolve()
  ninja = Path(ninja_value).resolve()
  required_source_files = (
    source_directory / "DEPS",
    source_directory / "include/core/SkCanvas.h",
  )
  missing = [path for path in required_source_files if not path.is_file()]
  if missing:
    raise OfflineInputError(
      "SKIA_SOURCE_DIR is incomplete: " + ", ".join(str(path) for path in missing)
    )
  if not ninja.is_file():
    raise OfflineInputError(f"SKIA_NINJA_COMMAND is not a file: {ninja}")

  result = subprocess.run(
    [str(ninja), "--version"],
    capture_output=True,
    text=True,
    check=False,
  )
  if result.returncode != 0 or result.stdout.strip() != "1.13.2":
    raise OfflineInputError(
      "SKIA_NINJA_COMMAND must identify pinned Ninja 1.13.2"
    )

  source_lock = load_skia_source_lock(root)
  locked_root = source_lock["root"]
  if not isinstance(locked_root, dict):
    raise OfflineInputError("Skia source lock root must be a JSON object")
  verify_git_source(source_directory, str(locked_root["revision"]))
  repositories = source_lock["repositories"]
  if not isinstance(repositories, list):
    raise OfflineInputError("Skia source lock repositories must be a JSON array")
  for repository in repositories:
    if not isinstance(repository, dict):
      raise OfflineInputError("Skia source lock repository must be a JSON object")
    verify_git_source(
      source_directory / str(repository["path"]),
      str(repository["revision"]),
    )

  # An explicit local source path keeps skia-bindings out of its download branch.
  environment["SKIA_SOURCE_DIR"] = str(source_directory)
  environment["SKIA_NINJA_COMMAND"] = str(ninja)
  return environment


def build_release(root: Path) -> None:
  """Build exactly the GUI and HKX helper from the frozen offline graph.

  Raises:
    OfflineInputError: Required offline native build inputs are unavailable.
    subprocess.CalledProcessError: Cargo cannot build the frozen release graph.
    OSError: Cargo or Ninja cannot be launched.
  """
  cargo = os.environ.get("CARGO", "cargo")
  environment = offline_build_environment(root)
  command = [
    cargo,
    "build",
    "--frozen",
    "--release",
    "--target",
    TARGET,
  ]
  for package_name in RELEASE_BINARIES:
    command.extend(("--package", package_name))
  subprocess.run(
    command,
    cwd=root,
    env=environment,
    check=True,
  )


def cargo_target_directory(root: Path) -> Path:
  """Resolve Cargo's target directory using Cargo's relative-path semantics."""
  configured = os.environ.get("CARGO_TARGET_DIR")
  if configured is None:
    return root / "target"
  target_directory = Path(configured)
  if not target_directory.is_absolute():
    target_directory = root / target_directory
  return target_directory.resolve()


def stage_binaries(root: Path, destination: Path) -> None:
  """Copy the two allow-listed executables into a new staging directory.

  Raises:
    FileExistsError: The destination already exists and might contain stale files.
    FileNotFoundError: A required composition-root executable was not built.
    OSError: A staging directory or binary cannot be created or copied.
  """
  if destination.exists():
    raise FileExistsError(
      f"staging destination already exists; provide a new path: {destination}"
    )
  source_directory = cargo_target_directory(root) / TARGET / "release"
  missing = [
    source_directory / source_name
    for source_name, _ in RELEASE_BINARIES.values()
    if not (source_directory / source_name).is_file()
  ]
  if missing:
    raise FileNotFoundError(
      "release build did not produce required binaries: "
      + ", ".join(str(path) for path in missing)
    )

  destination.mkdir(parents=True)
  # Exact paths, rather than executable globs, keep tools and stale artifacts out.
  for source_name, staged_path in RELEASE_BINARIES.values():
    output_path = destination / staged_path
    output_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source_directory / source_name, output_path)


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
  """Parse the required staging destination."""
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument(
    "--output",
    type=Path,
    required=True,
    help="New directory that will receive the staged executables",
  )
  return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
  """Build the frozen release graph and stage its allow-listed executables."""
  options = parse_arguments(arguments if arguments is not None else sys.argv[1:])
  root = Path(__file__).resolve().parents[1]
  destination = options.output.resolve()
  try:
    validate_workspace(root)
    build_release(root)
    stage_binaries(root, destination)
  except (
    OSError,
    ContractError,
    OfflineInputError,
    subprocess.CalledProcessError,
  ) as error:
    print(f"release staging failed: {error}", file=sys.stderr)
    return 1
  print(f"Staged Tracetide production binaries in {destination}")
  return 0


if __name__ == "__main__":
  raise SystemExit(main())
