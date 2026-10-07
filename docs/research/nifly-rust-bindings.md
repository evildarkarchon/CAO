# Binding nifly mesh operations from Rust

Research for [#460](https://github.com/evildarkarchon/CAO/issues/460) on the map
[#458 Port CAO to Rust and Slint](https://github.com/evildarkarchon/CAO/issues/458).

**Question.** How should Rust bind the nifly mesh operations CAO uses, and how is nifly built from
Cargo?

## Answer

- **Write a hand-written plain C ABI shim, one `.cpp` file of about 150 lines.** Wrap it in a small
  safe Rust type. Do not use `cxx`. Rule out `autocxx`.
- **Build nifly with the `cc` crate.** Compile the 13 nifly `.cpp` files and the shim directly from
  `build.rs`. Do not use CMake or vcpkg. This is how the `directxtex` crate, which the port already
  adopts, builds DirectXTex.
- **Vendor nifly at the pinned commit.** Use `5504832da8009248ff68a9d306973ee1ba61a0a4` with
  `fix-const-matrix-equality.patch` already applied. Compile it as C++17, which is nifly's own
  declared standard. The patch is only needed under C++20 (verified below), so it is harmless
  insurance, not load-bearing.
- **Catch every C++ exception inside the shim.** Pass paths as raw UTF-16 (`encode_wide`). Treat
  texture paths as bytes, not `String`. Each `NifFile` handle is `Send` but not `Sync`. Meshes stay
  sequential on the single Run Worker, as in C++ CAO today.

A throwaway prototype (outside the repo) proved the recommendation end to end on this machine
(MSVC 14.51, rustc 1.99, `cc` 1.6.0). It built nifly and the shim with `cc` and loaded a NIF via a
non-ASCII UTF-16 path. It ran `OptimizeFor` LE→SSE, both normal and head-part. It rewrote texture
references in place, saved, and reloaded. Its output was **byte-identical** to nifly's own expected
test files (`TestNifFile_Optimize_LE_to_SE_expected.nif` and
`TestNifFile_Optimize_Dynamic_LE_to_SE_expected.nif`) in both debug C++17 and release C++20+patch
builds.

## The surface CAO actually uses

From the local C++ sources:

| CAO call site | nifly API | Notes |
|---|---|---|
| `MeshesOptimizer::loadMesh` (`src/MeshesOptimizer.cpp:180-193`) | `NifFile::Load(path, NifLoadOptions{isTerrain})` | Any non-zero return is "Cannot load mesh". |
| `MeshesOptimizer::saveMesh` (`src/MeshesOptimizer.cpp:195-202`) | `NifFile::Save(path)` | Any non-zero return is a failure. The executor then fingerprints the staged file (`src/AssetExecution/AssetExecutor.cpp:536`). |
| `MeshesOptimizer::scan` (`src/MeshesOptimizer.cpp:37-49`) | `IsValid`, `IsSSECompatible`, `NiVersion::SetFile/SetStream/SetUser`, `NiVersion::IsSK` | `IsSK` is evaluated on the **profile's target version**, not the file's version. |
| `MeshesOptimizer::optimize` (`src/MeshesOptimizer.cpp:120-170`) | `OptimizeFor(OptOptions{targetVersion, headParts, removeParallax=false})` | Logs `dupesRenamed` and the five `shapes*` name lists. `versionMismatch` is never read, and `calcBounds` keeps its default `true`. |
| `hasReferencedTgaTexture` / `replaceReferencedTgaTextureNames` (`src/MeshReferenceMaintenance.cpp:43-70`) | `GetShapes`, `GetTexturePathRefs` | Mutates the `std::string&` references in place. |
| `MainOptimizer::loadMesh` and friends (`src/MainOptimizer.cpp:222-283`) | holds `std::unique_ptr<nifly::NifFile>` | One loaded mesh at a time. |
| Tests (`tests/MeshReferenceMaintenanceTests.cpp:29-50`, `tests/MainOptimizerTests.cpp:115-140`) | `Create`, `CreateShapeFromData`, `SetTextureSlot`, `GetTextureSlot` | Needed only if these scenarios are ported as Rust tests. |

The `NiFileVersion` enum is also used as a raw integer in profiles (`src/Profiles.cpp:119`,
`src/OptimizerProfileSnapshot.h:56`). That is plain `u32` data and needs no binding.

### Verified facts about the pinned nifly

All links point at commit `5504832` (the `REF` in `cmake/ports/nifly/portfile.cmake`).

- **`Load` and `Save` take `std::filesystem::path`, not `std::u16string`.**
  [`NifFile.hpp:90-93`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/include/NifFile.hpp#L90-L93).
  CAO's `toStdU16String()` only works through an implicit conversion to `path`.
- **`Load` returns 0 on success, 1 for an unopenable file or bad header, 2 for an unsupported
  version, and 3 for an unknown block in a pre-20.2.0.5 file.** It reports this through return
  codes, not exceptions.
  [`NifFile.cpp:183-237`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/NifFile.cpp#L183-L237).
- **`Save` returns 1 only if the `ofstream` fails to open.** It never checks stream state after
  writing, so a short write still returns 0. It also mutates the in-memory file
  (`FinalizeData`, `Optimize`, `PrettySortBlocks`), so it needs `&mut`.
  [`NifFile.cpp:1322-1369`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/NifFile.cpp#L1322-L1369).
- **`OptimizeFor(OptOptions&)` takes a non-const reference.** It only converts SK→SSE or SSE→SK.
  Any other pair sets `versionMismatch` and returns without changes.
  [`NifFile.cpp:1378-1387`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/NifFile.cpp#L1378-L1387).
- **`OptOptions` and `OptResult` are not FFI-safe.** `OptOptions` embeds a `NiVersion`, which holds a
  `std::string` header string. `OptResult` holds `std::vector<std::string>`.
  [`NifFile.hpp:28-44`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/include/NifFile.hpp#L28-L44).
- **`NiVersion::SetFile` rebuilds the header version string, not just the number.** That string is
  written into the output through `hdr.SetVersion(options.targetVersion)`, so the shim must call
  `SetFile`, `SetStream`, and `SetUser` exactly as CAO does.
  [`BasicTypes.cpp:30-53`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/BasicTypes.cpp#L30-L53).
  `IsSK()` is `file == V20_2_0_7 && stream == 83`.
  [`BasicTypes.hpp:139`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/include/BasicTypes.hpp#L139).
- **`GetTexturePathRefs` returns `std::vector<std::reference_wrapper<std::string>>`.** The
  references point into texture-set, effect-shader, and `NiSourceTexture` blocks. A shared texture
  set is visited once per shape, so a reference can repeat. CAO's `.tga`→`.dds` rewrite is
  idempotent, so the repeat is harmless.
  [`NifFile.cpp:808-850`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/NifFile.cpp#L808).
  `GetShapes` returns `std::vector<NiShape*>`.
  [`NifFile.cpp:2083-2091`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/NifFile.cpp#L2083-L2091).
- **NIF strings are raw bytes in an unspecified codepage.** They are read into a 2049-byte buffer and
  truncated at the first NUL.
  [`BasicTypes.cpp:55-88`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/BasicTypes.cpp#L55-L88).
- **nifly never `throw`s.** `grep throw` over `src/` and `include/` finds nothing. The library still
  makes standard-library allocations sized by counts read from the file, for example
  `blockTypes.resize(numBlockTypes)`
  ([`BasicTypes.cpp:657-661`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/BasicTypes.cpp#L657-L661)),
  so `std::bad_alloc` and `std::length_error` are reachable on corrupt input.
- **nifly's only mutable global is the block factory registry.** It is a function-local static
  ([`Factory.cpp:20-21`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/Factory.cpp#L20-L21))
  and is read-only after construction. The other file-scope statics are `const std::string`. There
  are no `thread_local`s.
- **nifly's own build is plain.** It is a single `STATIC` library of 13 sources with no generated
  headers and no configure step. It builds with C++17, `/EHsc /bigobj` (public), and `/Zc:inline`
  (private).
  [`CMakeLists.txt:4`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/CMakeLists.txt#L4),
  [`src/CMakeLists.txt:42-63`](https://github.com/ousnius/nifly/blob/5504832da8009248ff68a9d306973ee1ba61a0a4/src/CMakeLists.txt#L42-L63).
  The sources total 368 KB in `src/`, 344 KB in `include/`, and 228 KB in `external/`
  (`half.hpp`, `Miniball.hpp`).
- **The patch is a backport of upstream
  [`ousnius/nifly@3fbe3a0`](https://github.com/ousnius/nifly/commit/3fbe3a0bfc5de0ff86bfc6190c3c4ee6ac189cf7)**
  (2023-06-08). Upstream `main` is 102 commits ahead of the pin, so bumping the pin to drop the
  patch would change mesh behaviour. Do not bump it during the port.

## `cxx` vs a plain C ABI shim

Neither option binds nifly directly. Several nifly signatures cannot cross either boundary as
written: `Load(const std::filesystem::path&, const NifLoadOptions&)`, `OptimizeFor(OptOptions&)`
(a non-POD struct holding a `std::string`), `std::vector<NiShape*>`, and
`std::vector<std::reference_wrapper<std::string>>`. cxx supports only the types in its table
(`String`, `&str`, slices, `CxxString`, `UniquePtr`, `CxxVector<T>` by reference, raw pointers, and
`Result`). It has no `std::filesystem::path` and no `reference_wrapper`
([cxx 1.0.202 crate docs, builtin types table](https://docs.rs/cxx/1.0.202/cxx/);
[CxxVector restrictions](https://cxx.rs/binding/cxxvector.html)). **Both routes therefore need a
hand-written C++ wrapper of about the same size.** The question is only which boundary that
wrapper exposes.

| | Plain C ABI (`extern "C"` + `unsafe extern "C"` in Rust) | `cxx` bridge |
|---|---|---|
| Extra build pieces | None beyond `cc` | `cxx` runtime crate, `cxx-build` code generator, `rust/cxx.h` |
| Exceptions | Hand-written `try`/`catch (...)` in each entry point (one template helper) | `Result<T>` return converts `std::exception` to `cxx::Exception`. Non-`std::exception` throws and functions not declared `Result` call `std::terminate` ([cxx book: Result](https://cxx.rs/binding/result.html)) |
| Ownership | Opaque pointer plus Rust `Drop` | `UniquePtr<NifHandle>` |
| Strings | `(*const u8, usize)` bytes and `(*const u16, usize)` paths | `&CxxString` / `&[u8]` / `&[u16]` |
| Unsafe Rust | About a dozen `unsafe` calls hidden behind one safe type | Mostly safe at the call site |
| Precedent in the port's stack | `directxtex` 1.3.0 uses exactly this: `ffi/main.cpp` with `extern "C"` `noexcept` functions and `unsafe extern "C"` in `src/ffi.rs` | — |

**Recommendation: plain C ABI.** With about 15 entry points, the shim is small enough that cxx's
safety and exception conversion save little. Its codegen step and `cxx.h` would be a second interop
mechanism next to `directxtex`'s C ABI. A C ABI also keeps exception policy fully in our hands:
`catch (...)` instead of cxx's default `std::exception`-only `trycatch`. cxx remains a reasonable
fallback if the surface grows a lot. Nothing in this design blocks switching later.

### Rule out `autocxx`

- **It is archived and unmaintained.** `google/autocxx` is `archived: true` on GitHub. Its README
  says "Autocxx is no longer maintained by Google" and points new users to cxx or crubit
  (`gh api repos/google/autocxx`, checked 2026-10-06). The published manual carries the same notice
  ([manual](https://google.github.io/autocxx/workflow.html)).
- **It needs libclang at build time.** autocxx 0.30.0 depends on `autocxx-bindgen` 0.73.0, which
  depends on `clang-sys` (features `clang_11_0`, optional `runtime`/`static`)
  (`autocxx-bindgen-0.73.0/Cargo.toml`). That adds an LLVM install to every build machine, a new
  system dependency the port does not otherwise need.
- **It would not remove the wrapper.** It still sits on cxx's type model, so `std::filesystem::path`
  parameters and `reference_wrapper` vectors still need hand-written shims. Overloads become `Load`,
  `Load1`, and so on, and default parameters are unsupported
  ([manual: C++ functions](https://google.github.io/autocxx/cpp_functions.html)). It would parse
  the whole nifly header graph to bind about six methods.

## Building nifly from `build.rs`

### Recommended: `cc` crate, vendored sources

```rust
// crates/nifly-sys/build.rs (sketch)
fn main() {
    let root = std::path::Path::new("vendor/nifly"); // pinned 5504832 + patch, see VENDORED note
    let sources = ["Animation", "BasicTypes", "bhk", "ExtraData", "Factory", "Geometry",
                   "NifFile", "Nodes", "Objects", "Particles", "Shaders", "Skin", "Object3d"];
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++17")                       // nifly's declared standard (CMakeLists.txt:4)
        .warnings(false)
        .include(root.join("include"))
        .include(root.join("external"))
        .files(sources.iter().map(|s| root.join("src").join(format!("{s}.cpp"))))
        .file("shim/cao_nif.cpp")
        // cc passes no exception model by default; without /EHsc MSVC gives no unwind
        // semantics, and the shim's try/catch would not run destructors (C4530).
        .flag("/EHsc").flag("/bigobj").flag("/Zc:inline"); // mirror nifly's CMake options
    b.compile("nifly_cao");
    println!("cargo:rerun-if-changed=shim/cao_nif.cpp");
    println!("cargo:rerun-if-changed=vendor/nifly");
}
```

Facts behind this:

- **`cc` matches the CRT to Rust's.** It passes `-MD`, or `-MT` with `crt-static`, from the
  target's `crt-static` feature, in debug and release alike, never `-MDd`
  (`cc-1.6.0/src/lib.rs:2526-2535`). The prototype's cl.exe command line showed `-MD` in a `dev`
  build. That matches Rust's MSVC default.
- **`cc` adds no `-std` or `/EH` flags unless asked** (`cc-1.6.0/src/lib.rs:980-992` for `std`).
  `grep EHsc` finds nothing in `cc`. Pass `/EHsc` explicitly. Microsoft's docs say that without it
  objects are not unwound
  ([C4530](https://learn.microsoft.com/cpp/error-messages/compiler-warnings/compiler-warning-level-1-c4530)),
  and that `/EHc` makes the compiler assume `extern "C"` functions never throw
  ([/EH](https://learn.microsoft.com/cpp/build/reference/eh-exception-handling-model#arguments)).
  That assumption is true for our shim because every entry point catches everything.
- **Build time is small.** A full nifly and shim compile took 24.5 s in debug and 37 s in release on
  this machine, without `cc`'s `parallel` feature.
- **Precedent.** `directxtex` 1.3.0's `build.rs` builds DirectXTex the same way: `cc::Build`,
  `.cpp(true).std("c++17")`, an explicit file list, and vendored sources under `external/`.

**Applying the patch.** Vendor `src/`, `include/`, `external/`, and `LICENSE` from the pinned
commit (about 940 KB) into the Cargo workspace. Apply `fix-const-matrix-equality.patch` once at
vendoring time with `git apply`. Keep the `.patch` file and a short note recording the upstream SHA
next to the vendored tree. This needs no network access in `build.rs`, no patch tool at build time,
and no submodule. Verified:

- The patch applies cleanly to `5504832` (`git apply --check`).
- **Under `-std:c++20` the unpatched headers fail** with `C2666 'nifly::Matrix3::operator ==':
  overloaded functions have similar conversions` and the same error for `Matrix4`
  (`Object3d.hpp(506)`, `(708)`). This is the C++20 reversed-candidate rule that upstream `3fbe3a0`
  fixes.
- **Under `-std:c++17` the unpatched tree compiles.** The output was byte-identical to nifly's
  expected test files.
- **C++20 with the patch also compiles and is byte-identical.**

The patch is therefore required only if anything compiles nifly headers as C++20. C++ CAO does
(`src/CMakeLists.txt` uses `cxx_std_20`). Applying it anyway costs nothing and keeps the vendored
source identical to what the oracle's vcpkg overlay builds.

### Alternatives considered

- **`cmake` crate driving nifly's own CMakeLists.txt.** This works. A configure-only check with the
  flags the `cmake` crate injects (`CMAKE_CXX_FLAGS_DEBUG=-nologo -MD -Z7`) produced
  `<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>` for every configuration, so there is no
  `/MDd` mismatch. The crate maps `opt-level=0` to `CMAKE_BUILD_TYPE=Debug` and passes `/MD` or
  `/MT` via `CMAKE_<LANG>_FLAGS_<CONFIG>` for the Visual Studio generator
  (`cmake-0.1.58/src/lib.rs:110-116`, `:745-767`). It still adds a CMake install requirement, a
  configure step, and an install-tree layout, all to build 13 files with no configuration. It also
  needs a separate mechanism to patch sources. Not worth it.
- **vcpkg (the existing overlay port).** The C++ oracle already uses it, but it would keep vcpkg
  bootstrapping as a Rust build prerequisite after the C++ tree is deleted, solely for nifly. Not
  recommended.
- **git submodule plus a build-time patch.** This needs `--recursive` clones and either a patch
  crate or copying headers into `OUT_DIR`. Vendoring is simpler.

## Error and exception handling across the boundary

- **Rust side.** Unwinding into Rust from a function declared `extern "C"` is undefined behaviour
  ("Causing an unwind into Rust code from a foreign function that was called via a function
  declaration … with a non-unwinding ABI, such as `"C"` … For example, this case occurs when such a
  function written in C++ throws an exception that is uncaught and propagates to Rust";
  [Rust Reference: panic, unwinding across FFI boundaries](https://doc.rust-lang.org/reference/panic.html)).
  `"C-unwind"` exists, but `catch_unwind` on a foreign exception either aborts or returns an opaque
  error, unspecified which (same page). So **no C++ exception may leave the shim.**
- **Shim side.** Every entry point is `noexcept` and wraps its body in
  `try { … } catch (const std::exception& e) { store e.what(); return -1; } catch (...) { return -1; }`.
  The message is kept on the handle and exposed through `cao_nif_last_error`. The handle constructor
  uses `new (std::nothrow)`.
- **Mapping to CAO semantics.** Return codes map to the outcomes `AssetExecutor` already
  distinguishes:
  - Load codes 1, 2, and 3 → `LoadFailed`, "Failed to load Mesh."
  - Save code 1 → `SaveFailed`.
  - Shim code `-1` (a C++ exception) → `BackendException`, which C++ CAO reaches through
    `catch (const std::exception&)` (`src/AssetExecution/AssetExecutor.cpp:558`).

  Exceptions are non-fatal per asset, as they are today.
- **Things the shim cannot catch.** Access violations from out-of-bounds reads on malformed files
  are SEH, not C++ exceptions, under `/EHsc`
  ([Handle structured exceptions in C++](https://learn.microsoft.com/cpp/cpp/exception-handling-differences)).
  C++ CAO has the same exposure. Equivalent behaviour means not adding SEH translation. That would
  be a new feature.
- **Corrupt-input probe.** In the prototype, overwriting 4 header bytes with `0x7FFFFFFF` produced no
  C++ exception in the sampled offsets. It did produce **loads that took 40 to 126 seconds and
  returned 0 ("success")** at offsets 35, 43-44, and 130 of a valid LE file. The probe was
  time-boxed and is not exhaustive. A nifly call cannot be interrupted, so cancellation waits for it
  (see Risks).

## Path handling (`std::u16string` today)

- **Rust → shim.** Pass `path.as_os_str().encode_wide()` as `(*const u16, usize)`. On Windows,
  `encode_wide` yields "potentially ill-formed UTF-16" (WTF-16), so unpaired surrogates survive
  ([`OsStrExt::encode_wide`](https://doc.rust-lang.org/std/os/windows/ffi/trait.OsStrExt.html#tymethod.encode_wide)).
- **Shim → nifly.** Build `std::filesystem::path(std::wstring(reinterpret_cast<const wchar_t*>(p), n))`.
  `wchar_t` is 16-bit UTF-16 on Windows, so this is a pass-through with no codepage conversion. It is
  at least as faithful as CAO's `QString::toStdU16String()` → implicit `path` conversion.
- **Verified.** The prototype loaded from and saved to `...\out\anim_ñ_日本.nif`.
- **Long paths.** nifly opens files with `std::ifstream` / `std::ofstream`. MAX_PATH behaviour is
  whatever MSVC's iostreams give for the path string passed. Rust's `std::fs::canonicalize` returns
  `\\?\` verbatim paths, so if the Rust run code uses canonical paths, those reach nifly too.
  Test long and verbatim paths in the parity corpus rather than assuming.
- **Texture strings are bytes.** Expose them as `&[u8]` and set them from `&[u8]`. Do the `.tga`
  match and replace over bytes, which matches CAO's ASCII `tolower` comparison
  (`src/MeshReferenceMaintenance.cpp:14-40`). Decode lossily only for the eligibility comparison and
  for logs. C++ CAO decodes with `QString::fromStdString`, which is UTF-8, in `MainOptimizer.cpp`.

## Thread safety

- **C++ CAO processes meshes sequentially.** There is one Run Worker per run, a `std::jthread`
  (`src/Run/RunScheduler.cpp:28-55`), and one loaded mesh at a time
  (`MainOptimizer::_loadedMesh`). The map's posture is "port it simply", so keep it that way.
  No parallel mesh processing is needed for parity.
- **If parallelism were wanted later:**
  - Distinct `NifFile` objects share only the factory registry, a function-local static.
  - MSVC initialises function-local statics thread-safely by default (`/Zc:threadSafeInit`, on since
    VS2015; [docs](https://learn.microsoft.com/cpp/build/reference/zc-threadsafeinit-thread-safe-local-static-initialization)).
  - After initialisation the registry is only read (`GetFactoryByName`), so separate handles on
    separate threads are safe.
  - A single `NifFile` has no internal synchronisation.
- **Rust typing.** `unsafe impl Send for Nif {}`, because the handle owns its data and has no thread
  affinity. Do **not** implement `Sync`. Every operation, including the snapshot-building texture
  enumeration, mutates shim-side state.

## Minimal shim API (sketch)

C header (the prototype's version, verified working):

```c
typedef struct CaoNif CaoNif;              // owns nifly::NifFile + last OptResult + texture snapshot

CaoNif*  cao_nif_new(void);                // nullptr on OOM
void     cao_nif_free(CaoNif*);

// Return codes: 0 ok; >0 nifly's own code; -1 C++ exception (see cao_nif_last_error); -2 bad arg.
int32_t  cao_nif_load(CaoNif*, const uint16_t* path, size_t len, bool is_terrain); // 0/1/2/3
int32_t  cao_nif_save(CaoNif*, const uint16_t* path, size_t len);                  // 0/1
bool     cao_nif_is_valid(const CaoNif*);
int32_t  cao_nif_is_sse_compatible(CaoNif*, bool* out);

// Builds NiVersion with SetFile/SetStream/SetUser, as MeshesOptimizer does.
int32_t  cao_nif_optimize_for(CaoNif*, uint32_t file, uint32_t user, uint32_t stream,
                              bool head_parts, bool remove_parallax);
uint32_t cao_nif_opt_flags(const CaoNif*);              // bit0 versionMismatch, bit1 dupesRenamed
size_t   cao_nif_opt_name_count(const CaoNif*, int32_t field); // 0 vcolors,1 normals,2 parttri,
bool     cao_nif_opt_name(const CaoNif*, int32_t field, size_t i, // 3 tangents,4 parallax
                          const char** ptr, size_t* len);         // borrowed until next mutation

// Texture references: snapshot of GetShapes()×GetTexturePathRefs(), in nifly's order.
int32_t  cao_nif_texture_count(CaoNif*, size_t* out);  // (re)builds the snapshot
bool     cao_nif_texture_get(const CaoNif*, size_t i, const char** ptr, size_t* len);
int32_t  cao_nif_texture_set(CaoNif*, size_t i, const char* ptr, size_t len); // in-place assign

size_t   cao_nif_last_error(const CaoNif*, const char** ptr);
```

Design notes:

- **Snapshot invalidation.** Load, optimize, and save reallocate blocks, so they invalidate the
  texture snapshot. Get and set check validity, and an out-of-range or stale index returns
  `false` / `-2` instead of dereferencing.
- **Live references.** The snapshot holds the same `reference_wrapper`s CAO iterates today, so a
  repeated (shared) reference sees earlier writes.
- **TGA logic moves to Rust.** The `.tga` detection, eligibility filtering, and replacement in
  `MeshReferenceMaintenance.cpp` become plain Rust over `count`/`get`/`set`. No callbacks cross the
  boundary.
- **`IsSK` on the target version** is two integer comparisons. It can live in Rust as
  `file == 0x14020007 && stream == 83`, citing `BasicTypes.hpp:139`, or be a shim call. Either is
  equivalent.
- **OptResult logging.** Collect the five name lists into `Vec<String>` (lossy) to reproduce the
  `PLOGV` "Details of mesh optimization" block.
- **Test support.** The C++ tests build meshes in memory with `Create(NiVersion::getSSE())`,
  `CreateShapeFromData`, and `SetTextureSlot`, then read back with `GetTextureSlot`. If those
  scenarios are ported, add one test-only entry point such as
  `cao_nif_create_textured_sse(CaoNif*, const char* tex, size_t len)` behind a Cargo feature, or
  check in small fixture NIFs. `GetTextureSlot` is not needed: `texture_get(0)` reads the same
  slot-0 string.

The safe Rust wrapper is one type, `Nif`, holding `NonNull<CaoNif>`, with `Drop` calling
`cao_nif_free`, `unsafe impl Send`, and these methods:

- `load(&mut self, &Path, terrain) -> Result<(), NifError>`
- `save(&mut self, &Path) -> Result<(), NifError>`
- `is_valid(&self) -> bool`
- `is_sse_compatible(&mut self) -> Result<bool, NifError>`
- `optimize_for(&mut self, NifVersion, head_parts, remove_parallax) -> Result<OptReport, NifError>`
- `texture_paths(&mut self) -> Result<TexturePaths<'_>, NifError>`, a borrow that offers `len`,
  `get(i) -> &[u8]`, and `set(i, &[u8])`

`NifError` carries `LoadCode(1|2|3)`, `SaveFailed`, or `Exception(String)`.

## Risks and open items

- **Cancellation granularity.** A nifly call cannot be interrupted, and corrupt files can make
  `Load` run for minutes (probe above). Run Handle drop-cancels-and-waits therefore waits for the
  current mesh call, exactly as in C++ CAO. This matters for the threading mapping still open on
  #458 and for #468.
- **`Save` does not detect write errors.** CAO relies on the following staged-file fingerprint and
  publication checks. The port should keep that order. Adding a stream-state check in the shim would
  be a small behavioural improvement, so record it on the deviation list if adopted.
- **Floating-point equivalence.** Two compiler configurations build nifly: the oracle's vcpkg
  CMake Release and the port's `cc` with Cargo's opt-level. Output was byte-identical on nifly's two
  LE→SSE fixtures, but `OptimizeFor` recomputes tangents, normals, and bounds in `float`, and other
  inputs could differ in the last ulp. #467's mesh equivalence rule should allow that, or the parity
  harness should confirm byte equality on the corpus first.
- **Pin and patch drift.** Upstream is 102 commits ahead. Keep the pin for the port. The patch is
  needed only for C++20 compilation of nifly headers.
- **Behaviour to port as-is, not "fix":**
  - `scan()` tests `IsSK()` on the profile's target version. Every valid mesh is therefore a
    `criticalIssue` under the TES5 profile.
  - `OptimizeFor` is a silent no-op (`versionMismatch`) for FO4 targets, yet the mesh may still be
    reported as changed and resaved.
  - `versionMismatch` is not logged.

  These look deliberate or harmless. They are not on #458's deviation list, so port them as they
  are.

## Sources

- **nifly source at the pin:** https://github.com/ousnius/nifly/tree/5504832da8009248ff68a9d306973ee1ba61a0a4.
  Files: `include/NifFile.hpp`, `include/BasicTypes.hpp`, `include/Factory.hpp`, `src/NifFile.cpp`,
  `src/BasicTypes.cpp`, `src/Factory.cpp`, `CMakeLists.txt`, `src/CMakeLists.txt`, and `tests/`
  (fixtures and `TestNifFile.cpp:190-216`).
- **Upstream fix backported by the overlay patch:**
  https://github.com/ousnius/nifly/commit/3fbe3a0bfc5de0ff86bfc6190c3c4ee6ac189cf7
- **Local:**
  - `cmake/ports/nifly/portfile.cmake`, `cmake/ports/nifly/fix-const-matrix-equality.patch`
  - `src/MeshesOptimizer.cpp`, `src/MeshReferenceMaintenance.cpp`, `src/MainOptimizer.cpp`,
    `src/AssetExecution/AssetExecutor.cpp`, `src/Run/RunScheduler.cpp`, `src/CMakeLists.txt`
  - `tests/MeshReferenceMaintenanceTests.cpp`, `tests/MainOptimizerTests.cpp`
- **cxx 1.0.202:** crate source (`src/lib.rs` type table, `book/src/binding/result.md`,
  `book/src/binding/cxxvector.md`). Online: https://cxx.rs/binding/result.html,
  https://cxx.rs/binding/cxxvector.html
- **autocxx 0.30.0 / autocxx-bindgen 0.73.0:** crate `Cargo.toml`; GitHub repo metadata
  (`archived: true`) and README; https://google.github.io/autocxx/
- **cc 1.6.0 and cmake 0.1.58:** crate source (`src/lib.rs`, lines cited inline).
- **directxtex 1.3.0:** crate source (`build.rs`, `ffi/main.cpp`, `src/ffi.rs`).
- **Rust Reference:** https://doc.rust-lang.org/reference/panic.html (unwinding across FFI) and
  https://doc.rust-lang.org/reference/items/functions.html (ABI unwinding table).
- **Rust std:** `OsStrExt::encode_wide` docs.
- **Microsoft Learn:** `/EH`, C4530, `/Zc:threadSafeInit`, and structured exceptions in C++ (linked
  inline).
- **Prototype:** throwaway Cargo crate in the system temp directory, not committed. It was built
  and run on 2026-10-06 with rustc 1.99.0, MSVC 14.51.36231, cc 1.6.0, and CMake 4.4.3 for the
  configure-only check.
