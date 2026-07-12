# Texture backend strategy

Research date: 2026-07-12

## Decision

Use Microsoft's [DirectXTex May 2026 release](https://github.com/microsoft/DirectXTex/releases/tag/may2026), pinned to commit [`4feb3e11a020f35b796fc769a74216a555d4f5ef`](https://github.com/microsoft/DirectXTex/tree/4feb3e11a020f35b796fc769a74216a555d4f5ef), as the texture engine. Compile it from source as a static library with its Direct3D 11 support enabled, and expose only a small CAO-owned C ABI that accepts and returns byte buffers, fixed-width options, metadata, and structured errors.

Keep profile policy, operation planning, dry-run behavior, cancellation, path handling, temporary files, verification, replacement, logging, and user-facing error classification in Rust. The native adapter should own DirectXTex's `ScratchImage`, optional D3D11 device, and all HRESULT translation. No DirectXTex, COM, D3D, C++ standard-library, or allocator-owned type should cross the ABI.

This intentionally selects a narrow native adapter over a Rust-native stack. The legacy optimizer already uses DirectXTex for every material texture operation, including the D3D11 GPU overload for BC6H/BC7 compression ([current implementation](../../src/TexturesOptimizer.cpp)). DirectXTex's public API directly supplies DDS and TGA loading, DDS saving, conversion, resize, mip generation, CPU and D3D11 GPU BC compression, decompression, DXGI format helpers, and texture metadata ([May 2026 public header](https://github.com/microsoft/DirectXTex/blob/may2026/DirectXTex/DirectXTex.h)). The latest release is MIT-licensed, actively maintained, and locally built successfully for Windows x64.

The strongest pure-Rust candidate, [`dds` 0.2.0](https://crates.io/crates/dds/0.2.0), is worth monitoring and using as an independent decoder in tests. It does not displace DirectXTex for this parity milestone because its official format matrix marks BC6H encoding unsupported, while CAO can decompress, modify, and then recompress an existing BC6H texture to its original format ([`dds` matrix](https://github.com/image-rs/image-dds/blob/v0.2.0/supported-formats.md), [CAO pipeline](../../src/TexturesOptimizer.cpp)). Combining `dds`, `image`, and a separate native compressor would also split header repair, metadata, resize, mip, error, and lossy-codec semantics across multiple engines without closing the full custom-profile DXGI compatibility question.

## What the released behavior requires

CAO's texture path is not just a DDS reader and BC encoder. The current implementation:

- loads DDS and TGA from paths or memory;
- converts typeless DDS formats to a concrete UNORM format when possible and rejects them otherwise;
- identifies incompatible textures from profile-defined DXGI format lists, compressed non-power-of-two dimensions, and a cubemap/alpha rule;
- optionally converts TGA, repairs incompatible textures, compresses uncompressed textures, resizes down by powers of two, and regenerates complete mip chains;
- decompresses a compressed source before resize or mip work and normally recompresses it to the original BC format unless policy selects another target;
- supports profile-selected BC1, BC3, BC5, BC7, and uncompressed BGRA output in the GUI while custom profiles can name the much broader `DXGI_FORMAT` set represented by [`texturesformats.h`](../../src/texturesformats.h);
- uses `TEX_FILTER_SEPARATE_ALPHA`, forces the non-WIC resize path, and requests BC7 three-subset mode;
- tries D3D11 DirectCompute for BC6H/BC7 and falls back to the CPU codec when no suitable adapter is available; and
- writes DDS with DirectXTex's default header policy and preserves arrays, cubemaps, depth, mip count, alpha mode, resource dimension, and other `TexMetadata` fields through transformations.

The same source also contains two source-level defect candidates that the port must not classify silently. `dryOptimize` does not use the same resize calculation as the real operation, and `compareInfo` joins field comparisons with logical OR rather than AND. The behavioral oracle must confirm each discrepancy before it can enter the discrepancy register as a proposed correction and receive characterization tests and maintainer approval.

## Why the selected DirectXTex pin fits

### Complete operation and format surface

The selected header exposes `LoadFromDDSMemory/File`, `LoadFromTGAMemory/File`, `SaveToDDSMemory/File`, `Resize`, `Convert`, `GenerateMipMaps`, `Compress`, and `Decompress`. It also exposes DXGI predicates and transforms such as `IsCompressed`, `IsTypeless`, `HasAlpha`, `MakeTypelessUNORM`, pitch calculation, `TexMetadata::IsCubemap`, and alpha-mode access ([public API](https://github.com/microsoft/DirectXTex/blob/may2026/DirectXTex/DirectXTex.h#L73-L207), [I/O and processing declarations](https://github.com/microsoft/DirectXTex/blob/may2026/DirectXTex/DirectXTex.h#L584-L966)). This is the same conceptual surface used by CAO today, so the port can preserve operation ordering and flags instead of approximating them above unrelated codecs.

DirectXTex implements both CPU compression and a D3D11-device overload. The latter is specifically for GPU BC6H/BC7 compression, and the selected source includes the DirectCompute shader implementation when `BUILD_DX11` is enabled ([CMake sources](https://github.com/microsoft/DirectXTex/blob/may2026/CMakeLists.txt#L169-L176), [official `Compress` documentation](https://github.com/microsoft/DirectXTex/wiki/Compress)). That matters because lossy encoders and mode selection can produce semantically valid but visibly different blocks. Keeping the same engine, overload choice, flags, and alpha weight gives the golden corpus the narrowest expected difference.

DirectXTex supports standard and legacy DDS headers, DX10 extension metadata, cubemaps, arrays, volumes, mip chains, typeless handling, and many legacy pixel-format mappings. Microsoft's DDS guide explains the legacy header plus optional `DDS_HEADER_DXT10` model, while the DXGI enumeration is the authoritative numeric format namespace used by profiles ([DDS programming guide](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dx-graphics-dds-pguide), [`DXGI_FORMAT`](https://learn.microsoft.com/en-us/windows/win32/api/dxgiformat/ne-dxgiformat-dxgi_format)). DirectXTex also contains explicit compatibility flags for DWORD-aligned legacy data, malformed headers, bad DXTn mip tails, missing mip data, luminance expansion, forced DX9 output, and forced DX10 output ([DDS flags](https://github.com/microsoft/DirectXTex/blob/may2026/DirectXTex/DirectXTex.h#L231-L275), [official `texconv` options](https://github.com/microsoft/DirectXTex/wiki/texconv#directdraw-surface-dds-file-options)).

The initial adapter should nevertheless use the legacy application's `DDS_FLAGS_NONE` load/save behavior. Repair flags are evidence-backed tools for a future explicitly approved compatibility correction; enabling `PERMISSIVE`, `BAD_DXTN_TAILS`, or `IGNORE_MIPS` by default would accept and rewrite inputs the released executable may reject.

### Errors and malformed input

DirectXTex public processing functions are `noexcept` and report failure through HRESULT values. The official documentation lists controlled argument, overflow, allocation, unsupported-format, file, and codec failures for the individual calls; for example the DDS I/O functions return HRESULT rather than throwing ([DDS I/O documentation](https://github.com/microsoft/DirectXTex/wiki/DDS-I-O-Functions), [public declarations](https://github.com/microsoft/DirectXTex/blob/may2026/DirectXTex/DirectXTex.h#L584-L629)). The adapter should still catch C++ exceptions at every exported function because its own allocation and diagnostic construction can throw even when the DirectXTex call cannot.

The release notes and changelog show ongoing malformed-input and codec fixes, including the May 2026 malformed HDR read fix and earlier DDS/BC correctness fixes ([May 2026 release](https://github.com/microsoft/DirectXTex/releases/tag/may2026), [changelog](https://github.com/microsoft/DirectXTex/blob/may2026/CHANGELOG.md)). This is evidence of maintenance, not proof that arbitrary DDS/TGA input is safe. Native integer overflow, allocation pressure, assertions, access violations, device removal, or a process abort remain outside Rust's unwind guarantees and require the negative corpus and helper-process contingency below.

### Version, maintenance, and platform status

Pin the signed `may2026` release tag's peeled commit, not the moving `main` branch. GitHub published that release on 2026-05-08 as the latest stable release, with NuGet and vcpkg distribution coordinates ([release metadata](https://github.com/microsoft/DirectXTex/releases/tag/may2026)). The repository continued receiving commits after the release, so it is actively maintained, but those unreleased changes should enter only through a corpus-gated upgrade ([commit history](https://github.com/microsoft/DirectXTex/commits/main/)).

The selected README requires Visual Studio 2022 or 2026 and Windows 10 SDK 19041 or later, and states that Windows 7 and Windows 8.0 support was retired in March 2025 ([build requirements](https://github.com/microsoft/DirectXTex/blob/may2026/README.md#L13-L23), [platform note](https://github.com/microsoft/DirectXTex/blob/may2026/README.md#L86-L91)). This fits the initial Windows x64 target, but the migration blueprint must state the minimum supported Windows version rather than accidentally inheriting the old Qt application's Windows 7 aspirations.

## Recommended boundary

Use an operation-oriented adapter rather than exposing a general C++ object model:

```text
Rust TextureService / TexturePolicy
        |
        | byte buffers, stable Rust request/result types
        v
Rust DirectXTexBackend
        |
        | versioned CAO C ABI
        v
C++ cao_texture_bridge + static DirectXTex may2026
        |
        +-- CPU codecs
        `-- optional D3D11 device for BC6H/BC7
```

An illustrative ABI shape is:

```c
cao_texture_result cao_texture_inspect(const uint8_t *input, size_t len,
                                       cao_texture_input_kind kind,
                                       const cao_texture_load_options *options);

cao_texture_result cao_texture_transform(const uint8_t *input, size_t len,
                                         cao_texture_input_kind kind,
                                         const cao_texture_transform_options *options);

void cao_texture_result_free(cao_texture_result *result);
```

The transform request should explicitly carry target DXGI format, target dimensions, mip policy, filter flags, compression flags, alpha threshold/weight, load/save DDS flags, and whether GPU compression is permitted. The result should contain stable error category and HRESULT fields, UTF-8 diagnostic text, normalized metadata, whether each stage changed the image, whether GPU or CPU compression ran, and owned DDS bytes on success.

Use memory I/O at the ABI. Rust then owns Unicode and extended-length paths, archive entry buffers, same-directory temporary output, close/reopen verification, atomic replacement, backup policy, and preservation of the original after failure. The bridge owns all `ScratchImage` lifetime and returns a copied buffer through an explicitly paired allocator/free function.

Create the D3D11 device lazily and keep it inside the backend. Failure to create a suitable device is a nonfatal capability result that selects the CPU path, matching the released application's intent. Device creation, adapter selection, feature-level behavior, and device-removal fallback still need corpus characterization because the existing code asks for adapter zero and uses a specific feature-level check.

Rust cancellation can be guaranteed between load, decompress, resize, mip generation, convert/compress, encode, verify, and commit stages. DirectXTex does not expose cooperative cancellation inside a single codec call. Do not advertise finer-grained cancellation until the implementation can safely isolate or interrupt that work.

## Alternatives considered

### Rust `directxtex` 1.3.0

[`directxtex` 1.3.0](https://crates.io/crates/directxtex/1.3.0) is an MIT-licensed Rust wrapper by Ryan McKenzie. Its public mapping covers memory DDS/TGA loads, DDS save, metadata, resize, convert, mip generation, CPU compress, and decompress ([API mapping](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/blob/v1.3.0/src/lib.rs#L39-L101)). It statically builds vendored DirectXTex C++ through its build script, and its Rust error type retains the HRESULT ([build script](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/blob/v1.3.0/build.rs), [error type](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/blob/v1.3.0/src/hresult.rs)).

It is the closest off-the-shelf integration and a useful source for ABI tests, but do not select it unmodified:

- v1.3.0 vendors DirectXTex commit [`9260384a…`](https://github.com/microsoft/DirectXTex/tree/9260384a375e0b39e7b22c8c67fd0d060f0c948a), the October 2024 release, while Microsoft has issued multiple later releases;
- its wrapper exports only the CPU `Compress` overload, not the legacy path's D3D11-device overload ([wrapper compression functions](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/blob/v1.3.0/ffi/main.cpp#L690-L732));
- its last tagged update was 2025-01-08 and the repository still points to that version ([v1.3.0 release](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/releases/tag/v1.3.0)); and
- it exposes a large representation-level binding, including ABI-sized C++ objects, where CAO needs only a small operation boundary.

A CAO fork that updates the vendored source and adds the GPU call could work. It would be more code to audit than the purpose-built ABI while still requiring a fork, so it is a contingency rather than the selected dependency. Local `cargo test --all-targets` on Windows x64 passed all 39 upstream tests; those tests primarily validate layout, metadata, and basic load/save fixtures, not CAO's transforms or malformed corpus.

### Pure Rust `dds` 0.2.0 plus `image` 0.25.10

[`dds` 0.2.0](https://crates.io/crates/dds/0.2.0) is a compelling MIT-or-Apache-2.0, 100%-safe-Rust DDS decoder/encoder. Its official README claims more than 70 decode formats, most encode formats, simple mipmap generation, and optional Rayon parallelism ([README](https://github.com/image-rs/image-dds/tree/v0.2.0#readme)). Its header parser is strict by default and has an explicit permissive mode that attempts to repair some invalid fields using the actual file length ([parse options](https://github.com/image-rs/image-dds/blob/v0.2.0/src/header.rs#L442-L488)). It models DX9 and DX10 headers, arrays, cubemaps, volumes, mips, alpha, FourCC, and DXGI formats, and has typed format/layout/decoding/encoding errors plus cooperative encoder cancellation ([errors](https://github.com/image-rs/image-dds/blob/v0.2.0/src/error.rs), [progress and cancellation](https://github.com/image-rs/image-dds/blob/v0.2.0/src/progress.rs)).

[`image` 0.25.10](https://crates.io/crates/image/0.25.10) can provide the TGA decoder. Its maintained implementation handles truecolor, grayscale, color-mapped and RLE TGA variants and image orientation, with typed image errors ([TGA decoder](https://github.com/image-rs/image/blob/v0.25.10/src/codecs/tga/decoder.rs), [error model](https://github.com/image-rs/image/blob/v0.25.10/src/error.rs)). CAO should not use `image`'s built-in DDS codec because its documented DDS support is limited to DXT1, DXT3, and DXT5 ([supported formats](https://github.com/image-rs/image#supported-image-formats)).

This stack loses the selection for two concrete reasons. First, `dds` decodes BC6H but explicitly cannot encode it ([format matrix](https://github.com/image-rs/image-dds/blob/v0.2.0/supported-formats.md#L76-L78)). CAO recompresses any compressed texture that it decompresses for resize or mip work, so modified BC6H would become unsupported or require a second encoder. Second, the crate's supported-format matrix is intentionally finite, while custom CAO profiles can classify the full DXGI list and DirectXTex preserves a broader family of legacy variants. Unsupported files could be passed through only when policy does not require a transform; they cannot satisfy repair or resize requests.

Even where both engines support a format, their resampling, mip filtering, header normalization, BC mode search, dithering, and error acceptance differ. Semantic equivalence permits different bytes, but it does not waive visual-quality, alpha, normal-map, cubemap, and game-loadability gates. Adopting the Rust stack now would therefore expand the parity investigation rather than reduce it.

### `ctt` 0.4.0 as a codec supplement

[`ctt` 0.4.0](https://crates.io/crates/ctt/0.4.0) presents a unified Rust API over several native encoders, including Intel ISPC, bc7enc-rdo, AMD Compressonator, etcpak, and astcenc. Its official capability table includes BC1 through BC7 and BC6 signed/unsigned, and the default package can use prebuilt ISPC libraries ([repository README](https://github.com/cwfitzgerald/ctt/tree/v0.4.0#readme)).

It can close `dds`'s BC6 encoding gap, but it does not restore one-engine semantics. Encoder choice affects quality and output; `Auto` would make upgrades or machines change behavior. The default feature set also adds multiple native codebases, licenses, build paths, and attack surfaces. If future corpus evidence favors a Rust DDS pipeline, pin an explicit minimal `ctt` feature/encoder set and treat it as a separately versioned codec backend. Do not make it the initial parity strategy.

### `image_dds`, `ddsfile`, `texpresso`, and standalone tools

[`image_dds` 0.7.2](https://crates.io/crates/image_dds/0.7.2) combines `ddsfile`, `bcdec_rs`, and Intel's native texture compressor for BC encode/decode. Its own documentation warns that the ISPC dependency lacks precompiled kernels for all targets, and it offers less legacy-header repair and format breadth than the newer `dds` crate ([official documentation](https://docs.rs/image_dds/0.7.2/image_dds/)). It is no longer the best Rust composite for this use.

[`ddsfile` 0.6.0](https://crates.io/crates/ddsfile/0.6.0) is a useful MIT DDS container and metadata parser, but its upstream README says it does not perform pixel encoding or decoding ([repository](https://github.com/cwfitzgerald/ddsfile/tree/0.6.0#readme)). [`texpresso` 2.0.2](https://crates.io/crates/texpresso/2.0.2) is pure Rust and MIT, but its delivered format enum covers BC1 through BC5; BC6, BC7, and DDS container support remain roadmap items ([repository](https://github.com/jansol/texpresso/tree/v2.0.2#readme)). Neither is a complete backend.

Bundling Microsoft's `texconv.exe` would expose most required operations and give process isolation. It has a command-line interface rather than a stable structured protocol, would require temporary-file orchestration and output parsing, and would make per-stage metadata and legacy policy harder to preserve. Keep `texconv` as an independent diagnostic oracle and use a purpose-built CAO helper containing the same adapter only if in-process safety fails.

## Licensing and distribution

DirectXTex is MIT-licensed ([license](https://github.com/microsoft/DirectXTex/blob/may2026/LICENSE)). Static linking does not impose copyleft terms on CAO, but the self-contained ZIP must retain the DirectXTex copyright and permission notice. The release audit must also account for DirectX-Headers, DirectXMath, any bundled shader bytecode/source and notices, the selected C/C++ runtime linkage, and Windows SDK redistributable rules.

Build `BUILD_SHARED_LIBS=OFF`, `BUILD_DX11=ON`, and `BUILD_DX12=OFF` unless another scoped feature proves necessary. DirectXTex's CMake default is a static library, so no DirectXTex DLL is expected ([CMake options](https://github.com/microsoft/DirectXTex/blob/may2026/CMakeLists.txt#L26-L39)). That does not prove the final executable is self-contained: inspect imports, decide `/MT` versus redistributing the supported VC runtime, and test the exact ZIP on a clean VM.

If `ba2` 3.0.1 remains the archive choice, its separate `directxtex` Rust dependency may compile another older DirectXTex copy. Do not assume the two can be unified without testing because their wrappers, compile definitions, and expected source revisions differ. Record both in the SBOM; consider deduplication only after archive and texture corpora pass against one shared pinned engine.

## Local Windows x64 build evidence

On 2026-07-12, the selected tag was cloned recursively and its peeled commit verified as `4feb3e11a020f35b796fc769a74216a555d4f5ef`. The following configuration succeeded:

```text
CMake 4.4.0
Visual Studio 18 2026 Build Tools / MSVC 19.51.36248
Windows SDK 10.0.26100.0
x64 Release
BUILD_SHARED_LIBS=OFF
BUILD_DX11=ON
BUILD_DX12=OFF
BUILD_SAMPLE=OFF
```

The build compiled the CPU codecs, `BCDirectCompute.cpp`, `DirectXTexCompressGPU.cpp`, and `DirectXTexD3D11.cpp`, then produced a 1,682,132-byte `DirectXTex.lib`. Enabling the tools produced `texconv.exe`; running `texconv --version` succeeded and reported `texconv version 211 (library)`. This proves the pinned source and GPU-enabled library compile on the available Windows x64 toolchain. It does not prove runtime GPU compression, clean-machine redistribution, ABI correctness, malformed-input safety, or CAO parity.

## Mandatory acceptance gates

These gates block the functional-parity milestone and release, not the backend decision.

### 1. Reproducible native build and ABI

- Pin the peeled commit and fetched archive SHA-256; never fetch a moving tag or branch during the build.
- Build the real Rust/Slint release graph for `x86_64-pc-windows-msvc` with documented MSVC and Windows SDK versions.
- Keep the bridge source small, version every ABI request/result, use fixed-width fields, and add compile-time ABI size/alignment assertions.
- Run bridge unit tests for null pointers, empty buffers, every HRESULT category, allocation failure, result freeing, repeated initialization, and concurrent inspection if concurrency is ever enabled.
- Inspect imports and run the exact self-contained ZIP on a clean supported Windows x64 VM with no Visual Studio, Rust, repository, or unshipped runtime installed.
- Generate the SBOM and license bundle and verify all native source pins and notices.

### 2. Golden texture corpus

Build a redistributable or locally supplied corpus containing:

- DDS with legacy and DX10 headers, including valid legacy FourCC variants and known malformed-but-accepted files;
- BC1, BC2, BC3, BC4 UNORM/SNORM, BC5 UNORM/SNORM, BC6H UF16/SF16, BC7 UNORM/SRGB, and every uncompressed/packed format found in the scoped games and custom profiles;
- typeless formats with valid and invalid UNORM mappings;
- 1D, 2D, 3D/volume, arrays, cubemaps, cubemap arrays, full mips, one mip, partial mips, and malformed tails;
- opaque, straight, premultiplied, custom, and unknown alpha metadata; ordinary color, normal, mask, landscape, and interface textures;
- power-of-two and non-power-of-two sizes, dimensions below 4, one-dimensional edges, maximum practical dimensions, and aspect-ratio extremes; and
- uncompressed, color-mapped, grayscale, RLE/non-RLE, 15/16/24/32-bit, origin/orientation, ID-field, and malformed TGA variants.

For each applicable fixture, inspect, load, transform, save, reopen, and independently decode. Compare dimensions, format, resource dimension, array/depth/cubemap flags, alpha mode, mip inventory, pitches, header style, and decoded pixels. Byte identity is required only for pass-through and operations the parity contract designates deterministic.

### 3. Differential behavior and visual quality

- Run every policy combination through the authenticated v5.3.15 executable and the new adapter: necessary repair, compression, mip generation, size and ratio resize, interface exclusions, landscape handling, TGA conversion, and custom unwanted-format lists.
- Exercise CPU and GPU BC6H/BC7 paths separately. Record GPU identity, adapter, feature level, DirectXTex path, and flags so results are reproducible enough to diagnose.
- Compare decoded output with format-appropriate metrics and targeted channel tests; establish thresholds separately for color, alpha/masks, and tangent-space normal maps.
- Test BC1 alpha threshold, BC4/BC5 signedness, BC6 HDR range, BC7 subsets, sRGB/linear metadata, transparent-edge mip behavior, and cubemap seams.
- Load outputs in the scoped game/tool environments where automated validation is insufficient.
- Enter every difference in the discrepancy register; no unexplained difference is accepted merely because both files decode.

### 4. Repair and legacy compatibility

- Characterize `DDS_FLAGS_NONE` first. Separately run `PERMISSIVE`, `BAD_DXTN_TAILS`, `IGNORE_MIPS`, DWORD alignment, luminance expansion, forced DX9, and forced DX10 modes against the malformed corpus.
- Keep repair flags disabled unless a specific legacy defect is documented, approved, and regression-tested.
- Verify whether the released executable preserves or normalizes legacy headers and alpha metadata after every modification.
- Inventory custom profiles from the authenticated package and maintainer fixtures. Every named DXGI format must have an explicit action: transform, pass through, or controlled unsupported error.
- Add approved corrections for the dry-run resize mismatch and `compareInfo` OR bug; do not let either correction alter unrelated policy.

### 5. Negative, resource, and native-safety corpus

- Test truncated magic/headers/data, contradictory dimensions and flags, overflowed pitches/counts, excessive arrays/mips/depth, zero dimensions, missing faces, bogus FourCC/DXGI values, invalid TGA RLE packets, decompression bombs, and device-loss conditions.
- Impose Rust-side input/output and dimension limits before native allocation where the format metadata can be inspected safely.
- Fuzz the byte-buffer inspect and transform calls under time and memory limits; use native sanitizers where available plus Windows Application Verifier or equivalent diagnostics.
- Require controlled errors, no exception across the ABI, no leak, no hang, no source replacement, and no partial destination.
- A reproducible valid input that terminates the process, or malformed input that cannot be contained promptly, triggers the helper-process contingency.

### 6. Transaction, cancellation, and packaging

- Write output to a unique same-directory temporary file, close it, reopen and verify it, then atomically replace according to the approved backup contract.
- Test disk-full, permissions, target-in-use, rename failure, verification failure, cancellation at every advertised stage boundary, and interruption between transaction phases.
- Preserve the source and any archive entry until verification and commit succeed.
- Document that a single native compression call is not cooperatively cancellable. Measure worst-case BC6H/BC7 CPU and GPU latency on maximum supported textures.
- Verify the final package on systems with no usable GPU, software/WARP devices, old supported drivers, multiple adapters, and device removal where practical.

### 7. Maintenance containment

- Keep DirectXTex and D3D references inside the native bridge and one Rust backend module.
- Record source pin, archive checksum, build flags, shader-generation method, compiler/SDK versions, patches, and corpus results.
- Make upgrades explicit dependency changes with changelog review and full corpus reruns.
- Maintain a small CAO fork only when a released pin needs a proven fix; publish its exact source and retain upstream attribution.
- Keep `dds` 0.2.0 and `texconv` as independent test oracles so the project can detect shared assumptions and reassess a Rust-native migration later.

## Contingency

First, patch a narrowly maintained CAO fork of the selected DirectXTex pin behind the unchanged ABI. If process-terminating behavior cannot be fixed promptly, move the same one-shot bridge into a bundled CAO helper executable and communicate through versioned length-delimited files or pipes. Rust applies time/resource limits and treats helper termination as one texture's controlled failure.

Reconsider a Rust-native production backend when one stack can pass the same corpus for the complete scoped transform surface. At minimum it must encode and decode every required BC format including BC6H, preserve or deliberately normalize the required DDS metadata and legacy variants, support the custom-profile DXGI actions, and meet the visual-quality thresholds without an uncontrolled encoder-selection policy. `dds` is the most promising base, but version 0.2.0 plus `image` and `ctt` does not yet meet that threshold.

## Final recommendation

Adopt DirectXTex `may2026` at `4feb3e11a020f35b796fc769a74216a555d4f5ef` through a CAO-owned byte-buffer C ABI, statically built with D3D11 enabled. This preserves the legacy engine, operation order, CPU/GPU BC6H/BC7 choice, DDS/TGA behavior, DXGI metadata, and custom-profile reach while confining native code to a small replaceable module.

Do not adopt `directxtex` 1.3.0 unmodified: it is pinned to October 2024 and lacks the GPU compression binding. Do not replace the engine with `dds` 0.2.0 plus `image` and `ctt` for the initial parity release: BC6 encoding, broad DXGI/custom-profile behavior, and cross-engine filtering/error semantics remain material gaps. Keep both Rust options as test oracles and future migration candidates after the mandatory corpus establishes a safe replacement threshold.
