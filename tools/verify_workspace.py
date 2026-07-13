"""Validate Tracetide's Rust workspace and release-root contract offline."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any, Mapping, NamedTuple, Sequence


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
EXPECTED_PACKAGE_NAMES = {path: path.rsplit("/", 1)[-1] for path in EXPECTED_MEMBERS}
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
    "def6ff8de3367744c51d29ed903d7cbc03b939b8fef1803222d6900abd30fdab"
)
EXPECTED_LOCAL_LOCK_PACKAGES = {
    (name, "0.0.0") for name in EXPECTED_PACKAGE_NAMES.values()
} | {
    ("ba2", "3.0.1"),
    ("havok_classes", "1.0.1"),
    ("havok_serde", "1.0.1"),
    ("havok_types", "1.0.1"),
    ("havok_types_derive", "1.0.1"),
    ("havok_types_derive_proc_macros", "0.1.0"),
    ("serde_hkx", "1.0.1"),
    ("winnow_ext", "0.1.0"),
}
PRODUCTION_ROOTS = ("cao-gui", "cao-hkx-helper")
FORBIDDEN_PRODUCTION_PACKAGES = ("cao-oracle-capture", "cao-verification")
FORBIDDEN_PUBLIC_TYPE_ROOTS = (
    "ba2",
    "cao_hkx_protocol",
    "crossbeam_channel",
    "directxtex",
    "ffi",
    "havok_classes",
    "nifly",
    "serde_hkx",
    "slint",
    "win32",
    "windows",
    "windows_sys",
)
# Keep rationale association local so one distant comment cannot bless several operations.
SAFETY_COMMENT_LOOKBACK_LINES = 3


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
        raise ContractError(
            f"cannot read required UTF-8 file {path}: {error}"
        ) from error
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
        raise ContractError(
            f"cannot load required JSON file {path}: {error}"
        ) from error


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
            raise ContractError(
                f"vendored source trees cannot contain symlinks: {path}"
            )
        if not path.is_file():
            continue
        relative_path = path.relative_to(root).as_posix().encode("utf-8")
        try:
            content = path.read_bytes()
        except OSError as error:
            raise ContractError(
                f"cannot read vendored source file {path}: {error}"
            ) from error
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
    require_equal(toolchain.get("components"), ["clippy", "rustfmt"], "Rust components")
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
    require_equal(set(dependency), set(expected), f"{name} dependency fields")


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
    for (
        dependency_name,
        dependency_path,
    ) in EXPECTED_WORKSPACE_PATH_DEPENDENCIES.items():
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
        require_equal(
            package.get("name"), package_name, f"{relative_path} package name"
        )
        for field in WORKSPACE_INHERITED_PACKAGE_FIELDS:
            require_equal(
                package.get(field),
                {"workspace": True},
                f"{package_name} inherited {field}",
            )
        for field in DISABLED_CARGO_AUTO_TARGET_FIELDS:
            require_equal(package.get(field), False, f"{package_name} package {field}")
        require_equal(
            package.get("build"), False, f"{package_name} build script policy"
        )

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
        unexpected_sources = [
            path for path in implicit_target_sources if path.is_file()
        ]
        if unexpected_sources:
            raise ContractError(
                f"{package_name} has undeclared Rust target sources: "
                + ", ".join(str(path) for path in unexpected_sources)
            )


class RustToken(NamedTuple):
    """One comment/literal-free Rust token with its source line."""

    text: str
    line: int


def lex_rust(
    source: str,
    source_path: Path,
    safety_comments: dict[int, str] | None = None,
    comment_lines: set[int] | None = None,
) -> list[RustToken]:
    """Tokenize Rust while discarding nested comments and every literal form.

    When supplied, ``safety_comments`` maps source lines to substantive text
    following ``SAFETY:`` in actual Rust comments, while ``comment_lines`` receives
    every line owned by a comment. Literal contents never enter either accumulator.

    Raises:
      ContractError: A comment or literal is unterminated.
    """
    tokens: list[RustToken] = []
    index = 0
    line = 1

    def record_safety_comment(comment: str, comment_line: int) -> None:
        """Record a non-empty rationale found on one actual comment line."""
        if safety_comments is None or "SAFETY:" not in comment:
            return
        rationale = comment.partition("SAFETY:")[2].strip()
        if rationale.endswith("*/"):
            rationale = rationale[:-2].rstrip()
        if rationale:
            safety_comments[comment_line] = rationale

    def skip_quoted(start: int, quote: str) -> int:
        """Return the index after one escaped quoted literal."""
        cursor = start + 1
        while cursor < len(source):
            if source[cursor] == "\n":
                nonlocal line
                line += 1
            if source[cursor] == "\\":
                cursor += 2
                continue
            if source[cursor] == quote:
                return cursor + 1
            cursor += 1
        raise ContractError(f"{source_path}:{line} unterminated Rust literal")

    while index < len(source):
        character = source[index]
        if character.isspace():
            line += character == "\n"
            index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            comment_end = len(source) if newline < 0 else newline
            record_safety_comment(source[index:comment_end], line)
            if comment_lines is not None:
                comment_lines.add(line)
            index = comment_end
            continue
        if source.startswith("/*", index):
            comment_start = index
            comment_start_line = line
            depth = 1
            index += 2
            while index < len(source) and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    line += source[index] == "\n"
                    index += 1
            if depth:
                raise ContractError(
                    f"{source_path}:{line} unterminated Rust block comment"
                )
            if comment_lines is not None:
                comment_lines.update(range(comment_start_line, line + 1))
            for offset, comment in enumerate(source[comment_start:index].splitlines()):
                record_safety_comment(comment, comment_start_line + offset)
            continue

        raw = re.match(r"(?:br|rb|cr|rc|r)(#*)\"", source[index:])
        if raw:
            terminator = '"' + raw.group(1)
            content_start = index + raw.end()
            content_end = source.find(terminator, content_start)
            if content_end < 0:
                raise ContractError(
                    f"{source_path}:{line} unterminated Rust raw string"
                )
            line += source.count("\n", content_start, content_end + len(terminator))
            index = content_end + len(terminator)
            continue
        if source.startswith(('b"', 'c"'), index):
            index = skip_quoted(index + 1, '"')
            continue
        if character == '"':
            index = skip_quoted(index, '"')
            continue
        if source.startswith("b'", index):
            index = skip_quoted(index + 1, "'")
            continue
        if character == "'":
            lifetime = re.match(r"'[A-Za-z_][A-Za-z0-9_]*", source[index:])
            if lifetime and not source.startswith("'", index + lifetime.end()):
                tokens.append(RustToken("'", line))
                index += 1
                continue
            index = skip_quoted(index, "'")
            continue

        raw_identifier = re.match(r"r#([A-Za-z_][A-Za-z0-9_]*)", source[index:])
        if raw_identifier:
            tokens.append(RustToken(raw_identifier.group(1), line))
            index += raw_identifier.end()
            continue
        identifier = re.match(r"[A-Za-z_][A-Za-z0-9_]*", source[index:])
        if identifier:
            tokens.append(RustToken(identifier.group(0), line))
            index += identifier.end()
            continue
        punctuation = next(
            (
                item
                for item in ("::", "->", "=>", "..=", "..")
                if source.startswith(item, index)
            ),
            character,
        )
        tokens.append(RustToken(punctuation, line))
        index += len(punctuation)
    return tokens


def matching_rust_delimiter(
    tokens: Sequence[RustToken], index: int, opening: str, closing: str
) -> int:
    """Return the matching delimiter index or fail closed on malformed source."""
    depth = 0
    for cursor in range(index, len(tokens)):
        depth += tokens[cursor].text == opening
        depth -= tokens[cursor].text == closing
        if depth == 0:
            return cursor
    raise ContractError(
        f"Rust delimiter {opening!r} on line {tokens[index].line} is unmatched"
    )


def split_rust_fields(tokens: Sequence[RustToken]) -> list[Sequence[RustToken]]:
    """Split a struct/tuple field list at top-level commas."""
    fields: list[Sequence[RustToken]] = []
    start = 0
    depths = {"(": 0, "[": 0, "{": 0, "<": 0}
    closings = {")": "(", "]": "[", "}": "{", ">": "<"}
    for index, token in enumerate(tokens):
        if token.text in depths:
            depths[token.text] += 1
        elif token.text in closings:
            opening = closings[token.text]
            depths[opening] = max(depths[opening] - 1, 0)
        elif token.text == "," and not any(depths.values()):
            fields.append(tokens[start:index])
            start = index + 1
    fields.append(tokens[start:])
    return fields


class RustPublicApiScanner:
    """Reject forbidden leaf namespaces at externally reachable Rust seams."""

    ITEM_KEYWORDS = {
        "const",
        "crate",
        "enum",
        "fn",
        "impl",
        "macro",
        "mod",
        "static",
        "struct",
        "trait",
        "type",
        "union",
        "use",
    }

    def __init__(self, roots: Sequence[str]) -> None:
        """Create a scanner for one package's forbidden namespace roots."""
        self.roots = set(roots)
        self.file_taints: dict[Path, dict[str, str]] = {}

    def leaked_root(
        self,
        tokens: Sequence[RustToken],
        aliases: Mapping[str, str],
        use_declaration: bool = False,
    ) -> str | None:
        """Return the first directly referenced or imported leaf namespace."""
        token_text = [token.text for token in tokens]
        if "*" in aliases:
            return aliases["*"]
        for alias, root in aliases.items():
            parts = alias.split("::")
            pattern = [parts[0]]
            for part in parts[1:]:
                pattern.extend(("::", part))
            width = len(pattern)
            if any(
                token_text[index : index + width] == pattern
                for index in range(len(token_text) - width + 1)
            ):
                return root
            glob_pattern = pattern[:-1] + ["*"]
            glob_width = len(glob_pattern)
            if use_declaration and any(
                token_text[index : index + glob_width] == glob_pattern
                for index in range(len(token_text) - glob_width + 1)
            ):
                return root
        for index, token in enumerate(tokens):
            if "::" not in token.text and token.text in aliases:
                return aliases[token.text]
            if token.text in self.roots and (
                use_declaration
                or (index + 1 < len(tokens) and tokens[index + 1].text == "::")
            ):
                return token.text
        return None

    def reject_leak(
        self,
        tokens: Sequence[RustToken],
        aliases: Mapping[str, str],
        source_path: Path,
        use_declaration: bool = False,
    ) -> None:
        """Raise a line-anchored error if an API token slice exposes a leaf.

        Raises:
          ContractError: The public token slice names a forbidden leaf type.
        """
        leaked = self.leaked_root(tokens, aliases, use_declaration)
        if leaked:
            line = tokens[0].line if tokens else 1
            raise ContractError(
                f"{source_path}:{line} public API leaks leaf type {leaked}"
            )

    def item_bounds(
        self,
        tokens: Sequence[RustToken],
        start: int,
        end: int,
        keyword: str,
    ) -> tuple[int, int | None, int]:
        """Return signature end, optional body opener, and complete item end.

        Raises:
          ContractError: The item is incomplete or has unbalanced delimiters.
        """
        round_depth = 0
        square_depth = 0
        brace_depth = 0
        for cursor in range(start, end):
            text = tokens[cursor].text
            if text == "(":
                round_depth += 1
            elif text == ")":
                round_depth = max(round_depth - 1, 0)
            elif text == "[":
                square_depth += 1
            elif text == "]":
                square_depth = max(square_depth - 1, 0)
            elif keyword == "use" and text == "{":
                brace_depth += 1
            elif keyword == "use" and text == "}":
                brace_depth = max(brace_depth - 1, 0)
            elif not round_depth and not square_depth and not brace_depth:
                if text == ";":
                    return cursor + 1, None, cursor + 1
                if text == "{":
                    body_end = matching_rust_delimiter(tokens, cursor, "{", "}")
                    return cursor, cursor, body_end + 1
        raise ContractError(
            f"Rust {keyword} item on line {tokens[start].line} is incomplete"
        )

    @staticmethod
    def bare_public_visibility(tokens: Sequence[RustToken]) -> bool:
        """Return whether a field/item slice starts with unrestricted `pub`.

        Raises:
          ContractError: An attribute on the field has unbalanced delimiters.
        """
        cursor = 0
        while cursor < len(tokens) and tokens[cursor].text == "#":
            cursor += 1
            if cursor < len(tokens) and tokens[cursor].text == "[":
                cursor = matching_rust_delimiter(tokens, cursor, "[", "]") + 1
        return (
            cursor < len(tokens)
            and tokens[cursor].text == "pub"
            and (cursor + 1 == len(tokens) or tokens[cursor + 1].text != "(")
        )

    def record_use_aliases(
        self,
        declaration: Sequence[RustToken],
        aliases: dict[str, str],
        leaked: str,
    ) -> None:
        """Taint names introduced by one private forbidden-root import.

        Raises:
          ContractError: A grouped import has unbalanced delimiters.
        """
        for index, token in enumerate(declaration[:-1]):
            if token.text == "as" and declaration[index + 1].text not in {"_", "self"}:
                aliases[declaration[index + 1].text] = leaked
        if "{" in (token.text for token in declaration):
            brace = next(
                index for index, token in enumerate(declaration) if token.text == "{"
            )
            brace_end = matching_rust_delimiter(declaration, brace, "{", "}")
            for field in split_rust_fields(declaration[brace + 1 : brace_end]):
                identifiers = [
                    token.text
                    for token in field
                    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token.text)
                    and token.text not in {"as", "crate", "self", "super"}
                ]
                if identifiers:
                    aliases[identifiers[-1]] = leaked
            return
        identifiers = [
            token.text
            for token in declaration
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token.text)
            and token.text not in {"as", "crate", "self", "super", "use"}
        ]
        if identifiers:
            aliases[identifiers[-1]] = leaked

    def glob_import_aliases(
        self,
        declaration: Sequence[RustToken],
        known_aliases: Mapping[str, str],
    ) -> dict[str, str]:
        """Return tainted names introduced by direct module glob imports."""
        token_text = [token.text for token in declaration]
        imported: dict[str, str] = {}
        for alias, root in known_aliases.items():
            parts = alias.split("::")
            pattern = [parts[0]]
            for part in parts[1:-1]:
                pattern.extend(("::", part))
            pattern.extend(("::", "*"))
            width = len(pattern)
            if any(
                token_text[index : index + width] == pattern
                for index in range(len(token_text) - width + 1)
            ):
                imported[parts[-1]] = root
        return imported

    def module_import_aliases(
        self,
        declaration: Sequence[RustToken],
        known_aliases: Mapping[str, str],
    ) -> dict[str, str]:
        """Carry descendant taints through one simple module-path rename."""
        texts = [token.text for token in declaration]
        if not texts or "{" in texts or "*" in texts:
            return {}
        try:
            use_index = texts.index("use")
        except ValueError:
            return {}
        as_index = texts.index("as") if "as" in texts else len(texts)
        path_parts = [
            text
            for text in texts[use_index + 1 : as_index]
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", text) and text != "self"
        ]
        if not path_parts:
            return {}
        source_prefix = "::".join(path_parts)
        imported_name = (
            texts[as_index + 1] if as_index + 1 < len(texts) else path_parts[-1]
        )
        imported: dict[str, str] = {}
        for alias, root in known_aliases.items():
            if alias == source_prefix:
                imported[imported_name] = root
            elif alias.startswith(source_prefix + "::"):
                suffix = alias[len(source_prefix) :]
                imported[imported_name + suffix] = root
        return imported

    def use_tree_import_aliases(
        self,
        declaration: Sequence[RustToken],
        known_aliases: Mapping[str, str],
    ) -> dict[str, str]:
        """Expand a Rust use-tree and propagate every imported descendant taint."""
        texts = [token.text for token in declaration]
        try:
            start = texts.index("use") + 1
        except ValueError:
            return {}
        end = len(texts) - 1 if texts and texts[-1] == ";" else len(texts)
        imports: list[tuple[tuple[str, ...], str | None, bool]] = []

        def parse_tree(index: int, prefix: tuple[str, ...]) -> int:
            """Parse a use-tree below ``prefix`` and return its next token index.

            Each resolved leaf is appended to the enclosing ``imports`` accumulator.
            """
            path = list(prefix)
            while index < end and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", texts[index]):
                segment = texts[index]
                if segment != "self":
                    path.append(segment)
                index += 1
                if index < end and texts[index] == "::":
                    if index + 1 < end and texts[index + 1] == "{":
                        index += 1
                        break
                    index += 1
                    continue
                break

            if index < end and texts[index] == "{":
                index += 1
                while index < end and texts[index] != "}":
                    index = parse_tree(index, tuple(path))
                    if index < end and texts[index] == ",":
                        index += 1
                return index + 1 if index < end else index
            if index < end and texts[index] == "*":
                imports.append((tuple(path), None, True))
                return index + 1
            if index < end and texts[index] == "as":
                imported_name = texts[index + 1] if index + 1 < end else ""
                imports.append((tuple(path), imported_name, False))
                return min(index + 2, end)
            if path:
                imports.append((tuple(path), path[-1], False))
            return index

        parse_tree(start, ())
        imported: dict[str, str] = {}
        for source_parts, imported_name, glob in imports:
            if not source_parts:
                continue
            source_prefix = "::".join(source_parts)
            if source_parts[0] in self.roots:
                root = source_parts[0]
                if glob:
                    imported["*"] = root
                elif imported_name:
                    imported[imported_name] = root
                continue
            for alias, root in known_aliases.items():
                if glob and alias.startswith(source_prefix + "::"):
                    imported[alias.rsplit("::", 1)[-1]] = root
                elif alias == source_prefix and imported_name:
                    imported[imported_name] = root
                elif alias.startswith(source_prefix + "::") and imported_name:
                    suffix = alias[len(source_prefix) :]
                    imported[imported_name + suffix] = root
        return imported

    def scan_file(
        self,
        source_path: Path,
        reachable: bool = True,
        inherited_aliases: Mapping[str, str] | None = None,
        crate_taints: dict[str, str] | None = None,
        module_path: tuple[str, ...] = (),
    ) -> dict[str, str]:
        """Scan a Rust module file and external children in their module context.

        ``inherited_aliases`` resolves paths relative to an external module's
        parent, while ``crate_taints`` shares absolute crate paths across files.
        ``module_path`` is the file's crate-relative identity used to qualify new
        descendant taints.

        Returns:
          Public names whose signatures expose forbidden leaf types.

        Raises:
          ContractError: The source is unreadable, malformed, or leaks a leaf type.
        """
        source_path = source_path.resolve()
        cacheable = inherited_aliases is None and crate_taints is None
        if cacheable and source_path in self.file_taints:
            return self.file_taints[source_path]
        # Seed the cache before descent so an invalid recursive module cannot loop forever.
        if cacheable:
            self.file_taints[source_path] = {}
        try:
            source = source_path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise ContractError(
                f"cannot read required UTF-8 file {source_path}: {error}"
            ) from error
        tokens = lex_rust(source, source_path)
        module_directory = (
            source_path.parent
            if source_path.name in {"lib.rs", "mod.rs"}
            else source_path.parent / source_path.stem
        )
        shared_crate_taints = crate_taints if crate_taints is not None else {}
        taints = self.scan_scope(
            tokens,
            0,
            len(tokens),
            reachable,
            source_path,
            module_directory,
            inherited_aliases=inherited_aliases,
            crate_taints=shared_crate_taints,
            module_path=module_path,
        )
        if cacheable:
            self.file_taints[source_path] = taints
        return taints

    def scan_scope(
        self,
        tokens: Sequence[RustToken],
        start: int,
        end: int,
        reachable: bool,
        source_path: Path,
        module_directory: Path,
        default_public: bool = False,
        inherited_aliases: Mapping[str, str] | None = None,
        crate_taints: dict[str, str] | None = None,
        module_path: tuple[str, ...] = (),
        passes_remaining: int | None = None,
    ) -> dict[str, str]:
        """Scan one module/trait/impl token scope with local aliases and types.

        ``inherited_aliases`` names parent-visible taints in this scope;
        ``crate_taints`` carries absolute module identities between siblings.
        ``module_path`` qualifies child taints, and ``passes_remaining`` bounds
        the order-independent alias-resolution passes.

        Returns:
          Public names whose signatures expose forbidden leaf types.

        Raises:
          ContractError: The scope is malformed, does not converge, or leaks a leaf.
        """
        aliases = dict(inherited_aliases or {})
        initial_aliases = dict(aliases)
        shared_crate_taints = crate_taints if crate_taints is not None else {}
        initial_crate_taints = dict(shared_crate_taints)
        # Each productive pass adds at least one taint derived from a source token,
        # so the token count is a strict upper bound on convergence work.
        remaining = (
            passes_remaining if passes_remaining is not None else end - start + 1
        )
        exported_taints: dict[str, str] = {}
        public_types: set[str] = set()
        private_types: set[str] = set()
        cursor = start
        while cursor < end:
            macro_export = False
            while cursor < end and tokens[cursor].text == "#":
                attribute_start = cursor
                cursor += 1
                if cursor < end and tokens[cursor].text == "!":
                    cursor += 1
                if cursor < end and tokens[cursor].text == "[":
                    attribute_end = matching_rust_delimiter(tokens, cursor, "[", "]")
                    macro_export |= any(
                        token.text == "macro_export"
                        for token in tokens[attribute_start : attribute_end + 1]
                    )
                    cursor = attribute_end + 1
                else:
                    break
            if cursor >= end:
                break

            bare_public = False
            if tokens[cursor].text == "pub":
                bare_public = cursor + 1 >= end or tokens[cursor + 1].text != "("
                cursor += 1
                if not bare_public and cursor < end and tokens[cursor].text == "(":
                    cursor = matching_rust_delimiter(tokens, cursor, "(", ")") + 1

            item_start = cursor
            while cursor < end and tokens[cursor].text in {
                "async",
                "default",
                "extern",
                "safe",
                "unsafe",
            }:
                cursor += 1
            if cursor < end and tokens[cursor].text == "const":
                if cursor + 1 < end and tokens[cursor + 1].text == "fn":
                    cursor += 1
            if cursor >= end or tokens[cursor].text not in self.ITEM_KEYWORDS:
                cursor = item_start + 1
                continue
            keyword = tokens[cursor].text
            keyword_index = cursor
            signature_end, body_start, item_end = self.item_bounds(
                tokens, keyword_index, end, keyword
            )
            declaration = tokens[keyword_index:signature_end]
            declared_public = bare_public or default_public or macro_export
            item_public = reachable and declared_public
            name = next(
                (
                    token.text
                    for token in tokens[keyword_index + 1 : signature_end]
                    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token.text)
                ),
                "",
            )

            if keyword == "use":
                glob_imports = self.glob_import_aliases(declaration, aliases)
                module_imports = self.module_import_aliases(declaration, aliases)
                use_tree_imports = self.use_tree_import_aliases(declaration, aliases)
                imported_taints = use_tree_imports or glob_imports or module_imports
                leaked = self.leaked_root(declaration, aliases, use_declaration=True)
                leaked = leaked or next(iter(imported_taints.values()), None)
                if leaked and item_public:
                    raise ContractError(
                        f"{source_path}:{declaration[0].line} "
                        f"public API leaks leaf type {leaked}"
                    )
                elif leaked and declared_public:
                    reexports = imported_taints
                    if not reexports:
                        reexports = {}
                        self.record_use_aliases(declaration, reexports, leaked)
                    exported_taints.update(reexports)
                elif leaked:
                    if imported_taints:
                        aliases.update(imported_taints)
                    else:
                        self.record_use_aliases(declaration, aliases, leaked)
            elif keyword == "crate":
                leaked = self.leaked_root(declaration, aliases, use_declaration=True)
                if leaked and item_public:
                    self.reject_leak(
                        declaration, aliases, source_path, use_declaration=True
                    )
                elif leaked:
                    self.record_use_aliases(declaration, aliases, leaked)
            elif keyword == "mod":
                child_reachable = item_public
                child_module_path = (*module_path, name)
                child_aliases = {
                    f"super::{alias}": leaked for alias, leaked in aliases.items()
                }
                child_aliases.update(
                    {
                        f"crate::{alias}": leaked
                        for alias, leaked in shared_crate_taints.items()
                    }
                )
                if body_start is not None:
                    body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                    child_taints = self.scan_scope(
                        tokens,
                        body_start + 1,
                        body_end,
                        child_reachable,
                        source_path,
                        module_directory / name,
                        inherited_aliases=child_aliases,
                        crate_taints=shared_crate_taints,
                        module_path=child_module_path,
                    )
                elif name:
                    candidates = (
                        module_directory / f"{name}.rs",
                        module_directory / name / "mod.rs",
                    )
                    child = next(
                        (candidate for candidate in candidates if candidate.is_file()),
                        None,
                    )
                    if child is None:
                        raise ContractError(
                            f"{source_path}:{tokens[keyword_index].line} cannot resolve Rust module {name}"
                        )
                    child_taints = self.scan_file(
                        child,
                        child_reachable,
                        child_aliases,
                        shared_crate_taints,
                        child_module_path,
                    )
                else:
                    child_taints = {}
                for child_name, leaked in child_taints.items():
                    aliases[f"{name}::{child_name}"] = leaked
                    absolute_name = "::".join(
                        (*child_module_path, *child_name.split("::"))
                    )
                    shared_crate_taints[absolute_name] = leaked
                    aliases[f"crate::{absolute_name}"] = leaked
                    if declared_public:
                        exported_taints[f"{name}::{child_name}"] = leaked
            elif keyword in {"struct", "union"}:
                (public_types if declared_public else private_types).add(name)
                exposed_fragments: list[Sequence[RustToken]] = []
                if body_start is not None:
                    exposed_fragments.append(declaration)
                    body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                    exposed_fragments.extend(
                        field
                        for field in split_rust_fields(
                            tokens[body_start + 1 : body_end]
                        )
                        if self.bare_public_visibility(field)
                    )
                else:
                    opening = next(
                        (
                            index
                            for index, token in enumerate(declaration)
                            if token.text == "("
                        ),
                        None,
                    )
                    if opening is None:
                        exposed_fragments.append(declaration)
                    else:
                        closing = matching_rust_delimiter(
                            declaration, opening, "(", ")"
                        )
                        exposed_fragments.append(declaration[:opening])
                        exposed_fragments.extend(
                            field
                            for field in split_rust_fields(
                                declaration[opening + 1 : closing]
                            )
                            if self.bare_public_visibility(field)
                        )
                leaked_fragment = next(
                    (
                        (fragment, leaked)
                        for fragment in exposed_fragments
                        if (leaked := self.leaked_root(fragment, aliases))
                    ),
                    None,
                )
                leaked = leaked_fragment[1] if leaked_fragment else None
                if leaked and item_public:
                    self.reject_leak(leaked_fragment[0], aliases, source_path)
                elif leaked and declared_public and name:
                    exported_taints[name] = leaked
            elif keyword == "enum":
                (public_types if declared_public else private_types).add(name)
                complete = tokens[keyword_index:item_end]
                leaked = self.leaked_root(complete, aliases)
                if leaked and item_public:
                    self.reject_leak(complete, aliases, source_path)
                elif leaked and declared_public and name:
                    exported_taints[name] = leaked
            elif keyword == "trait":
                (public_types if declared_public else private_types).add(name)
                leaked = self.leaked_root(declaration, aliases)
                if leaked and item_public:
                    self.reject_leak(declaration, aliases, source_path)
                body_taints: dict[str, str] = {}
                if body_start is not None:
                    body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                    body_taints = self.scan_scope(
                        tokens,
                        body_start + 1,
                        body_end,
                        item_public,
                        source_path,
                        module_directory,
                        default_public=True,
                        inherited_aliases=aliases,
                        crate_taints=shared_crate_taints,
                        module_path=module_path,
                    )
                if declared_public and not item_public and name:
                    leaked = leaked or next(iter(body_taints.values()), None)
                    if leaked:
                        exported_taints[name] = leaked
            elif keyword == "impl":
                header_names = [
                    token.text
                    for token in declaration[1:]
                    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token.text)
                    and token.text not in {"for", "where"}
                ]
                self_name = header_names[-1] if header_names else ""
                impl_reachable = reachable and self_name not in private_types
                trait_impl = any(token.text == "for" for token in declaration)
                if impl_reachable:
                    self.reject_leak(declaration, aliases, source_path)
                body_taints = {}
                if body_start is not None:
                    body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                    body_taints = self.scan_scope(
                        tokens,
                        body_start + 1,
                        body_end,
                        impl_reachable,
                        source_path,
                        module_directory,
                        default_public=trait_impl,
                        inherited_aliases=aliases,
                        crate_taints=shared_crate_taints,
                        module_path=module_path,
                    )
                leaked = self.leaked_root(declaration, aliases)
                leaked = leaked or next(iter(body_taints.values()), None)
                if leaked and self_name in public_types and not impl_reachable:
                    exported_taints[self_name] = leaked
            elif keyword == "type":
                leaked = self.leaked_root(declaration, aliases)
                if leaked and item_public:
                    self.reject_leak(declaration, aliases, source_path)
                elif leaked and declared_public and name:
                    exported_taints[name] = leaked
                elif leaked and name:
                    aliases[name] = leaked
            elif keyword in {"const", "static"}:
                type_end = next(
                    (
                        index
                        for index, token in enumerate(declaration)
                        if token.text == "="
                    ),
                    len(declaration),
                )
                leaked = self.leaked_root(declaration[:type_end], aliases)
                if leaked and item_public:
                    self.reject_leak(declaration[:type_end], aliases, source_path)
                elif leaked and declared_public and name:
                    exported_taints[name] = leaked
            elif keyword in {"fn", "macro"}:
                fragment = (
                    tokens[keyword_index:item_end]
                    if keyword == "macro"
                    else declaration
                )
                leaked = self.leaked_root(fragment, aliases)
                if leaked and item_public:
                    self.reject_leak(fragment, aliases, source_path)
                elif leaked and declared_public and name:
                    exported_taints[name] = leaked

            cursor = item_end

        # Public items in a private scope remain locally nameable, so their taint
        # must participate in later aliases even when the module itself is hidden.
        aliases.update(exported_taints)
        if aliases != initial_aliases or shared_crate_taints != initial_crate_taints:
            if remaining <= 1:
                raise ContractError(
                    f"{source_path} leaf-type alias resolution did not converge"
                )
            return self.scan_scope(
                tokens,
                start,
                end,
                reachable,
                source_path,
                module_directory,
                default_public,
                aliases,
                shared_crate_taints,
                module_path,
                remaining - 1,
            )
        return exported_taints


def validate_leaf_type_boundaries(root: Path) -> None:
    """Reject leaf-owned types in externally reachable first-party APIs.

    Raises:
      ContractError: A public signature, field, variant, alias, or re-export leaks.
    """
    for relative_path in EXPECTED_LIBRARY_TARGETS:
        source_path = root / relative_path / "src/lib.rs"
        scanner = RustPublicApiScanner(FORBIDDEN_PUBLIC_TYPE_ROOTS)
        scanner.scan_file(source_path)


def validate_allowlisted_unsafe_source(source_path: Path) -> None:
    """Require private documented unsafe operations behind connected safe wrappers.

    Raises:
      ContractError: Unsafe code is public, undocumented, uncontained, or unwrapped.
    """
    visited: set[Path] = set()
    unsafe_modules: set[str] = set()
    unsafe_items: dict[str, dict[str, str]] = {}
    wrapper_unsafe_references: dict[tuple[str, str], set[tuple[str, str]]] = {}
    wrappers_with_unsafe: set[tuple[str, str]] = set()
    public_wrapper_calls: set[tuple[str, str]] = set()

    def scan_file(
        path: Path,
        private_module_root: str | None,
        module_path: tuple[str, ...],
    ) -> None:
        """Scan one module file while preserving its parent's visibility context.

        ``module_path`` preserves full identity for same-named unsafe items nested
        below an already-private parent.

        Raises:
          ContractError: The module is malformed or violates unsafe containment.
        """
        path = path.resolve()
        if path in visited:
            return
        visited.add(path)
        try:
            source = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise ContractError(
                f"cannot read required UTF-8 file {path}: {error}"
            ) from error
        safety_comments: dict[int, str] = {}
        comment_lines: set[int] = set()
        tokens = lex_rust(source, path, safety_comments, comment_lines)
        used_safety_comments: set[int] = set()
        lines = source.splitlines()
        module_directory = (
            path.parent
            if path.name in {"lib.rs", "main.rs", "mod.rs"}
            else path.parent / path.stem
        )

        def item_is_public(index: int, scope_start: int) -> bool:
            """Return whether the item prefix before one token contains visibility."""
            cursor = index - 1
            while cursor >= scope_start and tokens[cursor].text not in {";", "{", "}"}:
                if tokens[cursor].text == "pub":
                    return True
                cursor -= 1
            return False

        def has_safety_rationale(line_number: int) -> bool:
            """Consume an adjacent, unused, non-macro safety rationale.

            Raises:
              ContractError: A macro token tree has unbalanced delimiters.
            """
            first_line = max(1, line_number - SAFETY_COMMENT_LOOKBACK_LINES)
            macro_ranges = macro_token_ranges(0, len(tokens))
            for comment_line in range(line_number - 1, first_line - 1, -1):
                if (
                    comment_line in safety_comments
                    and comment_line not in used_safety_comments
                ):
                    if any(
                        tokens[begin].line <= comment_line <= tokens[finish].line
                        for begin, finish in macro_ranges
                    ):
                        continue
                    intervening_lines = range(comment_line + 1, line_number)
                    if any(
                        lines[index - 1].strip() and index not in comment_lines
                        for index in intervening_lines
                    ):
                        return False
                    # One invariant cannot bless multiple unsafe operations whose
                    # preconditions or lifetime constraints may differ.
                    used_safety_comments.add(comment_line)
                    return True
            return False

        def macro_token_ranges(start: int, end: int) -> list[tuple[int, int]]:
            """Return token intervals owned by macro invocations.

            Raises:
              ContractError: A macro token tree has unbalanced delimiters.
            """
            delimiters = {"(": ")", "[": "]", "{": "}"}
            ranges: list[tuple[int, int]] = []
            cursor = start
            while cursor + 1 < end:
                tree_start: int | None = None
                if tokens[cursor].text == "!" and tokens[cursor + 1].text in delimiters:
                    tree_start = cursor + 1
                elif (
                    tokens[cursor].text == "!"
                    and cursor > start
                    and tokens[cursor - 1].text == "macro_rules"
                    and cursor + 2 < end
                    and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", tokens[cursor + 1].text)
                    and tokens[cursor + 2].text in delimiters
                ):
                    tree_start = cursor + 2
                if tree_start is not None:
                    opener = tokens[tree_start].text
                    tree_end = matching_rust_delimiter(
                        tokens, tree_start, opener, delimiters[opener]
                    )
                    # A macro token tree may stringify or discard a path, so its text
                    # cannot prove that a safe wrapper executes the referenced item.
                    ranges.append((tree_start, tree_end))
                    cursor = tree_end + 1
                    continue
                cursor += 1
            return ranges

        def closure_token_ranges(start: int, end: int) -> list[tuple[int, int]]:
            """Return token intervals owned by closure bodies.

            Closure definitions do not execute their bodies, so even an invoked
            closure is conservatively excluded from static wrapper-link evidence.

            Raises:
              ContractError: A braced closure body has unbalanced delimiters.
            """
            closure_prefixes = {
                "=",
                "(",
                "[",
                "{",
                ",",
                "=>",
                "return",
                "move",
                "async",
            }
            ranges: list[tuple[int, int]] = []
            cursor = start
            while cursor < end:
                if tokens[cursor].text != "|":
                    cursor += 1
                    continue
                previous = tokens[cursor - 1].text if cursor > start else "="
                if previous not in closure_prefixes:
                    cursor += 1
                    continue
                closing_pipe = cursor + 1
                round_depth = 0
                square_depth = 0
                while closing_pipe < end:
                    text = tokens[closing_pipe].text
                    if text == "(":
                        round_depth += 1
                    elif text == ")":
                        round_depth = max(0, round_depth - 1)
                    elif text == "[":
                        square_depth += 1
                    elif text == "]":
                        square_depth = max(0, square_depth - 1)
                    elif text == "|" and not round_depth and not square_depth:
                        break
                    elif text in {";", "{"} and not round_depth and not square_depth:
                        closing_pipe = end
                        break
                    closing_pipe += 1
                if closing_pipe >= end:
                    cursor += 1
                    continue
                body_start = closing_pipe + 1
                while body_start < end and tokens[body_start].text not in {
                    "{",
                    ";",
                    ",",
                }:
                    body_start += 1
                if body_start < end and tokens[body_start].text == "{":
                    body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                else:
                    body_end = body_start
                ranges.append((closing_pipe + 1, body_end))
                cursor = body_end + 1
            return ranges

        def nonexecuted_token_ranges(start: int, end: int) -> list[tuple[int, int]]:
            """Return macro and closure ranges that cannot prove runtime linkage.

            Raises:
              ContractError: A macro or closure token tree is unbalanced.
            """
            return macro_token_ranges(start, end) + closure_token_ranges(start, end)

        def qualified_calls(start: int, end: int) -> set[tuple[str, str]]:
            """Return non-macro qualified function calls in a token interval.

            Raises:
              ContractError: A macro token tree has unbalanced delimiters.
            """
            nonexecuted_ranges = nonexecuted_token_ranges(start, end)
            calls: set[tuple[str, str]] = set()
            cursor = start
            while cursor < end:
                if any(
                    begin <= cursor <= finish for begin, finish in nonexecuted_ranges
                ):
                    cursor += 1
                    continue
                if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", tokens[cursor].text):
                    parts = [tokens[cursor].text]
                    lookahead = cursor + 1
                    while (
                        lookahead + 1 < end
                        and tokens[lookahead].text == "::"
                        and re.fullmatch(
                            r"[A-Za-z_][A-Za-z0-9_]*",
                            tokens[lookahead + 1].text,
                        )
                    ):
                        parts.append(tokens[lookahead + 1].text)
                        lookahead += 2
                    if (
                        len(parts) >= 2
                        and lookahead < end
                        and tokens[lookahead].text == "("
                    ):
                        normalized = parts[1:] if parts[0] == "crate" else parts
                        calls.add(("::".join(normalized[:-1]), normalized[-1]))
                cursor += 1
            return calls

        def scan_scope(
            start: int,
            end: int,
            private_root: str | None,
            current_module_path: tuple[str, ...],
            active_wrapper: tuple[str, str] | None = None,
        ) -> None:
            """Scan nested bodies and resolve module visibility recursively.

            ``private_root`` is the full identity of the private unsafe module;
            ``current_module_path`` preserves that identity while descending into
            same-named modules. Neither value is merely a visibility flag.
            ``active_wrapper`` names the visible safe function whose body owns any
            unsafe operation encountered in this scope.

            Raises:
              ContractError: Unsafe code lacks private containment or documentation.
            """
            cursor = start
            while cursor < end:
                token = tokens[cursor]
                if token.text == "mod" and cursor + 1 < end:
                    name = tokens[cursor + 1].text
                    declared_private = not item_is_public(cursor, start)
                    child_module_path = (*current_module_path, name)
                    child_private_root = (
                        "::".join(child_module_path)
                        if private_root is not None or declared_private
                        else None
                    )
                    body_start = next(
                        (
                            index
                            for index in range(cursor + 2, min(end, cursor + 5))
                            if tokens[index].text in {"{", ";"}
                        ),
                        None,
                    )
                    if body_start is not None and tokens[body_start].text == "{":
                        body_end = matching_rust_delimiter(tokens, body_start, "{", "}")
                        scan_scope(
                            body_start + 1,
                            body_end,
                            child_private_root,
                            child_module_path,
                            None,
                        )
                        cursor = body_end + 1
                        continue
                    if body_start is not None and name:
                        candidates = (
                            module_directory / f"{name}.rs",
                            module_directory / name / "mod.rs",
                        )
                        child = next(
                            (
                                candidate
                                for candidate in candidates
                                if candidate.is_file()
                            ),
                            None,
                        )
                        if child is None:
                            raise ContractError(
                                f"{path}:{token.line} cannot resolve Rust module {name}"
                            )
                        scan_file(child, child_private_root, child_module_path)
                        cursor = body_start + 1
                        continue
                if token.text == "fn" and cursor + 1 < end:
                    function_name = tokens[cursor + 1].text
                    function_public = item_is_public(cursor, start)
                    prefix_cursor = cursor - 1
                    unsafe_function = False
                    while prefix_cursor >= start and tokens[prefix_cursor].text not in {
                        ";",
                        "{",
                        "}",
                    }:
                        unsafe_function |= tokens[prefix_cursor].text == "unsafe"
                        prefix_cursor -= 1
                    _, body_start, _ = RustPublicApiScanner(()).item_bounds(
                        tokens, cursor, end, "fn"
                    )
                    body_end = (
                        matching_rust_delimiter(tokens, body_start, "{", "}")
                        if body_start is not None
                        else None
                    )
                    if (
                        private_root
                        and function_public
                        and not unsafe_function
                        and function_name
                        and body_start is not None
                        and body_end is not None
                    ):
                        wrapper = (private_root, function_name)
                        wrapper_unsafe_references.setdefault(wrapper, set())
                        scan_scope(
                            body_start + 1,
                            body_end,
                            private_root,
                            current_module_path,
                            wrapper,
                        )
                        cursor = body_end + 1
                        continue
                    elif (
                        private_root is None
                        and function_public
                        and not unsafe_function
                        and body_start is not None
                        and body_end is not None
                    ):
                        public_wrapper_calls.update(
                            qualified_calls(body_start + 1, body_end)
                        )
                        scan_scope(
                            body_start + 1,
                            body_end,
                            None,
                            current_module_path,
                            None,
                        )
                        cursor = body_end + 1
                        continue
                    elif body_start is not None and body_end is not None:
                        scan_scope(
                            body_start + 1,
                            body_end,
                            private_root,
                            current_module_path,
                            None,
                        )
                        cursor = body_end + 1
                        continue
                static_mut = (
                    token.text == "static"
                    and cursor + 1 < end
                    and tokens[cursor + 1].text == "mut"
                )
                if token.text == "unsafe" or static_mut:
                    if private_root is None:
                        raise ContractError(
                            f"{path}:{token.line} unsafe code must reside in a private named module"
                        )
                    if item_is_public(cursor, start):
                        raise ContractError(
                            f"{path}:{token.line} unsafe items must not be public wrappers"
                        )
                    if not has_safety_rationale(token.line):
                        raise ContractError(
                            f"{path}:{token.line} unsafe code requires a nearby SAFETY: rationale"
                        )
                    unsafe_modules.add(private_root)
                    if active_wrapper is not None:
                        wrappers_with_unsafe.add(active_wrapper)
                        if (
                            token.text == "unsafe"
                            and cursor + 1 < end
                            and tokens[cursor + 1].text == "{"
                        ):
                            unsafe_end = matching_rust_delimiter(
                                tokens, cursor + 1, "{", "}"
                            )
                            references = wrapper_unsafe_references.setdefault(
                                active_wrapper, set()
                            )
                            # `self::` fixes resolution to the private module, preventing a
                            # shadowing local/closure or stringified name from satisfying linkage.
                            nonexecuted_ranges = nonexecuted_token_ranges(
                                cursor + 2, unsafe_end
                            )
                            for index in range(cursor + 2, unsafe_end):
                                if any(
                                    begin <= index <= finish
                                    for begin, finish in nonexecuted_ranges
                                ):
                                    continue
                                if (
                                    tokens[index].text == "self"
                                    and index + 2 < unsafe_end
                                    and tokens[index + 1].text == "::"
                                ):
                                    name = tokens[index + 2].text
                                    references.add((name, "access"))
                                    if (
                                        index + 3 < unsafe_end
                                        and tokens[index + 3].text == "("
                                    ):
                                        references.add((name, "call"))
                    elif static_mut and cursor + 2 < end:
                        unsafe_items.setdefault(private_root, {})[
                            tokens[cursor + 2].text
                        ] = "access"
                    elif token.text == "unsafe":
                        function_index = next(
                            (
                                index
                                for index in range(cursor + 1, min(end, cursor + 5))
                                if tokens[index].text == "fn"
                            ),
                            None,
                        )
                        if function_index is not None and function_index + 1 < end:
                            unsafe_items.setdefault(private_root, {})[
                                tokens[function_index + 1].text
                            ] = "call"
                if token.text == "{":
                    body_end = matching_rust_delimiter(tokens, cursor, "{", "}")
                    scan_scope(
                        cursor + 1,
                        body_end,
                        private_root,
                        current_module_path,
                        active_wrapper,
                    )
                    cursor = body_end + 1
                    continue
                cursor += 1

        scan_scope(0, len(tokens), private_module_root, module_path)

    scan_file(source_path, None, ())
    for module_name in sorted(unsafe_modules):
        wrappers = {
            wrapper for wrapper in wrappers_with_unsafe if wrapper[0] == module_name
        }
        if not wrappers:
            raise ContractError(
                f"{source_path} private unsafe module {module_name} must expose a safe wrapper"
            )
        wrapped_references = set().union(
            *(wrapper_unsafe_references[wrapper] for wrapper in wrappers)
        )
        unwrapped_items = {
            name
            for name, reference_kind in unsafe_items.get(module_name, {}).items()
            if (name, reference_kind) not in wrapped_references
        }
        if unwrapped_items:
            raise ContractError(
                f"{source_path} unsafe module {module_name} has unwrapped items: "
                + ", ".join(sorted(unwrapped_items))
            )
        if not (wrappers & public_wrapper_calls):
            raise ContractError(
                f"{source_path} must route a public safe API through unsafe module {module_name}"
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
            path
            for path in (source_directory / "lib.rs", source_directory / "main.rs")
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
            if package_name in allowed:
                validate_allowlisted_unsafe_source(source_file)


def validate_vendored_sources(root: Path) -> None:
    """Validate the local source adaptations required by the production graph.

    Raises:
      ContractError: A vendored pin or required source patch is absent or changed.
    """
    ba2_manifest = load_toml(root / "vendor/ba2-3.0.1/Cargo.toml")
    require_equal(
        ba2_manifest.get("package", {}).get("version"), "3.0.1", "ba2 version"
    )
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
        raise ContractError(
            "serde-hkx vendoring record does not contain the pinned commit"
        )

    for relative_path, expected_digest in EXPECTED_VENDOR_TREE_DIGESTS.items():
        require_equal(
            source_tree_digest(root / relative_path),
            expected_digest,
            f"{relative_path} authenticated source tree SHA-256",
        )


def validate_lockfile(root: Path) -> None:
    """Validate every locked source plus the production package selections.

    Raises:
      ContractError: A source moves, lacks authentication, or a pin is absent.
    """
    lockfile = load_toml(root / "Cargo.lock")
    packages = lockfile.get("package", [])
    registry_source = "registry+https://github.com/rust-lang/crates.io-index"
    for package in packages:
        identity = (package.get("name"), package.get("version"))
        source = package.get("source")
        checksum = package.get("checksum")
        if source is None:
            if identity not in EXPECTED_LOCAL_LOCK_PACKAGES:
                raise ContractError(f"unapproved local Cargo source for {identity!r}")
            if checksum is not None:
                raise ContractError(f"local Cargo package {identity!r} has a checksum")
            continue
        if source == registry_source:
            if (
                not isinstance(checksum, str)
                or re.fullmatch(r"[0-9a-f]{64}", checksum) is None
            ):
                raise ContractError(
                    f"registry Cargo source for {identity!r} lacks a SHA-256 checksum"
                )
            continue
        if isinstance(source, str) and source.startswith("git+"):
            source_without_revision, separator, revision = source.rpartition("#")
            revision_query = re.search(
                r"[?&]rev=([0-9a-f]{40})(?:&|$)", source_without_revision
            )
            if (
                separator != "#"
                or re.fullmatch(r"[0-9a-f]{40}", revision) is None
                or revision_query is None
                or revision_query.group(1) != revision
                or re.search(r"[?&](?:branch|tag)=", source_without_revision)
            ):
                raise ContractError(f"moving Cargo source for {identity!r}: {source}")
            if checksum is not None:
                raise ContractError(
                    f"Git Cargo package {identity!r} has a registry checksum"
                )
            continue
        raise ContractError(f"unapproved Cargo source for {identity!r}: {source!r}")

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
        raise ContractError(
            f"Cargo.lock is missing selected production pins: {missing}"
        )

    first_party_names = set(EXPECTED_PACKAGE_NAMES.values())
    locked_first_party = {
        name
        for name, version in identities
        if name in first_party_names and version == "0.0.0"
    }
    require_equal(
        locked_first_party,
        first_party_names,
        "first-party packages represented in Cargo.lock",
    )

    skia_bindings = [
        package
        for package in packages
        if package.get("name") == "skia-bindings" and package.get("version") == "0.99.0"
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
    require_equal(
        source_lock.get("tools"),
        {
            "gn": {
                "source_uri": (
                    "https://chrome-infra-packages.appspot.com/dl/gn/gn/"
                    "windows-amd64/+/git_revision:"
                    "b2afae122eeb6ce09c52d63f67dc53fc517dbdc8"
                ),
                "revision": "b2afae122eeb6ce09c52d63f67dc53fc517dbdc8",
                "version": "2175 (b2afae122eeb)",
                "size_bytes": 2371072,
                "executable_sha256": (
                    "cf28c1ea5c5c6c2ec1c2e4e12b360f276595ceea8de6a35368dfd1f9b39b527d"
                ),
            },
            "llvm": {
                "source_uri": (
                    "https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.8"
                ),
                "revision": "ca7933e47d3a3451d81e72ac174dcb5aa28b59d1",
                "version": "22.1.8",
                "clang_cl_sha256": (
                    "986af49c2d1eefeac324f724ff54597753d1053927fa5d58a37a457f578e1cf8"
                ),
            },
        },
        "Skia build tool pins",
    )


def production_cargo_graph(
    root: Path, build_environment: Mapping[str, str] | None = None
) -> dict[str, Any]:
    """Resolve and normalize the frozen Windows production dependency graph.

    The normalized graph retains activated features, normal/build edges, source
    identities, and custom-build targets without embedding checkout-local paths.
    A supplied build environment selects the authenticated absolute Cargo command.

    Raises:
      ContractError: Cargo cannot resolve the locked offline production graph.
    """
    command = [
        (
            build_environment.get("CAO_CARGO_COMMAND", "cargo")
            if build_environment is not None
            else "cargo"
        ),
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--offline",
        "--filter-platform",
        "x86_64-pc-windows-msvc",
        "--manifest-path",
        str(root / "Cargo.toml"),
    ]
    try:
        result = subprocess.run(
            command,
            cwd=root,
            env=dict(build_environment) if build_environment is not None else None,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        raise ContractError(f"cannot launch Cargo metadata: {error}") from error
    if result.returncode != 0:
        diagnostic = result.stderr.strip() or result.stdout.strip()
        raise ContractError(f"cannot resolve production Cargo graph: {diagnostic}")
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ContractError(f"Cargo metadata returned invalid JSON: {error}") from error

    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root_ids: dict[str, str] = {}
    for package_id, package in packages.items():
        if package["name"] in PRODUCTION_ROOTS and package["source"] is None:
            root_ids[package["name"]] = package_id
    require_equal(set(root_ids), set(PRODUCTION_ROOTS), "production Cargo graph roots")

    reached_by: dict[str, set[str]] = {}
    for root_name in PRODUCTION_ROOTS:
        pending = [root_ids[root_name]]
        reached = reached_by.setdefault(root_name, set())
        while pending:
            package_id = pending.pop()
            if package_id in reached:
                continue
            reached.add(package_id)
            for dependency in nodes[package_id]["deps"]:
                if any(
                    dependency_kind["kind"] in (None, "build")
                    for dependency_kind in dependency["dep_kinds"]
                ):
                    pending.append(dependency["pkg"])

    reachable = set().union(*reached_by.values())
    forbidden = sorted(
        packages[package_id]["name"]
        for package_id in reachable
        if packages[package_id]["name"] in FORBIDDEN_PRODUCTION_PACKAGES
    )
    if forbidden:
        raise ContractError(
            "verification/oracle packages reached staged production roots: "
            + ", ".join(forbidden)
        )

    def normalized_source(package: Mapping[str, Any]) -> str:
        """Return a stable registry/Git source or repository-relative path.

        Raises:
          ContractError: A local dependency escapes the repository root.
        """
        source = package.get("source")
        if source is not None:
            return str(source)
        manifest_path = Path(package["manifest_path"]).resolve()
        try:
            relative_path = manifest_path.parent.relative_to(root).as_posix()
        except ValueError as error:
            raise ContractError(
                f"local Cargo package escapes the repository: {manifest_path}"
            ) from error
        return f"path:{relative_path}"

    stable_ids = {
        package_id: (
            f"{package['name']}@{package['version']}|{normalized_source(package)}"
        )
        for package_id, package in packages.items()
        if package_id in reachable
    }
    normalized_packages: list[dict[str, Any]] = []
    for package_id in sorted(reachable, key=lambda value: stable_ids[value]):
        package = packages[package_id]
        dependencies = []
        for dependency in nodes[package_id]["deps"]:
            if dependency["pkg"] not in reachable:
                continue
            dependency_kinds = sorted(
                {
                    "normal" if item["kind"] is None else item["kind"]
                    for item in dependency["dep_kinds"]
                    if item["kind"] in (None, "build")
                }
            )
            if dependency_kinds:
                dependencies.append(
                    {
                        "kinds": dependency_kinds,
                        "name": dependency["name"],
                        "package": stable_ids[dependency["pkg"]],
                    }
                )
        dependencies.sort(
            key=lambda item: (item["name"], item["package"], item["kinds"])
        )
        normalized_packages.append(
            {
                "build_script": any(
                    "custom-build" in target["kind"] for target in package["targets"]
                ),
                "dependencies": dependencies,
                "features": sorted(nodes[package_id]["features"]),
                "id": stable_ids[package_id],
                "roots": [
                    root_name
                    for root_name in PRODUCTION_ROOTS
                    if package_id in reached_by[root_name]
                ],
            }
        )

    return {
        "network_capable_build_scripts": [
            {
                "guard": "authenticated-local-source",
                "package": "skia-bindings@0.99.0",
                "required_environment": [
                    "CAO_CARGO_COMMAND",
                    "CAO_CARGO_HOME",
                    "CAO_GIT_COMMAND",
                    "CAO_MSVC_TOOLCHAIN_DIR",
                    "CAO_RUSTC_COMMAND",
                    "CAO_RUSTUP_HOME",
                    "CAO_WINDOWS_SDK_DIR",
                    "LLVM_HOME",
                    "SKIA_GN_COMMAND",
                    "SKIA_NINJA_COMMAND",
                    "SKIA_SOURCE_DIR",
                ],
            }
        ],
        "packages": normalized_packages,
        "roots": list(PRODUCTION_ROOTS),
        "schema_version": 1,
        "target": "x86_64-pc-windows-msvc",
    }


def validate_production_cargo_graph(
    root: Path, build_environment: Mapping[str, str] | None = None
) -> None:
    """Compare the live locked graph with its committed review artifact.

    A supplied build environment is forwarded to Cargo graph resolution.

    Raises:
      ContractError: Package, edge, feature, source, or build-script policy drifts.
    """
    path = root / "verification/build-inputs/production-cargo-graph.json"
    expected = load_json(path)
    actual = production_cargo_graph(root, build_environment)
    if actual != expected:
        raise ContractError(
            "production Cargo graph differs from "
            "verification/build-inputs/production-cargo-graph.json"
        )


def validate_release_script(root: Path) -> None:
    """Require the guarded offline build and allowlist-driven staging entry points.

    Raises:
      ContractError: A required build entry point is missing.
    """
    for relative_path in ("tools/build_workspace.py", "tools/stage_release.py"):
        script = root / relative_path
        if not script.is_file():
            raise ContractError(f"missing required file: {script}")


def validate(root: Path, build_environment: Mapping[str, str] | None = None) -> None:
    """Validate every machine-verifiable W1 workspace invariant.

    Build entry points supply their authenticated environment for Cargo resolution;
    standalone validation uses the caller's Cargo command.

    Raises:
      ContractError: Any workspace, source, lockfile, or staging invariant fails.
    """
    validate_toolchain(root)
    validate_workspace_manifest(root)
    validate_package_manifests(root)
    validate_leaf_type_boundaries(root)
    validate_unsafe_boundaries(root)
    validate_vendored_sources(root)
    validate_lockfile(root)
    validate_skia_source_lock(root)
    validate_production_cargo_graph(root, build_environment)
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
