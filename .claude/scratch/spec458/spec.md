Part of #458. This is the locked migration spec that map aims for. It gathers the resolutions of #459–#473 into one contract. The implementation backlog (vertical-slice tickets with blocking edges) is cut from it next.

## Problem Statement

Cathedral Assets Optimizer (CAO) is a C++/Qt 5 desktop app. It optimizes Textures, Meshes, Animations and Archives in Bethesda game mods. Its stack is costly to keep alive:

- Qt 5.15 is end-of-life.
- The vcpkg/CMake/Qt build is heavy.
- bethutil and rsm-bsa wrap archive work in void mutators with no mutation facts, and fail on some inputs. For example, FO4 DX10 cubemaps in BC7, BC6H or sRGB can't be extracted.
- Several behaviours are clearly unintended:
  - profiles and logs resolve against the working directory;
  - Cancel in the unwanted-formats dialog does not revert;
  - a plugin parser can hang;
  - FO4 can pack unchunked textures into the Main BA2, which can make the game unstable;
  - and others.

The CLI was never released.

The users are mod authors and mod-list curators. For them, the risk of any rewrite is regression. CAO mutates their mod folders in place. They rely on:

- its safety guarantees: Run Outcome classification, Temporary Ownership, Safety Cleanup, staged publication, Loose-over-Archived precedence, Archive Precedence, and a Dry Run that never mutates;
- existing `profiles/` working unchanged;
- output their games load the same way.

A previous attempt to plan this port (#65) failed because its scope was over-engineered.

## Solution

Port CAO, keeping it simple, to a Rust Cargo workspace with a Slint GUI. Keep the same native engines:

- DirectXTex, including GPU BC7 encoding;
- nifly;
- `hkxcmd.exe`;
- archives through the `ba2` crate.

The user sees the same windows, tabs, controls, labels, enable/disable rules and workflow, in a dark Fluent style. Their existing profiles, settings and staging leftovers keep working. Their output is **equivalent** to what C++ CAO produces, not byte-identical. A short, explicit deviation list fixes the clearly unintended behaviours instead of copying them.

The workspace grows next to the C++ tree in this repo. A C++ CLI build serves as a differential **parity oracle**. A corpus of generated mod trees is run through both builds, and their outcomes, evidence and output trees are compared under per-Asset-Kind equivalence rules. Contract scenarios the corpus can't reach (fault injection, cancellation, crash windows) are ported from the C++ suites as Rust tests. Once the corpus passes, all C++ is deleted.

## User Stories

### Running optimizations

1. As a CAO user, I want to pick one mod folder or a mods directory and click Run, so that my mods are optimized the way they were in C++ CAO.
2. As a CAO user, I want Dry Run to evaluate my Loose Assets without changing anything on disk, so that I can preview a run safely.
3. As a CAO user, I want Several Mods selection to process each child folder as its own Mod Root, so that one mod's Archives never mix with another's.
4. As a Mod Organizer 2 user, I want children whose names end in `_separator` excluded from Several Mods, so that separators are never treated as mods.
5. As a CAO user, I want a mod with "separator" elsewhere in its name to be processed, so that a mod isn't skipped just because of its name.
6. As a CAO user, I want mods named in my profile's ignored-mods list excluded and reported, so that I control which mods are touched.
7. As a CAO user, I want each Mod Exclusion reported as a Run Diagnostic that never changes the Run Outcome, so that exclusions are visible but harmless.
8. As a CAO user, I want a Several Mods child named like `.cao-staging…` silently ignored, so that CAO's own reserved namespace is never treated as a mod.
9. As a CAO user, I want the run to end in exactly one Run Outcome (Succeeded, Completed With Failures, Cancelled, or Failed), so that I know what happened.
10. As a CAO user, I want a run I cancel to stop between Assets, keep its Committed Mutations, and still run Safety Cleanup, so that cancelling never leaves temporary debris.
11. As a CAO user, I want closing the window during a run to cancel it and close the window once the run reports its outcome, so that quitting is safe.
12. As a CAO user, I want only one Optimization Run active at a time, so that two runs never race over the same files.
13. As a CAO user, I want a run request that contradicts my profile's capabilities (for example, mesh work under FO4) rejected with a Start Error, so that I never get a half-valid run.
14. As a CAO user, I want resize width and height validated only when resizing by size is enabled, so that an unused odd value doesn't block my run.

### Textures

15. As a CAO user, I want Textures converted, resized, mipmapped and compressed with the same DirectXTex decisions as before, so that my textures look and load the same.
16. As a CAO user with a GPU, I want BC7 and BC6H encoded on the GPU with a CPU fallback, so that BC7-heavy runs are no slower than before.
17. As a CAO user, I want TGA-to-DDS conversion to update every Mesh that references the converted Texture (Mesh Reference Maintenance), so that my meshes don't point at missing files.
18. As a CAO user, I want very long texture paths to work, so that deep mod trees don't overflow a fixed-size path buffer.
19. As a CAO user, I want texture metadata checks that actually compare what they claim to compare, so that a mismatched texture is not passed through silently.

### Meshes and Animations

20. As a CAO user, I want Meshes optimized with nifly at the same levels as before, so that my LE meshes convert to SSE the same way.
21. As a CAO user, I want Headpart Meshes recognised from my profile's headpart list, from HDPT records in any plugin across the Mod Selection, and from facegen paths, so that head meshes get head-part-specific treatment.
22. As a CAO user, I want Dry Run to apply the facegen rule just as Apply does, so that the preview matches the real run.
23. As a CAO user, I want headpart paths matched within the Mod Root, so that a `meshes` folder above my mod doesn't confuse matching.
24. As a CAO user, I want a truncated or malformed plugin reported as unreadable while the run carries on, so that a broken plugin can't hang or crash CAO.
25. As a CAO user, I want compressed HDPT records decompressed, so that headparts in compressed plugins are recognised correctly.
26. As an SSE user, I want LE Animations converted through `hkxcmd.exe` as before, so that my animations work in SSE.

### Archives

27. As a CAO user, I want enabled Archives extracted into the Effective Asset Tree with Loose Assets taking precedence, so that the game's own precedence is preserved.
28. As a CAO user, I want Archive Collisions inside a Mod Root resolved by Archive Precedence and reported before extraction, so that I know which Archive won.
29. As a CAO user, I want any Unsafe Game Path in a selected Archive to fail the run before extraction, so that a malicious or broken Archive can't write outside my mod.
30. As a CAO user, I want device names such as `NUL .txt` recognised even with trailing spaces and dots, so that Windows device aliases are always blocked.
31. As a CAO user, I want the Capacity Check to stop work that would run out of disk space, so that I don't end up with half-written Archives.
32. As a CAO user, I want new Archives created with the same per-game versions, flags, naming, split points and Loading Plugins as before, so that my game loads them the same way.
33. As an FO4 user, I want textures always packed into a separate, compressed, DX10 Textures BA2, so that unchunked textures never land in the Main BA2.
34. As an FO4 user, I want "create texture archive" checked and disabled under the FO4 profile, including when settings load from the INI, so that I can't select the unstable layout.
35. As an FO4 user, I want DX10 cubemaps in BC7, BC6H and sRGB formats extracted successfully, so that those Archives no longer fail.
36. As a CAO user, I want an Archive that would exceed 4 GiB to fail with an error, so that a hand-edited size limit can't produce a corrupt Archive.
37. As a CAO user, I want Packing Exclusions from `FilesToNotPack.txt` matched against the path within my Mod Root, so that rules behave the same wherever my mods folder lives.
38. As a CAO user, I want Archive Finalization to create, keep or remove Loading Plugins and Dummy Plugins as before, so that my archives keep loading.
39. As a CAO user, I want an all-digit plugin name such as `2.esp` handled correctly, so that archive naming never hits undefined behaviour.
40. As a CAO user, I want Textures and Meshes that fail to load quarantined as `.caobad` (Apply only), so that later runs and Archive creation leave them alone.
41. As a CAO user, I want empty directories left inside each Mod Root pruned after Archive Finalization, so that packed mods stay tidy.

### Safety and recovery

42. As a CAO user whose previous run crashed, I want the next Apply run to recover `CAO-STAGING` v1, v2 and v3 manifests, including ones written by C++ CAO, so that leftover staging is cleaned up safely.
43. As a CAO user, I want recovery to fail closed on unverifiable staging, naming the path and saying what to do, so that CAO never deletes something it doesn't provably own.
44. As a CAO user, I want every output staged and published atomically with a recorded mutation fact, so that a crash never leaves a half-written Texture, Mesh, Animation, Archive or Loading Plugin.
45. As a CAO user, I want Dry Run never to create, recover or clean staging, so that a preview never touches disk.

### Profiles and settings

46. As an existing CAO user, I want my `profiles/` folder (`common.ini`, `settings.ini`, `profile.ini`, auxiliary text files) to load unchanged, so that upgrading costs me nothing.
47. As an existing CAO user, I want settings saved by Rust CAO to stay readable by Qt's QSettings, so that the files remain compatible both ways during the transition.
48. As a CAO user who picked one unwanted format, I want that one-element list preserved on save and load, so that my choice isn't silently turned into "none".
49. As a CAO user who hand-edits INI files, I want `texturesUnwantedFormats=85` read as a one-element list, `#` lines treated as comments, and UTF-8 without a BOM decoded correctly, so that reasonable hand edits work.
50. As a CAO user with a corrupt profile, I want an unsupported `bsaGame` value to make the profile unreadable with a message naming the value, so that I'm not silently switched to SSE tables.
51. As a CAO user, I want FO4, SSE and TES5 profiles available, including new profiles based on them, so that I can keep my per-game setups.
52. As a CAO user who launches CAO from a shortcut with another start-in folder, I want `profiles/`, `logs/`, `bin/hkxcmd.exe` and `translations/` resolved next to the exe, so that CAO always finds its own files.

### GUI

53. As a CAO user, I want the same main window, tabs, groups, controls, labels and tooltips as before, so that I don't have to relearn CAO.
54. As a CAO user, I want a dark theme by default and a Tools menu toggle for light, with the menu bar following that toggle, so that the whole window matches.
55. As a CAO user, I want to reorder tabs by dragging, so that the layout I'm used to still works.
56. As a CAO user, I want help tooltips that wrap, stay inside the window, and show on disabled controls, so that I can always read them.
57. As a CAO user, I want the What's This help cursor, so that I can click a control to learn about it.
58. As a CAO user, I want to drop a folder from Explorer onto the window to select it, so that choosing a mod is quick.
59. As a CAO user, I want the Dry Run and Several Mods rules applied when settings load from the INI, not only when I click, so that the window never shows an inconsistent state.
60. As a CAO user, I want Cancel in the unwanted-formats dialog to discard my edits, so that Cancel means cancel.
61. As a CAO user, I want a searchable, checkable list of unwanted formats, so that I can find a format among 75 quickly.
62. As a CAO user, I want the run lock to disable tabs, profile selection and New, turn Run into Cancel, and disable Cancel once clicked, so that I can't change settings mid-run.
63. As a CAO user, I want a progress bar with phase text and a status bar during a run, so that I can follow progress.
64. As a CAO user, I want modal dialogs (New Profile, unwanted formats, messages, folder picker, About) to block the main window and return focus properly, so that the app never gets stuck disabled or loses clicks.
65. As a CAO user, I want "About Slint" where "About Qt" was, so that licence credits stay accurate.

### Application Log

66. As a CAO user, I want the Application Log written to the same location and in the same HTML format, with the same rotation, so that my log habits and tools still work.
67. As a CAO user, I want toggling debug logging between runs to keep logging, so that I don't silently lose a log.
68. As a CAO user, I want the Log tab to show coloured severity rows, including startup records before the first run and not truncated to the current rotation chunk, so that I see the whole session.
69. As a CAO user, I want "Open log file" to show records up to the moment I click it, so that I can diagnose a run in progress.
70. As a CAO user, I want a clear error, with the run not started, when the run's log file can't be opened, so that runs are never unlogged.

### Maintainers and implementing agents

71. As a CAO maintainer, I want a differential corpus that runs the C++ oracle and the Rust port on the same generated mod trees and reports Identical, Equivalent or Different per artifact, so that I can prove parity before deleting C++.
72. As a CAO maintainer, I want a report for each Different, with the fact diff, the broken rule, both captures, both logs and an exact replay command, so that I can diagnose a regression quickly.
73. As a CAO maintainer, I want deviation triggers kept out of the corpus by a guard, with each deviation pinned by a Rust-only test, so that fixes are never reported as regressions.
74. As a CAO maintainer, I want cases that need missing local assets, `hkxcmd.exe` or symlink rights reported as not run, never as passing, so that coverage gaps stay visible.
75. As a CAO maintainer, I want the BC7/BC6H PSNR threshold calibrated once from the oracle against itself (GPU against forced CPU), so that the tolerance is measured, not guessed.
76. As an implementing agent, I want `cao-core` and its ported scenario tests to build without a C++ toolchain, so that I can iterate on run logic quickly.
77. As an implementing agent, I want an exact Rust toolchain pin, so that parallel sessions use the same compiler and clippy lints.
78. As an implementing agent, I want `unsafe` confined to `cao-winfs`, `nifly-sys` and the D3D11 device module, so that unsafe code stays reviewable.
79. As an implementing agent, I want the C++ seams ported one-to-one as traits, so that ported scenarios map directly onto the Rust code.
80. As a CAO maintainer, I want all C++ deleted and the glossary and ADRs updated once parity holds, so that the repo has one implementation and docs that match it.

## Implementation Decisions

### Workspace and crates (#468)

- The repo root holds a virtual Cargo workspace, edition 2024, with members under `crates/`. CMake ignores `crates/` and Cargo ignores the C++ tree, so nothing moves when C++ is deleted.
- The root `[patch.crates-io]` points `directxtex` at CAO's fork. `[workspace.dependencies]` pins the decided versions.
- There are eight crates:
  - **`cao-core`**: Asset Routing, asset execution, the Run lifecycle, Run Evidence, the Run Executor, the Optimization Run Service, staging and Temporary Ownership, and the logic of Archive discovery and Archive Finalization. It depends only on `cao-winfs`. It reads Archives through an archive-reader trait (list entries, extract an entry) and never depends on `ba2`.
  - **`cao-winfs`**: every `unsafe` call and `windows-sys` import for file safety.
  - **`cao-profiles`**: the QSettings INI port, profile discovery, and the options and settings model. It has no notion of a Run Request.
  - **`cao-archive`**: the bethutil port over `ba2`.
  - **`nifly-sys`**: a C shim over vendored nifly, plus a safe `Nif` type.
  - **`cao-optimizers`**: the texture, mesh, animation and archive backends, plus the **composition root**. The composition root is a library, so both binaries share the production wiring. It takes the **app directory** as a parameter and translates the options model into the Run Request, Run Configuration and per-run optimizer settings. The Application Log sink also lives here.
  - **`cao-gui`** (binary): the Slint app. It owns profile selection.
  - **`cao-parity`** (binary, never shipped): the differential driver and harness.
- Leaves (`cao-winfs`, `cao-profiles`, `cao-archive`, `nifly-sys`) have no workspace dependencies. `cao-optimizers` implements core's traits over leaf types, because the orphan rule puts the adapters there.
- **Seams** are ported one-to-one as object-safe `dyn` traits, with no generic parameters threaded through the service:
  - Run Scheduler
  - Run Work Service
  - Safety Cleanup Service
  - Run Configuration Provider
  - Run Observation Sink and work milestones
  - one Asset Execution Backend over all optimizers
  - the archive reader
  - the capacity and volume-identity probes

  A filesystem abstraction, a clock and the staging nonce are **not** seams.
- **Errors.**
  - Domain records stay plain data: `StartError` (returned synchronously), `RunFailure`, `OperationFailure` (mutation state plus safe-to-continue), and `RunOutcome`.
  - C++ exception boundaries become `Result` returns at the same places.
  - Each library crate has a `thiserror` enum. `anyhow` is allowed only in the two binaries.
  - Cancellation during asset initialization is an ordinary `Err` variant, not control flow.
- **Panics.** `panic = "unwind"`.
  - A panic in an asset backend call becomes an Operation Failure with `PartialOrUnknown` mutation, unsafe to continue.
  - A panic in the Run Worker becomes a Failed outcome, and Safety Cleanup still runs.
  - A Run Evidence invariant violation panics only after cleanup.
- **Threading.**
  - The Run Worker is one `std::thread`, and there is no async runtime.
  - The Run Handle's `Drop` requests cancellation (an atomic flag checked between Assets) and joins. A nifly call in progress can't be interrupted, so the join waits for it.
  - The Run Event Dispatcher is a boxed `Fn(RunEvent) + Send` that the adapter supplies.
  - `rayon` is used only inside archive packing and compression.
- **Process-wide state.**
  - One active run per process, through a static weak slot in the service.
  - There is no profile singleton. The Run Request carries the profile identity, and the provider re-reads `profile.ini` during Preparing.
  - Libraries log through the `log` facade only.
  - COM and the D3D11 device are initialized per worker thread by the texture backend.
- **Windows only.** No `cfg(windows)` gates and no POSIX branches. `[workspace.lints]` sets `unsafe_code = "forbid"`. Only `cao-winfs`, `nifly-sys` and the D3D11 module override it.

### Toolchain and build (#471)

- `rust-toolchain.toml` pins Rust exactly at 1.99.0 with rustfmt and clippy, and `rust-version` equals the pin.
- VS 2026 (v145) Build Tools and Windows SDK 10.0.26100 are documented as a minimum, not enforced.
- `embed-resource` 3.x embeds today's application manifest unchanged: `longPathAware` and the UTF-8 active code page. There is no ComCtl32 v6, no DPI entry and no version resource.
  - `cao-gui` also gets the icon and uses the Windows subsystem.
  - `cao-parity`, and the test binaries of `cao-winfs`, `cao-archive`, `cao-optimizers` and `nifly-sys`, get the manifest through `compile_for_everything`.
  - A `cao-winfs` test asserts that the active code page is UTF-8, which proves the manifest reached the test binary.
- Keep the dynamic CRT.

### File safety (#463)

- Open handles with `std` `OpenOptionsExt`, always passing `share_mode`. Do every query and mutation with `windows-sys` on the raw handle: file identity (`FileIdInfo` plus a 64-bit fallback), link count, change time, rename with and without replace, identity-bound delete, write-through moves, volume paths, and ordinal case-insensitive comparison.
- `owner.lock` is a share-mode-0 open, never `File::lock`. Sharing violations map to `StagingActive`.
- Reparse points are rejected by attribute, not `is_symlink()`.
- New dependencies: `getrandom` for nonces and `dunce`. `same-file`, `fs4` and the `windows` crate are rejected for this layer.
- An **MSVC-canonical path helper**, with a matching weakly-canonical form, reproduces MSVC `canonical` text exactly, so that C++-written `CAO-STAGING` manifests recover.
- The exe directory is `current_exe().parent()` plus `dunce::simplified`, never canonicalized. It is resolved once in `cao-gui`'s `main`.

### Staging and recovery

- Port the staging-ownership contract as documented:
  - the reserved `.cao-staging` namespace;
  - the `owner.lock` and `ownership.manifest` control files;
  - one Run-ID-derived run child;
  - `S` sibling registrations;
  - the three-state publication receipt (`NotPublished`, `PublishedStillOwned`, `PublishedAndReleased`), which carries the mutation fact and safe-to-continue;
  - `Replace` and `NoReplace` publication;
  - ancestor pinning;
  - nonrecursive removal in reverse registration order;
  - the `StagingActive`, `StagingOwnershipUnverified` and `StagingRecoveryFailed` failures.
- The writer emits v3. Recovery reads v1–v3, with the same bounds (8 MiB, 100,000 registrations) and quoting grammar.
- Quarantine uses a no-replace rename instead of check-then-rename.

### Profiles and INI (#464, #469)

- A dependency-free port of the Qt 5.15 QSettings INI reader and writer, about 300 lines. It covers:
  - Latin-1 or BOM-UTF-8 reading;
  - `[General]` handling;
  - `;` comments;
  - key escapes;
  - value escapes, including greedy `\x` and octal;
  - quotes and comma lists;
  - last-duplicate-wins and case-insensitive lookup that keeps the original case and order;
  - `@Variant` one-element lists and `@Invalid()` empty lists;
  - QVariant's lenient conversions;
  - keeping unknown keys on save;
  - atomic whole-file rewrite with CRLF.
- The load result can carry a format error to run setup.
- Deviations: a plain scalar reads as a one-element list; a `#` line is a comment; a file without a BOM decodes as UTF-8 when the whole file is valid, Latin-1 otherwise. **The writer is unchanged**, so the C++ oracle can still read Rust-written files.
- `bsaGame` must be 3, 4 or 5. Anything else makes the profile unreadable, naming the value.
- Auxiliary files (`customHeadparts.txt`, `FilesToNotPack.txt`, `ignoredMods.txt`) keep their read rules and the `profiles/SSE` fallback. Dead data stays untouched.

### Archives (#461, #469)

- Pin `ba2 = "=3.0.1"` unpatched. Set every option default that differs from rsm-bsa explicitly:
  - FO4 string tables on;
  - BSA v104 or v105;
  - the `COMPRESSED` flag;
  - DX10 read options for texture archives;
  - FO4 DX10 Textures BA2s always compressed.
- Hand-port into `cao-archive`:
  - bethutil's per-game tables, with versions, flags, suffixes, plugin extensions and effective maximum sizes;
  - the 49-byte Dummy Plugins (ESL flag for SSE and FO4);
  - file-type classification by first path component;
  - the `ArchiveData` split (strict `>`) and merge (strict `<`);
  - `FilePath` naming, handling all-digit stems.
- Sources are sorted before splitting, as C++ does.
- Drop memory-mapped Archive and file objects before deleting or renaming their source.
- **FO4 deviation:** textures are never merged into the `GNRL` Main BA2. "Create texture archive" is forced on and disabled under FO4.
- Unsafe Game Path, Capacity Check (the behaviour is contract, the estimate formulas are internal), Packing Exclusion within the Mod Root, Quarantine, Headpart Mesh and Mod Exclusion behave as the #469 resolution and the glossary define them.

### Textures (#459)

- Fork `directxtex-rs`, staying on 1.x:
  - compile the GPU compress sources with the 14 prebuilt shaders vendored;
  - add one `compress_gpu` wrapper;
  - bump the DirectXTex submodule to `may2026` (**mandatory**);
  - add `Send` for `ScratchImage`.
- `ba2` shares the same copy through the workspace patch.
- The D3D11 device is created with the `windows` crate, one per worker thread, and falls back to the CPU `compress` as C++ does.
- The worker calls `CoInitializeEx`, because mipmaps may use WIC.
- Pass `TEX_COMPRESS_DEFAULT` instead of C++'s no-op separate-alpha flag.

### Meshes (#460)

- A hand-written plain C ABI shim of about 150 lines: opaque handles; `noexcept` entry points that catch every exception and return -1; UTF-16 paths; texture paths as bytes.
- Built with `cc` (C++17, `/EHsc /bigobj /Zc:inline`) over nifly vendored at `5504832` with the const-matrix patch applied. Never bump the pin during the port.
- The shim must call `SetFile`, `SetStream` and `SetUser` as C++ does.
- Meshes stay sequential on the Run Worker.
- TGA→DDS reference rewriting moves to Rust.
- Port as-is: `scan()`'s target-version quirk, OptimizeFor being a no-op for FO4, and the unlogged version mismatch.
- A `corpus` Cargo feature exposes `Create`, `CreateShapeFromData` and `SetTextureSlot` for the generator.

### Animations

- `hkxcmd.exe convert <src> -o <dst> -v AMD64` as a subprocess, resolved from the app directory's `bin/`.

### Application Log (#470)

- A hand-written `log::Log` sink of about 200 lines, depending only on `log` and `chrono`. It keeps plog's bytes:
  - a BOM and `<style>` header only in an empty file;
  - `<br><font color=…>` per record, with the same colour table;
  - the info and debug layouts.
- Rotation is unchanged: 250,000 bytes checked before each write, 1,000 files, `name.N.html`.
- `cao-gui` stamps `logs/<profile>/yy.MM.dd.hh.mm.html` when a profile is selected. A redirect at each run start swaps the handle, path, layout and max level under one lock.
- Writes are unbuffered, and a poisoned lock is recovered.
- Each record is formatted once. HTML goes to the file. Plain `{severity, text}` rows go to a subscriber through a buffer that schedules one `invoke_from_event_loop` when it goes from empty to non-empty. The subscriber holds 10,000 rows and replays them on attach, and a redirect clears it. Run-detail rows stay GUI-only.
- Failure scopes:
  - a bootstrap failure shows a critical box and exits 1;
  - a redirect failure refuses to start the run;
  - per-record write failures are swallowed.
- Deviations:
  - `{module::path@line}` replaces `{Func@line}`;
  - `fatal` is written as `ERROR`;
  - message text is HTML-escaped;
  - each record ends with `\n`;
  - toggling debug logging no longer drops the log;
  - the Log tab isn't truncated and shows startup records.

### GUI (#462, #466)

- Slint `~1.18` with the `unstable-winit-030` and `raw-window-handle-06` features, on the winit and femtovg backend, in the **Fluent** style. Dark is the default, and Tools → Enable dark theme toggles the palette at runtime.
- `SLINT_NO_MUDA` is set before the first window, so the Slint-drawn menu bar follows the toggle.
- Structural parity with the Qt main window and the unwanted-formats dialog. All strings are wrapped in `@tr`, and only English ships.
- Custom widgets, about 400 lines:
  - the tab bar, with drag reorder on release and per-tab enable;
  - the checkable group box;
  - help-aware checkbox, label, radio and area, plus one clamped tooltip layer;
  - a double spin box for the maximum archive size;
  - a progress bar;
  - a custom radio button for the Downsizing grid.
- **Constraints:**
  - Never call winit `focus_window()`.
  - Modal Slint dialogs set their owner, disable the main window through a depth-counted guard, re-enable the owner *before* hiding the dialog, tear down on the next turn, and handle the title-bar close.
  - `rfd` 0.17 folder and message dialogs use `set_parent`, with `common-controls-v6` off.
  - Folder drop hooks winit's `DroppedFile` and accepts any existing path.
  - There is no enabled cascade, so every tab-page control ANDs in the run lock.
  - Programmatic radio selection uses two-way `checked` bindings.
  - Worker events arrive through `Weak::upgrade_in_event_loop`, and events after the loop ends are dropped.
- **UI rules live in one place.** The Dry Run rule, the Several Mods rule (which forces the "Necessary" mesh level and disables the heavier levels) and the FO4 texture-archive rule are applied both on user change and on settings load.
- The unwanted-formats dialog edits a scratch copy through a filter model. OK commits the copy and Cancel discards it.
- Accepted visual deviations: tabs reorder on release, CAO-drawn clamped tooltips, help-cursor overlay, a dialog titled "Unwanted formats", coloured non-selectable Log rows, and About Slint.

### Exe-relative resolution

- `profiles/`, `logs/`, `bin/hkxcmd.exe` and `translations/` resolve against the app directory passed to the composition root. Only `cao-gui`'s `main` derives it from the exe. This is a one-time effect: users who launched C++ CAO from another start-in folder will find their old profiles and logs there.

### Parity oracle and harness (#472)

- **Oracle.** A Release build of the C++ CLI with the GUI off, in its own build directory. One vcpkg `overrides` entry pins DirectXTex to `may2026`. The harness takes the oracle exe from a flag or environment variable, with no discovery.
- **One small oracle-only C++ change,** deleted with the C++ tree:
  - five CLI flags for the archive options (compress, dummies, merge Incompressible, merge textures, delete sources);
  - escaping of `\`, `|`, CR and LF in the event stream's free-text fields;
  - an environment variable that forces CPU BC7.
- **`cao-parity` subcommands:** `run` (the Rust driver, writing `RunFacts` JSON), `corpus`, `case <id>`, and `calibrate`.
- **Facts.**
  - The oracle's stdout and exit code are parsed into a raw `RunFacts`, reading to process exit. Exhaustive hand-written tables map `RunFailureCode`, `SkipReason`, `StartError` and phase, outcome, skip-reason and mutation names. An unknown value is a harness error.
  - The Rust side builds `RunFacts` from typed Run Events and the final Run Evidence.
  - One normaliser and one comparator serve both sides.
  - `cao-parity run` builds the options model from `case.json`, as the GUI does from its widgets, and never builds a `RunRequest` directly.
- **Per-case isolation.** Each case has `input/`, `oracle/` and `rust/` copies on one volume, with identical Mod Root names and private `profiles/`, `bin/hkxcmd.exe` and `logs/`. The oracle's working directory is its case `oracle/` folder. The Rust driver gets `rust/` as its app directory.
- Cases run sequentially, oracle first, with a per-case timeout. Passing cases are deleted.
- **Locality.** Local only, on a Windows dev machine with a GPU. `cargo test -p cao-parity` covers only the parser, normaliser, comparator and mapping tables, using captured transcripts.

### Equivalence (#467)

- Verdicts are Identical, Equivalent or Different. Both builds run on the same host and driver with the same DirectXTex.
- **Output tree.** The same case-sensitive relative paths. Untouched files are byte-identical. Timestamps and attributes are ignored.
- **Textures.** The DDS header is byte-identical. Pixels are byte-identical, except BC7/BC6H, which must reach PSNR ≥ 40 dB per mip and face after decoding. The 40 dB figure is provisional until calibration.
- **Meshes, Animations and Loading Plugins:** byte-equal. If nifly drifts between the two builds, align compiler flags before loosening any rule.
- **Archives.** Compare format, version, flags, container kind and name-table presence. Compare entry names, compression state, DX10 chunk counts and mip ranges, and each entry's decompressed content under its own Asset Kind's rule. Never compare compressed bytes, offsets or entry order.
- **Run facts.**
  - Compared in order: Run Outcome, final Run Phase, Cancellation Observed, the phase sequence with Phase Skip Reasons, and each phase's final progress tuple.
  - Compared as multisets: Mod Roots, Run Failures, Asset Failures, Archive Failures, Archive Collisions, Skip Reason counts, Committed Mutations Retained, cleanup failures, and Run Diagnostics.
  - Message text is never compared.
- **Leftovers.** `.caobad` and `.bak` files are byte-identical. Staging paths are compared after placeholder normalisation. `ownership.manifest` is compared semantically. `owner.lock` is compared for presence only.
- The HTML log is not compared.

### Corpus generator (#473)

- **`case.json`** holds three parts: the `CaseSpec`, GUI-reachable `profile_overrides` (written into each side's private `profile.ini` with the INI writer), and a declarative tree recipe:
  - content entries;
  - `fs_shape` operations: hardlink, junction, file symlink, read-only, reserved name;
  - a `raw` escape.
- **Materialisation.** One seeded materialiser writes content once into `input/` and copies it byte for byte. Filesystem-shape operations are applied to each side separately.
- **Generated cases** come from a deterministic pairwise covering array over options, overrides and tree features. Faults are not dimensions.
  - **Budget:** about 30 minutes, 150–250 cases, at most 64 files and 32 MB each. Generated cases are trimmed before seeds.
  - Generated cases are never committed. A `GENERATOR_VERSION` constant is recorded in each report.
- **Textures are synthetic**, built with `directxtex`.
- **Local asset pool.** LE/SSE meshes and animations come from a gitignored local pool of the user's own Skyrim BSAs, hash-pinned in a committed list. A missing or changed pool makes the case not run. Synthetic meshes come from the `nifly-sys` `corpus` feature.
- **Seeds.**
  - Each static C++ scenario becomes one seed recipe, with an origin field naming the C++ test.
  - Untranscribable scenarios are listed with their reason, for the Rust-test backlog.
  - A fault seed family is provoked purely by the tree.
- **Deviation guard.** One rule per deviation-list entry, with a unit test that every deviation has a rule. A trigger is a harness error. The guard also checks the work directory's path and rejects profile values the GUI can't produce.
- **Naming.** A fixed name vocabulary keeps triggers out. Game paths stay ASCII, and absolute paths are capped at 400 UTF-16 units.
- **Calibration.** `calibrate` runs the BC7/BC6H cases through the oracle on the GPU and with CPU forced. The threshold stays 40 dB unless the observed floor is lower, in which case it becomes the floor minus 1 dB. It is recorded as a constant with host and driver.

### Deviation list (fix, don't copy)

This is the authoritative list. Each item is pinned by a Rust-only test and kept out of the corpus.

1. `profiles/`, `logs/`, `bin/hkxcmd.exe` and `translations/` resolve relative to the exe.
2. Cancel in the unwanted-formats dialog reverts edits.
3. The Dry Run and Several Mods UI rules also apply on INI load.
4. The translation loader bug. This is moot while only English ships.
5. No fixed-size path buffer overflow in the texture loader.
6. The texture metadata check really compares. In C++ it uses `||` and is a no-op.
7. FO4 DX10 cubemaps in BC7, BC6H and sRGB extract successfully.
8. An Archive over 4 GiB errors instead of being written corrupt.
9. All-digit plugin stems are handled.
10. Dead data files are left alone (`customLandscape.txt`, the profile `DummyPlugin.esp`, `animationFormat`, `bBsaLeastBSA`).
11. `bsaGame` outside 3/4/5 makes the profile unreadable. There is no SSE fallback and no TES3, TES4 or FNV tables.
12. A scalar where a list is expected reads as a one-element list.
13. A `#` line is a comment. Any other non-`;` line with no `=` still makes the profile unreadable.
14. A profile INI without a BOM decodes as UTF-8 when valid, Latin-1 otherwise. The writer is unchanged.
15. Device-name detection trims trailing spaces and dots from the stem.
16. Packing Exclusions match within the Mod Root.
17. Headpart Meshes:
    - Dry Run applies the facegen rule.
    - Paths are matched within the Mod Root.
    - The plugin scan skips CAO staging.
    - The HDPT parser can't overflow or hang. A truncated HDPT group or record makes the plugin unreadable, and the run carries on.
    - Compressed HDPT records are decompressed.
18. Resize target width and height are validated only when resizing by size is enabled.
19. A Several Mods child named `.cao-staging…` is never a Mod Root and is skipped silently.
20. A separator Mod Exclusion is a child name ending in `_separator` (case-sensitive). The full-path "separator" rule in empty-directory pruning is removed.
21. FO4 never merges textures into `GNRL`, and "create texture archive" is checked and disabled under FO4, including on INI load.
22. Application Log format: `{module::path@line}`, `fatal` written as `ERROR`, HTML-escaped text, and `\n` after each record.
23. Toggling debug logging between runs no longer drops the log.
24. The Log tab is fed by the sink, isn't truncated, and shows startup records.

### Glossary and ADR updates (resolves the map's "not yet specified" item)

- **ADR 0003, early.** Record the port to Rust and Slint in a new ADR in the first slice:
  - the C++ CLI survives only as a parity oracle and is then deleted;
  - output must be equivalent, not byte-identical;
  - deviations are fixed, not copied.

  It is hard to reverse, surprising without context, and a real trade-off.
- **At C++ deletion:**
  - Amend ADR-0001's wording. Standard-C++ scheduler → `std::thread` Run Worker. Qt confined to presentation dispatch → the Slint event-loop dispatcher. "The CLI waits on the handle" → the parity driver waits.
  - Amend ADR-0002. bethutil → the `cao-archive` port over `ba2`.
  - Update the glossary entries that name the CLI or standard C++: the Optimization Run Service mentions "GUI and CLI adapters", and the Run Scheduler says "standard C++".
  - Update the staging-ownership doc. Drop the POSIX paragraphs. The manifest's quoting grammar stays defined as it is, so recovery keeps reading C++-written manifests.
  - Remove the CLI doc.

### Slice order (input to the backlog breakdown)

Migrate in vertical slices. Each slice lands with its tests and, where the oracle can reach it, a corpus comparison.

1. **Workspace scaffold.** Virtual workspace, toolchain pin, lints, manifest embedding with the code-page test, and ADR 0003.
2. **Oracle-only C++ change.** Archive-option flags, event escaping, the CPU-BC7 override, the DirectXTex `overrides` pin, and the oracle build doc.
3. **`cao-parity` fact pipeline.** `RunFacts`, the oracle parser and mapping tables, the normaliser, the comparator, the case layout and `CaseSpec`. Unit-tested against captured transcripts.
4. **Tracer bullet.** A Dry Run over loose Textures in one Mod Root, through the composition root, compared with the oracle. It stands up thin versions of `cao-winfs`, `cao-profiles` (INI port and options model), `cao-core` (routing, executor, service) and the texture backend.
5. **Widen.** In parallel where the crates allow:
   - Apply-mode staging, publication and recovery;
   - the remaining texture behaviour and GPU BC7 through the directxtex fork;
   - `nifly-sys` and Meshes, with Headpart Meshes and Mesh Reference Maintenance;
   - Animations;
   - Archive discovery, extraction and the Capacity Check over `cao-archive`;
   - Archive Finalization with Loading Plugins;
   - the Application Log sink.
6. **Corpus generator pieces.** Each must land before the engine slices it covers can be compared:
   - the recipe format and materialiser;
   - the deviation guard;
   - the local asset pool;
   - the `nifly-sys` `corpus` feature;
   - seed transcription.
7. **`cao-gui`.** Port the prototype onto the real composition root: windows, dialogs, run wiring, Log tab and UI rules.
8. **Parity sign-off.** A full corpus pass, then BC7 calibration.
9. **Deletion.** Delete all C++, the oracle change and the CMake/vcpkg files. Make the glossary, ADR and doc updates above.

## Testing Decisions

- **What makes a good test here.**
  - Assert external behaviour only: Run Outcome, Run Evidence, Run Events, and the filesystem state a user would see. Never assert private structure.
  - Port each C++ scenario as a scenario (intent, fixture tree, assertions) against the Rust seams. Never transliterate C++ fixtures or test C++-only seams.
  - Each deviation gets one Rust-only test that asserts the fixed behaviour.
- **Seam 1: the differential corpus** (`cao-parity` over the shared composition root). The only oracle for engine output and whole-run facts: Texture decisions, mesh levels, `hkxcmd`, created Archive content, and Loading Plugins. It runs locally with a GPU, the oracle and the local asset pool. A case missing a prerequisite is reported as not run.
- **Seam 2: the Optimization Run Service in `cao-core`,** with fakes of the ported seam traits plus real temporary directories. About 235 contract scenarios the corpus can't reach (fault injection, cancellation, crash windows) are ported here. Priority:
  1. DurableStaging
  2. RunExecutor
  3. AssetExecution
  4. AssetRun
  5. ArchiveFinalization (pure logic with a fake reader and probes; real-archive cases go in `cao-optimizers`)
  6. ArchiveFirstAssetDiscovery (injected cases)
  7. AssetRouting
  8. GuiRun
  9. the Run Handle subset of OptimizationRunService
  10. selected RunEvidence, MainOptimizer, ApplicationRunSetup/RunSetup and MeshReferenceMaintenance cases

  The CLI suites, the plog suite and the Qt dispatch suites are dropped. Their intent is re-derived from ADR-0001 where it still applies.
- **Seam 3: leaf modules.**
  - `cao-profiles`: INI round-trip fixtures, including TES5 behaviour, `@Variant` and `@Invalid()`, and Qt-written bytes.
  - `cao-winfs`: unit tests of the primitives, the MSVC-canonical helper and the code-page check.
  - `cao-optimizers`: archive finalization against real Archives (Loading Plugins, Dummy Plugin bytes, capacity), FO4 DX10 Textures BA2s always compressed, and golden tests of the Application Log format.
- **Seam 4: the GUI,** through Slint's headless testing backend driving the real main window and dialog components. Find elements by accessible label, invoke their actions, and read their checked and enabled state. This covers:
  - the Dry Run and Several Mods rules on INI load and on click;
  - FO4's forced, disabled "create texture archive";
  - Cancel reverting the unwanted-formats dialog;
  - the run lock's enable/disable rules.

  It is the one new seam. It sits at the highest point, with no extracted view-model, so the `.slint` bindings themselves are under test.
- **New tests the C++ suites lack:** a literal v2 manifest fixture, TES5 profile behaviour, the INI round-trip, and the GUI rules on the deviation list.
- **Prior art.**
  - The C++ QtTest suites (scenario intent and fixture shapes).
  - The nifly prototype from #460, whose output was byte-identical to nifly's own fixtures.
  - The Slint prototype on branch `prototype/slint-main-window` (UI structure and run wiring).
  - The research notes linked from #459–#465.

## Out of Scope

- The CLI as a product. It was never released and is dropped. The C++ CLI survives only as the parity oracle.
- New features, including Starfield archives, a version resource, and writing run-detail rows to the Application Log.
- Activating translations. Only English ships, though strings are wrapped in `@tr`.
- Cloning QDarkStyle pixel for pixel.
- POSIX or any non-Windows target.
- Release packaging, an installer, or a static CRT.
- CI. The corpus and tests run locally.
- Bumping nifly beyond `5504832`, or adopting `autocxx`, `cxx`, `tracing`, tokio or an INI crate.
- TES3, TES4 and FNV archive tables, and a TES4 profile.
- Reusing the abandoned "Plan the Rust and Slint conversion" map (#65), its tickets, or its blueprint.

## Further Notes

- **Map.** #458 holds the Notes, the full Decisions so far, and the authoritative deviation list. Each decision above traces to a closed child ticket, #459–#473. Read the ticket for its cited facts and research notes before implementing that area.
- **Performance bar.** No meaningful regression on a BC7-heavy run. This is why GPU BC7 and parallel archive compression are kept.
- **Open risks to watch during implementation:**
  - CAO now owns a `directxtex-rs` fork. The GPU binding was spike-verified only up to the shader compile.
  - nifly float drift between the `cc` and vcpkg builds. Align flags first. A structural ulp comparator is the recorded fallback.
  - The Slint dependency on `unstable-winit-030` and the undocumented `SLINT_NO_MUDA` is why `slint ~1.18` is pinned.
  - MSVC-canonical text for volumes with no drive letter, and for mapped network drives, was not verified live.
  - Dropping a `ba2` memory map before deleting or renaming its source must be verified in the extraction slice.
  - The `@Variant` encodings come from reading Qt's source. The INI slice must confirm them against the C++ build.
- **Next step.** Break the slice order into backlog tickets with blocking edges, as the map's final step.
