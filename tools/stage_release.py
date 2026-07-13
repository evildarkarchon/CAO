"""Build and stage only Tracetide's two production composition roots."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence

from verify_workspace import (
    ContractError,
    validate as validate_workspace,
    validate_skia_source_lock,
)


TARGET = "x86_64-pc-windows-msvc"
MSVC_VERSION = "14.51.36231"
MSVC_COMPILER_VERSION = "19.51.36231"
WINDOWS_SDK_VERSION = "10.0.26100.0"
FORBIDDEN_BUILD_OVERRIDES = {
    "CARGO",
    "CARGO_ENCODED_RUSTFLAGS",
    "CL",
    "CLANGCC",
    "CLANGCXX",
    "DOCS_RS",
    "FORCE_SKIA_BINARIES_DOWNLOAD",
    "LIBCLANG_PATH",
    "LINK",
    "RUSTC",
    "RUSTC_BOOTSTRAP",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "_CL_",
}
ALLOWED_SKIA_ENVIRONMENT = {
    "SKIA_GN_COMMAND",
    "SKIA_NINJA_COMMAND",
    "SKIA_SOURCE_DIR",
}
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


def is_forbidden_toolchain_override(variable: str) -> bool:
    """Return whether an inherited variable can outrank authenticated tools."""
    name = variable.upper()
    normalized_name = name.replace("-", "_")
    if name in FORBIDDEN_BUILD_OVERRIDES:
        return True
    if name.startswith("BINDGEN_EXTRA_CLANG_ARGS"):
        return True
    compiler_names = ("AR", "ARFLAGS", "CC", "CFLAGS", "CPPFLAGS", "CXX", "CXXFLAGS")
    if any(
        name == compiler
        or name.startswith(f"{compiler}_")
        or name.startswith(f"HOST_{compiler}")
        or name.startswith(f"TARGET_{compiler}")
        for compiler in compiler_names
    ):
        return True
    return normalized_name.startswith(
        "CARGO_BUILD_RUSTC"
    ) or normalized_name.startswith("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_")


def verify_file_digest(
    path: Path,
    expected_sha256: str,
    label: str,
    expected_size: int | None = None,
) -> None:
    """Authenticate one pre-acquired build executable by size and SHA-256.

    Raises:
      OfflineInputError: The executable is absent or its bytes do not match.
    """
    try:
        if not path.is_file():
            raise OfflineInputError(f"{label} is not a file: {path}")
        if expected_size is not None and path.stat().st_size != expected_size:
            raise OfflineInputError(f"{label} size mismatch: {path}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise OfflineInputError(f"cannot read {label}: {path}: {error}") from error
    if digest != expected_sha256:
        raise OfflineInputError(f"{label} SHA-256 mismatch: {path}")


def verify_git_source(path: Path, expected_revision: str, git_command: Path) -> None:
    """Require one clean source repository at its exact locked revision.

    ``git_command`` must be the absolute runner command selected by the
    authenticated offline environment.

    Raises:
      OfflineInputError: The path is not a clean checkout of the locked revision.
      OSError: Git cannot be launched.
    """
    revision = subprocess.run(
        [str(git_command), "-C", str(path), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    if revision.returncode != 0 or revision.stdout.strip() != expected_revision:
        raise OfflineInputError(
            f"source repository must be at {expected_revision}: {path}"
        )
    status = subprocess.run(
        [
            str(git_command),
            "-C",
            str(path),
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
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
    """Build the authenticated offline Cargo/Rustup, rustc, Git, and native environment.

    The source-lock bytes must already have passed ``validate_skia_source_lock``.
    Returned variables remove higher-priority compiler/source overrides and bind
    every production subprocess to the reviewed local inputs.

    Raises:
      OfflineInputError: A Cargo/Rustup, rustc, Git, native, or Windows input is untrusted.
      OSError: A configured compiler, source-control, or native tool cannot launch.
    """
    environment = os.environ.copy()
    required_variables = (
        "SKIA_SOURCE_DIR",
        "SKIA_NINJA_COMMAND",
        "SKIA_GN_COMMAND",
        "LLVM_HOME",
        "CAO_MSVC_TOOLCHAIN_DIR",
        "CAO_WINDOWS_SDK_DIR",
        "CAO_CARGO_COMMAND",
        "CAO_GIT_COMMAND",
        "CAO_RUSTC_COMMAND",
        "CAO_CARGO_HOME",
        "CAO_RUSTUP_HOME",
    )
    missing_variables = [
        variable for variable in required_variables if not environment.get(variable)
    ]
    if missing_variables:
        raise OfflineInputError(
            "set every authenticated offline build input before staging: "
            + ", ".join(missing_variables)
        )
    forbidden_variables = sorted(
        variable
        for variable, value in environment.items()
        if value
        and (
            is_forbidden_toolchain_override(variable)
            or (
                variable.startswith("SKIA_")
                and variable not in ALLOWED_SKIA_ENVIRONMENT
            )
        )
    )
    if forbidden_variables:
        raise OfflineInputError(
            "remove inherited build overrides before staging: "
            + ", ".join(forbidden_variables)
        )

    source_directory = Path(environment["SKIA_SOURCE_DIR"]).resolve()
    ninja = Path(environment["SKIA_NINJA_COMMAND"]).resolve()
    gn = Path(environment["SKIA_GN_COMMAND"]).resolve()
    llvm_home = Path(environment["LLVM_HOME"]).resolve()
    msvc_home = Path(environment["CAO_MSVC_TOOLCHAIN_DIR"]).resolve()
    windows_sdk = Path(environment["CAO_WINDOWS_SDK_DIR"]).resolve()
    configured_cargo_command = Path(environment["CAO_CARGO_COMMAND"])
    configured_git_command = Path(environment["CAO_GIT_COMMAND"])
    configured_rustc_command = Path(environment["CAO_RUSTC_COMMAND"])
    if not configured_cargo_command.is_absolute():
        raise OfflineInputError("CAO_CARGO_COMMAND must be an absolute path")
    if not configured_git_command.is_absolute():
        raise OfflineInputError("CAO_GIT_COMMAND must be an absolute path")
    if not configured_rustc_command.is_absolute():
        raise OfflineInputError("CAO_RUSTC_COMMAND must be an absolute path")
    # Preserve rustup's cargo proxy filename; resolving its link yields rustup.exe.
    cargo_command = configured_cargo_command.absolute()
    git_command = configured_git_command.absolute()
    rustc_command = configured_rustc_command.absolute()
    cargo_home = Path(environment["CAO_CARGO_HOME"]).resolve()
    rustup_home = Path(environment["CAO_RUSTUP_HOME"]).resolve()
    if not cargo_command.is_file():
        raise OfflineInputError(
            f"CAO_CARGO_COMMAND must be an absolute executable: {cargo_command}"
        )
    if not git_command.is_file():
        raise OfflineInputError(
            f"CAO_GIT_COMMAND must be an absolute executable: {git_command}"
        )
    if not rustc_command.is_file():
        raise OfflineInputError(
            f"CAO_RUSTC_COMMAND must be an absolute executable: {rustc_command}"
        )
    if not cargo_home.is_dir() or not rustup_home.is_dir():
        raise OfflineInputError(
            "CAO_CARGO_HOME and CAO_RUSTUP_HOME must be authenticated directories"
        )
    # Home-level Cargo config can redirect sources or inject wrappers ahead of repo policy.
    unreviewed_cargo_configs = [
        path
        for path in (cargo_home / "config", cargo_home / "config.toml")
        if path.exists()
    ]
    if unreviewed_cargo_configs:
        raise OfflineInputError(
            "authenticated Cargo home must not contain ambient configuration: "
            + ", ".join(str(path) for path in unreviewed_cargo_configs)
        )
    command_environment = environment.copy()
    command_environment["CARGO_HOME"] = str(cargo_home)
    command_environment["RUSTUP_HOME"] = str(rustup_home)
    command_environment.pop("RUSTUP_TOOLCHAIN", None)
    cargo_version = subprocess.run(
        [str(cargo_command), "--version"],
        cwd=root,
        env=command_environment,
        capture_output=True,
        text=True,
        check=False,
    )
    if cargo_version.returncode != 0 or not cargo_version.stdout.startswith(
        "cargo 1.97.0 "
    ):
        raise OfflineInputError(
            f"Cargo command does not report the locked 1.97.0 release: {cargo_command}"
        )
    rustc_version = subprocess.run(
        [str(rustc_command), "--version"],
        cwd=root,
        env=command_environment,
        capture_output=True,
        text=True,
        check=False,
    )
    if rustc_version.returncode != 0 or not rustc_version.stdout.startswith(
        "rustc 1.97.0 "
    ):
        raise OfflineInputError(
            f"Rust compiler does not report the locked 1.97.0 release: {rustc_command}"
        )
    git_version = subprocess.run(
        [str(git_command), "--version"],
        env=command_environment,
        capture_output=True,
        text=True,
        check=False,
    )
    if git_version.returncode != 0 or not git_version.stdout.startswith("git version "):
        raise OfflineInputError(f"Git command is not usable: {git_command}")
    source_lock = load_skia_source_lock(root)
    tools = source_lock.get("tools")
    if not isinstance(tools, dict):
        raise OfflineInputError("Skia source lock tools must be a JSON object")
    gn_lock = tools.get("gn")
    llvm_lock = tools.get("llvm")
    if not isinstance(gn_lock, dict) or not isinstance(llvm_lock, dict):
        raise OfflineInputError("Skia source lock must pin GN and LLVM")

    verify_file_digest(
        gn,
        str(gn_lock["executable_sha256"]),
        "GN executable",
        int(gn_lock["size_bytes"]),
    )
    gn_version = subprocess.run(
        [str(gn), "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    if gn_version.returncode != 0 or gn_version.stdout.strip() != gn_lock["version"]:
        raise OfflineInputError(f"GN version does not match the source lock: {gn}")

    clang_cl = llvm_home / "bin/clang-cl.exe"
    verify_file_digest(
        clang_cl,
        str(llvm_lock["clang_cl_sha256"]),
        "LLVM clang-cl executable",
    )
    clang_version = subprocess.run(
        [str(clang_cl), "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    expected_clang_identity = (
        f"clang version {llvm_lock['version']} "
        f"(https://github.com/llvm/llvm-project {llvm_lock['revision']})"
    )
    if (
        clang_version.returncode != 0
        or expected_clang_identity not in clang_version.stdout.splitlines()
        or not (llvm_home / f"lib/clang/{llvm_lock['version']}").is_dir()
    ):
        raise OfflineInputError(
            f"LLVM installation does not match the source lock: {llvm_home}"
        )

    msvc_bin = msvc_home / "bin/Hostx64/x64"
    cl = msvc_bin / "cl.exe"
    linker = msvc_bin / "link.exe"
    librarian = msvc_bin / "lib.exe"
    required_msvc_paths = (
        cl,
        linker,
        librarian,
        msvc_home / "include",
        msvc_home / "lib/x64",
    )
    if any(not path.exists() for path in required_msvc_paths):
        raise OfflineInputError(
            f"MSVC {MSVC_VERSION} installation is incomplete: {msvc_home}"
        )
    compiler_version = subprocess.run(
        [str(cl)],
        capture_output=True,
        text=True,
        check=False,
    )
    if MSVC_COMPILER_VERSION not in (compiler_version.stdout + compiler_version.stderr):
        raise OfflineInputError(
            f"MSVC compiler does not report {MSVC_COMPILER_VERSION}: {cl}"
        )

    sdk_include = windows_sdk / f"Include/{WINDOWS_SDK_VERSION}"
    sdk_lib = windows_sdk / f"Lib/{WINDOWS_SDK_VERSION}"
    sdk_bin = windows_sdk / f"bin/{WINDOWS_SDK_VERSION}/x64"
    required_sdk_paths = (
        sdk_include / "ucrt",
        sdk_include / "shared",
        sdk_include / "um",
        sdk_include / "winrt",
        sdk_lib / "ucrt/x64",
        sdk_lib / "um/x64",
        sdk_bin / "rc.exe",
    )
    if any(not path.exists() for path in required_sdk_paths):
        raise OfflineInputError(
            f"Windows SDK {WINDOWS_SDK_VERSION} installation is incomplete: {windows_sdk}"
        )

    if sys.version_info[:3] != (3, 14, 5):
        raise OfflineInputError("offline Skia builds require pinned Python 3.14.5")
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
        raise OfflineInputError("SKIA_NINJA_COMMAND must identify pinned Ninja 1.13.2")

    locked_root = source_lock["root"]
    if not isinstance(locked_root, dict):
        raise OfflineInputError("Skia source lock root must be a JSON object")
    verify_git_source(source_directory, str(locked_root["revision"]), git_command)
    repositories = source_lock["repositories"]
    if not isinstance(repositories, list):
        raise OfflineInputError("Skia source lock repositories must be a JSON array")
    for repository in repositories:
        if not isinstance(repository, dict):
            raise OfflineInputError("Skia source lock repository must be a JSON object")
        verify_git_source(
            source_directory / str(repository["path"]),
            str(repository["revision"]),
            git_command,
        )

    # An explicit local source path keeps skia-bindings out of its download branch.
    environment["SKIA_SOURCE_DIR"] = str(source_directory)
    environment["SKIA_NINJA_COMMAND"] = str(ninja)
    environment["SKIA_GN_COMMAND"] = str(gn)
    environment["LLVM_HOME"] = str(llvm_home)
    environment["CAO_CARGO_COMMAND"] = str(cargo_command)
    environment["CAO_GIT_COMMAND"] = str(git_command)
    environment["CAO_RUSTC_COMMAND"] = str(rustc_command)
    environment["CARGO_HOME"] = str(cargo_home)
    environment["RUSTUP_HOME"] = str(rustup_home)
    environment["GIT"] = str(git_command)
    environment["RUSTC"] = str(rustc_command)
    environment["FORCE_SKIA_BUILD"] = "1"
    environment["CLANGCC"] = str(clang_cl)
    environment["CLANGCXX"] = str(clang_cl)
    environment["LIBCLANG_PATH"] = str(llvm_home / "bin")
    environment["VCToolsInstallDir"] = str(msvc_home) + os.sep
    environment["VCINSTALLDIR"] = str(msvc_home.parents[2]) + os.sep
    environment["WindowsSdkDir"] = str(windows_sdk) + os.sep
    environment["WindowsSDKVersion"] = WINDOWS_SDK_VERSION + os.sep
    environment["UniversalCRTSdkDir"] = str(windows_sdk) + os.sep
    environment["UCRTVersion"] = WINDOWS_SDK_VERSION
    environment["INCLUDE"] = os.pathsep.join(
        (
            str(msvc_home / "include"),
            str(sdk_include / "ucrt"),
            str(sdk_include / "shared"),
            str(sdk_include / "um"),
            str(sdk_include / "winrt"),
        )
    )
    environment["LIB"] = os.pathsep.join(
        (
            str(msvc_home / "lib/x64"),
            str(sdk_lib / "ucrt/x64"),
            str(sdk_lib / "um/x64"),
        )
    )
    environment["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"] = str(linker)
    environment["CC_x86_64_pc_windows_msvc"] = str(cl)
    environment["CC_x86_64-pc-windows-msvc"] = str(cl)
    environment["CXX_x86_64_pc_windows_msvc"] = str(cl)
    environment["CXX_x86_64-pc-windows-msvc"] = str(cl)
    environment["AR_x86_64_pc_windows_msvc"] = str(librarian)
    environment["AR_x86_64-pc-windows-msvc"] = str(librarian)
    for variable in ("CC", "CXX", "SDKROOT", "SDKTARGETSYSROOT"):
        environment.pop(variable, None)
    # skia-bindings probes `python`; put the authenticated interpreter first.
    environment["PATH"] = os.pathsep.join(
        (
            str(Path(sys.executable).parent),
            str(cargo_command.parent),
            str(git_command.parent),
            str(rustc_command.parent),
            str(llvm_home / "bin"),
            str(msvc_bin),
            str(sdk_bin),
            environment.get("PATH", ""),
        )
    )
    return environment


def build_release(root: Path, environment: Mapping[str, str]) -> None:
    """Build exactly the GUI and HKX helper from the frozen offline graph.

    ``environment`` must be the authenticated result of
    :func:`offline_build_environment` for ``root``.

    Raises:
      subprocess.CalledProcessError: Cargo cannot build the frozen release graph.
      OSError: Cargo or Ninja cannot be launched.
    """
    cargo = environment["CAO_CARGO_COMMAND"]
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
        validate_skia_source_lock(root)
        environment = offline_build_environment(root)
        validate_workspace(root, environment)
        build_release(root, environment)
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
