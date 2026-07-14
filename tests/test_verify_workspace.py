"""Public-boundary tests for the Rust workspace contract validator."""

from __future__ import annotations

import hashlib
import importlib.util
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
        shutil.copytree(REPOSITORY_ROOT / "assets", destination / "assets")
        shutil.copytree(REPOSITORY_ROOT / "apps", destination / "apps")
        shutil.copytree(REPOSITORY_ROOT / "crates", destination / "crates")
        shutil.copytree(REPOSITORY_ROOT / "resources", destination / "resources")
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
        shutil.copy2(
            REPOSITORY_ROOT / "tools/build_workspace.py",
            destination / "tools/build_workspace.py",
        )
        shutil.copy2(
            REPOSITORY_ROOT / "tools/acquire_skia_binary.py",
            destination / "tools/acquire_skia_binary.py",
        )
        shutil.copy2(
            REPOSITORY_ROOT / "tools/export_branding.py",
            destination / "tools/export_branding.py",
        )
        (destination / "verification/build-inputs").mkdir(parents=True)
        shutil.copy2(
            REPOSITORY_ROOT / "verification/build-inputs/skia-source-lock.json",
            destination / "verification/build-inputs/skia-source-lock.json",
        )
        shutil.copy2(
            REPOSITORY_ROOT / "verification/build-inputs/skia-binary-lock.json",
            destination / "verification/build-inputs/skia-binary-lock.json",
        )
        shutil.copy2(
            REPOSITORY_ROOT / "verification/build-inputs/production-cargo-graph.json",
            destination / "verification/build-inputs/production-cargo-graph.json",
        )
        shutil.copytree(
            REPOSITORY_ROOT / "vendor/ba2-3.0.1",
            destination / "vendor/ba2-3.0.1",
        )
        shutil.copytree(
            REPOSITORY_ROOT / "vendor/serde-hkx",
            destination / "vendor/serde-hkx",
        )

    def test_release_staging_includes_only_authenticated_profile_resources(self) -> None:
        """The executable-relative startup contract must survive ZIP staging."""
        spec = importlib.util.spec_from_file_location("stage_release_under_test", STAGER)
        self.assertIsNotNone(spec)
        self.assertIsNotNone(spec.loader)
        module = importlib.util.module_from_spec(spec)
        tools_path = str(REPOSITORY_ROOT / "tools")
        sys.path.insert(0, tools_path)
        try:
            spec.loader.exec_module(module)
        finally:
            sys.path.remove(tools_path)

        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            target = Path(temporary_directory) / "target"
            destination = Path(temporary_directory) / "stage"
            release = target / module.TARGET / "release"
            release.mkdir(parents=True)
            for source_name, _ in module.RELEASE_BINARIES.values():
                (release / source_name).write_bytes(b"executable")
            for resource in module.RELEASE_RESOURCES:
                source = root / "resources" / resource
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_bytes((REPOSITORY_ROOT / "resources" / resource).read_bytes())

            original_target = os.environ.get("CARGO_TARGET_DIR")
            os.environ["CARGO_TARGET_DIR"] = str(target)
            try:
                module.stage_release_artifacts(root, destination)
            finally:
                if original_target is None:
                    os.environ.pop("CARGO_TARGET_DIR", None)
                else:
                    os.environ["CARGO_TARGET_DIR"] = original_target

            self.assertEqual(
                (destination / "resources/profiles/built-ins.state").read_bytes(),
                (REPOSITORY_ROOT / "resources/profiles/built-ins.state").read_bytes(),
            )
            self.assertFalse((destination / "profiles").exists())

    def test_modified_built_in_profile_inventory_is_rejected(self) -> None:
        """The workspace gate must pin the reviewed oracle-derived inventory bytes."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            inventory = root / "resources/profiles/built-ins.state"
            inventory.write_bytes(inventory.read_bytes() + b"SSE.identity=FO4\n")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("built-in profile inventory SHA-256", result.stderr)

    def replace_recorded_branding_hash(
        self, root: Path, original: bytes, modified: bytes
    ) -> None:
        """Update one sandbox provenance hash to model a self-consistent rebaseline.

        Raises:
          OSError: The sandbox provenance record cannot be read or written.
          AssertionError: The original asset hash is absent from the record.
        """
        readme = root / "assets/branding/README.md"
        documentation = readme.read_text(encoding="utf-8")
        old_hash = hashlib.sha256(original).hexdigest()
        new_hash = hashlib.sha256(modified).hexdigest()
        self.assertIn(old_hash, documentation)
        readme.write_text(
            documentation.replace(old_hash, new_hash, 1), encoding="utf-8"
        )

    def assert_leaf_fragment_rejected(self, fragment: str) -> None:
        """Require one Rust fragment to fail the public leaf-type boundary.

        Raises:
          OSError: Sandbox files or the validator process cannot be created.
          AssertionError: The fragment passes or fails without the leaf diagnostic.
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-bsa/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(content + "\n" + fragment, encoding="utf-8")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("public API leaks leaf type ba2", result.stderr)

    def assert_application_leaf_fragment_rejected(
        self, fragment: str, leaf_type: str
    ) -> None:
        """Require one application-seam fragment to fail leaf-type validation.

        Raises:
          OSError: Sandbox files or the validator process cannot be created.
          AssertionError: The fragment passes or lacks the expected diagnostic.
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-application/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(content + "\n" + fragment, encoding="utf-8")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn(
                f"public API leaks leaf type {leaf_type}", result.stderr
            )

    def add_local_runner_commands(self, environment: dict[str, str]) -> None:
        """Add absolute Cargo, rustc, Git, and tool-home inputs to a test env.

        Raises:
          AssertionError: Cargo, rustc, or Git is unavailable on the test machine.
        """
        cargo = shutil.which("cargo")
        git = shutil.which("git")
        rustc = shutil.which("rustc")
        self.assertIsNotNone(cargo)
        self.assertIsNotNone(git)
        self.assertIsNotNone(rustc)
        environment["CAO_CARGO_COMMAND"] = str(Path(cargo).absolute())
        environment["CAO_GIT_COMMAND"] = str(Path(git).absolute())
        environment["CAO_RUSTC_COMMAND"] = str(Path(rustc).absolute())
        environment["CAO_CARGO_HOME"] = str(Path.home() / ".cargo")
        environment["CAO_RUSTUP_HOME"] = str(Path.home() / ".rustup")

    def assert_allowlisted_unsafe_fragment_rejected(
        self, fragment: str, diagnostic: str
    ) -> None:
        """Require one allowlisted-crate unsafe fragment to fail validation.

        Raises:
          OSError: Sandbox files or the validator process cannot be created.
          AssertionError: The fragment passes or lacks the expected diagnostic.
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(content + "\n" + fragment, encoding="utf-8")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn(diagnostic, result.stderr)

    def test_committed_workspace_contract_passes(self) -> None:
        """Accept the exact production, verification, and oracle package graph."""
        result = self.run_validator(REPOSITORY_ROOT)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Rust workspace contract verified", result.stdout)

    def test_workspace_rejects_static_crt_rustflags(self) -> None:
        """The public validator rejects a static CRT override in Cargo configuration."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            cargo_config = root / ".cargo/config.toml"
            cargo_config.write_text(
                '[build]\ntarget = "x86_64-pc-windows-msvc"\n\n'
                + "[target.x86_64-pc-windows-msvc]\n"
                + 'rustflags = ["-C", "target-feature=+crt-static"]\n',
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("dynamic CRT Cargo configuration", result.stderr)

    def test_workspace_rejects_build_level_static_crt_rustflags(self) -> None:
        """The public validator rejects static CRT flags from the build table."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            cargo_config = root / ".cargo/config.toml"
            cargo_config.write_text(
                '[build]\ntarget = "x86_64-pc-windows-msvc"\n'
                + 'rustflags = ["-C", "target-feature=+crt-static"]\n',
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("dynamic CRT Cargo configuration", result.stderr)

    def test_workspace_requires_reviewed_branding_contract(self) -> None:
        """Reject a production workspace without the owned branding artifacts."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            (root / "assets/branding/tracetide.ico").unlink()

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing required branding file", result.stderr)

    def test_workspace_rejects_branding_asset_hash_drift(self) -> None:
        """Reject a reviewed branding asset whose recorded bytes have changed."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            mark = root / "assets/branding/tracetide-mark.svg"
            mark.write_bytes(mark.read_bytes() + b"\n")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("branding SHA-256", result.stderr)

    def test_workspace_rejects_unreviewed_branding_rebaseline(self) -> None:
        """Reject structurally valid asset bytes outside the reviewed hash set."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            mark = root / "assets/branding/tracetide-mark.svg"
            original = mark.read_bytes()
            modified = original.replace(b"?>\n<svg", b"?>\n\n<svg", 1)
            self.assertNotEqual(original, modified)
            mark.write_bytes(modified)

            self.replace_recorded_branding_hash(root, original, modified)

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("reviewed branding SHA-256", result.stderr)

    def test_workspace_rejects_unapproved_icon_layer_sizes(self) -> None:
        """Reject a rehashed ICO whose directory omits an approved native size."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            icon = root / "assets/branding/tracetide.ico"
            content = bytearray(icon.read_bytes())
            self.assertEqual(content[6], 16)
            content[6] = 17
            icon.write_bytes(content)

            self.replace_recorded_branding_hash(
                root,
                (REPOSITORY_ROOT / "assets/branding/tracetide.ico").read_bytes(),
                bytes(content),
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("ICO layer", result.stderr)

    def test_workspace_rejects_unapproved_canonical_mark_geometry(self) -> None:
        """Reject a rehashed vector master that changes the approved construction."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            mark = root / "assets/branding/tracetide-mark.svg"
            original = mark.read_bytes()
            modified = original.replace(b'stroke-width="3"', b'stroke-width="4"', 1)
            self.assertNotEqual(original, modified)
            mark.write_bytes(modified)

            self.replace_recorded_branding_hash(root, original, modified)

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("canonical SVG geometry", result.stderr)

    def test_workspace_rejects_colored_high_contrast_mark(self) -> None:
        """Reject a rehashed High Contrast source that is no longer monochrome."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            mark = root / "assets/branding/tracetide-mark-monochrome.svg"
            original = mark.read_bytes()
            modified = original.replace(b'#000000', b'#5A43C0', 1)
            self.assertNotEqual(original, modified)
            mark.write_bytes(modified)

            self.replace_recorded_branding_hash(root, original, modified)

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("monochrome SVG colors", result.stderr)

    def test_workspace_requires_semantic_theme_tokens(self) -> None:
        """Reject a production workspace without its approved semantic palette."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            (root / "apps/cao-gui/ui/theme.slint").unlink()

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing semantic theme token file", result.stderr)

    def test_workspace_rejects_semantic_theme_token_drift(self) -> None:
        """Reject palette drift or lifecycle roles coupled to brand colors."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            theme = root / "apps/cao-gui/ui/theme.slint"
            content = theme.read_text(encoding="utf-8")
            self.assertIn("#f6f6fb", content)
            theme.write_text(
                content.replace("#f6f6fb", "#f5f5fa", 1), encoding="utf-8"
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("semantic theme tokens", result.stderr)

    def test_workspace_requires_public_identity_metadata(self) -> None:
        """Reject a workspace without approved identity and release recheck data."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            (root / "assets/branding/identity.json").unlink()

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing public identity metadata", result.stderr)

    def test_workspace_rejects_public_identity_metadata_drift(self) -> None:
        """Reject valid JSON that changes approved identity or recheck policy."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            identity = root / "assets/branding/identity.json"
            content = identity.read_text(encoding="utf-8")
            self.assertIn('"display_name": "Tracetide"', content)
            identity.write_text(
                content.replace(
                    '"display_name": "Tracetide"',
                    '"display_name": "Cathedral Assets Optimizer"',
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("public identity metadata contract", result.stderr)

    def test_workspace_rejects_incomplete_branding_provenance(self) -> None:
        """Reject branding records without authorship, license, or export checks."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            readme = root / "assets/branding/README.md"
            content = readme.read_text(encoding="utf-8")
            self.assertIn("License: `GPL-3.0-only`", content)
            readme.write_text(
                content.replace("License: `GPL-3.0-only`", "License: unrecorded", 1),
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("branding provenance record", result.stderr)

    def test_workspace_requires_branding_export_tool(self) -> None:
        """Reject a documented branding procedure whose exporter is unavailable."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            (root / "tools/export_branding.py").unlink()

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing branding export tool", result.stderr)

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

    def test_moving_git_source_in_lockfile_must_be_rejected(self) -> None:
        """Reject a locked package redirected to a moving Git branch source."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            lockfile = root / "Cargo.lock"
            content = lockfile.read_text(encoding="utf-8")
            package = (
                'name = "crossbeam-channel"\n'
                'version = "0.5.16"\n'
                'source = "registry+https://github.com/rust-lang/crates.io-index"'
            )
            redirected = (
                'name = "crossbeam-channel"\n'
                'version = "0.5.16"\n'
                'source = "git+https://example.invalid/crossbeam?branch=main'
                '#0123456789abcdef0123456789abcdef01234567"'
            )
            self.assertIn(package, content)
            lockfile.write_text(
                content.replace(package, redirected, 1), encoding="utf-8"
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("moving Cargo source", result.stderr)

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

    def test_skia_binary_lock_must_remain_byte_exact(self) -> None:
        """Reject any unreviewed change to the authenticated Skia binary archive."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            binary_lock = root / "verification/build-inputs/skia-binary-lock.json"
            content = binary_lock.read_text(encoding="utf-8")
            binary_lock.write_text(
                content.replace(
                    "9e6c3d1da63ae202bff9938329ccaf81afc24acb4193aec15d6f0aac72a5960f",
                    "0" * 64,
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Skia binary lock SHA-256", result.stderr)

    def test_production_graph_lock_drift_must_be_rejected(self) -> None:
        """Reject a resolved production feature/build graph outside review."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            graph_lock = root / "verification/build-inputs/production-cargo-graph.json"
            graph_lock.write_text("{}\n", encoding="utf-8")

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("production Cargo graph", result.stderr)

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
            source.write_text(
                content + "\n// Unreviewed source mutation.\n", encoding="utf-8"
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("authenticated source tree SHA-256", result.stderr)

    def test_leaf_dependency_type_must_not_cross_a_public_api(self) -> None:
        """Reject a backend library type exposed through a public adapter signature."""
        self.assert_leaf_fragment_rejected(
            "pub fn leak_archive(_: ba2::fo4::Archive) {}\n"
        )

    def test_application_handle_must_hide_its_queue_transport(self) -> None:
        """Reject a crossbeam sender exposed by the public application handle."""
        self.assert_application_leaf_fragment_rejected(
            "pub struct LeakyApplicationHandle {\n"
            "    pub sender: crossbeam_channel::Sender<Intent>,\n"
            "}\n",
            "crossbeam_channel",
        )

    def test_snapshot_sink_must_not_expose_a_ui_type(self) -> None:
        """Reject a Slint value exposed by a public snapshot-sink contract."""
        self.assert_application_leaf_fragment_rejected(
            "pub trait LeakySnapshotSink {\n"
            "    fn publish(&self, snapshot: slint::SharedString);\n"
            "}\n",
            "slint",
        )

    def test_aliased_leaf_type_must_not_cross_a_public_api(self) -> None:
        """Reject a private import alias used by a public adapter signature."""
        self.assert_leaf_fragment_rejected(
            "use ba2::fo4::Archive as NativeArchive;\n"
            "pub fn leak_archive(_: NativeArchive) {}\n"
        )

    def test_leaf_type_in_public_enum_variant_must_be_rejected(self) -> None:
        """Reject a leaf type in an implicitly public enum variant field."""
        self.assert_leaf_fragment_rejected(
            "pub enum Leak { Archive(ba2::fo4::Archive) }\n"
        )

    def test_private_module_leaf_reexport_must_be_rejected(self) -> None:
        """Reject a leaf alias exported through an otherwise private module."""
        self.assert_leaf_fragment_rejected(
            "mod hidden { pub type Native = ba2::fo4::Archive; }\n"
            "pub use hidden::Native;\n"
        )

    def test_private_module_public_field_reexport_must_be_rejected(self) -> None:
        """Reject a re-exported struct whose public field exposes a leaf type."""
        self.assert_leaf_fragment_rejected(
            "mod hidden {\n"
            "    pub struct Native { pub archive: ba2::fo4::Archive }\n"
            "}\n"
            "pub use hidden::Native;\n"
        )

    def test_private_module_leaf_glob_reexport_must_be_rejected(self) -> None:
        """Reject a leaf alias exported through a module glob."""
        self.assert_leaf_fragment_rejected(
            "mod hidden { pub type Native = ba2::fo4::Archive; }\n"
            "pub use hidden::*;\n"
            "pub fn leak_archive(_: Native) {}\n"
        )

    def test_nested_private_module_leaf_reexport_must_be_rejected(self) -> None:
        """Reject a leaf alias exported through a public child of a private module."""
        self.assert_leaf_fragment_rejected(
            "mod outer {\n"
            "    pub mod inner { pub type Native = ba2::fo4::Archive; }\n"
            "}\n"
            "pub use outer::inner::*;\n"
            "pub fn leak_archive(_: Native) {}\n"
        )

    def test_private_module_renames_preserve_descendant_leaf_taint(self) -> None:
        """Reject named and crate-qualified module aliases that carry leaf types."""
        cases = (
            ("use outer as renamed;\npub use renamed::inner::Native;\n"),
            ("use outer::inner as renamed;\npub use renamed::Native;\n"),
            ("use crate::outer::inner as renamed;\npub use renamed::Native;\n"),
        )
        for reexport in cases:
            with self.subTest(reexport=reexport):
                self.assert_leaf_fragment_rejected(
                    "mod outer {\n"
                    "    pub mod inner { pub type Native = ba2::fo4::Archive; }\n"
                    "}\n" + reexport
                )

    def test_grouped_use_trees_preserve_leaf_taint(self) -> None:
        """Reject grouped named, glob, and renamed leaf-bearing imports."""
        cases = (
            "pub use outer::{inner::Native};\n",
            "pub use outer::{inner::*};\npub fn leak(_: Native) {}\n",
            "use outer::{inner as renamed};\npub use renamed::Native;\n",
            ("use outer::inner::{self as renamed};\npub use renamed::Native;\n"),
        )
        for reexport in cases:
            with self.subTest(reexport=reexport):
                self.assert_leaf_fragment_rejected(
                    "mod outer {\n"
                    "    pub mod inner { pub type Native = ba2::fo4::Archive; }\n"
                    "}\n" + reexport
                )

    def test_child_modules_resolve_parent_and_crate_leaf_paths(self) -> None:
        """Reject sibling leaf aliases reached through child-relative paths."""
        cases = (
            "pub use super::leaf::Native;\n",
            "pub use crate::leaf::Native;\n",
            "pub type Leak = super::leaf::Native;\n",
        )
        for child_api in cases:
            with self.subTest(child_api=child_api):
                self.assert_leaf_fragment_rejected(
                    "mod leaf { pub type Native = ba2::fo4::Archive; }\n"
                    "pub mod api {\n"
                    f"    {child_api}"
                    "}\n"
                )

    def test_child_modules_preserve_crate_qualified_module_aliases(self) -> None:
        """Reject flat and grouped crate-qualified aliases of tainted modules."""
        imports = (
            "use crate::leaf as imported;\n",
            "use crate::{leaf as imported};\n",
        )
        for module_import in imports:
            with self.subTest(module_import=module_import):
                self.assert_leaf_fragment_rejected(
                    "mod leaf { pub type Native = ba2::fo4::Archive; }\n"
                    "pub mod api {\n"
                    f"    {module_import}"
                    "    pub use imported::Native;\n"
                    "}\n"
                )

    def test_extern_crate_leaf_alias_must_be_rejected(self) -> None:
        """Reject a public signature reached through an extern-crate alias."""
        self.assert_leaf_fragment_rejected(
            "extern crate ba2 as archive;\n"
            "pub fn leak_archive(_: archive::fo4::Archive) {}\n"
        )

    def test_private_leaf_storage_and_body_usage_are_allowed(self) -> None:
        """Allow leaf implementation details that cannot cross a public seam."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-bsa/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content
                + "\n// pub fn comment(_: ba2::fo4::Archive) {}\n"
                + 'pub const NOTE: &str = r#"ba2::fo4::Archive"#;\n'
                + "pub struct SafeArchive { native: ba2::fo4::Archive }\n"
                + "pub fn use_archive_in_body() {\n"
                + "  let _: Option<ba2::fo4::Archive> = None;\n"
                + "}\n"
                + "mod private_adapter {\n"
                + "  pub fn internal(_: ba2::fo4::Archive) {}\n"
                + "}\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_safe_private_module_adapter_reexport_is_allowed(self) -> None:
        """Allow re-exporting an adapter that keeps its leaf storage private."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-bsa/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content
                + "\nmod hidden {\n"
                + "    pub struct Adapter { native: ba2::fo4::Archive }\n"
                + "}\n"
                + "pub use hidden::Adapter;\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_commented_unsafe_forbid_attribute_must_be_rejected(self) -> None:
        """Reject a non-allowlisted crate whose unsafe lint is only a comment."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-domain/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content.replace(
                    "#![forbid(unsafe_code)]",
                    "// #![forbid(unsafe_code)]\npub unsafe fn unchecked_domain_entry() {}",
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must declare #![forbid(unsafe_code)]", result.stderr)

    def test_allowlisted_unsafe_requires_a_private_documented_module(self) -> None:
        """Reject undocumented unsafe code placed at an allowlisted crate root."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content + "\nunsafe fn native_call() {}\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("private named module", result.stderr)

    def test_allowlisted_static_mut_requires_private_containment(self) -> None:
        """Reject a root-level mutable static even without an unsafe keyword."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content + "\npub static mut NATIVE_STATE: usize = 0;\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("private named module", result.stderr)

    def test_private_unsafe_module_requires_a_connected_safe_wrapper(self) -> None:
        """Reject private unsafe code that no public safe API routes through."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content
                + "\nmod native {\n"
                + "    // SAFETY: The empty skeleton performs no unchecked operation.\n"
                + "    unsafe fn native_call() {}\n"
                + "}\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("safe wrapper", result.stderr)

    def test_unrelated_safe_function_does_not_wrap_unsafe_item(self) -> None:
        """Reject module-name coincidence without a wrapper unsafe operation."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content
                + "\nmod native {\n"
                + "    // SAFETY: The empty skeleton performs no unchecked operation.\n"
                + "    unsafe fn danger() {}\n"
                + "    pub(super) fn fake() {\n"
                + "        // SAFETY: This unrelated block performs no operation.\n"
                + "        unsafe {}\n"
                + "        stringify!(danger);\n"
                + "    }\n"
                + "}\n"
                + "pub fn safe_adapter() { native::fake(); }\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unwrapped items", result.stderr)

    def test_shadowed_unsafe_function_name_does_not_satisfy_linkage(self) -> None:
        """Reject local closures/functions that shadow the module unsafe item."""
        bindings = (
            "let danger = || {};",
            "fn danger() {}",
        )
        for binding in bindings:
            with self.subTest(binding=binding):
                self.assert_allowlisted_unsafe_fragment_rejected(
                    "mod native {\n"
                    "    // SAFETY: The empty skeleton performs no unchecked operation.\n"
                    "    unsafe fn danger() {}\n"
                    "    pub(super) fn call() {\n"
                    f"        {binding}\n"
                    "        // SAFETY: This calls only the shadowing local.\n"
                    "        unsafe { danger(); }\n"
                    "    }\n"
                    "}\n"
                    "pub fn safe_adapter() { native::call(); }\n",
                    "unwrapped items",
                )

    def test_macro_tokens_do_not_satisfy_unsafe_or_public_linkage(self) -> None:
        """Reject stringified calls in both unsafe and public wrapper bodies."""
        fragments = (
            (
                "mod native {\n"
                "    // SAFETY: The declaration is isolated for the test.\n"
                "    unsafe fn danger() {}\n"
                "    pub(super) fn call() {\n"
                "        // SAFETY: This macro does not call the declaration.\n"
                "        unsafe { stringify!(self::danger()); }\n"
                "    }\n"
                "}\n"
                "pub fn safe_adapter() { native::call(); }\n",
                "unwrapped items",
            ),
            (
                "mod native {\n"
                "    // SAFETY: The declaration is isolated for the test.\n"
                "    unsafe fn danger() {}\n"
                "    pub(super) fn call() {\n"
                "        // SAFETY: The wrapper calls the private declaration.\n"
                "        unsafe { self::danger(); }\n"
                "    }\n"
                "}\n"
                "pub fn safe_adapter() { stringify!(native::call()); }\n",
                "route a public safe API",
            ),
        )
        for fragment, diagnostic in fragments:
            with self.subTest(diagnostic=diagnostic):
                self.assert_allowlisted_unsafe_fragment_rejected(fragment, diagnostic)

    def test_macro_definition_does_not_satisfy_unsafe_linkage(self) -> None:
        """Reject an uninvoked macro definition containing an unsafe item name."""
        self.assert_allowlisted_unsafe_fragment_rejected(
            "mod native {\n"
            "    // SAFETY: The declaration is isolated for the test.\n"
            "    unsafe fn danger() {}\n"
            "    pub(super) fn call() {\n"
            "        // SAFETY: Defining this macro does not call the declaration.\n"
            "        unsafe {\n"
            "            macro_rules! fake { () => { self::danger() } }\n"
            "        }\n"
            "    }\n"
            "}\n"
            "pub fn safe_adapter() { native::call(); }\n",
            "unwrapped items",
        )

    def test_uninvoked_closure_does_not_satisfy_unsafe_linkage(self) -> None:
        """Reject an unsafe item call that only appears in a stored closure."""
        self.assert_allowlisted_unsafe_fragment_rejected(
            "mod native {\n"
            "    // SAFETY: The declaration is isolated for the test.\n"
            "    unsafe fn danger() {}\n"
            "    pub(super) fn call() {\n"
            "        // SAFETY: Defining this closure does not call the declaration.\n"
            "        unsafe {\n"
            "            let _never = || { self::danger(); };\n"
            "        }\n"
            "    }\n"
            "}\n"
            "pub fn safe_adapter() { native::call(); }\n",
            "unwrapped items",
        )

    def test_nested_unsafe_modules_keep_distinct_item_identity(self) -> None:
        """Reject an unwrapped nested item sharing its parent's unsafe name."""
        self.assert_allowlisted_unsafe_fragment_rejected(
            "mod native {\n"
            "    // SAFETY: The declaration is isolated for the test.\n"
            "    unsafe fn danger() {}\n"
            "    pub(super) fn call() {\n"
            "        // SAFETY: This calls the outer declaration only.\n"
            "        unsafe { self::danger(); }\n"
            "    }\n"
            "    pub(super) mod inner {\n"
            "        // SAFETY: The nested declaration is isolated for the test.\n"
            "        unsafe fn danger() {}\n"
            "    }\n"
            "}\n"
            "pub fn safe_adapter() { native::call(); }\n",
            "native::inner",
        )

    def test_safety_string_literal_does_not_document_unsafe_block(self) -> None:
        """Reject SAFETY text that is not carried by a Rust comment."""
        literals = (
            '        let _not_a_comment = "SAFETY: this is only data";\n',
            '        let _not_a_comment = r#"\n// SAFETY: this is only data\n"#;\n',
            '        let _not_a_comment = r#"\n* SAFETY: this is only data\n"#;\n',
        )
        for literal in literals:
            with self.subTest(literal=literal):
                self.assert_allowlisted_unsafe_fragment_rejected(
                    "mod native {\n"
                    "    // SAFETY: The declaration is isolated for the test.\n"
                    "    unsafe fn danger() {}\n"
                    "    pub(super) fn call() {\n"
                    f"{literal}"
                    "        unsafe { self::danger(); }\n"
                    "    }\n"
                    "}\n"
                    "pub fn safe_adapter() { native::call(); }\n",
                    "SAFETY: rationale",
                )

    def test_safety_rationales_must_be_substantive_and_unique(self) -> None:
        """Reject empty or reused safety comments for distinct unsafe operations."""
        fragments = (
            (
                "mod native {\n"
                "    // SAFETY:\n"
                "    unsafe fn danger() {}\n"
                "    pub(super) fn call() {\n"
                "        // SAFETY:\n"
                "        unsafe { self::danger(); }\n"
                "    }\n"
                "}\n"
                "pub fn safe_adapter() { native::call(); }\n"
            ),
            (
                "mod native {\n"
                "    // SAFETY: This rationale documents the declaration only.\n"
                "    unsafe fn danger() {}\n"
                "    pub(super) fn call() {\n"
                "        unsafe { self::danger(); }\n"
                "    }\n"
                "}\n"
                "pub fn safe_adapter() { native::call(); }\n"
            ),
        )
        for fragment in fragments:
            with self.subTest(fragment=fragment):
                self.assert_allowlisted_unsafe_fragment_rejected(
                    fragment, "SAFETY: rationale"
                )

    def test_safety_rationale_must_belong_to_the_unsafe_occurrence(self) -> None:
        """Reject rationales owned by an uninvoked macro or a closed scope."""
        unrelated_contexts = (
            (
                "        macro_rules! fake { () => {\n"
                "            // SAFETY: This comment belongs to the macro body.\n"
                "        } }\n"
            ),
            (
                "        {\n"
                "            // SAFETY: This comment belongs to the closed scope.\n"
                "        }\n"
            ),
        )
        for unrelated_context in unrelated_contexts:
            with self.subTest(unrelated_context=unrelated_context):
                self.assert_allowlisted_unsafe_fragment_rejected(
                    "mod native {\n"
                    "    // SAFETY: The declaration is isolated for the test.\n"
                    "    unsafe fn danger() {}\n"
                    "    pub(super) fn call() {\n"
                    f"{unrelated_context}"
                    "        unsafe { self::danger(); }\n"
                    "    }\n"
                    "}\n"
                    "pub fn safe_adapter() { native::call(); }\n",
                    "SAFETY: rationale",
                )

    def test_private_documented_unsafe_with_safe_wrapper_is_allowed(self) -> None:
        """Allow a documented unsafe operation hidden behind a safe public API."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repository"
            self.copy_contract_root(root)
            source = root / "crates/cao-backend-nif/src/lib.rs"
            content = source.read_text(encoding="utf-8")
            source.write_text(
                content
                + "\nmod native {\n"
                + "    // SAFETY: The empty skeleton performs no unchecked operation.\n"
                + "    unsafe fn danger() {}\n"
                + "    pub(super) fn native_call() {\n"
                + "        // SAFETY: The private declaration is called behind this wrapper.\n"
                + "        unsafe { self::danger(); }\n"
                + "    }\n"
                + "}\n"
                + "pub fn safe_adapter() { native::native_call(); }\n",
                encoding="utf-8",
            )

            result = self.run_validator(root)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_release_staging_requires_every_offline_build_guard(self) -> None:
        """Reject staging before Cargo runs when a Skia network guard is absent."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            environment = os.environ.copy()
            environment["SKIA_SOURCE_DIR"] = temporary_directory
            environment["SKIA_NINJA_COMMAND"] = str(
                Path(temporary_directory) / "ninja.exe"
            )
            environment.pop("SKIA_GN_COMMAND", None)
            environment.pop("LLVM_HOME", None)
            # A missing PATH proves the guard rejects before validation can launch Cargo.
            environment["PATH"] = ""
            result = subprocess.run(
                [
                    sys.executable,
                    str(STAGER),
                    "--output",
                    str(Path(temporary_directory) / "staged"),
                ],
                cwd=REPOSITORY_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SKIA_GN_COMMAND", result.stderr)

    def test_release_staging_rejects_unauthenticated_gn(self) -> None:
        """Reject a substituted GN executable before a production build starts."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source = root / "skia"
            (source / "include/core").mkdir(parents=True)
            (source / "DEPS").write_text("# test source\n", encoding="utf-8")
            (source / "include/core/SkCanvas.h").write_text(
                "// test source\n",
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment["SKIA_SOURCE_DIR"] = str(source)
            environment["SKIA_NINJA_COMMAND"] = sys.executable
            environment["SKIA_GN_COMMAND"] = sys.executable
            environment["LLVM_HOME"] = str(root / "llvm")
            environment["CAO_MSVC_TOOLCHAIN_DIR"] = str(root / "msvc")
            environment["CAO_WINDOWS_SDK_DIR"] = str(root / "sdk")
            self.add_local_runner_commands(environment)
            result = subprocess.run(
                [
                    sys.executable,
                    str(STAGER),
                    "--output",
                    str(root / "staged"),
                ],
                cwd=REPOSITORY_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("GN executable", result.stderr)
            self.assertIn("mismatch", result.stderr)

    def test_release_staging_rejects_inherited_skia_overrides(self) -> None:
        """Reject native source overrides before authenticating build tools."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            environment = os.environ.copy()
            environment["SKIA_SOURCE_DIR"] = str(root / "skia")
            environment["SKIA_NINJA_COMMAND"] = str(root / "ninja.exe")
            environment["SKIA_GN_COMMAND"] = str(root / "gn.exe")
            environment["LLVM_HOME"] = str(root / "llvm")
            environment["CAO_MSVC_TOOLCHAIN_DIR"] = str(root / "msvc")
            environment["CAO_WINDOWS_SDK_DIR"] = str(root / "sdk")
            environment["SKIA_LIBRARY_SEARCH_PATH"] = str(root / "unreviewed")
            self.add_local_runner_commands(environment)
            result = subprocess.run(
                [
                    sys.executable,
                    str(STAGER),
                    "--output",
                    str(root / "staged"),
                ],
                cwd=REPOSITORY_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SKIA_LIBRARY_SEARCH_PATH", result.stderr)

    def test_release_staging_rejects_higher_priority_compiler_overrides(self) -> None:
        """Reject the hyphenated target compiler variable preferred by cc-rs."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            environment = os.environ.copy()
            environment["SKIA_SOURCE_DIR"] = str(root / "skia")
            environment["SKIA_NINJA_COMMAND"] = str(root / "ninja.exe")
            environment["SKIA_GN_COMMAND"] = str(root / "gn.exe")
            environment["LLVM_HOME"] = str(root / "llvm")
            environment["CAO_MSVC_TOOLCHAIN_DIR"] = str(root / "msvc")
            environment["CAO_WINDOWS_SDK_DIR"] = str(root / "sdk")
            environment["CC_x86_64-pc-windows-msvc"] = str(root / "unreviewed-cl.exe")
            self.add_local_runner_commands(environment)
            result = subprocess.run(
                [
                    sys.executable,
                    str(STAGER),
                    "--output",
                    str(root / "staged"),
                ],
                cwd=REPOSITORY_ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("CC_X86_64-PC-WINDOWS-MSVC", result.stderr.upper())

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
