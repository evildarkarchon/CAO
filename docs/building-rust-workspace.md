# Building the Rust workspace

The root Cargo workspace is locked to Rust 1.97.0 and the `x86_64-pc-windows-msvc` target. Its normal production build is intentionally offline: acquire the registry sources and native tool cache first, then use only frozen commands.

## Acquire inputs

In a network-enabled acquisition environment, install the toolchain from `rust-toolchain.toml` and populate Cargo's source cache with:

```powershell
cargo fetch --locked
```

Acquire Ninja 1.13.2 from the URI in `verification/baseline/implementation-inputs.json` and verify it with `tools/verify_baseline.py`. Check out the Skia root and every build-required external repository at the exact revisions in `verification/build-inputs/skia-source-lock.json`. The source directory supplied to the production build must be a clean Git checkout of that complete closure; the build is not allowed to synchronize it.

Set these standard `skia-bindings` variables to the verified offline inputs:

```powershell
$env:SKIA_SOURCE_DIR = 'D:\offline-cache\skia'
$env:SKIA_NINJA_COMMAND = 'D:\offline-cache\ninja-1.13.2\ninja.exe'
```

`tools/stage_release.py` rejects missing or wrong-version inputs. Providing `SKIA_SOURCE_DIR` also forces `skia-bindings` down its documented offline source-build path, preventing its normal binary-download and source-synchronization fallback.

## Verify and build

```powershell
python tools/verify_baseline.py
python tools/verify_workspace.py
cargo build --locked --offline
python tools/stage_release.py --output D:\staging\tracetide
```

The normal build uses the eleven default members and leaves behavioral-oracle capture opted out. To verify all twelve skeletons explicitly, run `cargo build --workspace --locked --offline`.

The staging command verifies every Skia Git revision, uses a frozen release build, and explicitly selects only `cao-gui` and `cao-hkx-helper`. It copies those exact executables to `tracetide.exe` and `bin/tracetide-hkx-helper.exe`; it never scans the target directory or stages verification and oracle tooling.

To run the release-staging integration test after acquiring the same offline inputs:

```powershell
$env:CAO_RUN_RELEASE_STAGING_TEST = '1'
python -m unittest tests.test_verify_workspace -v
```
