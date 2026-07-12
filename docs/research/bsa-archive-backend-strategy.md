# BSA archive backend strategy

Research date: 2026-07-12

## Decision

Adopt [`ba2` 3.0.1](https://crates.io/crates/ba2/3.0.1), the Rust port of Ryan McKenzie's BSA library, as the low-level archive engine. Keep all Cathedral Assets Optimizer behavior above the archive format in first-party Rust: game selection, file classification, splitting and size limits, archive naming, plugin association, dummy-plugin creation, backup and replacement, loose-file exclusion, source deletion, cancellation, logging, and user-facing errors.

This selects the implementation strategy now; the mandatory gates below block the functional-parity milestone and release, not the decision. If the Fallout 4 DX10 corpus exposes a defect that cannot be fixed upstream, use a narrowly maintained Rust fork first, then the mature C++ `bsa` library behind a DX10-scoped FFI adapter. A bundled subprocess is the last contingency.

No fatal capability gap is visible in the published API for the scoped games:

| Required format | `ba2` capability |
| --- | --- |
| Classic Skyrim BSA | TES4-family `Version::v104`; read/write and zlib compression |
| Skyrim Special Edition BSA | `Version::v105`; read/write and LZ4 compression |
| Fallout 4 general BA2 | FO4 `Format::GNRL`, including version 1; read/write and zlib compression |
| Fallout 4 texture BA2 | FO4 `Format::DX10`, including version 1; DDS ingestion, chunking, compression, read, and DDS reconstruction on extraction |
| Later Fallout 4 archive revisions encountered by the scoped legacy behavior | FO4 versions 7 and 8 are represented by the library, but must be proven by fixtures rather than assumed from the enum alone |

The crate is licensed under [0BSD](https://github.com/Ryan-rsm-McKenzie/bsa-rs/blob/v3.0.1/LICENSE), which is compatible with the project's MPL-2.0 distribution. Its native DirectXTex dependency and all other transitive notices still need to be included in the release license audit.

## Why this boundary

`ba2` is a DOM-style archive reader/writer and codec, not a Cathedral Assets Optimizer workflow library. Its public modules expose archive containers, keys, versions, formats, file/chunk compression, and serialization. They do not implement the legacy application's higher-level policy. Recreating that policy in first-party Rust gives the port an explicit, testable compatibility boundary and avoids coupling product behavior to an archive crate's incidental helpers.

The recommended internal boundary should accept normalized CAO archive requests and return inventories, extracted streams/files, or verified archive artifacts. `ba2` types, byte-string conventions, and compression options should not escape that adapter. This isolation also preserves the option to patch, fork, or replace the engine later.

## Capability ownership

| Capability | Owner | Notes |
| --- | --- | --- |
| Parse and serialize BSA/BA2 headers, records, hashes, offsets, and data | `ba2` | Pin version and lockfile checksum. |
| zlib and LZ4 compression/decompression | `ba2` | CAO must select the correct game/version/format options and file/chunk compression state. |
| Fallout 4 DX10 DDS-to-chunks and chunks-to-DDS conversion | `ba2` plus its `directxtex` dependency | Validate semantically; reconstructed DDS headers need not be byte-identical. |
| Game/profile selection | CAO | Do not infer a destructive write format from extension alone. |
| Loose-file eligibility and exclusion lists | CAO | Preserve `filesToNotPack` and profile-specific behavior. |
| Standard, texture, and incompressible classification and merge policy | CAO | Fallout 4 textures select DX10; other files select GNRL. |
| Archive splitting and size-limit enforcement | CAO | Plan conservatively, measure encoded output, and retry or re-split on representational overflow. |
| Archive naming and plugin association | CAO | Preserve legacy game-specific naming rules. |
| Dummy-plugin detection, cleanup, and creation | CAO | Must be characterized against the legacy reference. |
| Backup naming and collision behavior | CAO | Preserve the approved legacy contract or document an approved discrepancy. |
| Atomic write, verification, replacement, rollback, and source deletion | CAO | Never delete sources before durable verification and commit. |
| Extraction path safety and Windows-name collision handling | CAO | Reject traversal, rooted paths, alternate data streams, reserved names, and case-insensitive collisions. |
| Cancellation and progress | CAO | Initial safe boundaries are between archive entries and workflow phases. |

## Alternatives considered

### Ryan McKenzie's C++ `bsa` library through FFI

The Rust crate is a port of the author's [C++ library](https://github.com/Ryan-rsm-McKenzie/bsa). The C++ implementation is described upstream as more mature, so a narrow FFI adapter is the strongest contingency if a proven format defect cannot be fixed promptly in Rust. It is not the first choice because it expands the unsafe ABI surface, complicates Rust ownership and error translation, and does not remove the need for CAO policy code.

### Keep or bind the existing BethUtil/rsm-bsa stack

The current C++ application obtains archive behavior through BethUtil and `rsm-bsa`, as shown by [`BsaOptimizer.cpp`](../../src/BsaOptimizer.cpp) and the [BethUtil vcpkg manifest](../../cmake/ports/bethutil/vcpkg.json). Retaining that stack would reduce behavioral discovery in the short term, but it would preserve a large C++ integration surface and frustrate the migration's preference for a Rust-native backend. It remains a useful differential oracle, not the preferred production boundary.

### BSArch or another bundled subprocess

A subprocess adapter around [BSArch](https://github.com/TES5Edit/TES5Edit/tree/dev/Tools/BSArchive) isolates native crashes and can provide an independent compatibility oracle. It also introduces executable distribution, subprocess lifecycle, output parsing, cancellation, temporary-file, provenance, and license-audit burdens. It is best reserved as a scoped fallback after a failed in-process acceptance gate, rather than made the default architecture.

### `dream_archive`

[`dream_archive` 0.1.6](https://crates.io/crates/dream_archive/0.1.6) is a newer pure-Rust reader/extractor/writer advertising BA2 and BSA support with separable format features. Its GPL-3.0-only license would materially change the combined application's distribution obligations, and its early 0.1 maturity requires an equally strong corpus. It does not currently displace the permissively licensed recommendation, but it is worth monitoring as a technical fallback if licensing policy changes.

### Write the formats from scratch

A first-party implementation would maximize control and eliminate upstream maintenance risk, but it duplicates a subtle binary-format and texture-chunking implementation before product behavior can be ported. It has the highest correctness and schedule risk and should only be considered if every maintained backend fails the required corpus.

## Risks and counterarguments

### Maturity and maintenance

The [`ba2` README](https://github.com/Ryan-rsm-McKenzie/bsa-rs/tree/v3.0.1#readme) says the Rust port is "not nearly as mature" as its C++ cousin, although it reuses the C++ test corpus. Version 3.0.1 was published in December 2024, and the repository has had sparse activity since early 2025. A single small dependency is attractive, but adopting it without a wrapper and fork procedure would transfer an unacceptable maintenance dependency to one upstream maintainer.

### Native DirectXTex dependency

Fallout 4 DX10 support unconditionally pulls [`directxtex` 1.1.0](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/tree/v1.1.0). Its build script compiles vendored C++17 DirectXTex sources into static libraries, so a separate runtime DLL is not expected, but the build requires a functioning C++ compiler and Windows SDK. Upstream [`bsa-rs` issue 14](https://github.com/Ryan-rsm-McKenzie/bsa-rs/issues/14) records build and cross-compilation complaints. The initial product's Windows x64/MSVC-only scope is favorable, but the real release build and clean-machine ZIP must prove this rather than relying on source inspection.

The native dependency also means the archive path is not wholly memory-safe Rust. Native assertions or malformed DDS behavior can still terminate a process. Job-level panic handling cannot catch a native abort.

### DX10 extraction is semantically, not byte, stable

DX10 stores texture metadata and mip chunks rather than the original DDS file byte-for-byte. Extraction reconstructs a DDS header. In [`bsa-rs` issue 3](https://github.com/Ryan-rsm-McKenzie/bsa-rs/issues/3), users observed size-correct but hash-different DDS output, and cubemap handling previously triggered a DirectXTex assertion before a fix. CAO must compare decoded texture metadata, mip inventory, and pixel/block payload semantics. Original DDS hashes are not a valid general oracle.

### Compression configuration is easy to misuse

Archive options do not turn the library into an automatic compression policy. Callers construct each TES4 file or FO4 chunk in compressed or decompressed form and must match archive version, format, codec, and flags. The CAO adapter must expose typed game-specific constructors so an arbitrary combination cannot reach `ba2`.

### Memory, mapping, and cancellation

The DOM model uses memory mapping for path/file reads and allocates buffers for compression and decompression. It is efficient for browsing but is not a streaming transactional service. Mapped inputs must remain stable while in use; concurrent external mutation is unsafe under memory-map contracts. Large or malicious declared sizes can also create allocation pressure. The initial cooperative cancellation contract should promise safe boundaries between entries or phases, not interruption inside native texture conversion or a codec call.

### Malformed archives and extraction safety

The Rust parser checks EOF, integer truncation, and overflow, but the upstream repository does not present a fuzzing claim. Archive counts, offsets, decompressed sizes, and DDS metadata are untrusted input. CAO must impose resource ceilings and ensure a malformed archive cannot leave partially committed output.

Archive keys compare by format hash. A hash collision or duplicate insertion can replace an existing map entry unless the adapter checks insertion results. Extraction also needs canonical containment checks and Windows-aware collision detection before creating any path.

### Size limits and transactionality

The legacy profiles use maximum uncompressed sizes as a splitting input, but binary formats also constrain encoded sizes and offsets. Compression ratios and metadata overhead make prediction imperfect. CAO must treat `IntegralOverflow` as a controlled planning failure and re-split before commit.

`Archive::write` serializes to a caller-provided stream; it does not implement atomic replacement, backup, rollback, or verification. Those behaviors are part of product correctness, not optional hardening.

### Path encoding

The crate intentionally represents archive names as raw byte strings because Creation Engine archives use legacy system-code-page behavior rather than reliable Unicode. The adapter needs a reversible Windows-path-to-archive-byte policy. Blind UTF-8 conversion can change on-disk names and hashes for extended characters.

## Mandatory acceptance gates

All gates below block the functional-parity milestone and release.

### 1. Windows x64 build and packaging

- Pin exactly `ba2 = 3.0.1` and commit the Cargo lockfile.
- Build and test the real release graph with the selected MSVC Rust target and documented Windows SDK/Build Tools version.
- Inspect the produced executable's dependencies and confirm no unshipped DirectXTex, zlib, LZ4, or C++ runtime DLL is unexpectedly required.
- Run the self-contained ZIP on a clean supported Windows x64 VM without Rust, Visual Studio, or the repository installed.
- Run the dependency license/SBOM audit and ship required notices for `ba2`, DirectXTex, DirectX-Headers, DirectXMath, compression libraries, and other transitive crates.

### 2. Golden format corpus

Build a redistributable or locally supplied corpus containing:

- Classic Skyrim v104 BSA: compressed, uncompressed, and mixed per-file compression states.
- Skyrim Special Edition v105 BSA: compressed, uncompressed, and mixed entries using LZ4.
- Fallout 4 v1 GNRL BA2: compressed and uncompressed entries.
- Fallout 4 v1 DX10 BA2: compressed and uncompressed chunks.
- Any v7/v8 archive actually accepted or produced by the scoped legacy release.
- Vanilla archives, legacy CAO-produced archives, and archives produced by an independent trusted tool.
- Zero-length entries, extended-byte names, duplicate/hash-collision attempts, deep paths, and archives near each practical size limit.
- DX10 DDS fixtures covering ordinary 2D textures, cubemaps, arrays if present in scope, full mip chains and truncated mip chains, BC1/2/3/4/5/6H/7, sRGB, and other formats found by inventorying the scoped game fixtures.

For every applicable fixture, read, enumerate, extract, repack, reopen, and extract again. Validate inventory, normalized archive names, decompressed payloads, format metadata, compression state, and game/tool loadability. Require deterministic bytes only for operations designated deterministic by the parity contract.

### 3. Differential compatibility

- Run the same corpus through the authenticated legacy v5.3.15 executable and the current BethUtil/rsm-bsa path.
- Add an independent comparison using a Bethesda or established community archive tool where redistribution and automation permit it.
- Compare ordinary files byte-for-byte after decompression.
- Compare DX10 textures semantically: dimensions, format, array/cubemap flags, mip count, and mip payload or decoded pixels. Record expected header-only differences.
- Any discrepancy must enter the migration discrepancy register and receive maintainer approval before it can cease blocking.

### 4. Negative and resource-safety corpus

- Exercise truncated headers and records, invalid versions and formats, out-of-range offsets, overlapping ranges, excessive counts, integer limits, invalid compressed streams, false decompressed sizes, compression bombs, and malformed DDS metadata.
- Exercise traversal (`..`), absolute/drive/UNC/device paths, alternate data streams, trailing dot/space names, Windows reserved names, case-fold collisions, and archive-hash collisions.
- Add fuzzing around the CAO adapter's parse, inventory, extraction-plan, and write-plan boundaries.
- Require controlled errors with bounded resource use, no process abort, no path escape, no overwritten collision, no source deletion, and no partially committed destination.

### 5. Transaction, backup, and cancellation

- Write to a unique same-directory temporary file, close it, reopen it with `ba2`, verify expected inventory and sampled or complete payloads, and only then commit replacement.
- Test backup collision naming and the exact approved compatibility behavior.
- Test disk-full, permission failure, target-in-use, verification failure, cancellation at every advertised safe boundary, and simulated interruption between transaction phases.
- Prove rollback leaves the original archive and loose sources intact.
- Delete packed loose sources only after durable archive commit; report individual deletion failures without invalidating the verified archive.

### 6. Size and split policy

- Characterize metadata overhead and compressed output across all formats.
- Test immediately below, at, and above each configured and representational limit.
- Prove an overflow or oversized result triggers deterministic re-splitting or a non-destructive error.
- Verify name allocation and plugin association remain valid after re-splitting.

### 7. Maintenance containment

- Place all crate interaction behind CAO-owned archive interfaces and game-specific option constructors.
- Prevent `ba2` types from leaking into orchestration or UI modules.
- Record the exact upstream source, crate checksum, native dependency versions, and local build prerequisites.
- Document how to carry a patched crate or fork and how golden-corpus evidence is rerun for an upgrade.
- Define the fallback trigger: an unresolved corpus failure, native crash, security defect, or inability to reproduce the Windows package blocks release and starts the fork-or-adapter decision.

## Final recommendation

`ba2` 3.0.1 is the selected fit because it covers the required archive families through a small, permissively licensed Rust API while leaving CAO-specific behavior under project control. Classic Skyrim and SSE support appear comparatively straightforward. Fallout 4 DX10 and its native DirectXTex path dominate residual risk.

Proceed with the adapter and validation work, not with unconditional trust in the crate. Completion of the mandatory gates is required parity evidence, not deferred implementation polish.
