"""Build all twelve Tracetide packages from authenticated offline inputs."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence

from stage_release import OfflineInputError, offline_build_environment
from verify_workspace import (
    ContractError,
    validate as validate_workspace,
    validate_skia_source_lock,
)


TARGET = "x86_64-pc-windows-msvc"


def build_workspace(root: Path, environment: Mapping[str, str]) -> None:
    """Build the complete locked workspace without any acquisition fallback.

    ``environment`` must be the authenticated result of
    :func:`offline_build_environment` for ``root``.

    Raises:
      subprocess.CalledProcessError: Cargo cannot build the frozen workspace.
      OSError: Cargo or a native build tool cannot be launched.
    """
    cargo = environment["CAO_CARGO_COMMAND"]
    subprocess.run(
        [
            cargo,
            "build",
            "--workspace",
            "--frozen",
            "--target",
            TARGET,
        ],
        cwd=root,
        env=environment,
        check=True,
    )


def main(arguments: Sequence[str] | None = None) -> int:
    """Validate and build the workspace, returning a process exit code."""
    if arguments is None:
        arguments = sys.argv[1:]
    if arguments:
        print("offline workspace build takes no arguments", file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parents[1]
    try:
        validate_skia_source_lock(root)
        environment = offline_build_environment(root)
        validate_workspace(root, environment)
        build_workspace(root, environment)
    except (
        OSError,
        ContractError,
        OfflineInputError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"offline workspace build failed: {error}", file=sys.stderr)
        return 1
    print("Built all twelve Tracetide packages from authenticated offline inputs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
