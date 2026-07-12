# HKX animation backend strategy

Research date: 2026-07-12

## Decision

Adopt [`SARDONYX-sard/serde-hkx` 1.0.1 at commit `6c1bee56d42de7def991cf6fba025a9df7492d83`](https://github.com/SARDONYX-sard/serde-hkx/tree/6c1bee56d42de7def991cf6fba025a9df7492d83) as the production HKX engine, called through an **in-process CAO-owned `HkxConverter`**. Pin the peeled commit and vendor its source; do not track `main` or rely only on the movable tag name.

Use the authenticated v5.3.15 `hkxcmd.exe` only as the behavioral oracle. Do not redistribute it, link Havok 2010, or bundle Bethesda's `HavokBehaviorPostProcess.exe` unless a separate written grant covers the exact artifact and use.

This changes the earlier conditional legacy-helper recommendation. `serde-hkx` is a current, pure-Rust, MIT-or-Apache-2.0 implementation that directly deserializes Havok 2010.2 packfiles and serializes Skyrim LE Win32 or Skyrim SE AMD64 packfiles. Its release README explicitly claims 32↔64 conversion, and its conversion code selects concrete LE and SE headers rather than shelling out to Havok ([README](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/README.md), [format serializer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/serde/ser.rs), [header constructors](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/bytes/serde/hkx_header.rs)). It is therefore the first investigated backend that can plausibly preserve CAO's operation without carrying Havok's redistribution problem.

`HkxConverter` should expose explicit `parse`, `serialize_amd64`, `verify`, and `commit` phases over owned byte buffers, checking cooperative cancellation between every phase. It must not use upstream's directory traversal, parallel conversion, or direct file writer.

Keep a CAO-owned one-file helper executable as the containment contingency, not the default. The upstream author documents an unresolved debug-build stack overflow, the release fixture set is small, and upstream's release-LTO CLI profile uses `panic = "abort"` ([README](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/README.md), [workspace profiles](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/Cargo.toml)). If CAO cannot eliminate/reproduce that overflow, bound stack and allocation use, and contain ordinary panics in the mandatory malformed corpus, switch the unchanged converter interface to the one-shot helper before release. A helper is a safety deployment mode for the same selected Rust engine, not a different backend.

## Required CAO behavior

The released source performs one operation: read an HKX and ask `hkxcmd` to write an AMD64 packfile. It does not edit animation content. Specifically:

- every selected `.hkx` is copied to fixed working-directory names;
- `bin/hkxcmd.exe convert <temp.hkx> -v AMD64` is launched;
- the process is waited on without cancellation or timeout;
- output containing `not loadable` is treated as “probably already converted”;
- apparent success deletes the original before renaming the output; and
- current code does not consult the profile's `animationFormat`.

Those behaviors are visible in [`AnimationsOptimizer.cpp`](../../src/AnimationsOptimizer.cpp) and [`MainOptimizer.cpp`](../../src/MainOptimizer.cpp). The shipped SSE profile enables animation processing, while TES5 and FO4 disable it ([SSE](../../profiles/SSE/profile.ini), [TES5](../../profiles/TES5/profile.ini), [FO4](../../profiles/FO4/profile.ini)). A custom profile can enable the same unconditional AMD64 conversion regardless of its declared animation format.

The new backend should preserve successful LE-to-AMD64 semantics, already-AMD64 no-op behavior, dry-run behavior, file eligibility, sequential orchestration, and per-file continuation. It should deliberately correct the fixed-temp-name and delete-before-rename defects through the project's discrepancy process. The legacy `not loadable` substring is not a trustworthy format classifier: Figment's own README says some files cannot be loaded because its SDK/class support is incomplete ([upstream README](https://github.com/figment/hkxcmd/blob/dc4c75bf44303d874cc2656f56f107527f79ac29/README.txt)). The new implementation should inspect the header first, return `already_amd64` without rewriting, and preserve a distinct `unsupported_or_invalid` result for genuine parse failures.

## Why `serde-hkx` is the selected engine

### Direct 32/64 implementation

The convenience layer detects HKX architecture from the header, deserializes either binary form into a typed class map, sorts it for binary output, and serializes with `HkxHeader::new_skyrim_le()` or `HkxHeader::new_skyrim_se()` ([format detection and conversion](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/convert/mod.rs), [deserializer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/serde/de.rs), [serializer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/serde/ser.rs)). Conversion is direct binary-to-object-to-binary; XML is not required as an intermediate.

The headers are explicitly Havok packfile version 8, little-endian, `hk_2010.2.0-r1`, with pointer size 4 for LE and 8 for SE ([header source](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/bytes/serde/hkx_header.rs)). That matches the architecture conversion CAO currently requests from `hkxcmd`.

### Class coverage

The pinned source contains 678 generated Havok classes and 678 corresponding class-information JSON files. The set includes animation (`hka*`), Behavior (`hkb*`), physics/ragdoll (`hkp*`), scene (`hkx*`), core Havok, and Bethesda `BS*` extensions ([generated classes](https://github.com/SARDONYX-sard/serde-hkx/tree/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/havok_classes/src/generated), [class metadata](https://github.com/SARDONYX-sard/serde-hkx/tree/6c1bee56d42de7def991cf6fba025a9df7492d83/assets/classes)). This is materially broader than an animation-only parser and is important because CAO visits arbitrary `.hkx` files, including skeletons and behavior graphs.

Coverage is not proof of correctness. Upstream says the 32-bit metadata comes from `hkxcmd Report`, the 64-bit metadata comes from an SKSE/HKX2Library dump, and missing 64-bit layouts were automatically inferred from C++ conventions; it explicitly says those inferred values are not guaranteed correct. It also documents multiple-inheritance omissions, unknown padding exceptions, and one differing class signature ([metadata provenance and exceptions](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/assets/readme.md)). These are reasons for corpus gates, not reasons to retain a legally encumbered engine.

### Published evidence

The checked-in tests provide real but narrow binary evidence:

- a wisp skeleton XML fixture is serialized byte-for-byte to recorded Win32 and AMD64 outputs;
- a recorded AMD64 wisp skeleton is deserialized through XML and reserialized byte-for-byte;
- parser/header/fixup/type unit tests cover many lower-level cases; and
- the release commit passed tests on Windows x64, Linux x64, and Apple ARM64, plus Miri and three-platform linting ([round-trip test](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/tests/re_convert.rs), [32/64 fixture tests](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/tests/verify.rs), [release CI run](https://github.com/SARDONYX-sard/serde-hkx/actions/runs/27475513975)).

The repository also records a manual campaign that converted every HKX extracted from Skyrim SE's `Animation.bsa` to XML in about one minute. That evidence demonstrates broad SE deserialization reach, but it is not a checked-in CI corpus, does not exercise LE-to-SE serialization, and notes a skeleton with a global-fixup pattern whose corresponding virtual fixup was not found ([campaign note](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/tests/readme.md)). CAO must reproduce and extend this campaign with legally usable/local fixtures before pre-release.

The active repository history also shows substantive fixes driven by real mod behavior, including Nemesis compatibility and reproducibility fixes for a DMCO behavior file. Those issue/PR records are useful maintenance evidence, while also showing that uncommon array/string/flag layouts have produced serializer defects in the past ([Nemesis support PR](https://github.com/SARDONYX-sard/serde-hkx/pull/37), [DMCO serializer fix](https://github.com/SARDONYX-sard/serde-hkx/pull/31), [related bug](https://github.com/SARDONYX-sard/serde-hkx/issues/30)).

CAO's 2026-07-12 Windows x64 validation of the peeled release commit strengthens that upstream evidence:

- `cargo test --release --workspace --locked` passed every non-ignored test under Rust 1.96;
- the released CLI converted the checked-in Win32 wisp skeleton fixture directly to AMD64, byte-identical to the repository's expected AMD64 fixture with SHA-256 `2021B080B731E9BE291BFED623DABFEE8D04B1F9C09753973F8EFCDEC4AB50F1`; and
- passing the repository README as an HKX exited with code 1 and reported a header/offset diagnostic instead of producing output.

This is direct proof of the exact CAO operation on one published fixture and of one malformed-input error path. It does not replace the broader corpus below.

### Errors, panics, and malformed input

The low-level parser and serializer expose typed `snafu` errors for EOF, trailing bytes, absent classes/fixups, mismatched class names, invalid endianness or pointer size, missing serialization fixups/classes, UTF-8/NUL failures, and I/O failures ([deserialization errors](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/errors/de.rs), [serialization errors](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/errors/ser.rs), [feature-layer errors](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/error.rs)). This is substantially better than legacy substring matching.

It is not yet a panic-free untrusted-input boundary. The author explicitly reports an unexplained debug-build stack overflow that does not occur in release builds. Production-path internals also contain `unimplemented!`/`unreachable!` guards for serializer protocol methods that are documented as impossible under generated-class usage; an invariant bug could still reach them ([binary map deserializer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/bytes/de/map.rs), [XML map deserializer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/serde_hkx/src/xml/de/map.rs)). The in-process adapter must catch ordinary unwind panics at its outer boundary and map them to a stable failure. Stack exhaustion and aborts are not catchable; they trigger the helper-process contingency if the acceptance corpus cannot rule them out.

Core binary HKX operation is safe Rust. The only `unsafe` in the pinned repository is in optional extra-format JSON handling and the C FFI crate; CAO does not need either. Build only the minimal binary conversion crates/features and prohibit `extra_fmt`, CLI UI, directory parallelism, and FFI in the production graph.

### Maintenance and redistribution

Version 1.0.1 was released on 2026-06-13 from the annotated tag object `949f9e6f2ee4cd5edc32b8db12cb779718095aeb`, peeled to commit `6c1bee56…`. The release publishes Windows x64, Linux x64, and Apple ARM64 artifacts with SHA-256 digests ([release](https://github.com/SARDONYX-sard/serde-hkx/releases/tag/1.0.1)). The project has active fixes through that release and automated tests, linting, dependency review, and release builds.

All workspace crates declare `MIT OR Apache-2.0`, and the repository includes both license texts ([workspace manifest](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/Cargo.toml), [MIT](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/LICENSE-MIT), [Apache-2.0](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/LICENSE-APACHE)). Select one license consistently in notices—Apache-2.0 is a reasonable default for its explicit patent grant—and include the vendored source/license in the normal third-party bundle. The class metadata's own provenance discussion should remain in the source bundle; complete a release review, but there is no linked Havok SDK or proprietary executable in the production graph.

The pin requires Rust 1.95 and edition 2024 ([manifest](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/Cargo.toml)). CAO must pin a compatible toolchain in `rust-toolchain.toml` and build the vendored dependency in CI; do not consume upstream release binaries.

## Alternatives considered

| Candidate | Fit | Decision |
| --- | --- | --- |
| `serde-hkx` 1.0.1 behind in-process `HkxConverter` | Pure Rust; direct Havok 2010 LE/SE conversion; 678 classes; permissive; typed errors; phased cancellation | **Selected** |
| Same converter in a CAO one-shot helper | Same engine with crash/stack containment and forceful cancellation | Contingency if in-process safety gates fail |
| Exact v5.3.15 `hkxcmd.exe` | Highest released-behavior provenance | Oracle only; public redistribution right not established |
| Havok 2010 + native adapter | Exact legacy machinery possible | Reject: tool redistribution prohibited by historical license, old Win32 SDK, in-process native failure |
| `ck-cmd` native/library extraction | Descendant of `hkxcmd` | Reject: same Havok SDK constraint plus much larger dependency surface |
| Bethesda `HavokBehaviorPostProcess.exe` | Official Creation Kit conversion tool | External contingency only; Creation Kit EULA does not grant repackaging |
| `ret2end/HKX2Library` | MIT managed serializer | Reject: README limits it to Skyrim SE and leaves XML-to-64-bit as TODO |
| `Dexesttp/hkxpack` | MIT Java serializer | Reject: Havok 2014/Fallout 4 focus; required class data omitted |

### Legacy licensing remains relevant to the oracle

Figment's original source is BSD only for its own code and explicitly excludes included libraries from that grant ([license](https://github.com/figment/hkxcmd/blob/dc4c75bf44303d874cc2656f56f107527f79ac29/LICENSE.TXT)). The Havok license historically carried in CAO prohibits redistributing Havok as a middleware, engine, or tool and permits compatibility-tool work only when no Havok component is redistributed ([historical license](https://github.com/evildarkarchon/CAO/blob/e65e981dc8cb46ac756226f9dfb843cbd8cd360f/external/hkxcmd/havok_2010_2_0/Havok%20Limited%20Use%20License%20Agreement%20for%20XS%20PC%20R1%20v8%20053008.txt)). Current Havok terms also condition redistribution on an accompanying product license, and downloads are restricted to registered developers ([terms](https://www.havok.com/terms-of-use/), [download portal](https://downloads.havok.com/login/?next=download)).

Accordingly, keep the authenticated legacy executable in a non-redistributed local oracle kit. Its prior appearance in a CAO package is provenance evidence, not a license grant for the fork.

### Other open serializers

[`HKX2Library` at `5156f4f…`](https://github.com/ret2end/HKX2Library/tree/5156f4f21fa04fad31ab39b07db83053941e09a5) is limited by its own README to Skyrim SE; it crosses out platform conversion, directs XML-to-HKX users to `hkxcmd`, and lists malformed FNIS inputs it cannot deserialize ([README](https://github.com/ret2end/HKX2Library/blob/5156f4f21fa04fad31ab39b07db83053941e09a5/README.md)).

[`HKXPack` at `659f1f0…`](https://github.com/Dexesttp/hkxpack/tree/659f1f03185aac7c48272809b9d05270b58b73fa) targets Havok 2014.1/Fallout 4, calls itself a proof of concept, documents missing cloth data, and omits required class XML for legal reasons ([README](https://github.com/Dexesttp/hkxpack/blob/659f1f03185aac7c48272809b9d05270b58b73fa/README.md)). Neither matches the direct LE/SE implementation now available in `serde-hkx`.

## Recommended boundary

Use a tiny, versioned helper protocol. The helper receives one input and writes one output; it never discovers directories, mutates the source, chooses policy, or performs replacement.

```text
Rust GUI / sequential optimizer
        |
        | versioned request + private paths
        v
cao-hkx-helper (Rust, one file, one process)
        |
        | serde_hkx low-level deserialize/serialize
        v
validated AMD64 bytes in private workspace
```

Request fields should include protocol version, input path, output path, target `amd64`, maximum input size, and optional diagnostic level. The result should carry protocol version, engine ID `serde-hkx`, pinned commit, status, typed error code, bounded diagnostic text, input/output headers, class count, elapsed time, and output size/hash.

Stable statuses should distinguish `converted`, `already_amd64`, `unsupported_header`, `unsupported_class`, `malformed_input`, `resource_limit`, `serialization_failed`, `output_invalid`, `cancelled`, `timed_out`, and `helper_failed`. Preserve the upstream typed error as a diagnostic cause, but do not expose Rust type names as the stable product API.

Use the low-level library surface rather than upstream's convenience file writer. Upstream `convert_file` reads a file, performs synchronous CPU work, and ultimately calls `tokio::fs::write`, which truncates/replaces the destination directly ([converter](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/convert/tokio.rs), [file writer](https://github.com/SARDONYX-sard/serde-hkx/blob/6c1bee56d42de7def991cf6fba025a9df7492d83/crates/serde_hkx_features/src/fs.rs)). CAO needs byte buffers and its own transaction, so the helper should call `serde_hkx::from_bytes`/`to_bytes` with the generated class map and write only a create-new private output.

Patch the vendored integration, not generated class files, to add:

- explicit input and object/count/depth/total-allocation limits;
- an outer `catch_unwind` for ordinary panics, while still assuming stack overflow or `panic=abort` terminates the helper;
- a stable error mapping;
- no directory traversal or parallel conversion; and
- a post-serialization reparse before reporting success.

Keep upstream code changes in a minimal CAO patch queue and upstream generally useful hardening.

## Temp-file safety and replacement

The GUI/orchestrator, not the helper, owns this sequence:

1. Inspect the real input header. Return `already_amd64` without launching the helper when pointer size is already 8 and the scoped compatibility policy says no rewrite is required.
2. Create a private random per-file workspace outside the asset tree with an owner-only DACL.
3. Copy the input to a create-new workspace file with a short ASCII name; close it and record size/hash/header.
4. Create no output in advance. Pass a random nonexistent output path to the helper.
5. Require normal helper completion, a newly created nonempty output, expected HKX magic/version/AMD64 header, independent reparse, and semantic checks.
6. Copy validated bytes to a create-new staging file beside the target; flush and close it.
7. Replace with the approved backup policy using `ReplaceFileW`. Microsoft documents that it combines replacement operations, preserves important original ACLs/attributes, and requires all participating files to be on the same volume ([`ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)).
8. On any failure or cancellation, delete only private/staging artifacts and leave the original untouched.

Do not let the helper see the asset tree path or perform an in-place write. The short workspace path also avoids reintroducing the legacy narrow-path behavior while Rust owns the real Unicode/extended-length path.

## Cancellation and diagnostics

Run each helper in a Windows Job Object with only redirected standard handles inherited. On cancellation, terminate the job, drain/close bounded diagnostic pipes, wait for exit, discard workspace output, and report `cancelled`. `TerminateJobObject` ends all associated processes ([Microsoft Job Object API](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/)).

This gives prompt cancellation during synchronous parse/serialize even though upstream's conversion API has no cancellation token. It also contains an abort or stack overflow. Processing remains sequential; cancellation before the final replacement is always safe because the real target has not changed.

Launch the absolute helper path directly, set its working directory explicitly, pass a minimal environment, and restrict inherited handles. Windows documents executable-search/current-directory behavior in `CreateProcessW` and the access implications of inherited handles ([`CreateProcessW`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw), [handle inheritance](https://learn.microsoft.com/en-us/windows/win32/procthread/inheritance)).

Diagnostics should include the stable CAO status, upstream typed error chain, class name/fixup/offset when present, input/output headers, elapsed time, and helper exit classification. Bound all strings and retain only diagnostic tails so a malicious file cannot produce unbounded logs.

## Mandatory acceptance gates

These gates block the functional-parity milestone and release, not the architectural choice.

### 1. Dependency and build

- vendor peeled commit `6c1bee56d42de7def991cf6fba025a9df7492d83`, record archive/source checksums, and pin Rust 1.95;
- build only minimal crates/features; prove no C/C++, FFI, `extra_fmt`, network, Java, .NET, or Havok dependency enters the helper;
- run upstream workspace tests, Miri where practical, CAO adapter tests, `cargo deny`, and license/SBOM generation;
- compile release and debug variants with enlarged/controlled helper stack and reproduce/investigate the documented debug overflow;
- verify protocol versioning, engine commit reporting, helper imports, and the final ZIP on clean Windows x64; and
- include MIT/Apache selection, notices, vendored source, patches, and lockfile.

### 2. Golden format corpus

Use redistributable or locally supplied fixtures covering:

- LE Win32 and SSE AMD64 animation packfiles with interleaved, spline-compressed, annotation/event, root-motion, and binding variants;
- skeleton, behavior, ragdoll, physics, and Bethesda `BS*` classes;
- vanilla Skyrim LE, vanilla Skyrim SE, FNIS, Nemesis, and representative modern mod outputs;
- every class/layout exception documented in upstream `assets/readme.md`;
- unknown classes, duplicate fields, absent/misaligned fixups, truncated sections, invalid counts, extreme nesting, huge arrays/strings, and random mutations; and
- already-AMD64 input, Unicode/long/locked/read-only targets, and custom profiles under every shipped `animationFormat` value.

For every legacy-successful LE input, compare:

1. v5.3.15 `hkxcmd` AMD64 output;
2. `serde-hkx` AMD64 output; and
3. both outputs after independent parse into semantic class graphs.

Compare root/container and class identities, pointer graph, array/string content, skeleton bones/parents/reference pose, animation type/duration/tracks/transforms/compressed payload interpretation, bindings, annotations/events, behavior variables/transitions, ragdoll/physics data, and engine/tool loadability. Byte identity is not required across different writers; semantic equivalence is.

For legacy failures, record whether `serde-hkx` safely succeeds or returns a typed error. A broader success is a proposed discrepancy, not automatically accepted behavior.

### 3. Campaign evidence

- rerun the upstream full Skyrim SE `Animation.bsa` parse campaign and preserve machine-readable results;
- run the corresponding Skyrim LE animation archive through LE parse → AMD64 serialize → reparse;
- run a legally available mod corpus, including the upstream-documented FNIS/Nemesis/DMCO edge families;
- require zero unexplained crashes, hangs, panics, stack overflows, or semantic graph differences at the release gate; and
- store hashes, class inventories, engine versions, timings, and discrepancies so upgrades are reproducible.

### 4. Fuzzing and resource safety

- fuzz headers, section tables, classnames, local/global/virtual fixups, strings, arrays, nesting, and every generated class deserializer;
- run sanitizer-equivalent Rust checks, Miri for suitable targets, and memory/time/stack measurements;
- prove configured input, allocation, class-count, depth, output-size, and elapsed-time limits fail closed;
- treat helper abnormal exit and watchdog expiry as one file failure with the original byte-identical; and
- keep the subprocess boundary until a sustained malformed corpus shows no abort/overflow and the upstream debug issue is understood.

### 5. Transaction and cancellation

- cancel before spawn, during parse, during serialize, during validation, and immediately before replacement;
- inject helper crash/abort/hang, disk-full, ACL/sharing/antivirus failures, and replacement failures;
- prove stale output cannot be mistaken for current output and two CAO instances cannot collide; and
- verify no helper remains after cancellation and no failed path deletes or truncates the source.

## Upgrade and fallback policy

Pin 1.0.1 for characterization. An upstream upgrade must rerun the same corpus and semantic diff; do not adopt `main` because generated layouts or sorting rules can change output globally.

If a narrow corpus defect is found, maintain a small CAO patch and upstream it. If malformed-input robustness remains insufficient, keep or tighten the helper sandbox rather than reverting to Havok. If a broad class/layout gap prevents parity, escalate in this order:

1. extend/fix `serde-hkx` with corpus-backed layouts;
2. use the exact legacy helper locally to diagnose expected semantics;
3. consider an external user-installed Creation Kit tool for a specifically approved fallback; and
4. defer only the unsupported class family with an explicit parity discrepancy.

Do not bundle the legacy executable as an expedient fallback. The technical fallback ladder does not override redistribution rights.

## Bottom line

`serde-hkx` 1.0.1 is the selected engine because it is the only investigated maintained, redistributable backend that directly implements CAO's Havok 2010.2 Win32-to-AMD64 operation. Its 678-class model, typed errors, real binary fixtures, active mod-driven fixes, cross-platform tests, and permissive licensing are sufficient to choose it over a proprietary legacy subprocess.

Its evidence is not sufficient to place it uncontained in the GUI or to waive parity testing. Ship it first in a CAO-owned one-shot Rust helper, keep `hkxcmd` solely as the authenticated oracle, and make the full LE/SSE semantic corpus, debug-stack investigation, fuzz/resource bounds, and transaction tests release gates.
