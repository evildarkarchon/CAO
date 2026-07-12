# NIF mesh backend strategy

Research date: 2026-07-12

## Decision

Use [`ousnius/nifly` at commit `965a1da1be7bff145b7b3435def5c04d6e8c8cce`](https://github.com/ousnius/nifly/tree/965a1da1be7bff145b7b3435def5c04d6e8c8cce) as the NIF engine, compiled from source and reached only through a small CAO-owned C ABI. Pin that full commit in the build; do not track `main`, use the repository's `NIFLY_VERSION`, or invent a semver mapping.

Keep traversal, profile interpretation, headpart detection, optimization-level policy, dry-run decisions, logging, cancellation, transactionality, and discrepancy policy in Rust. The native adapter should accept NIF bytes plus explicit options, catch every C++ exception, return owned bytes and structured results, and expose no nifly or C++ standard-library type.

This is intentionally a narrow C++ FFI choice rather than a Rust-native choice. No maintained Rust-native library found in the official crate and repository metadata covers CAO's required combination of classic Skyrim and Skyrim SE conversion, Fallout 4 read/resave, BTO/BTR terrain handling, texture-reference mutation, and full NIF serialization. The closest Rust crate is read-only and targets a different NIF version.

The selected pin is newer than the C++ application's [`5504832da8009248ff68a9d306973ee1ba61a0a4`](../../cmake/ports/nifly/portfile.cmake) pin. That is acceptable because the migration contract permits a newer backend, and the current revision includes subsequent validation and format fixes. The mandatory parity gates below still decide whether each observable difference is accepted.

## Why nifly fits the actual CAO behavior

CAO does more than inspect geometry. Its current mesh path:

- loads `.nif`, `.bto`, and `.btr`, setting terrain mode for the latter two;
- rejects invalid files and classifies Skyrim SE compatibility;
- converts between classic Skyrim (file `20.2.0.7`, user `12`, stream `83`) and Skyrim SE (stream `100`);
- gives headparts and facegen meshes special conversion treatment;
- deliberately preserves parallax;
- rewrites referenced `.tga` texture paths to `.dds` when profile policy enables it;
- resaves according to optimization level even when no `OptimizeFor` call occurs; and
- can load Fallout 4 stream `130`, although the shipped FO4 profile disables mesh processing by default.

Those behaviors are visible in [`MeshesOptimizer.cpp`](../../src/MeshesOptimizer.cpp) and the shipped [`TES5`](../../profiles/TES5/profile.ini), [`SSE`](../../profiles/SSE/profile.ini), and [`FO4`](../../profiles/FO4/profile.ini) profiles. nifly directly exposes the matching primitives: stream and path loading, `NifLoadOptions::isTerrain`, `IsSSECompatible`, `OptimizeFor`, texture-path references, and stream/path saving in [`NifFile.hpp`](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/include/NifFile.hpp#L27-L127). Its own README claims read/write support for Fallout 4, classic Skyrim, and Skyrim SE and preservation of unknown blocks ([upstream README](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/README.md)).

The library's `OptimizeFor` implementation is explicitly an LE-to-SE or SE-to-LE converter. Other source/target combinations return `versionMismatch` without conversion ([implementation](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/src/NifFile.cpp#L1518-L1531)). Therefore Fallout 4 support in the initial port means inspection, texture-reference editing, and controlled resave—not a newly invented FO4 optimizer. CAO currently ignores the returned `versionMismatch`; the Rust policy layer must preserve that observable outcome until characterization approves a correction.

Upstream tests contain binary round-trip fixtures for static and skinned Fallout 4 NIFs, LE/SE conversions, static and skinned Skyrim files, unknown/loose blocks, and a corrupted-input rejection ([tests](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/tests/TestNifFile.cpp)). The selected commit's Windows, Ubuntu, and macOS CI jobs passed ([workflow run](https://github.com/ousnius/nifly/actions/runs/28531269696)). These tests are useful evidence, but they do not replace CAO's corpus.

## Pin choice and version ambiguity

### Selected: current revision `965a1da1…`

Pin the research-date `main` revision exactly:

```text
965a1da1be7bff145b7b3435def5c04d6e8c8cce
```

Reasons:

- Upstream remains active; the selected revision was committed on 2026-07-01 and passed the repository's three-platform build/test matrix.
- Since the legacy pin, upstream added bounds/index validation, corrupt-file exception behavior, Fallout format fixes, texture-path fixes, and regression fixtures. For example, issue 55 documents a corrupt NIF that formerly hung and leaked; the fix adds checked string indices, block indices, and array sizes and a throwing regression test ([issue 55](https://github.com/ousnius/nifly/issues/55), [validation code](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/include/BasicTypes.hpp#L804-L882)).
- The user accepts a newer nifly revision, so recreating an uncertain historical binary dependency would add work without making the legacy executable any less necessary as the behavioral oracle.

### Legacy pin: oracle, not production dependency

The repository's vcpkg overlay labels nifly `1.0.3` and pins `5504832da8009248ff68a9d306973ee1ba61a0a4`, a 2022-06-10 commit ([port manifest](../../cmake/ports/nifly/vcpkg.json), [commit](https://github.com/ousnius/nifly/commit/5504832da8009248ff68a9d306973ee1ba61a0a4)). Preserve a build of that revision only when useful for differential diagnosis. It should not constrain the new production dependency.

The v5.3.15 release material's “nifly 3.2.0” wording is not a resolvable upstream library version. The nifly repository has no releases or matching tag, and its CMake project still declares `NIFLY_VERSION 1.0.0` at the selected revision ([CMakeLists](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/CMakeLists.txt#L12)). Separately, SSE NIF Optimizer used a `3.2.0` product version and now reports `3.2.2`, so the number may refer to the optimizer rather than a library release ([SSE NIF Optimizer history](https://github.com/ousnius/SSE-NIF-Optimizer/commits/main/)). Treat it as a provenance clue, not a dependency coordinate. The authenticated v5.3.15 executable remains the authority for released behavior.

## Recommended boundary

Use two internal layers:

```text
Rust MeshService / MeshPolicy
        |
        | safe Rust request/result types
        v
Rust NiflyBackend
        |
        | CAO-owned C ABI; byte buffers, integers, explicit ownership
        v
C++ cao_nif_bridge + statically linked pinned nifly
```

The C ABI should be small and operation-oriented, for example:

```c
cao_nif_result cao_nif_inspect(const uint8_t *input, size_t len,
                               bool terrain);

cao_nif_result cao_nif_transform(const uint8_t *input, size_t len,
                                 const cao_nif_transform_options *options);

void cao_nif_result_free(cao_nif_result *result);
```

`cao_nif_transform_options` should carry target file/user/stream versions, terrain mode, headpart mode, `remove_parallax`, `calc_bounds`, save optimization, save sorting, and the requested texture-extension rewrite. The result should carry a stable error code, UTF-8 diagnostic text, structured optimization counters/names, whether a version mismatch occurred, whether texture references changed, and an output byte buffer when successful.

Why bytes rather than paths:

- Rust retains Windows path handling and atomic replacement.
- The adapter avoids repeating the legacy non-ASCII path regression.
- `NifFile` already supports `std::istream`/`std::ostream` ([API](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/include/NifFile.hpp#L92-L95)).
- The ABI never depends on `std::filesystem::path`, allocator identity, exception ABI, or Rust/C++ string encoding.

The wrapper must catch `std::exception` and unknown exceptions at every exported entry point. No exception, `std::string`, `std::vector`, reference wrapper, `NifFile*`, or allocator-owned memory may cross the ABI. Rust must copy returned bytes and call the paired free function. Handles are unnecessary for CAO's one-file-at-a-time sequential flow.

Rust owns same-directory temporary output, reopen/verification, atomic replacement, backup behavior, cancellation between files/phases, and preservation of the original on failure. A native crash cannot be caught by Rust; this residual risk is covered by the malformed corpus and subprocess contingency.

## Behavior that must remain CAO policy

| Behavior | Owner | Compatibility note |
| --- | --- | --- |
| File discovery and extension matching | Rust | Include `.nif`, `.bto`, `.btr`; preserve case-insensitive Windows behavior. |
| Terrain selection | Rust request, native execution | Must be true for BTO/BTR because it changes texture cleanup and duplicate-shape behavior. |
| Profile target version | Rust | Preserve custom profile values rather than infer from extension. |
| Scan classification | Rust over native inspection | Preserve `doNotProcess`, `good`, `lightIssue`, and `criticalIssue` outcomes until simplified by an approved discrepancy. |
| Optimization levels | Rust | Level 2 can cause a resave without `OptimizeFor`; do not collapse levels into a boolean. |
| Headpart/facegen selection | Rust | Preserve plugin-derived and `customHeadparts.txt` matching. |
| LE/SE conversion | nifly | Pass explicit target and `headParts`; keep `removeParallax=false`. |
| FO4 behavior | Rust plus nifly read/write | Do not claim `OptimizeFor` support; shipped FO4 mesh processing is disabled. |
| `.tga` to `.dds` reference rewrite | Native mutation under Rust policy | Match case-insensitive substring replacement across every returned texture reference. |
| Dry run and logging | Rust | Dry run must not load a different policy path or write output. |
| Save settings and transaction | Rust request plus Rust filesystem layer | Make nifly defaults explicit; never rely on changing upstream defaults. |

## Known regression hazards and legacy quirks

### Terrain mode is semantic

CAO added `isTerrain` after a specific 2021 bug fix and passes it only for `.btr`/`.bto` ([CAO commit](https://github.com/evildarkarchon/CAO/commit/965a9751702c0d09571ebafcf495c061db4fbddf)). nifly uses the flag when cleaning terrain texture paths and skips duplicate-shape renaming during conversion. Omitting it can silently change shape names or texture references, not merely loading performance.

### Non-ASCII paths previously failed

CAO changed load/save from narrow strings to UTF-16 after a non-ASCII filename defect ([CAO commit](https://github.com/evildarkarchon/CAO/commit/ce58405a)). The byte-buffer ABI avoids reintroducing the problem. The Rust file layer must still test Unicode, extended-length, and mixed-case Windows paths.

### Parallax removal is intentionally disabled

nifly defaults `removeParallax` to true, while CAO explicitly sets it false ([CAO source](../../src/MeshesOptimizer.cpp), [nifly option](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/include/NifFile.hpp#L28-L33)). Missing that one option changes shaders, texture slots, and sometimes vertex-color handling.

### Resave can make meshes invisible

CAO disabled resaving by default in 2019 after reports of meshes becoming invisible ([CAO commit](https://github.com/evildarkarchon/CAO/commit/606c6ae3)). nifly's save defaults still run `Optimize`, which recalculates every shape's bounds and deletes unreferenced blocks ([save defaults](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/include/NifFile.hpp#L59-L61), [implementation](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/src/NifFile.cpp#L1465-L1516)). Upstream issue 51 confirms overly tight recalculated bounds can cause culling and notes that SSE NIF Optimizer saves with both `optimize=false` and `sortBlocks=false` ([issue 51](https://github.com/ousnius/nifly/issues/51), [optimizer source](https://github.com/ousnius/SSE-NIF-Optimizer/blob/dbba8b359531fa4a4b89d906b93cdcb579abc8d0/src/Optimizer.cpp#L248-L255)).

The adapter must expose both save flags. Start characterization with legacy-equivalent defaults (`true`, `true`), but test the safer upstream-tool settings as a proposed confirmed-defect correction. Do not silently switch: invisible-mesh prevention is desirable, but changed bounds, block order, and unreferenced-block retention require a discrepancy entry and maintainer approval.

### Optimization levels contain a resave-only branch

For otherwise compatible meshes, level 3 calls `OptimizeFor`; level 2 does not, but still sets `modifiedMesh` and saves. Level 1 converts only critical files. Headparts can be converted independently. The new orchestrator must characterize this matrix directly rather than interpreting “medium” as a reduced native optimization call.

### Fallout 4 is inspection/resave only in scope

The shipped FO4 profile sets stream `130` but `meshesEnabled=false`. A custom profile can enable the code path; `OptimizeFor` then reports a version mismatch, which CAO logs only as part of the result and may still save based on level/resave policy. Preserve this quirk until a discrepancy explicitly defines a more useful warning or refusal.

### Unknown and malformed blocks need independent proof

The README says unknown blocks remain untouched, but save-time sorting/optimization and string-table updates can still affect a file. Current validation covers three classes of corrupt metadata, not arbitrary malicious input. Treat all NIF sizes, indices, strings, and graph depth as untrusted and bound work in the Rust layer where possible.

## Alternatives considered

### Rust crate `nif` 0.5.0

[`nif` 0.5.0](https://crates.io/crates/nif/0.5.0) is MIT-declared and Rust-native, but its own README calls it a “super-primitive” parser targeting Gamebryo `20.0.0.4` ([repository](https://github.com/amPerl/nif/tree/v0.5.0)). Its header parser asserts exactly `0x14000004`, and its public `Nif` type derives `BinRead` only ([header](https://github.com/amPerl/nif/blob/v0.5.0/src/header.rs), [library](https://github.com/amPerl/nif/blob/v0.5.0/src/lib.rs)). It cannot read CAO's `20.2.0.7` Bethesda targets, serialize modified NIFs, convert LE/SE geometry, or supply terrain/headpart optimization. Reject for production; it is not a near-term fallback.

### PyNifly / NiflyDLL

[`BadDogSkyrim/PyNifly`](https://github.com/BadDogSkyrim/PyNifly) is actively maintained, GPL-3.0, supports the scoped games, and contains a large exported C wrapper ([wrapper header](https://github.com/BadDogSkyrim/PyNifly/blob/V27.4.0/NiflyDLL/include/NiflyWrapper.hpp)). However, the wrapper is designed for Blender import/export, exposes a much broader mutation surface, accepts narrow `char*` paths, and does not export CAO's `OptimizeFor`, SSE-compatibility scan, or terrain-mode contract. Adopting it would add another abstraction and forked nifly dependency without satisfying the core operation. Its wrapper is useful design evidence, not the selected backend.

### niflib

[`niftools/niflib`](https://github.com/niftools/niflib) has a permissive BSD-style license and broad generated read/write machinery, but its official repository's last code commit is from 2021, its latest tag is old, and its API does not provide nifly's CAO-specific LE/SE optimizer. Reconstructing conversion, terrain cleanup, shader policy, and save semantics above it would be a new optimizer implementation. Reject.

### SSE NIF Optimizer subprocess

[`ousnius/SSE-NIF-Optimizer`](https://github.com/ousnius/SSE-NIF-Optimizer) is maintained, GPL-3.0, uses nifly, handles `.nif`/`.bto`/`.btr`, and provides strong differential evidence for LE/SE conversion and safe save settings ([optimizer source](https://github.com/ousnius/SSE-NIF-Optimizer/blob/dbba8b359531fa4a4b89d906b93cdcb579abc8d0/src/Optimizer.cpp)). It is a wxWidgets GUI application, not a stable headless protocol, and it does not own CAO's profile, headpart-list, dry-run, texture-rewrite, or FO4 behavior. Bundling it would duplicate GUI/runtime and license packaging while requiring fragile process control. Reserve a purpose-built helper subprocess using the same CAO C ABI implementation for crash isolation; do not bundle the upstream GUI as the default backend.

### NifSkope or a first-party parser

NifSkope is a broad interactive editor rather than a CAO conversion API. A new first-party parser/serializer would require hundreds of block layouts plus the subtle conversion behavior already in nifly. Both are valuable independent inspection oracles; neither is a credible initial production backend.

## Licensing and distribution

nifly is GPL-3.0-only ([license](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/LICENSE)) and builds as a static library ([CMake target](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/src/CMakeLists.txt#L43)). The C ABI is an engineering boundary, not a license boundary. Plan the linked executable and its Corresponding Source distribution as GPLv3.

CAO's MPL-2.0 source can be combined with GPLv3 unless a covered file is marked “Incompatible With Secondary Licenses.” Mozilla's official guidance says the resulting Larger Work is additionally distributed under GPL while the original MPL files remain available under MPL ([MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/#combining), [developer guidelines](https://www.mozilla.org/en-US/MPL/2.0/combining-mpl-and-gpl/)). The current source headers use the ordinary MPL notice, not Exhibit B. This is a distribution plan, not legal advice; complete a release license review before shipping.

For the self-contained Windows ZIP, the simplest compliance posture is to make the exact machine-readable Corresponding Source available alongside the same download. It should include:

- the pinned nifly source and license;
- the complete C ABI wrapper source;
- the Rust/Slint CAO source governed by the combined-work terms, while retaining MPL notices on existing MPL files;
- Cargo lockfiles, CMake/vcpkg configuration, build scripts, patches, and interface-generation inputs needed to reproduce the binary;
- copyright and third-party notices; and
- any Installation Information required by GPLv3 section 6 for the distributed User Product.

GPLv3 section 6 lists the permitted object-code/source delivery mechanisms and requires equivalent network access when that option is used ([license section 6](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/LICENSE#L245-L337)). A link to upstream `main` is insufficient because it is neither the exact pin nor the wrapper/application Corresponding Source. Do not add EULA or ZIP terms that restrict GPL rights.

## Mandatory parity gates

These gates block the functional-parity milestone and release, not the backend decision.

### 1. Native build, ABI, and package

- Pin the full selected SHA and record the fetched archive checksum.
- Build the real Windows x64 release graph with the documented MSVC toolset and Rust target.
- Run upstream nifly tests, then the CAO adapter tests, in CI.
- Verify exported symbols and ABI struct sizes; make every struct versioned and fixed-width.
- Test every exception path and allocator handoff under ASan where supported and Windows Application Verifier or equivalent native diagnostics.
- Inspect runtime dependencies and run the final ZIP on a clean supported Windows x64 VM.
- Produce the license/SBOM bundle and verify exact Corresponding Source availability.

### 2. Golden mesh corpus

Build a redistributable or locally supplied corpus with, at minimum:

- classic Skyrim stream 83 and Skyrim SE stream 100 static, skinned, animated, furniture/collision, tree, parallax, model-space-normal, vertex-color, headpart, facegen, and root-nonzero files;
- Fallout 4 stream 130 static and skinned files, including materials/texture references and files a custom profile attempts to resave;
- `.bto` and `.btr` terrain files with relative and non-relative texture paths and duplicate shape names;
- vanilla files, mod files, legacy CAO outputs, and files produced by current SSE NIF Optimizer/NifSkope where redistribution permits;
- unknown blocks, loose/unreferenced blocks, empty strings, duplicate names, extended-byte internal strings, and non-ASCII/long Windows file paths; and
- known invisible-tree and too-tight-bound fixtures from the relevant upstream/legacy regressions.

For each applicable fixture, inspect, perform each enabled CAO operation, serialize, reopen with the selected backend, and inspect with an independent tool. Test both conversion directions and unchanged-target resaves.

### 3. Behavioral matrix

Run the authenticated v5.3.15 executable and the Rust port across:

- optimization levels 0, 1, 2, and 3;
- resave on/off;
- headpart processing on/off with custom-list, plugin-derived, and facegen matches;
- TGA conversion policy on/off;
- `.nif`, `.bto`, and `.btr` extension casing;
- TES5, SSE, FO4, and representative existing custom profiles; and
- dry run versus real run.

Compare file eligibility, log-level-relevant events, whether a file is written, reported optimization details, failure continuation, and final output. Explicitly cover level-2 resave-only behavior and FO4 `versionMismatch` handling.

### 4. Semantic output validation

- Require target header file/user/stream values.
- Compare block types/counts and graph references, allowing only registered discrepancies.
- Compare shape type, name, vertex/index data, normals/tangents, UVs, colors, skin partitions/weights, collision graphs, shaders/flags, texture slots, animation references, and bounds.
- Require TGA-to-DDS edits only when policy enables them and preserve unrelated path bytes.
- Verify terrain shape names and texture paths separately.
- Load outputs in NifSkope and, for the highest-risk fixtures, in the target game or Creation Kit test harness.
- Use byte identity only for operations characterized as deterministic; semantic equivalence is authoritative otherwise.

### 5. Save-mode and invisible-mesh decision

- Run every conversion/resave fixture with `(optimize=true, sortBlocks=true)` and `(false, false)`.
- Record bound changes, block removal/order changes, file size, independent-tool loadability, and in-game culling.
- Default to legacy-equivalent settings until evidence supports a discrepancy.
- Require explicit maintainer approval before adopting the safer SSE NIF Optimizer save settings.
- Never overwrite the source until the temporary output reopens and passes structural checks.

### 6. Malformed-input and native-failure safety

- Test truncated headers/blocks, invalid string and block indices, excessive array counts, cyclic/deep graphs, invalid triangle/skin indices, bogus declared sizes, and corrupt compressed or external data where applicable.
- Fuzz the byte-buffer `inspect` and `transform` entry points with resource limits.
- Require a controlled Rust error, no exception across FFI, no leak, no hang, no source replacement, and no partial destination.
- If a redistributable valid NIF can terminate the process or reproducible malformed input escapes the exception boundary, the in-process release is blocked and the subprocess contingency is triggered.

### 7. Maintenance containment

- Keep all nifly references inside the C++ bridge and Rust backend crate/module.
- Record the pin, archive checksum, compiler versions, compile flags, patches, and upstream test result.
- Make upgrades a corpus-gated dependency change; never accept a moving branch.
- Retain the legacy pin and SSE NIF Optimizer as differential tools, not runtime dependencies.
- Document a fork procedure and time-box upstream issue resolution for release blockers.

## Contingency

First preference is a narrowly maintained CAO fork of nifly at the selected pin. Apply the smallest patch possible, link it through the unchanged C ABI, publish Corresponding Source, and rerun every gate.

If valid or adversarial inputs can still crash/abort the process and the problem cannot be fixed promptly, move the same one-shot byte-buffer bridge into a bundled CAO helper executable. The Rust service communicates through versioned length-delimited files or pipes, applies time/resource limits, and treats helper termination as a per-file failure. This preserves the Rust policy and test surface while isolating native failure. It adds process lifecycle, antivirus/signing, source-distribution, cancellation, and temporary-data burdens, so it is a contingency rather than the default.

Replacing nifly with a Rust-native backend becomes credible only when a library can pass the same corpus for full read/mutate/write and LE/SE conversion. The current `nif` crate does not meet that threshold.

## Final recommendation

Adopt current pinned nifly `965a1da1be7bff145b7b3435def5c04d6e8c8cce` through a CAO-owned, byte-buffer C ABI. This is the smallest backend that preserves CAO's actual LE/SE optimizer, terrain, texture-reference, and FO4 inspection/resave behavior. Keep all product policy and filesystem transactionality in safe Rust.

Do not treat the choice as unconditional trust: the invisible-mesh/save-default hazard, native exception/crash boundary, GPLv3 distribution, and uncertain legacy version labeling are material risks. The parity corpus and explicit save-mode decision are release gates. If the in-process engine cannot satisfy them, patch the pinned fork first and isolate the identical adapter in a helper process second.
