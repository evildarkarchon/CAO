"""Acquire and authenticate the locked rust-skia binary archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tarfile
import urllib.request
from pathlib import Path
from typing import Any, Mapping, Sequence


class AcquisitionError(RuntimeError):
    """Report an unavailable, malformed, or unauthenticated cache input."""


def load_binary_lock(root: Path) -> Mapping[str, Any]:
    """Load the reviewed rust-skia binary identity from the repository.

    Raises:
      AcquisitionError: The lock is missing, malformed, or incomplete.
    """
    path = root / "verification/build-inputs/skia-binary-lock.json"
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise AcquisitionError(f"cannot load Skia binary lock: {error}") from error
    archive = document.get("archive") if isinstance(document, dict) else None
    if not isinstance(archive, dict):
        raise AcquisitionError("Skia binary lock archive must be a JSON object")
    return archive


def sha256_file(path: Path) -> str:
    """Hash one cache file without loading the native archive into memory.

    Raises:
      OSError: The archive cannot be read.
    """
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_archive_marker(archive_path: Path, marker: str) -> str:
    """Read one small rust-skia identity marker from an authenticated archive.

    Raises:
      AcquisitionError: The marker is absent, not a regular file, or malformed.
      OSError: The archive cannot be read.
      tarfile.TarError: The archive structure is invalid.
    """
    member_name = f"skia-binaries/{marker}"
    with tarfile.open(archive_path, mode="r:gz") as archive:
        try:
            member = archive.getmember(member_name)
        except KeyError as error:
            raise AcquisitionError(f"Skia archive is missing {member_name}") from error
        if not member.isfile() or member.size > 4096:
            raise AcquisitionError(f"Skia archive marker is invalid: {member_name}")
        stream = archive.extractfile(member)
        if stream is None:
            raise AcquisitionError(f"Skia archive marker is unreadable: {member_name}")
        try:
            return stream.read().decode("utf-8").strip()
        except UnicodeDecodeError as error:
            raise AcquisitionError(
                f"Skia archive marker is not UTF-8: {member_name}"
            ) from error


def verify_archive(path: Path, archive: Mapping[str, Any]) -> None:
    """Verify archive size, digest, release tag, and rust-skia feature key.

    Raises:
      AcquisitionError: Any authenticated identity differs from the binary lock.
      OSError: The archive cannot be read.
      tarfile.TarError: The archive structure is invalid.
    """
    expected_size = int(archive["size_bytes"])
    if not path.is_file() or path.stat().st_size != expected_size:
        raise AcquisitionError(f"Skia binary archive size mismatch: {path}")
    if sha256_file(path) != archive["sha256"]:
        raise AcquisitionError(f"Skia binary archive SHA-256 mismatch: {path}")
    if read_archive_marker(path, "tag.txt") != archive["tag"]:
        raise AcquisitionError(f"Skia binary archive tag mismatch: {path}")
    if read_archive_marker(path, "key.txt") != archive["key"]:
        raise AcquisitionError(f"Skia binary archive key mismatch: {path}")


def default_cache_root() -> Path:
    """Return the machine-local cache root outside the repository and Cargo home.

    Raises:
      AcquisitionError: No suitable Windows-local cache directory is available.
    """
    configured = os.environ.get("CAO_BUILD_CACHE")
    if configured:
        root = Path(configured).expanduser()
    else:
        local_app_data = os.environ.get("LOCALAPPDATA")
        if not local_app_data:
            raise AcquisitionError("set CAO_BUILD_CACHE or LOCALAPPDATA")
        root = Path(local_app_data) / "cao-build-cache"
    if not root.is_absolute():
        raise AcquisitionError("CAO_BUILD_CACHE must be an absolute path")
    return root


def paths_overlap(first: Path, second: Path) -> bool:
    """Return whether either resolved path contains the other."""
    first_resolved = first.resolve()
    second_resolved = second.resolve()
    return first_resolved.is_relative_to(
        second_resolved
    ) or second_resolved.is_relative_to(first_resolved)


def validate_cache_root(repository_root: Path, cache_root: Path) -> Path:
    """Resolve a cache root and enforce isolation from source and Cargo state.

    Raises:
      AcquisitionError: The cache root is relative or overlaps a protected tree.
    """
    if not cache_root.is_absolute():
        raise AcquisitionError("cache root must be absolute")
    resolved = cache_root.resolve()
    if paths_overlap(resolved, repository_root):
        raise AcquisitionError(f"cache root overlaps the repository: {resolved}")
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).expanduser()
    if not cargo_home.is_absolute():
        cargo_home = (Path.cwd() / cargo_home).resolve()
    if paths_overlap(resolved, cargo_home):
        raise AcquisitionError(f"cache root overlaps Cargo home: {resolved}")
    return resolved


def cache_paths(cache_root: Path, archive: Mapping[str, Any]) -> tuple[Path, Path]:
    """Return the content-addressed archive path and rust-skia URL-template root."""
    template_root = (
        cache_root / "rust-skia" / "v1" / str(archive["sha256"])
    ).resolve()
    archive_path = template_root / str(archive["tag"]) / str(archive["filename"])
    return archive_path, template_root


def download_archive(destination: Path, source_uri: str, expected_size: int) -> None:
    """Download exactly the locked byte count and atomically publish on success.

    Raises:
      AcquisitionError: The response length differs from the locked byte count.
      OSError: The cache directory or archive cannot be written.
      urllib.error.URLError: The reviewed source cannot be downloaded.
    """
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(f"{destination.name}.{os.getpid()}.tmp")
    try:
        request = urllib.request.Request(
            source_uri,
            headers={"User-Agent": "Tracetide-Skia-cache-acquisition/1"},
        )
        with urllib.request.urlopen(
            request, timeout=60
        ) as response, temporary.open("wb") as output:
            reported_length = response.headers.get("Content-Length")
            if reported_length is not None:
                try:
                    parsed_length = int(reported_length)
                except ValueError as error:
                    raise AcquisitionError(
                        "Skia archive response has an invalid Content-Length"
                    ) from error
                if parsed_length != expected_size:
                    raise AcquisitionError(
                        "Skia archive response size differs from the binary lock"
                    )

            remaining = expected_size
            while remaining:
                chunk = response.read(min(1024 * 1024, remaining))
                if not chunk:
                    break
                if len(chunk) > remaining:
                    raise AcquisitionError("Skia archive exceeded its locked size")
                output.write(chunk)
                remaining -= len(chunk)
            if remaining or response.read(1):
                raise AcquisitionError(
                    "Skia archive transfer differs from its locked size"
                )
        os.replace(temporary, destination)
    finally:
        # A failed transfer must never be mistaken for a reusable cache entry.
        temporary.unlink(missing_ok=True)


def acquire(root: Path, cache_root: Path, offline: bool) -> str:
    """Return a local rust-skia URL template after authenticating the cache.

    Raises:
      AcquisitionError: The cache is invalid and cannot be repaired under the mode.
      OSError: Repository or cache files cannot be read or written.
      tarfile.TarError: The downloaded archive structure is invalid.
      urllib.error.URLError: The reviewed source cannot be downloaded.
    """
    archive = load_binary_lock(root)
    archive_path, template_root = cache_paths(cache_root, archive)
    try:
        verify_archive(archive_path, archive)
    except (AcquisitionError, OSError, tarfile.TarError):
        if offline:
            raise
        download_archive(
            archive_path, str(archive["source_uri"]), int(archive["size_bytes"])
        )
        verify_archive(archive_path, archive)

    template = template_root / "{tag}" / "skia-binaries-{key}.tar.gz"
    # rust-skia 0.99 strips exactly `file://` and passes this Windows path to Path.
    return "file://" + template.as_posix()


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    """Parse cache location and acquisition mode."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--cache-root",
        type=Path,
        help="Absolute cache root; defaults to CAO_BUILD_CACHE or LOCALAPPDATA",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="Verify an existing cache entry without downloading",
    )
    parser.add_argument(
        "--print-cache-key",
        action="store_true",
        help="Print the tag and digest identity used by hosted cache keys",
    )
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    """Acquire the locked archive and print the local SKIA_BINARIES_URL template."""
    options = parse_arguments(arguments if arguments is not None else sys.argv[1:])
    root = Path(__file__).resolve().parents[1]
    if options.print_cache_key:
        try:
            archive = load_binary_lock(root)
            print(f'{archive["tag"]}-{archive["sha256"]}')
        except (AcquisitionError, KeyError, OSError) as error:
            print(f"Skia binary acquisition failed: {error}", file=sys.stderr)
            return 1
        return 0
    cache_root = (options.cache_root or default_cache_root()).expanduser()
    try:
        cache_root = validate_cache_root(root, cache_root)
        template = acquire(root, cache_root, options.offline)
    except (AcquisitionError, OSError, tarfile.TarError) as error:
        print(f"Skia binary acquisition failed: {error}", file=sys.stderr)
        return 1
    print(template)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
