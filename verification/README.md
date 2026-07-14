# Evidence and compliance baseline

This directory is the machine-verifiable W0 baseline for Tracetide. It records the authenticated behavioral oracle identity, blueprint-selected implementation inputs, licensing and runtime policy, and the versioned contracts for discrepancies, parity coverage, fixtures, and oracle evidence bundles. Restricted payloads are never required for ordinary validation and must not be committed.

## Validate

Run the offline validator with Python 3.11 or later. Release and deterministic-ZIP work uses the exact Python 3.14.5 input pinned in the manifest:

```powershell
python tools/verify_baseline.py
python -m unittest tests/test_verify_baseline.py -v
```

The first command validates every current record against its schema, rejects unsupported schema or baseline revisions, enforces the approved dependency and compliance identities, and resolves exact fixture/evidence/matrix references. It does not use the network.

To verify an acquired input, pass its manifest ID and local path. This hashes the file in place; it does not copy it into the repository:

```powershell
python tools/verify_baseline.py --verify-input "behavioral-oracle=D:\oracle-kit\Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z"
python tools/verify_baseline.py --verify-input "nifly=D:\sources\nifly-965a1da1be7bff145b7b3435def5c04d6e8c8cce.tar.gz"
python tools/verify_baseline.py --verify-input "rust-toolchain=C:\Users\me\.rustup\toolchains\1.97.0-x86_64-pc-windows-msvc\bin\rustc.exe"
python tools/verify_baseline.py --verify-input "msvc-toolchain=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231"
```

File inputs are verified by SHA-256 or their pinned version output. The installed MSVC input is a directory-tree identity: the validator hashes every file using its ordinal relative path, byte size, and content digest so a bootstrapper that resolves to different compiler bits cannot pass.

`--parity-gate` additionally rejects an empty, blocked, unknown, stale, or temporarily opt-in parity matrix. It is expected to fail until downstream parity work closes every required cell.

## Layout

- `baseline/implementation-inputs.json` pins the behavioral oracle; direct and selected native-transitive sources; Rust, the MSVC bootstrapper and installed compiler tree, SDK, CMake, and Ninja; compliance tools; and deterministic ZIP runtime. It records immutable archive or tree checksums and actionable version checks. The future production `Cargo.lock`, native graph, offline Visual Studio layout, and release tool cache must reconcile with these identities as those graphs are created.
- `baseline/patches/` contains reviewed, hash-verified source patches. The ba2 patch disables defaults and exposes zlib as an explicit feature so downstream manifests can comply with the no-default-features policy.
- `baseline/compliance-policy.json` records the GPLv3 combined-work and Slint choice while preserving MPL-2.0 file coverage and notices. It also fixes the standard dynamic MSVC runtime (`/MD`, with Rust's default dynamic CRT) and the Windows 10 22H2 x64 floor. The CRT rule applies to the future Tracetide production graph, not the legacy CMake evidence.
- `build-inputs/` records lock-derived transitive build closures. The Skia source lock is selected by the exact `skia-bindings` checksum, authenticates its complete local Git closure, and pins the GN and LLVM executables needed for the production source-build path. The Skia binary lock pins the ordinary/hosted-validation archive's release tag, exact feature key, URL, size, and SHA-256. The production Cargo graph lock records the Windows x64 GUI/helper closure, activated features, build scripts, and staged-root membership.
- `discrepancies/register.json` is the authoritative discrepancy register.
- `parity/coverage-matrix.json` is the authoritative parity coverage matrix. Its initial blocked state is deliberate.
- `schemas/` contains Draft 2020-12 JSON Schemas for the two baseline records plus discrepancy, matrix, fixture, and evidence records.
- `fixtures/` and `evidence/` receive immutable revisioned records in downstream verification work.
- `tracers/setup/` contains the non-parity engineering fixture and expected public-seam evidence for the W3 setup tracer. The runner copies governed verification inputs into a fresh sandbox, excludes the local oracle kit, validates the exact copied bytes, and only then replays each deterministic or injected-failure case.
- `local-oracle-kit/` is ignored except for its instructions.

## Evolve a contract

Schema and baseline revisions fail closed. A reviewed schema change must increment `schema_version`, advance the validator's supported revision, update affected records, and add a migration/semantic-diff note in the same change. A baseline authority change must likewise advance `baseline_revision`. Fixture and evidence expectation changes create new immutable `id@revision` records; never overwrite an old expectation or silently weaken a comparison profile.

The standard schemas check required fields and shapes. The validator adds the relationships JSON Schema cannot establish: an exact source/feature/license contract, recomputed source and patch hashes, policy constants, status-specific discrepancy requirements, engine freshness, Approved-only versioned discrepancies, recomputed artifact and manifest identities, cross-record revisions, and parity-gate closure.
