# GPU BC7 encoding with the `directxtex` crate

Research for [#459](https://github.com/evildarkarchon/CAO/issues/459) on the map
[#458 "Port CAO to Rust and Slint"](https://github.com/evildarkarchon/CAO/issues/458).

**Question.** The texture optimizer is moving to the `directxtex` Rust crate. How do we keep the
GPU BC6H/BC7 encoding that `src/TexturesOptimizer.cpp` does today? Does the crate cover every
other DirectXTex call that CAO makes?

**Answer.** Fork `directxtex-rs`, add bindings for the Direct3D 11 `Compress` overload, and
patch the fork in for the whole workspace with `[patch.crates-io]`, so `ba2` uses the same copy.
Create the D3D11 device in Rust with the `windows` crate. The CPU fallback stays on the crate's
existing `compress`. The fork is about 100 lines of hand-written code plus 14 generated shader
headers. It needs only the Windows SDK, which the MSVC Rust toolchain already requires. The
crate covers every other call CAO makes. Three calls need small adaptations (see
[API coverage](#api-coverage)).

Sources were checked on 2026-10-06 against `directxtex` 1.3.0 from crates.io, the upstream
`microsoft/DirectXTex` repository and wiki, and this repo's `src/` at `8767f86`.

## What CAO does today

- `TexturesOptimizer` constructor (`src/TexturesOptimizer.cpp:10-21`): it calls
  `createDevice(0, …)`, which loads `d3d11.dll` dynamically. It requests feature levels 11_0,
  10_1, and 10_0. Below 11_0 it requires
  `D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS::ComputeShaders_Plus_RawAndStructuredBuffers_Via_Shader_4_x`
  (`:44-109`). If no device is created, it logs "DirectCompute is not available, using
  BC6H / BC7 CPU codec". It then calls `CoInitializeEx(nullptr, COINIT_MULTITHREADED)`.
- `convertWithCompression` (`:511-525`) picks the codec:
  - For BC6H or BC7 with a device: GPU
    `Compress(device, img, nimg, info, format, TEX_COMPRESS_BC7_USE_3SUBSETS, 1.f, out)`.
  - Otherwise: CPU
    `Compress(img, nimg, info, format, TEX_COMPRESS_DEFAULT | TEX_FILTER_SEPARATE_ALPHA, TEX_THRESHOLD_DEFAULT, out)`.
- The C++ build gets DirectXTex from vcpkg port `directxtex` `2026-05-07` (the may2026
  release). Its features are `core;dx11`, and it is built with OpenMP
  (`build/vcpkg_installed/x64-windows-static-md/share/directxtex/vcpkg_abi_info.txt` and
  `directxtex-config.cmake`). CAO never passes `TEX_COMPRESS_PARALLEL`, so OpenMP has no effect
  on CAO's output.

## What the crate builds

`directxtex` 1.3.0, published 2025-01-08 from
[Ryan-rsm-McKenzie/directxtex-rs](https://github.com/Ryan-rsm-McKenzie/directxtex-rs) at
`8e44d151`. It is still the latest version.

- **Vendored DirectXTex.** The DirectXTex submodule is pinned to `9260384a`, which is the
  October 28, 2024 release (`oct2024`, `DIRECTX_TEX_VERSION 206`).
- **Build script.** `build.rs` compiles DirectXTex with the `cc` crate as C++17. On Windows it
  compiles the core files plus `DirectXTexFlipRotate.cpp` and `DirectXTexWIC.cpp`. It does
  **not** compile `BCDirectCompute.cpp`, `DirectXTexCompressGPU.cpp`, or `DirectXTexD3D11.cpp`,
  even though the packaged crate ships them (the `include` glob is
  `external/DirectXTex/DirectXTex/*.cpp`).
- **Shaders.** The crate does not ship `DirectXTex/Shaders/`, neither the HLSL nor any compiled
  output. Upstream does not commit compiled shaders either. At the pinned commit,
  `DirectXTex/Shaders/` holds only `BC6HEncode.hlsl`, `BC7Encode.hlsl`, and
  `CompileShaders.cmd`. `BCDirectCompute.cpp` `#include`s 14 generated headers:
  `BC7Encode_*CS.inc`, `BC6HEncode_*CS.inc`, and their `_cs40` variants.
- **Headers.** The headers are already DX11-aware. On `_WIN32`, `DirectXTex.h` includes
  `<d3d11_1.h>` from the Windows SDK, so the `Compress(ID3D11Device*, …)` declarations
  (`DirectXTex.h:819-835`) are visible when the crate's FFI compiles. Only the implementation
  objects, the shaders, and an FFI entry point are missing.
- **FFI surface.** `ffi/main.cpp` exports 80 `DirectXTexFFI_*` functions. None take an
  `ID3D11Device`. Searching `ffi/` and `src/` for `ID3D`/`Device` finds nothing.
- **No OpenMP.** `build.rs` never enables `/openmp`. Upstream returns `E_NOTIMPL` for
  `TEX_COMPRESS_PARALLEL` without OpenMP (`DirectXTexCompress.cpp:700-707`; the
  [Compress wiki](https://github.com/microsoft/DirectXTex/wiki/Compress) says the same). So the
  crate's CPU BC7 runs on **one thread**.
- **No `links` key.** `Cargo.toml` has no `links` key, so Cargo will not stop two copies of the
  native DirectXTex library from entering one link.
- **Thread safety.** `ScratchImage`, `Image`, and `Blob` contain raw pointers and have no
  `unsafe impl Send` or `unsafe impl Sync`, so none of them is `Send`.
- **Upstream activity.** The maintainer's last push was 2025-01-08. Open issue
  [#3 "GPU support"](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/issues/3) (2025-07-26)
  asks for exactly this binding and has no reply. The OpenMP PR
  [#4](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/pull/4) has been open and unmerged
  since 2025-09. In closed issue
  [#2](https://github.com/Ryan-rsm-McKenzie/directxtex-rs/issues/2), a user reports "minutes per
  texture" for BC7. The maintainer replied: "DirectXTex is just slow at BC7 encoding" and
  suggested `intel_tex_2` or `bc7enc_rdo`.

## Upstream's GPU build requirements

Sources: `CMakeLists.txt` at `9260384a` and
[the Compress wiki page](https://github.com/microsoft/DirectXTex/wiki/Compress).

- `BUILD_DX11` (default ON) adds `BCDirectCompute.{h,cpp}`, `DirectXTexCompressGPU.cpp`, and
  `DirectXTexD3D11.cpp`. A custom command runs `CompileShaders.cmd` to generate the `.inc` files
  with `fxc.exe`. With `USE_PREBUILT_SHADERS` and `COMPILED_SHADERS=<dir>` you can supply
  shaders compiled elsewhere. `Compress(device, …)` needs only the first two source files.
  `DirectXTexD3D11.cpp` provides `CreateTexture` and `CaptureTexture`, which CAO does not use.
- `CompileShaders.cmd` compiles each of 7 entry points twice: with `/Tcs_5_0`, and with
  `/Tcs_4_0 /DEMULATE_F16C`.
- **Local check.** I compiled all 14 shaders from the pinned HLSL with the installed
  `Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe`, using the script's flags. There were 0
  failures. It took 17.5 s and produced 3.47 MB of `.inc` text. No C++ was built.
- **Device requirements.** The wiki says the GPU path needs "a Direct3D 11 device with Feature
  Level 10.0 or greater that supports DirectCompute". It supports only `BC6H_UF16`, `BC6H_SF16`,
  `BC7_UNORM`, and `BC7_UNORM_SRGB`. It is "not yet supported for DirectX 12 devices". It
  "makes use of the device's immediate context", so one device must not compress from two
  threads at once.
- **Flags.** `TEX_COMPRESS_BC7_USE_3SUBSETS` turns on BC7 modes 0 and 2 in the GPU compressor
  (`BCDirectCompute.cpp:217-226`). The wiki says this "does not usually have significant quality
  impacts as modes 0 & 2 are rarely selected". `alphaWeight` `1.0` is
  `TEX_ALPHA_WEIGHT_DEFAULT`.
- **Internal dependencies.** `DirectXTexCompressGPU.cpp` uses
  `DirectX::Internal::{LoadScanline, ConvertScanline, StoreScanline}` to convert input to RGBA8
  or RGBA32F. These live in `DirectXTexConvert.cpp`, which is in the same static library.
- **Link libraries.** No new import libraries are needed: the device comes in from the caller,
  and `dxguid.lib` is pulled in by `#pragma comment` only under `_DEBUG`/`PROFILE`.

## Options

### A. Fork `directxtex-rs` and bind the D3D11 `Compress` (recommended)

Changes in the fork:

1. **`build.rs`.** Under `cfg(windows)`, also compile `BCDirectCompute.cpp` and
   `DirectXTexCompressGPU.cpp`, and add the compiled-shader directory to the include path.
   There are two ways to get the shaders:
   - **Vendored (preferred).** Commit the 14 generated `.inc` files, regenerated whenever the
     DirectXTex submodule moves. This mirrors upstream's `USE_PREBUILT_SHADERS` and keeps
     `fxc.exe` out of every build. The 3.5 MB is irrelevant for a git dependency.
   - **Build-time.** Have `build.rs` find `fxc.exe` under `Windows Kits\10\bin\<ver>\<arch>`
     and run the same commands. Every build gets 17 s slower and depends on locating the SDK.
2. **`ffi/main.cpp`.** Add an `extern "C"` wrapper for the multi-image
   `Compress(ID3D11Device*, const Image*, size_t, const TexMetadata&, DXGI_FORMAT, TEX_COMPRESS_FLAGS, float, ScratchImage&)`,
   guarded by `CONFIG_WINDOWS`. This is about 15 lines and follows the existing
   `DirectXTexFFI_Compress2` pattern.
3. **Rust.** Add an `unsafe fn compress_gpu(device: *mut c_void, images, metadata, format, flags, alpha_weight) -> Result<ScratchImage>`
   to `free_functions.rs`, plus a `ScratchImage` method, behind `cfg(windows)`. A raw pointer
   avoids tying the fork to one version of the `windows` crate. The caller passes
   `ID3D11Device::as_raw()`.
4. **Optional, for the port.**
   - Add `unsafe impl Send for ScratchImage` (it owns its heap memory).
   - Add an `&mut Image` accessor (see [API coverage](#api-coverage)).
   - Bump the DirectXTex submodule to the `may2026` tag so the Rust build and the C++ oracle
     run the same DirectXTex.
   - Keep the crate version semver-compatible with 1.x (for example `1.4.0`) so it satisfies
     `ba2`'s requirement.

In the CAO workspace:

- **Use the fork everywhere.** Point the workspace at the fork with
  `[patch.crates-io] directxtex = { git = "…/directxtex-rs", rev = "…" }`. `ba2` 3.0.1 depends
  on `directxtex = "1.1.0"` (non-optional; `src/fo4/file.rs` and `src/fo4/archive.rs` use it for
  DX10 texture archives), so the patch replaces its copy as well. That leaves exactly one native
  DirectXTex in the link. Without the patch, two copies of the same C++ symbols would be linked.
  Because the crate has no `links` key, Cargo would not catch this. You would get duplicate
  symbol errors or silent ODR violations.
- **Device creation.** Port `createDevice` to the `windows` crate:
  [`D3D11CreateDevice`](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Direct3D11/fn.D3D11CreateDevice.html),
  [`D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS`](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Direct3D11/struct.D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS.html),
  and `CreateDXGIFactory1`/`EnumAdapters` for adapter 0. It is about 50 lines. If no device can
  be created, fall back to the crate's CPU `compress`, as C++ does today.
- **Upstreaming.** Offer the change upstream as a PR answering issue #3. Do not wait for it,
  because upstream is dormant.

**Effort:** about 1–2 days, including a smoke test that GPU-encodes a BC7 texture and checks it
with `compute_mse`.

**Ongoing cost:** owning a small fork, and regenerating the `.inc` files when DirectXTex is
bumped.

**Fit with the map:** this keeps the same engine and the same code path, satisfying "Keep the
same native engines … including GPU BC7 encoding".

### B. Separate C++ shim crate next to `directxtex` (rejected)

- A shim crate that compiles only `BCDirectCompute.cpp` and `DirectXTexCompressGPU.cpp` must
  link against `DirectX::Internal::*Scanline` and `ScratchImage` members. Those symbols live
  inside the `directxtex` crate's native library.
- That couples the shim to the exact vendored DirectXTex commit and its compile flags. The crate
  exports no include path (no `links`/`DEP_*` metadata), so the shim would also need its own
  copy of headers that match that commit.
- The alternative is for the shim to compile its own DirectXTex. That puts duplicate C++ symbols
  in one link: the same hazard as above, with no Cargo guard.
- A variant that binds only `GPUCompressBC` and reimplements the conversion and mip loop of
  `DirectXTexCompressGPU.cpp` in Rust is more code than option A, and it still depends on
  `DirectXTexP.h` internals.
- Option B is the same work as option A plus extra coupling.

### C. CPU-only BC7 (rejected)

- **The crate's own CPU BC7.** This is the same engine. However, the crate has no OpenMP, so it
  runs single-threaded per call. DirectXTex's own docs call the software BC6H/BC7 encoder
  "computationally expensive and can be quite slow". The user report in directxtex-rs #2 is
  "minutes per texture". On any machine where C++ CAO gets a DirectCompute device, this is a
  large regression on a BC7-heavy run. That breaks the map's performance bar ("no meaningful
  regression"). Parallelizing across textures to compensate would be a re-architecture, and CAO
  processes textures one at a time today. It would also change output: the C++ CPU fallback
  does not set `BC7_USE_3SUBSETS`, so it uses a different mode set from the GPU path.
- **`intel_tex_2` 0.5.0** (2025-07-02, MIT/Apache-2.0,
  [Traverse-Research/intel-tex-rs-2](https://github.com/Traverse-Research/intel-tex-rs-2)).
  These are bindings to Intel's ISPCTextureCompressor. The README says the ISPC kernels are
  prebuilt, so no ISPC compiler is needed. It is fast and well regarded:
  [Aras Pranckevičius' 2020 benchmark](https://aras-p.info/blog/2020/12/08/Texture-Compression-in-2020/)
  places ISPC BC7 as a solid baseline, with bc7e better on both axes. However:
  - It is a different encoder, which breaks the map's "same native engines" constraint.
  - CAO would need new glue: RGBA8 block-aligned surfaces, per-mip and per-array-slice loops,
    and DDS assembly.
  - No primary source compares its quality directly with DirectXTex's GPU BC7.

  Its output could only count as "equivalent" under a perceptual or PSNR rule, never a
  structural one. Keep it only as a contingency if option A fails in practice.

## API coverage

Every other DirectXTex call CAO makes has a binding in `directxtex` 1.3.0. The ticket's list
leaves out `LoadFrom*Memory`, `IsTypeless`, `OverrideFormat`, `CopyRectangle`, and
`ScratchImage::Initialize`, which CAO also calls; they are included below.

| C++ call (site) | Crate equivalent | Notes |
|---|---|---|
| `LoadFromTGAFile` (`:247`), `LoadFromTGAMemory` (`:282`) | `ScratchImage::load_tga(&[u8], TGA_FLAGS, Option<&mut TexMetadata>)` | The crate has memory APIs only. Read the file with `std::fs::read`. Both C++ entry points share the TGA decoder in `DirectXTexTGA.cpp`. This also removes the `wchar_t[1024]` deviation by construction. |
| `LoadFromDDSFile` (`:251`), `LoadFromDDSMemory` (`:285`) | `ScratchImage::load_dds(&[u8], DDS_FLAGS, …)` (wraps `LoadFromDDSMemoryEx`) | Memory only, as above. |
| `IsTypeless` (`:254`) | `DXGI_FORMAT::is_typeless(partial_typeless)` | The C++ default is `partialTypeless = true`. Pass `true` explicitly. |
| `MakeTypelessUNORM` (`:255`) | `DXGI_FORMAT::make_typeless_unorm` | FFI passthrough. |
| `ScratchImage::OverrideFormat` (`:259`) | `ScratchImage::override_format` → `Option<()>` | |
| `Decompress(…, DXGI_FORMAT_UNKNOWN, …)` (`:315`) | `ScratchImage::decompress(DXGI_FORMAT_UNKNOWN)` | Multi-image overload. |
| `Resize(…, TEX_FILTER_SEPARATE_ALPHA \| TEX_FILTER_FORCE_NON_WIC, …)` (`:348-350`) | `ScratchImage::resize(w, h, TEX_FILTER_FLAGS)` | Both flags exist (`enums.rs:172, 224`). |
| `ScratchImage::Initialize` (`:393`) | `ScratchImage::initialize(&TexMetadata, CP_FLAGS)` | |
| `CopyRectangle` (`:403`) | `Image::copy_rectangle(&mut self /*dst*/, src, &Rect, filter, x, y)` | **Gap:** `ScratchImage` exposes only `image() -> &Image` and `images() -> &[Image]`, and `Image` is neither `Clone` nor `Copy`. Workarounds: build a destination `Image` from the public fields of `image(0, i, 0)` (its `pixels` points into the scratch memory), or copy mip 0 with `pixels_mut()`. A fork could instead add `image_mut`. |
| `GenerateMipMaps(…, TEX_FILTER_SEPARATE_ALPHA, levels, …)` (`:428`) | `ScratchImage::generate_mip_maps(filter, levels)` | Without `FORCE_NON_WIC`, upstream `UseWICFiltering` can select WIC for 8-bit non-sRGB formats (`DirectXTexMipmaps.cpp:628-690`). WIC needs COM, and the crate never initializes it, so the Rust worker thread must call [`CoInitializeEx`](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/Com/fn.CoInitializeEx.html) as the C++ constructor does. |
| `Convert(…, TEX_FILTER_DEFAULT, TEX_THRESHOLD_DEFAULT, …)` (`:478-480`) | `ScratchImage::convert(format, filter, threshold)` | `TEX_THRESHOLD_DEFAULT` = 0.5 (`constants.rs:6`). |
| CPU `Compress` (`:523-525`) | `ScratchImage::compress(format, TEX_COMPRESS_FLAGS, threshold)` | C++ ORs `TEX_FILTER_SEPARATE_ALPHA` (0x100) into the compress flags. That is a **no-op**, because upstream masks compress flags with `GetBCFlags` (`DirectXTexCompress.cpp:26-35`). Rust's bitflags type cannot express it anyway, so pass `TEX_COMPRESS_DEFAULT`. |
| GPU `Compress(device, …)` (`:520-521`) | **Missing.** Option A adds it. | |
| `SaveToDDSFile` (`:556`) | `ScratchImage::save_dds(DDS_FLAGS)` → `Blob`, then `std::fs::write(blob.buffer())` | Both upstream functions write the header through `EncodeDDSHeader` (`DirectXTexDDS.cpp:2320, 2545`), so the bytes are the same. |
| `IsCompressed` (`:223`, `:461`, `:570`) | `DXGI_FORMAT::is_compressed` | Reimplemented in Rust with the same BC1–BC7 list. |
| `HasAlpha` (`:573`) | `DXGI_FORMAT::has_alpha` | FFI passthrough. |
| `TexMetadata::IsCubemap` (`:569`) | `TexMetadata::is_cubemap` | Rust reimplementation (checks `TEX_MISC_TEXTURECUBE`). |
| `TexMetadata::GetAlphaMode` (`:572`) | `TexMetadata::get_alpha_mode` | Rust reimplementation (masks `misc_flags2`). |

`ba2` already depends on this crate, so after option A it uses the fork through the patch.

## Facts that bear on other tickets

- **#467 output equivalence.**
  - C++ BC7 output already depends on the hardware. With a DirectCompute device, CAO uses the
    GPU encoder with `BC7_USE_3SUBSETS`. Without one, it uses the CPU encoder without that flag.
    These are different encoders with different mode sets. A BC7 equivalence rule therefore has
    to decode and compare (for example PSNR or MSE; the crate binds `ComputeMSE` as
    `Image::compute_mse`) plus compare metadata (format, dimensions, mip count, array size,
    cubemap flag). Byte comparison will not work.
  - Byte identity on one machine with the same DirectXTex version is plausible but unverified.
  - The two builds use different DirectXTex versions: `oct2024` in the crate, `may2026` in C++.
    Every change since `oct2024` to `BC6HBC7.cpp`, `BCDirectCompute.cpp`, and the shaders is a
    formatting or lint commit (`cfd5f271`, `cb3be57e`, `55b96d1d`, `c66874e7`, plus a one-line
    `CompileShaders.cmd` change in `ead77dd1`). I checked `cb3be57e`'s `BC7Encode.hlsl` diff:
    whitespace only. I did not audit the other modules (convert, resize, mips, DDS writer).
    Bumping the fork's submodule to `may2026` removes this variable.
- **#468 workspace architecture.**
  - `ScratchImage`, `Image`, and `Blob` are not `Send`, so each texture must be processed
    entirely on one thread, or the fork must add `Send`.
  - The worker thread must initialize COM (multithreaded apartment, as today).
  - Keep one D3D11 device per worker thread, because the immediate context is not safe to use
    from several threads.
  - The workspace needs the `windows` crate (Direct3D11, Dxgi, Com features) and one
    `[patch.crates-io]` entry for `directxtex`.
  - DirectXTex itself needs no CMake or vcpkg: the crate builds it with `cc` (MSVC plus the
    Windows SDK). This settles the DirectXTex half of the map's "How the build integrates C++"
    question. nifly is separate.
- **#461 ba2.** `ba2` 3.0.1 requires `directxtex = "1.1.0"` (caret), so the fork must stay
  1.x-compatible for `[patch.crates-io]` to apply. FO4 texture archives go through the same
  DirectXTex copy, so a submodule bump also affects how `ba2` handles DDS. `ba2` calls
  `ScratchImage::load_dds` and `TexMetadata::encode_dds_header` (`src/fo4/file.rs:1296, 1393,
  1404`). It does no BC encoding.
- **Benchmark prototype (map "Not yet specified").** Option A keeps the engine identical, so a
  GPU BC7 benchmark is optional. A quick smoke timing of one large BC7 texture, GPU against the
  Rust CPU fallback, is enough to confirm the binding works.

## Open risks

- **Fork maintenance.** CAO owns the fork until upstream merges a GPU binding. Upstream is
  dormant, with an unmerged PR from 2025-09.
- **Unbuilt binding.** No Rust build of the GPU binding was attempted here; only the shader
  compile step was verified. `BCDirectCompute.cpp` and `DirectXTexCompressGPU.cpp` may produce
  warnings or errors under the crate's `cc` flags (`c++17`, warnings off), but they compile
  upstream under MSVC with the same headers.
- **GPU determinism.** It is unverified whether GPU BC7 output is deterministic across driver or
  GPU vendor changes. This does not matter if #467 adopts a decode-and-compare rule for block
  formats.
- **Mipmap filter path.** If COM initialization is forgotten, `GenerateMipMaps` fails on the WIC
  path. If `FORCE_NON_WIC` is added to "fix" that instead, the mip filter changes from today's.
