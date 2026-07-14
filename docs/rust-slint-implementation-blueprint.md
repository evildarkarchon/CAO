# Tracetide Rust/Slint implementation blueprint

Status: implementation-ready planning baseline

Target: initial Windows x64 pre-release parity milestone

Product: **Tracetide** (`tracetide`)

## 1. Purpose and authority

This document is the build order and acceptance contract for replacing the legacy Cathedral Assets Optimizer with Tracetide. It assembles the decisions from the [Rust/Slint functional-parity migration map](https://github.com/evildarkarchon/CAO/issues/2) without reopening them. When implementation details conflict with a linked decision, the linked decision controls until a reviewed amendment updates both the decision and this blueprint.

The destination is reached when a developer can implement the initial port without making another major product, architecture, backend, compatibility, verification, UI, or distribution choice. This document does not authorize a preview release or relax any parity gate.

The words **must**, **must not**, **required**, and **blocks** are normative. A gate is complete only when its evidence is checked into the repository or linked from a versioned manifest. A passing unit test without the required corpus, package, or clean-machine evidence does not satisfy a gate.

## 2. Fixed release boundary

The initial release is a GUI-only, English-only Windows x64 application distributed as a self-contained ZIP. It supports the built-in Fallout 4 (`FO4`), Skyrim Special Edition (`SSE`), and classic Skyrim (`TES5`) profiles plus custom profiles that the authenticated v5.3.15 release can load. It does not add Oblivion/TES4 support; the v5.3.15 distribution contains no TES4 built-in profile.

Functional parity covers observable decisions and effects rather than Qt layout or pixels. It includes accepted and rejected inputs, profile and option behavior, validation and confirmation points, operation ordering where meaningful, attempted and completed work, filesystem mutations and residue, diagnostics, meaningful progress phases, failure isolation, safe cancellation, and the four run outcomes:

- `Succeeded`
- `Succeeded with errors`
- `Failed`
- `Cancelled`

The shipped application remains sequential. It exposes no CLI or separately shipped headless product, performs no automatic update, uses no installer, and publishes no runnable preview artifacts. Parallel processing, other operating systems, localization, TES4 support, and legacy write-back are outside this blueprint.

## 3. Non-negotiable evidence and discrepancy rules

### 3.1 Behavioral authority

The behavioral oracle is the unmodified Nexus v5.3.15 archive:

- File: `Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z`
- SHA-256: `B25CF0C0C97160B602DD47C252AF2EDE735C27ABE565C9AB84D272306308ABB6`

The v5.3.14 source commit `9969c3f9aad0f9f6f0a3d4489cc573f16486826f` is explanatory only. Observed v5.3.15 behavior wins. Checked-in v5.3.14 profiles and a rebuilt executable must not substitute for the authenticated package.

Oracle campaigns must restore the complete extracted application and input sandbox for every case. The run tuple records the archive hash, VM image, GPU or fallback lane, profile-tree hash, selected profile, complete UI option vector, fixture hash, and process working directory. Retain raw INIs and HTML logs; initial and post-profile UI state; warnings and validation failures; progress/log sequence; screenshots and final state; process tree and helper invocation; cancellation point; normalized before/after filesystem manifests; and the raw evidence behind every normalization. Repeat byte-identity candidates enough to distinguish deterministic output from semantic equivalence.

### 3.2 Tiered output equivalence

Every parity fixture selects an explicit, non-fallback comparison profile:

1. Byte identity for an operation proven repeatably deterministic.
2. Normalized filesystem equivalence for paths, creations, deletions, renames, backups, attributes, and assigned content comparisons.
3. Format-aware structural or decoded equivalence for archives, textures, meshes, animations, and plugins.
4. Normalized interaction evidence for interaction parity: decisions, diagnostic meaning, phases, cancellation boundaries, attempted/completed work, and run outcome.

A failed strict comparison must not weaken itself automatically. Numeric tolerances are per format and fixture, derived from repeated oracle evidence, versioned, and reviewed. Production backends must not be their own sole inspectors.

### 3.3 Discrepancy register

Any observable difference from the oracle blocks parity until a discrepancy-register entry is approved. Each entry records a stable ID; one of `Proposed`, `Approved`, `Rejected`, or `Superseded`; classification and rationale; reproduced oracle evidence; affected games, profiles, options, inputs, operations, outputs, and workflows; compatibility, data-integrity, and migration risk; exact replacement behavior; required automated regression evidence and any separately required manual evidence; and dated maintainer approval.

The following approved directions still require concrete entries and regression evidence:

- faithful dry run with zero persistent writes;
- cooperative cancellation at every safe cancellation boundary;
- truthful four-way run outcomes;
- executable-root path resolution and an isolated portable state tree;
- deterministic missing-setting behavior;
- independent landscape-rule support;
- modern collision-proof UTF-8 run logs;
- portable-state single-writer enforcement;
- safer staged replacement behavior where it differs from the oracle;
- corrected provisional texture-format dialog cancellation.

Unknown, missing, stale, silently skipped, temporarily opt-in, or merely proposed cells block the advertised parity milestone.

## 4. Product and repository identity

The public product identity is:

- Display name: **Tracetide**
- Slug: `tracetide`
- Positioning line: **Trace every change from plan to outcome.**
- Description: **Asset optimization workbench for Fallout 4 and Skyrim.**
- Windows AppUserModelID: `io.github.evildarkarchon.tracetide`

Public UI, documentation, package names, metadata, SBOM names, and artifacts must not use Cathedral Assets Optimizer, `CAO`, `cao`, `AssetsOpt`, or related legacy identifiers except where compatibility or history explicitly requires them. Internal Cargo package names may retain the `cao-*` names fixed below.

Source-controlled branding requires independently reviewed, provenance-documented assets:

```text
assets/branding/tracetide-mark.svg
assets/branding/tracetide-mark-monochrome.svg
assets/branding/tracetide.ico
assets/branding/README.md
```

The canonical mark is the approved single trace with three square nodes. The ICO contains optically corrected 16, 20, 24, 32, 40, 48, 64, and 256 px layers. Theme implementation uses semantic tokens and the approved violet/teal identity; lifecycle and status colors remain separate and never communicate meaning through color alone. Fonts are Windows-installed Segoe UI, Consolas, and Trebuchet MS; no font files are bundled. Recheck the Tracetide name before external registration or release because the preliminary collision screen was not legal clearance.

The vector master is 1024 x 1024. Its canonical 32 px construction uses a 3 px cubic trace and three 4 x 4 px nodes; the 16 px optical variant uses a 2 px trace and three 3 x 3 px nodes. Wordmarks remain live text. Presentation tokens use these exact roles:

| Role | Light | Dark |
|---|---:|---:|
| Shell | `#F6F6FB` | `#10111B` |
| Panel | `#FFFFFF` | `#1A1B2A` |
| Text | `#18192A` | `#F2F1FA` |
| Border | `#CFD2E2` | `#40435D` |
| Accent | `#5A43C0` | `#AC9EFF` |
| Accent soft | `#EAE5FF` | `#2E2857` |
| Secondary | `#087983` | `#4EC9C5` |

## 5. Toolchain, workspace, and dependency graph

Pin Rust `1.97.0` in `rust-toolchain.toml`, use edition 2024 and resolver 3, commit the production `Cargo.lock`, pin every direct production dependency exactly, and set every package to `publish = false`. For every dependency that supports it, set `default-features = false` and list the required features explicitly. Production builds use no network-fetching build scripts.

The workspace has exactly twelve packages:

```text
crates/
  cao-domain
  cao-application
  cao-platform-windows
  cao-backend-bsa
  cao-backend-nif
  cao-backend-texture
  cao-backend-hkx
  cao-hkx-protocol
apps/
  cao-gui
  cao-hkx-helper
tools/
  cao-verification
  cao-oracle-capture
```

The default workspace includes all packages except `cao-oracle-capture`. Release scripts build and stage only `cao-gui` and `cao-hkx-helper`. The production GUI includes all four backend adapters as mandatory dependencies; there is no backend feature matrix, runtime plugin registry, or dynamically loaded adapter.

The enforced dependency direction is:

```text
cao-domain
    ^
cao-application
    ^                     cao-hkx-protocol
    |                         ^         ^
    +-- cao-platform-windows  |         +-- cao-hkx-helper -> pinned serde-hkx
    +-- cao-backend-bsa       +-- cao-backend-hkx
    +-- cao-backend-nif
    +-- cao-backend-texture
                 ^
          composition roots
          +-- cao-gui
          +-- cao-verification
          +-- cao-oracle-capture -> cao-verification
```

No concrete adapter depends on another concrete adapter. Only `cao-gui` depends on Slint. Verification and oracle-capture dependencies must not enter staged production binaries.

Apply `#![forbid(unsafe_code)]` to the domain, application, protocol, BSA, HKX client/helper, GUI, verification, and oracle-capture packages. Only `cao-platform-windows`, `cao-backend-nif`, and `cao-backend-texture` may contain Rust `unsafe`; they use `#![deny(unsafe_op_in_unsafe_fn)]`, isolate unsafe code in private named modules, expose safe wrappers, and document every safety invariant.

CI must reject illegal crate edges, leaf-type leakage, unsafe outside the allowlist, feature-tree drift, unpinned or moving production sources, test/oracle dependencies in staged binaries, and network-fetching build scripts.

## 6. Ownership and application contracts

### 6.1 Layer ownership

- `cao-domain` owns validated profile and configuration values, stable IDs, compatibility-policy enums, immutable run-plan values, validation issues, run outcomes, and backend-neutral domain behavior. It has no UI, filesystem, logging, threading, process, FFI, or backend dependency.
- `cao-application` owns plan construction, the application supervisor, sequential orchestration, runtime work-manifest construction, cancellation checkpoints, typed failure disposition, verification-before-commit, lifecycle reduction, and immutable projections. It defines every inward-facing port and cross-package request/result/error type.
- `cao-platform-windows` implements the executable root, portable state, lock, persistence, filesystem transactions, process containment, run logs, clocks/IDs, and Windows shell effects.
- Format adapters perform one bounded format operation. They do not discover the selected tree, interpret profiles or overlays, choose policy, own the authoritative log, assign failure disposition, or commit/delete real assets.
- `cao-gui` projects immutable snapshots and emits intents. It never invokes a backend or reconstructs lifecycle state from event ordering.
- `cao-hkx-helper` processes one private staged file. It owns no discovery, policy, real asset path, authoritative logging, or commit behavior.

### 6.2 Public UI-independent seam

The only application seam used by both Slint and the parity harness is equivalent to:

```rust
pub trait SnapshotSink: Send + Sync + 'static {
    fn publish(&self, snapshot: Arc<WorkbenchSnapshot>);
}

#[derive(Clone)]
pub struct ApplicationHandle { /* bounded intent sender only */ }

impl ApplicationHandle {
    pub fn submit(
        &self,
        intent: Intent,
    ) -> Result<IntentReceipt, IntentRejection>;
}

pub struct ApplicationRuntime { /* owns orderly supervisor shutdown */ }
```

`Intent` is a closed, application-owned enum covering profile/overlay edits, import and recovery transactions, asynchronous path validation, start, cancellation, terminal acknowledgement, close/exit-after-terminal, history, and log actions. Setup mutations carry a snapshot revision where stale application is unsafe, asynchronous validation carries a request ID, and run actions carry a `RunId`.

`IntentRejection` has stable variants `Busy`, `StaleRevision`, `InvalidLifecycle`, `ValidationBlocked`, and `NotFound`.

Queue acceptance is only a transport receipt. The next `WorkbenchSnapshot` is authoritative. The snapshot is immutable, owned, monotonic-revisioned, bounded, UI-neutral, and backend-neutral. It includes setup, effective plan, capabilities, validation, lifecycle, progress, activity, diagnostics, terminal state, and history projections. Backend types, native handles, Slint types, borrowed paths, and source errors must not cross this seam. The harness gets no test-only executor or reducer entry point.

### 6.3 Ports

`cao-application` owns these capability-level contracts:

- `PortableStateFactory -> PortableState`: executable-root state session, ownership lock, versioned load/validation, atomic configuration/profile/import/migration/recovery transactions, and stable history references.
- `RunEnvironmentFactory -> RunEnvironment`: unique scratch directory and run log plus `RunStore`, `RunLog`, `Clock`, `IdSource`, process capability, and fresh format sessions.
- `RunStore`: bounded inventory/read/write staging, verification support, atomic commit/replace/delete, dry-run write audit, residue reporting, and cleanup. It implements effects but never policy or outcome decisions.
- `OneShotProcess`: absolute helper invocation with explicit working directory, minimal environment, restricted handles, Job Object containment, timeout/cancellation, bounded output, and structured process facts.
- `Clock` and `IdSource`: small deterministic seams.
- `ArchiveSession`, `MeshSession`, `TextureSession`, and `AnimationSession`: synchronous one-operation format ports with factories/probes reporting CAO-owned capabilities and exact engine identities.

Format-shaped payloads preserve bounded operation and cancellation seams. Mesh and texture calls borrow bytes only for the synchronous call and return owned metadata/artifacts. Archive inspection borrows an application-owned seekable reader; extraction handles one member per call; creation uses `append` and `finish`. Animation sees only private staged capabilities.

Composition injects `Send + Sync + 'static` factories. Each factory creates a worker-owned `Box<dyn ... + Send + 'static>` session. Sessions need not be `Sync`, are never shared between runs, and make no unsupported native-backend thread-safety promise. Cross-thread and cross-package values are owned; a borrow lives only for one synchronous call, and native or mapped views are never retained.

Every port returns a CAO-owned `PortFailure` with stable port and operation IDs, an optional affected subject, a bounded diagnostic, and one of:

```text
InvalidInput, Unsupported, Unavailable, NotFound, PermissionDenied,
Conflict, ResourceExhausted, CorruptData, Io, Protocol,
BackendRejected, BackendCrashed, Integrity, Internal
```

Only `cao-application` maps failures to `RecoverableItem`, `FatalSetup`, or `FatalIntegrity`. Raw `io::Error`, HRESULT, native exceptions, helper-library errors, backend enums, `anyhow::Error`, and string-only errors do not cross interfaces. Cancellation is control flow, not an error.

## 7. Threading, messaging, lifecycle, and commits

One long-lived application-supervisor thread owns mutable setup state, the portable-state session and lease, intent ordering, lifecycle reduction, and snapshot publication. Each processing run receives one fresh worker thread owning its immutable `Arc<RunPlan>`, runtime work manifest, scratch/log context, backend sessions, staged artifacts, and active helper child. Only one worker exists, and all run resources are released before another run can be prepared.

Pin `crossbeam-channel` `0.5.16`; do not introduce Tokio.

- UI intent queue: FIFO capacity 64, non-blocking `try_send`, typed `IntentRejection::Busy` on saturation.
- Worker lossless-event queue: capacity 256 for lifecycle, phase, warning, item failure, integrity failure, and terminal events.
- Progress: one replaceable latest-value cell, flushed before a following phase or terminal event.
- Supervisor-to-Slint: one replaceable latest snapshot plus an atomic single-flight scheduled callback. The callback applies only the newest snapshot and safely re-arms if publication races with callback completion.
- Projection bounds: 500 diagnostic rows, 200 recent-activity rows, 100 retained log references, and 4 KiB of valid UTF-8 per displayed diagnostic with truncation metadata and a stable log reference.

Lifecycle and processing phase are distinct:

```text
Idle -> Starting -> Running -> Finalizing -> Terminal
                     |
Starting/Running -> Stopping -> Finalizing -> Terminal(Cancelled)
```

The supervisor freezes a run plan containing the active profile identity/definition, overlay, absolute revalidated asset path, effective choices, dry-run policy, limits, and backend/version identities. The worker derives a runtime work manifest phase by phase because extraction changes later discovery.

Every mutating item follows:

```text
stage -> backend operation -> independent verification -> atomic commit
```

Check cancellation before scheduling work, after every bounded backend/member operation, after verification, and before commit. A commit already in progress is non-interruptible. Once cancellation is accepted, publish `Stopping` immediately and start no new item or phase. Valid committed outputs remain; staged output is discarded; finalization flushes the log and attempts cleanup; no whole-run rollback occurs.

Recoverable item failures continue while integrity remains trustworthy and yield `Succeeded with errors`. Setup failures, missing required backends, log-initialization failure, corrupted orchestration, or unknown output integrity yield `Failed`. A caught worker panic is a typed fatal internal failure. Cleanup failure is diagnostic unless it makes integrity unknown.

Dry run uses the same plan and orchestration path. Its commit adapter reports predicted operations without persistent mutation; verification compares those predictions to a shadow real run.

Event counts, timing, and progress-update frequency are not compatibility contracts. Interaction parity compares meaningful phases, stopping state, diagnostics, attempted/completed work, and outcomes.

## 8. Portable state, profiles, and compatibility

### 8.1 Executable-relative roots

Capture the canonical executable root once at startup. The process current working directory has no contract meaning and must not be searched, changed, or used as fallback.

```text
<executable-root>/
  resources/      immutable profiles, rules, dummy payloads, UI assets
  bin/            tracetide-hkx-helper.exe
  data/
    config/       application configuration and migration backups
    profiles/     custom definitions and per-profile overlays
    imports/      provenance snapshots and import reports
    logs/         retained run logs and durable history summaries
```

Managed paths must remain canonically under their declared roots and reject `..`, symbolic-link, junction, mount-point, and other reparse-point escapes. Asset paths are separate absolute Windows paths, may be remembered while unavailable, and are revalidated before every run. An unwritable `data/` is a startup failure; there is no `%AppData%`, working-directory, or legacy-tree fallback.

Acquire an exclusive state-tree ownership lock with safe stale recovery. Another Tracetide extraction with a different state tree and the legacy application may run concurrently. Durable writes are atomic. Each run uses a unique operating-system temporary scratch directory with success cleanup and best-effort failure cleanup.

Version every fork schema. Apply ordered forward migrations transactionally after a restorable backup, preserve the original on failure, reject newer unsupported schemas, never silently downgrade, and preserve unknown compatible fields where possible.

### 8.2 Profile model

Ship immutable built-in definitions with IDs `FO4`, `SSE`, and `TES5`, sourced from the authenticated v5.3.15 package. A mutable overlay stores each profile's processing choices and remembered paths. Custom profiles use validated versioned manifests with generated stable IDs; display names are editable, case-insensitively unique metadata, not identity or directory names.

Only valid manifests represented under `data/profiles/` are live after import; an arbitrary directory containing `profile.ini` is never discovered. An imported directory name becomes its initial display name. A case-insensitive collision receives the deterministic suffix `Name (imported N)`, is reported, and never overwrites existing state.

SSE is the fresh and fallback default. A missing or invalid active selection visibly falls back to SSE. Failure to load the bundled SSE definition is a fatal installation-integrity error. Switching profiles never leaks settings.

Fresh/reset overlays use the resolved SSE baseline, with unsupported controls masked out:

- single-mod mode, empty asset path, dry-run/debug off;
- archive extraction/creation/backup deletion/content processing off;
- merge incompressible on, merge textures off;
- dummy creation/compression/source deletion on;
- necessary texture processing on; texture compression, mipmaps, fixed-size and ratio resize off;
- texture target 2048 x 2048 and width/height ratios 2 x 2;
- mesh level 0, headpart handling on, mesh resave off;
- animation optimization off.

Missing required custom-definition fields are field-specific errors, not false/zero/empty coercions.

### 8.3 Legacy import

Legacy import is explicit, previewed, non-destructive, and one-shot. It never auto-discovers installations, modifies the source, uses legacy files as live state, synchronizes, overwrites an existing import, or writes back.

Discovery examines immediate child directories containing a regular `profile.ini`, copies regular files without following reparse points, rejects case-insensitive duplicate paths and root escapes, and enforces 64 MiB per file and 256 MiB per profile. Stage and validate first; commit each selected profile independently. Fingerprint imports, skip an unchanged source, and import a changed source as a new custom profile.

Legacy `FO4`, `SSE`, and `TES5` definitions, rules, and dummies never replace built-ins; compatible choices may populate their overlays. Every custom-profile import retains an immutable provenance snapshot and report. Unknown or unsupported content remains inactive and reported.

Qt 5.15.2 INI import must implement the characterized direct-file behavior before typed validation:

- BOM-less bytes decode as Latin-1; BOM-marked bytes decode as UTF-8.
- Decode Qt key percent escapes and value/list escapes.
- Match keys and groups case-insensitively on Windows.
- Last physical duplicate wins.
- Only empty, `0`, and `false` are false through the legacy string-to-boolean path; every other string is true.
- Reject malformed/lossy Qt syntax with raw provenance instead of silent data loss.

`bBsaLeastBSA` is a predecessor, not a live alias. False maps to merge-incompressible true and merge-textures false; true maps to both true. If either replacement key occurs, ignore the predecessor, honor present replacements, use deterministic defaults for absent replacements, and report the conflict. `animationFormat` is obsolete provenance, not an alias.

The recognized mapping inventory is fixed:

- `common.ini`: import `profile` only when it resolves to a successfully imported custom profile or matching built-in; otherwise select SSE and report the fallback. Preserve the source file in provenance. Treat `bShowAdvancedSettings`, `bDarkMode`, `showTutorial`, and `notFirstStart` as provenance-only legacy UI state.
- `profile.ini`: import `bsaEnabled`, `maxBsaUncompressedSize`, `bsaGame`, `meshesEnabled`, `meshesFileVersion`, `meshesStream`, `meshesUser`, `animationsEnabled`, `texturesEnabled`, `texturesFormat`, `texturesConvertTga`, `texturesUnwantedFormats`, and `texturesCompressInterface` from their evidenced sections.
- `settings.ini`: import `bDryRun`, `bDebugLog`, `mode`, `userPath`, and the evidenced archive extraction/creation/backup/merge/content/dummy/compression/source, texture necessary/compress/mipmap/resize/target/ratio, mesh level/headpart/resave, and animation optimization choices. Preserve supported values one-for-one after type/range validation.

Accept renamed or obsolete keys only where oracle/source evidence establishes both meaning and precedence. Report unsupported recognized fields and never guess an alias. Absolute `userPath` values may remain remembered while unavailable; relative or malformed values are rejected.

Fork-owned text is UTF-8. Ordinary rule files prefer strict UTF-8; ambiguous invalid bytes require an explicit encoding choice in the import preview.

Materialize `ignoredMods.txt`, `FilesToNotPack.txt`, `customHeadparts.txt`, and `customLandscape.txt` at import time using custom-first, legacy-SSE-second resolution. There is no hidden runtime fallback. Parsing trims and collapses whitespace, ignores blanks and lines whose first non-whitespace character is `#`, removes duplicates case-insensitively, and omits malformed entries with line-specific warnings. Active matching is case-insensitive: `ignoredMods` exactly matches an immediate mod-directory name; `FilesToNotPack` substring-matches a normalized archive-relative path; `customHeadparts` exactly matches a normalized mesh-relative path; and `customLandscape` exactly matches a normalized texture-relative path with the characterized diffuse/normal counterpart behavior. The normalized landscape rule set is an approved correction because legacy landscape support is inert; its discrepancy tests cover case, slash, whitespace, diffuse/normal counterparts, plugin-derived paths, malformed entries, and duplicates.

Emit or ship the authenticated deterministic 49-byte dummy payload for the selected game and verify its exact hash:

| Game | SHA-256 |
|---|---|
| TES5 | `852F2DB6923C2203D60DAA176D5FA27D61A0D0E717B6819386BBFCBFF7FFFEFD` |
| SSE | `08F228B84E6798D468472D30E74D550F786226A87F4F69F2DBFDDEC576E5799A` |
| FO4 | `AFFCBDEA9D14FE2199440912B326B9F5B704C34F436B51CEF03730319F404CA1` |

Never activate an imported `DummyPlugin.esp`. Whole-workflow naming, collision/counter order, suppression, cleanup, and write-failure behavior remain parity gates.

### 8.4 Logs and recovery

Every run gets a unique UTF-8 log containing timestamp, active profile, effective configuration, significant events, severity, and run outcome. Debug changes verbosity, not log existence. Retain the newest 100 logs subject to a 100 MiB aggregate cap, pruning the oldest completed logs first. Use collision-proof names containing timestamp, profile ID, and unique run ID.

Malformed custom profiles are disabled and reported without mutation. If the active custom profile is disabled, visibly select SSE. Corrupt global configuration offers restoration from the newest valid backup or explicit reset; it is never silently reset. Corrupt built-ins are fatal. Recovery never deletes profiles, provenance, imports, or logs and always records a diagnostic report.

## 9. Backend implementation specifications

All backend pins are exact release inputs. All source archives, checksums, patches, compiler flags, and generated inputs are inventoried in the SBOM and Corresponding Source. Upgrades are corpus-gated dependency changes.

### 9.1 Archives

Use the 0BSD-licensed `ba2` `3.0.1` exactly behind `ArchiveSession`. It owns BSA/BA2 binary mechanics for TES5-family v104 BSA/zlib, SSE v105 BSA/LZ4, and FO4 GNRL/DX10 BA2/zlib. Its FO4 DX10 path statically builds vendored C++ DirectXTex and native LZ4; both native components, pins, features, and licenses enter the Windows build graph, notices, and SBOM. First-party Rust owns classification, path rules, split/size/merge/naming policy, exclusions, dummy lifecycle, backups, `.caobad`, deletion, staging, verification, commit, progress, and cancellation.

Use game/format-specific construction so invalid version/codec/flag combinations are unrepresentable. Extract and create one member per bounded call. FO4 DX10 is the highest-risk gate and compares reconstructed DDS semantics rather than original container bytes.

An unfixable FO4 DX10 corpus defect advances from a narrow Rust fork to a DX10-only FFI adapter over the mature C++ `bsa` library, and only then to a BSArch-style bundled subprocess as the last contingency. Non-DX10 failures, native crashes, security defects, and unreproducible packages remain blockers requiring a targeted fix or an explicit decision amendment; the DX10-only rung must not be misapplied to them.

### 9.2 Meshes

Build `ousnius/nifly` commit `965a1da1be7bff145b7b3435def5c04d6e8c8cce` from source as a static library behind a CAO-owned byte-buffer C ABI. The ABI accepts explicit version, terrain, headpart, parallax, save, and reference-rewrite options and returns owned bytes, stable status, bounded diagnostics, and structured metadata.

No paths, C++/STL types, exceptions, allocators, native objects, or backend enums cross the ABI. Every export catches standard and unknown exceptions. Rust owns routing, policy, dry run, staging, independent reopen/verification, backup, atomic replacement, cancellation, and outcomes. Fallout 4 remains inspection/reference mutation/controlled resave, not a new optimization mode.

Fallback ladder: narrow pinned fork behind the same ABI; then move the same one-shot bridge to a bundled helper if valid or adversarial inputs can still terminate the process. A Rust-native replacement requires the full mutation/conversion corpus.

`nifly` is GPL-3.0-only. The static combined work ships under GPLv3 while preserving MPL notices and source availability. Corresponding Source includes the exact NIF pin, bridge, Rust/Slint source, patches, lockfiles, build and packaging scripts, and notices.

### 9.3 Textures

Build DirectXTex May 2026 commit `4feb3e11a020f35b796fc769a74216a555d4f5ef` from source as a static D3D11-enabled library behind the same CAO-owned byte-buffer ABI pattern. The bridge owns `ScratchImage`, COM/D3D11, GPU state, HRESULTs, native allocation, and exception translation. Rust owns operation policy, Unicode paths, dry run, cancellation at safe cancellation boundaries, staging, independent verification, replacement, rollback, logs, and outcomes.

Preserve separate CPU and GPU BC6H/BC7 lanes. Begin with oracle `DDS_FLAGS_NONE`; any repair flag is a characterized discrepancy. Test metadata and decoded pixels, using bytes only where deterministic. A D3D11 capability failure selects the tested CPU path rather than failing the product.

Keep any older DirectXTex copy transitively built by `ba2` until corpus evidence proves deduplication safe; list both in the SBOM. Reject unmodified `directxtex` 1.3.0 and a `dds`/`image`/`ctt` production stack for the initial port, but retain independent Rust readers where useful as inspectors.

Fallback ladder: narrow pinned fork behind the unchanged ABI; then move the bridge to a bounded helper if native termination cannot be contained. Rust-native replacement requires the full custom-DXGI and BC6H encoding corpus.

### 9.4 Animations

Vendor MIT OR Apache-2.0 `serde-hkx` `1.0.1` at commit `6c1bee56d42de7def991cf6fba025a9df7492d83`. Build only the minimal binary-conversion crates/features. Production is helper-first: `serde-hkx` is absent from the GUI process and present only in `cao-hkx-helper`.

`AnimationSession` performs the bounded header inspection first. An already-AMD64 file returns the `already_amd64` semantic status without launching the helper. For a Win32 input only, create a private helper workspace and atomically write a UTF-8 JSON request. Launch the helper by absolute path with that request path only, a minimal environment, restricted handles, no console, and Job Object containment. Request and result are each limited to 64 KiB; diagnostics are limited to 16 KiB. Protocol v1 requires an exact match and no negotiation.

Stable semantic statuses are:

```text
converted, already_amd64, unsupported_header, unsupported_class,
malformed_input, resource_limit, serialization_failed, output_invalid,
cancelled, timed_out, helper_failed
```

A semantic result uses exit code 0; nonzero is reserved for usage/protocol, request/result I/O, panic, and helper-integrity failures. The result records protocol and engine identities, bounded diagnostic, input/output header summaries, class count, elapsed time, output size, and hash. The helper never sees the real asset path or commits the destination. Rust independently reparses and validates the converted output, stages it in a newly created sibling file, and performs verified `ReplaceFileW` replacement; every failure path preserves the original bytes.

Never redistribute `hkxcmd.exe`, Havok 2010 material, or Bethesda's postprocessor without exact written rights. They are oracle/external tools only. The fallback is a narrow vendored `serde-hkx` fork and tighter helper containment; broader unsupported class/layout coverage is added using local oracle evidence or handled only through an approved discrepancy.

### 9.5 Shared native ABI gates

The NIF and texture bridges use versioned fixed-width POD records with `abi_version`, `struct_size`, and Rust/C++ layout assertions. Result records are caller-owned and zero-initialized. Input pointers are checked byte slices valid only for the call. Successful output has one paired idempotent free that clears the record; failures leave output empty and may fill bounded UTF-8 diagnostics.

Mandatory conformance includes partial results, integer overflow, allocation failure, null/empty semantics, double-free defense, exception translation, invalid UTF-8 diagnostics, and Rust panic/unwind containment. Sanitizers, malformed-input campaigns, and process-termination gates run before parity closure.

## 10. Slint Workbench specification

Pin Slint `1.17.1`, disable default features, and enable only:

```text
std
compat-1-2
accessibility
backend-winit
renderer-skia
renderer-software
```

Do not enable Qt, FemtoVG, WGPU/unstable features, gettext, system tray, live preview, MCP, system testing, or broad image formats in the production package. Both normal Skia and forced `winit-software` clean-VM lanes are required.

### 10.1 Component tree and state ownership

```text
AppWindow
└─ WorkbenchShell
   ├─ PrimaryNavigation
   ├─ Workspace
   │  ├─ WorkspaceHeader
   │  │  ├─ ActiveProfileControl
   │  │  └─ AssetPathControl
   │  └─ PageRouter
   │     ├─ Overview
   │     ├─ Archives
   │     ├─ Textures
   │     ├─ Meshes
   │     ├─ Animations
   │     ├─ Profile Definition
   │     └─ Run History
   ├─ RunInspector
   │  ├─ EffectivePlanPanel
   │  ├─ ValidationSummary
   │  ├─ RunProgressPanel
   │  ├─ DiagnosticSummary
   │  └─ TerminalOutcomePanel
   └─ OverlayHost
      ├─ ConfirmationDialog
      ├─ ProfileWorkflowDialog
      └─ FormatSelectionDialog
```

All seven pages stay instantiated to preserve scroll and disclosure state. Category pages reuse private `SettingsSection`, `SettingField`, `ValidationMessage`, and `CapabilityUnavailable` components. Rust owns the authoritative `WorkbenchSnapshot`, raw text/numeric drafts, validation, persistence, and lifecycle. Slint owns only focus, hover, scrolling, disclosure, selected destination, and transient popovers.

Public two-way properties are forbidden. Leaf controls may hold private editing state, but accepted edits emit typed intents and are confirmed by the next Rust projection. Slint globals are presentation-only instance services, never application state or ordered event buses. `ModelRc` is main-thread-only; the GUI adapter holds a weak component handle and dispatches snapshot/model replacement onto the Slint event-loop thread.

### 10.2 Workbench behavior

The inspector shows exactly one of Setup, Active, Stopping/Finalizing, or Terminal. From Starting through Finalizing, profile, target, mode, and category controls are read-only; navigation and history remain inspectable. Terminal permits edits, but another run requires explicit **Prepare next run** acknowledgement.

Navigation starts at Overview and never commits or discards configuration. Returning from Run History restores the prior configuration destination. Unsupported destinations and settings stay visible and disabled with visible and accessible reasons; if the selected destination becomes unsupported, it remains selected and shows its capability explanation. Overview is read-only, links to the owning categories, and has no duplicate editors. Operation pages use fixed Processing, Output policy, Format and compatibility, and Advanced sections. Activating an inspector error navigates to its page, expands the owning section, focuses the control, and announces the message.

There is no global Apply or Save. Valid toggles/selectors update the active overlay immediately. Text/numeric drafts commit on Enter or focus loss when valid. Persistence is transactionally debounced and forcibly flushed before profile changes, Start, and normal exit. Invalid drafts or persistence failure remain visible and block Start.

The profile workflow provides searchable built-in/custom selection; New, Import, Reset choices, Rename, and Delete actions; staged legacy-import preview and independent per-profile results; and built-in read-only/custom editable definitions. Deleting the active custom profile visibly selects SSE.

The persistent target card provides absolute path editing, Browse, profile-scoped recent paths, single-directory drag/drop, Single mod/Several mods, and Dry run. Path states are Empty, Checking, Valid, Unavailable, and Invalid. Validation is asynchronous and request IDs discard stale results. Remembered unavailable paths remain visible but block Start. Mode changes recompute the effective plan without erasing saved overlay choices; constrained controls show saved and effective values plus the reason. Dry run changes write policy without clearing archive or backup choices. The frozen target, mode, and dry-run state remain visible and read-only during a run.

Start flushes edits, revalidates the target and required backends, and constructs a run plan against the current revision. It cannot enqueue duplicates. Destructive real runs receive one final summary confirmation; dry and non-destructive runs start directly. Cancel is one activation with no secondary confirmation.

Closing in Setup or Terminal flushes valid edits and exits; invalid drafts offer Stay or Discard. Terminal exit does not require terminal acknowledgement because the result is durable. Closing in Starting or Running offers Keep Running or Stop and Exit. Stop and Exit records exit-after-terminal, requests cancellation once, and keeps the Workbench visible until terminal publication and resource release. Closing in Stopping or Finalizing does not prompt again; it records exit-after-terminal and focuses the inspector. There is no ordinary force-exit path.

Run History is an incremental newest-first master/detail view with outcome/profile filters, frozen-plan and terminal summaries, and Open log/Reveal/Copy path actions. It has no Rerun action. The active run may appear first but is not terminal history.

Dialogs are single-instance in-window overlays with typed Rust-backed drafts, focus restoration, safe Escape handling, and no prompt stacking. Texture-format changes remain provisional until Confirm.

### 10.3 Accessibility and adaptive contract

Use navigation, exactly one main landmark, and a complementary inspector. Navigation exposes tab-list/tab/tab-panel semantics. Every custom interactive component declares a constant role, accessible label/description, enabled state, and actions. Unsupported reasons and validation are both visible and accessible.

Phase announcements are polite; Stopping, integrity failure, and terminal outcome are assertive. Progress exposes numeric min/current/max but does not announce ordinary ticks, item paths, or every diagnostic. Status never relies on color.

Keyboard behavior includes navigation arrows/Home/End, Enter/Space activation, Ctrl+1 through Ctrl+7 destinations, F6 region cycling, reversible Tab order, safe Escape behavior, focus restoration, and Alt+F4 through cooperative shutdown. There is no global Enter-to-run shortcut, and Escape never cancels a run.

Preferred size is 1280 x 800 logical pixels; minimum is 900 x 600. Preserve the three-region shell at all supported sizes. At 1120 px and wider, navigation is about 200 px and inspector 340-380 px; at 900-1119 px, navigation is about 168 px and inspector 300 px with vertical setting reflow. Regions scroll independently and the window has no whole-surface horizontal scrollbar.

Test both width bands, minimum size, common DPI levels, 200% text, long English paths/labels, System/Light/Dark, Windows High Contrast, reduced motion, color-vision simulations, keyboard-only operation, accessible names/roles/actions, and a Windows screen-reader smoke lane.

Use Slint Fluent as the Windows control baseline. Theme preference is System, Light, or Dark and defaults to System; it is fork-owned and never imported from legacy state. Compact desktop density uses a 4 px unit. The full Tracetide name and mark sit above navigation; selected navigation uses a violet capsule without implying a sequence. Trace nodes connect only actual processing phases in the inspector. The mark must not become a chart, ocean illustration, game emblem, cathedral/castle, gear, flame, or legacy-icon derivative.

## 11. Verification architecture and coverage

`cao-verification` is a manifest-driven Rust harness that drives the exact `ApplicationHandle`/`SnapshotSink` seam used by Slint. It has no source, build, link, or runtime dependency on the legacy C++ repository. It uses fresh copied sandboxes, deterministic and fault adapters, independent inspectors, and one evidence schema for replacement replay and oracle capture. It is never shipped.

`cao-oracle-capture` may invoke the authenticated legacy executable only as an opaque external process in a restored sandbox. It imports no legacy headers, libraries, types, or copied algorithms. Removing the legacy source repository or the capture tool must not affect replay of existing evidence.

Manual game or visual validation is not a parity requirement. If automated evidence cannot establish a required invariant, the parity cell remains blocked rather than passing through manual observation.

The parity fixture corpus has two layers:

1. A committed redistributable corpus of compact fixtures, deterministic generators, manifests, comparison profiles, and reviewed hash-addressed oracle evidence. Its complete replay runs on every Windows x64 pull request.
2. An untracked local oracle kit containing the authenticated v5.3.15 package and licensed/private inputs. The repository stores hashes and acquisition/generation instructions, never restricted payloads.

Each fixture manifest records a stable ID/revision, covered parity matrix cells, input hashes, origin/license class, deterministic recipe or acquisition instructions, required tool/backend versions, explicit comparison profile, oracle-evidence identity, discrepancy links, and any temporary opt-in reason/removal condition. Expectation changes create new immutable revisions with semantic diffs; never silently rebaseline.

Coverage is risk-weighted rather than a Cartesian product, but must include:

- an all-enabled end-to-end baseline for FO4, SSE, TES5, and representative custom profiles;
- boundary, malformed, no-op, and per-item failure cases for every category;
- pairwise option coverage within categories and across order-sensitive operations;
- exhaustive real/dry, backup/delete, extract/repack, collision, `.caobad`, commit-failure, and safe-cancellation cases;
- single/several-mod layouts, exclusions, Unicode/case/long paths, read-only/locked files, partial success, empty-directory cleanup, unsupported inputs, and all outcomes;
- all BSA/BA2 families and codecs; compressed, uncompressed, mixed, empty, and near-limit/split-boundary archives; cubemaps and relevant BC formats; DX10 chunks; extended-byte names; malformed offsets, sizes, records, and compression; traversal and Windows path/case collisions; disk, permission, backup-collision, staging, and every advertised cancellation-boundary failure;
- legacy and DX10 DDS headers; BC1 through BC7 including signed and sRGB variants; RGBA, uncompressed, packed, and scoped custom formats plus TGA; typeless mappings; arrays, cubemaps, volumes, alpha, normals, interface assets, mip topology, malformed/resource cases, and separate CPU/GPU BC6H/BC7 visual-threshold evidence;
- NIF target versions 83/100/130; ordinary/headpart/facegen/BTR/BTO and FO4 routes; version triples, block graphs/types/order, geometry/topology, normals/tangents/UV/colors, skinning, collision, shader flags, texture slots, animation references, terrain names/paths, and bounds; named terrain, UTF-16 path, parallax, nonzero-root/corrupt, invisible/tree-culling/tight-bounds, unknown-block, level-2-resave-only, and FO4-mismatch regressions; and an oracle comparison of legacy versus safer SSE save defaults, with approved discrepancies for contract-visible bounds, ordering, or unreferenced-block changes;
- classic, already-AMD64, and malformed HKX; animation, skeleton, Behavior, ragdoll/physics, Bethesda classes, and FNIS/Nemesis/mod fixtures; helper unavailable/crash/hang/cancel/resource failures; semantic graph checks; reproduction of upstream's full SSE `Animation.bsa` campaign; and a corresponding LE archive parse-to-AMD64-serialize-to-reparse campaign with zero unexplained crashes, hangs, semantic differences, or legacy-success inputs reported unsupported;
- dummy plugins, masters, HDPT/MODL and landscape records, archive-derived naming, collision, suppression, cleanup, and malformed plugins.

TES5 plugin-dependent scenarios may be temporarily opt-in only with a named removal condition. They must be required and passing before advertised TES5 parity.

Every run comparison includes a complete before/after filesystem manifest. Timestamps may be normalized only when repeated oracle evidence proves them unstable and contract-irrelevant. Normalize logs by severity, category, affected item, operation, and diagnostic meaning while stripping timestamps, source locations, generated IDs, and wording. Compare progress by meaningful phase transitions, monotonic completed/total work, stopping state, and run outcome rather than timing or event count.

Fault injection uses filesystem, backend, process, clock/ID, and logging ports keyed by operation and fixture path, not wall time. It covers read/open, staged write, flush, verification, rename/replace, delete, backup collision, permission/lock/disk exhaustion, cleanup, backend rejection or invalid output, panic translation, unavailable backends, helper failure or termination, and run-log initialization or write failure. Each case asserts diagnostic category, attempted/completed work, exact residue, continuation, and outcome. Safe cancellation boundary tests hold adapter barriers, submit the ordinary cancellation intent, release the barrier, and assert public snapshots and filesystem/log evidence.

Every port has a reusable conformance suite for production and deterministic/fault adapters. Native ABI and helper-protocol suites operate below the application seam. Controlled differential recapture is required when fixtures, comparison profiles, production backends, inspectors, or discrepancy expectations change and before parity closure.

Execution tiers are:

- Every pull request: schema/integrity checks, workspace enforcement, package/unit/conformance tests, and complete committed Windows x64 replay.
- Scheduled/on demand: determinism, fuzz/mutation seeds, resource limits, sanitizer/native safety, fault injection, and expensive independent inspection.
- Controlled oracle capture: human-triggered restored-sandbox differential campaigns.
- Parity/release gate: all required extended coverage and current evidence pass against the exact staged build.

## 12. Packaging and compliance contract

Release version `X.Y.Z` produces:

```text
tracetide-vX.Y.Z-windows-x64.zip
tracetide-vX.Y.Z-source.zip
tracetide-vX.Y.Z-symbols.zip
tracetide-vX.Y.Z.cdx.json
SHA256SUMS.txt
```

The binary ZIP has one root, `tracetide-vX.Y.Z-windows-x64/`, containing `tracetide.exe`, `bin/tracetide-hkx-helper.exe`, immutable resources, README, complete GPL/MPL/third-party notices, SBOM, and `release.json`. First run creates `data/`.

Do not ship mutable state, PDBs, Qt assets/translations, the legacy icon, Havok tools, behavioral oracle, local oracle kit, development tools, caches, repository tests, or pre-populated logs/configuration. PDBs go only in the symbols ZIP.

The compatibility floor is Windows 10 22H2 x64, disclosed as out of support; Windows 11 x64 is primary. Target ordinary x86-64 without AVX/AVX2 requirements. Use the standard dynamic MSVC runtime consistently (`/MD` for native code and Rust's default dynamic CRT), and make the supported Microsoft Visual C++ Redistributable an explicit prerequisite. The GUI uses the Windows subsystem; the helper has no visible console. Embed `asInvoker`, `uiAccess=false`, supported-Windows and long-path manifests, approved product/version metadata, and the reviewed icon. Enable the UTF-8 active code page only after the parity corpus proves legacy profile/path and backend behavior in that manifest mode.

Initial binaries are unsigned. Release metadata states that policy. SHA-256 checksums, GitHub provenance attestations, and SBOM attestations authenticate every artifact.

Both executables derive PE `FileVersion` as `X.Y.Z.0` and `ProductVersion` as `X.Y.Z`. Their fixed string metadata is:

| Field | GUI | HKX helper |
|---|---|---|
| `CompanyName` | `evildarkarchon` | `evildarkarchon` |
| `ProductName` | `Tracetide` | `Tracetide` |
| `FileDescription` | `Tracetide asset optimization workbench` | `Tracetide HKX processing helper` |
| `InternalName` | `tracetide` | `tracetide-hkx-helper` |
| `OriginalFilename` | `tracetide.exe` | `tracetide-hkx-helper.exe` |
| `LegalCopyright` | `Copyright © 2019 G'k; © 2026 evildarkarchon and contributors` | `Copyright © 2026 evildarkarchon and contributors` |
| `Comments` | `GPLv3 licensed; see bundled notices.` | `Bundled Tracetide component; see bundled notices.` |

The About surface separately preserves G'k/MPL and dependency attribution without presenting G'k as the fork publisher.

Acquire exact inputs separately, then perform a locked offline build with pinned Rust, MSVC Build Tools, Windows SDK, CMake, Ninja, audit, SBOM, notice, and ZIP tooling. Disable incremental builds, remap source/toolchain paths, fix locale/timezone, avoid volatile build dates, and derive versions from the immutable tag and commit.

Run two clean builds in equivalent fresh workspaces. Compare dependency graphs, staged inventories, PE imports/resources, notices, SBOMs, native archives, and unsigned hashes. The ZIP packer sorts ordinal paths, uses `/`, rejects absolute and parent paths, fixes timestamps and permissions, omits NTFS/host metadata, uses pinned compression, and writes UTF-8 names.

Recursively audit normal and delayed PE imports. Only allow-listed Windows 10 system libraries or inventoried app-local components may remain. Audit both Skia and forced software-renderer closures and every native backend/helper path.

Run pinned `cargo-deny`, `cargo-about`, and CycloneDX generation against the exact staged features. Fail closed on unknown licenses, moving sources, wildcard direct dependencies, unreviewed expressions, missing texts, or native/assets absent from reconciliation. Choose Slint's GPLv3 licensing option. Publish exact Corresponding Source for the GPLv3 combined work beside the binary and retain it for as long as that binary remains offered. It includes MPL-covered source/notices, vendored/retrievable crate and native sources, bridges, manifests, lockfiles, code-generation inputs, resources, patches, and build/package scripts.

After installing and verifying the declared Microsoft Visual C++ Redistributable prerequisite, the exact staged ZIP must launch through Explorer and PowerShell from unrelated, space-containing, Unicode, and near-long paths on clean Windows 10/11 VMs with no repository, toolchain, network, or prior application state. Exercise every backend, the helper, CPU texture fallback, a representative run, safe cancellation, relaunch, and delete-directory uninstall.

Publication is atomic across binary, source, symbols, SBOM, checksums, attestations, and notes. Never replace bytes under an existing version and never publish a runnable ZIP from ordinary CI or a pull request.

## 13. Dependency-ordered implementation plan

The work packages below are the implementation sequence. A package may start only when all dependencies are complete. Packages at the same layer may proceed in parallel, but their exit gates remain independent.

| ID | Work package | Depends on | Required exit evidence |
|---|---|---|---|
| W0 | Freeze sources, evidence, and compliance policy | None | Oracle hash check; pin/checksum manifest; discrepancy register schema; parity matrix schema; GPLv3/Slint/dynamic-CRT policy; support-floor record |
| W1 | Bootstrap the twelve-package workspace and CI enforcement | W0 | Rust 1.97/edition 2024/resolver 3 workspace; lockfile; exact dependencies; DAG/unsafe/feature/source checks; offline build skeleton |
| W2 | Implement domain values and application seam | W1 | Typed profiles/overlays/configuration/run plan/work manifest/outcomes/errors; `Intent`, `ApplicationHandle`, `SnapshotSink`, bounded immutable snapshot; domain/application unit and leakage tests |
| W3 | Build verification foundation and deterministic adapters | W2 | Manifest/evidence schemas; fresh sandbox runner; deterministic/fault adapters; conformance-suite framework; first application-seam tracer fixture in every-PR CI |
| W4 | Implement Windows portable-state and effect capabilities | W2, W3 | Executable-root containment; state lease; atomic persistence/migrations/recovery; run store/log/process/clock/ID ports; traversal/reparse/fault conformance |
| W5 | Implement profile compatibility and import | W4 | Authenticated built-ins; overlays/defaults; Qt decoding; preview/provenance; transactional import; rule materialization; dummy hashes; compatibility fixtures and discrepancy entries |
| W6A | Implement archive adapter | W2, W3, W4 | `ba2` pin/build; archive conformance; format/differential/negative/resource/cancellation/transaction gates or activated fallback |
| W6B | Implement NIF adapter and ABI | W2, W3, W4 | Exact nifly pin; ABI/ownership/exception/sanitizer gates; mesh corpus and semantic validation; source/license inventory or activated fallback |
| W6C | Implement texture adapter and ABI | W2, W3, W4 | Exact DirectXTex pin; CPU/GPU and ABI gates; texture corpus/quality/repair/native-safety/package evidence or activated fallback |
| W6D | Implement HKX protocol, client, and helper | W1, W2, W3, W4 | Exact serde-hkx pin; protocol/Job Object/resource/fuzz/semantic/transaction gates; no prohibited Havok payload; original preserved on every failure |
| W7A | Implement supervisor and sequential orchestration against deterministic/fault adapters | W3, W4, W5 | Lifecycle/messaging bounds; phase discovery; stage-verify-commit; dry-run audit; failure isolation; log/history; every safe cancellation boundary; four outcomes through public seam |
| W7B | Integrate all production backends into orchestration | W6A, W6B, W6C, W6D, W7A | Production capability probes/sessions; exact engine identities; mixed-category work manifest; verified commits and failure dispositions; end-to-end backend tracer evidence |
| W8A | Implement the Slint Workbench | W2, W3, W4, W5 | Component tree, intent/projection adapter, setup/profile/target pages, inspector, history, dialogs, lifecycle behavior, no backend calls, UI contract tests |
| W8B | Produce final branding assets and theme tokens | W0 | Reviewed SVG/monochrome/ICO/provenance hashes; exact Tracetide naming/metadata/token mapping; High Contrast assets; collision recheck scheduled |
| W9 | Integrate Workbench, orchestration, and branding | W7B, W8A, W8B | Real application composition; Skia/software lanes; all setup/run/close paths; stable logs; exact public identity; no legacy production assets/dependencies |
| W10 | Close the parity coverage matrix | W3, W6A, W6B, W6C, W6D, W7B, W9 | All required corpus and controlled differential campaigns; independent inspection; approved discrepancy regressions; no unknown/stale/skip/temp-opt-in cell |
| W11 | Qualify packaging and Corresponding Source | W1, W9, W10 | Exact staged binaries; notices/SBOM/source; PE/runtime audits; deterministic second build; clean Win10/11 VM tests; checksums/attestations |
| W12 | Admit the first pre-release build | W11 | Atomic draft artifact set reviewed; every parity/release gate green; no preview channel publication; retained evidence bundle |

### 13.1 Critical path and parallel lanes

The critical path is:

```text
W0 -> W1 -> W2 -> W3 -> W4 -> W5 -> W7A -> W7B -> W9 -> W10 -> W11 -> W12
```

After W4, W5 and all four backend packages can advance in parallel. W7A starts as soon as W5 completes and constrains the production adapters with tested orchestration, cancellation, failure, and commit semantics. W8A uses W3 deterministic adapters after the stable seam and portable/profile behavior exist while backend work continues. W7B is the backend join. W8B is independent after W0 and must finish before W9. W10 waits for every lane because parity is a whole-product property.

### 13.2 Required tracer bullets

Each package lands through the smallest vertical evidence path that proves its seam before breadth is added:

1. W2/W3: submit one setup intent, persist it in a deterministic adapter, and observe the confirmed bounded snapshot through the public seam.
2. W4/W5: open a fresh executable-relative state tree, load SSE, import one Qt-encoded custom profile transactionally, and reproduce it after restart.
3. W6 lanes: inspect/transform one redistributable fixture, independently verify the owned staged artifact, and prove cancellation/failure leaves the original intact.
4. W7A: process a mixed tiny tree with deterministic/fault adapters through discovery, verification, commit, log, terminal outcome, and replay evidence; repeat as dry run, recoverable failure, fatal failure, and cancelled run.
5. W7B: replace each deterministic format adapter with its production counterpart, then run one mixed-category tracer without changing the public application seam.
6. W8/W9: drive the same tracer through Slint intents and snapshots, including invalid setup, destructive confirmation, active read-only state, cancellation, terminal acknowledgement, log opening, and cooperative close.
7. W11: build the exact tracer release offline, stage it, run it on a clean VM with both render lanes, and compare the second clean build.

Tracer bullets do not waive breadth gates; they expose interface mistakes before corpus expansion.

## 14. Package completion checklists

### 14.1 No backend package is complete until

- its exact source/version/checksum and enabled features are recorded;
- its leaf types cannot cross the application port;
- application policy, discovery, logging, commit, and outcome ownership remain outside it;
- conformance, malformed, resource, cancellation, transaction, and package tests pass;
- an independent inspector or combined invariant strategy validates output;
- fallback triggers are tested and unresolved trigger conditions block downstream work;
- license, notice, SBOM, and Corresponding Source inputs are complete.

### 14.2 No Workbench package is complete until

- every callback emits a typed intent and no callback invokes a backend;
- stale revisions, stale path-validation requests, busy queues, and lifecycle rejection are visible and tested;
- all four outcomes, every lifecycle surface, safe cancellation, persistence failure, and terminal acknowledgement are projected;
- keyboard, focus, modal, validation navigation, accessible semantics, size, DPI, theme, High Contrast, reduced-motion, and screen-reader gates pass;
- Skia and forced software rendering pass on clean machines;
- public naming and theme roles match the Tracetide decision.

### 14.3 No parity package is complete until

- the fixture manifest and evidence hashes are valid and current;
- the comparison profile is explicit and never weakens on failure;
- production output is independently inspected;
- before/after filesystem state, diagnostics, progress phases, attempted/completed work, failure residue, and outcome are asserted where applicable;
- every difference is either eliminated or linked to an approved discrepancy with regression evidence;
- all advertised FO4, SSE, TES5, and supported custom-profile cells are passing or evidence-backed Not Applicable.

### 14.4 No release package is complete until

- the exact staged feature/dependency/native graph is audited;
- all runtime imports are allow-listed or inventoried;
- binary, source, symbols, SBOM, checksums, attestations, and notes agree on version and hashes;
- GPLv3 Corresponding Source and MPL/third-party notices are complete;
- two clean builds and clean Windows 10/11 VM tests pass;
- no restricted oracle input, Havok binary, PDB, mutable state, development artifact, Qt asset, translation, or legacy branding is present;
- the full artifact set can be published atomically without altering an existing version.

## 15. Decision traceability

This blueprint incorporates every closed map decision:

- [Establish a reproducible legacy reference baseline](https://github.com/evildarkarchon/CAO/issues/3)
- [Define the functional-parity and discrepancy contract](https://github.com/evildarkarchon/CAO/issues/4)
- [Choose the BSA archive backend strategy](https://github.com/evildarkarchon/CAO/issues/5)
- [Choose the NIF mesh backend strategy](https://github.com/evildarkarchon/CAO/issues/6)
- [Choose the texture backend strategy](https://github.com/evildarkarchon/CAO/issues/7)
- [Choose the HKX animation backend strategy](https://github.com/evildarkarchon/CAO/issues/8)
- [Define the portable profile and configuration compatibility contract](https://github.com/evildarkarchon/CAO/issues/9)
- [Prototype the Slint interaction contract](https://github.com/evildarkarchon/CAO/issues/10)
- [Choose the Rust application architecture and job lifecycle](https://github.com/evildarkarchon/CAO/issues/11)
- [Define the parity verification strategy and fixture corpus](https://github.com/evildarkarchon/CAO/issues/12)
- [Define the Windows ZIP packaging and dependency compliance plan](https://github.com/evildarkarchon/CAO/issues/13)
- [Characterize legacy profile encoding, landscape rules, and dummy plugins](https://github.com/evildarkarchon/CAO/issues/15)
- [Define the implementation-level Slint Workbench structure](https://github.com/evildarkarchon/CAO/issues/16)
- [Define the Rust workspace graph and internal interface contracts](https://github.com/evildarkarchon/CAO/issues/17)
- [Choose the fork product name and visual identity](https://github.com/evildarkarchon/CAO/issues/18)

No major product or technical choice is intentionally left open. The remaining choices are local implementation details constrained by the interfaces and gates above. If implementation evidence invalidates a backend, ABI, or behavior assumption, stop the affected lane, apply the documented fallback ladder or discrepancy process, update the responsible decision, and then update this blueprint before continuing downstream.
