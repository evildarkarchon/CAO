# Building the Rust workspace

The root Cargo workspace is locked to Rust 1.97.0 and the `x86_64-pc-windows-msvc` target. Its normal production build is intentionally offline: acquire the registry sources and native tool cache first, then use only frozen commands.

## Acquire inputs

In a network-enabled acquisition environment, install the toolchain from `rust-toolchain.toml` and populate Cargo's source cache with:

```powershell
cargo fetch --locked
```

Acquire Ninja 1.13.2 from the URI in `verification/baseline/implementation-inputs.json` and verify it with `tools/verify_baseline.py`. Check out the Skia root and every build-required external repository at the exact revisions in `verification/build-inputs/skia-source-lock.json`. The source directory supplied to the production build must be a clean Git checkout of that complete closure; the build is not allowed to synchronize it.

The same Skia lock pins the Windows x64 GN package selected by the tracked `bin/fetch-gn` script and the LLVM/clang-cl build toolchain. Acquire GN separately, verify its extracted executable against the locked size, SHA-256, and version, and install the locked LLVM release. Do not run `fetch-gn` or place generated tools inside the clean Skia checkout. Run the build entry points with Python 3.14.5, which is the interpreter they put first on `PATH` for Skia's GN probes.

Set these standard `skia-bindings` variables to the verified offline inputs:

```powershell
$env:SKIA_SOURCE_DIR = 'D:\offline-cache\skia'
$env:SKIA_NINJA_COMMAND = 'D:\offline-cache\ninja-1.13.2\ninja.exe'
$env:SKIA_GN_COMMAND = 'D:\offline-cache\gn-b2afae122eeb\gn.exe'
$env:LLVM_HOME = 'D:\offline-cache\llvm-22.1.8'
$env:CAO_MSVC_TOOLCHAIN_DIR = 'C:\BuildTools\VC\Tools\MSVC\14.51.36231'
$env:CAO_WINDOWS_SDK_DIR = 'C:\Program Files (x86)\Windows Kits\10'
$env:CAO_CARGO_COMMAND = (Get-Command cargo).Source
$env:CAO_GIT_COMMAND = (Get-Command git).Source
$env:CAO_RUSTC_COMMAND = (Get-Command rustc).Source
$env:CAO_CARGO_HOME = "$HOME\.cargo"
$env:CAO_RUSTUP_HOME = "$HOME\.rustup"
```

`tools/build_workspace.py` and `tools/stage_release.py` reject missing, substituted, wrong-version, or conflicting inherited inputs before Cargo runs. Providing `SKIA_SOURCE_DIR`, `SKIA_GN_COMMAND`, and `SKIA_NINJA_COMMAND` forces `skia-bindings` down its local source-build path and prevents its binary-download, GN-download, and source-synchronization fallbacks. The entry points reject alternate Skia library paths, GN arguments, system libraries, and binary URLs; force a source build; and bind clang/libclang to the authenticated `LLVM_HOME`.

The same environment builder activates the authenticated MSVC 14.51.36231 tree and Windows SDK 10.0.26100.0 for Cargo, `cc`, Skia, and the Rust linker. It replaces inherited compiler/include/library selection with exact `PATH`, `INCLUDE`, `LIB`, linker, compiler, and SDK variables. Cargo, rustc, and Git must be absolute commands; Cargo and rustc must report 1.97.0 under the selected Rustup home, and the selected Cargo home must contain the authenticated offline cache without an ambient `config` or `config.toml`. For local authenticated release verification, set `CAO_WINDOWS_SDK_ISO` to the retained immutable SDK ISO and verify that archive explicitly before using the installed kit.

## Verify and build

```powershell
python tools/verify_baseline.py
python tools/verify_baseline.py --verify-input "rust-toolchain=$env:CAO_RUSTC_COMMAND"
python tools/verify_baseline.py --verify-input "msvc-toolchain=$env:CAO_MSVC_TOOLCHAIN_DIR"
python tools/verify_baseline.py --verify-input "windows-sdk=$env:CAO_WINDOWS_SDK_ISO"
python tools/verify_workspace.py
python -m unittest tests.test_ci_workflow tests.test_verify_baseline tests.test_verify_workspace -v
python tools/build_workspace.py
python tools/stage_release.py --output D:\staging\tracetide
```

`tools/verify_workspace.py` resolves the Windows x64 GUI/helper closure with Cargo offline and compares every package, activated feature, normal/build edge, source identity, and custom-build target with `verification/build-inputs/production-cargo-graph.json`. The graph must not reach `cao-verification` or `cao-oracle-capture`. Any reviewed graph change updates that artifact together with its manifests and lockfile.

`tools/build_workspace.py` uses `cargo build --workspace --frozen` and therefore builds all twelve skeletons, including the explicitly opted-in behavioral-oracle capture package. The normal Cargo default still uses the eleven default members and excludes oracle capture.

The staging command verifies every Skia Git revision, uses a frozen release build, and explicitly selects only `cao-gui` and `cao-hkx-helper`. It copies those exact executables to `tracetide.exe` and `bin/tracetide-hkx-helper.exe`; it never scans the target directory or stages verification and oracle tooling.

To run the release-staging integration test after acquiring the same offline inputs:

```powershell
$env:CAO_RUN_RELEASE_STAGING_TEST = '1'
python -m unittest tests.test_verify_workspace -v
```

## GitHub Actions pull-request validation

`.github/workflows/rust-workspace.yml` runs on the GitHub-hosted `windows-2025` image. The workflow installs Rust 1.97.0 and acquires the locked Cargo dependency graph while network access is available. It then enables Cargo offline mode and loopback-only HTTP(S)/all-proxy values before comparing the production graph, running the Python contract tests, and testing the UI-independent domain, application, Windows portable-state, and verification libraries with frozen Cargo inputs.

This hosted workflow is a review gate, not the authenticated production build. It intentionally does not set `CAO_RUN_RELEASE_STAGING_TEST`, authenticate the complete MSVC/SDK/Skia tool cache, stage the production binaries, run `tools/build_workspace.py`, or publish a release. The exact GitHub-hosted image contents may be updated independently of this repository, so a passing pull-request check does not establish release-toolchain reproducibility.

## Local authenticated release verification

Run the complete build and staging commands in the **Verify and build** section manually on a provisioned release workstation containing the authenticated offline inputs. Set the eleven build environment variables above, plus `CAO_WINDOWS_SDK_ISO` when verifying the retained SDK archive, and keep the Cargo home free of user configuration. Local release verification resolves Cargo, rustc, and Git to absolute commands under the selected tool homes, re-hashes the pinned inputs, validates the exact tool versions, stages only the two production roots, and builds the complete workspace with Cargo frozen. GitHub Actions does not perform this process.

Dependency and tool acquisition remains a manual administrative step outside local release verification. During the build, Cargo offline mode and the exact graph/build-script lock prevent registry resolution drift; mandatory local Skia inputs prevent its download and synchronization fallbacks. Loopback proxy settings are an additional defense against command-based downloads, not a claim that Windows process networking is kernel-isolated.
