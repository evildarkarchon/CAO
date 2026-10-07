## Destination

A locked migration spec for rewriting CAO in Rust with a Slint GUI and the `ba2` crate, plus the implementation backlog (vertical slices with blocking edges) ready for agents to pick up.

## Notes

- **Posture: port it, keep it simple.** This is a port, not a re-architecture or a release-engineering programme. An earlier attempt ("Plan the Rust and Slint conversion", #65) was abandoned because its scope was over-engineered (worker AppContainers, journaled publication protocol, repair tool, SBOM/release gates). That map, its tickets, and its blueprint are **not evidence and carry no authority** here; do not cite or reuse them.
- **Planning only.** Resolve decisions; do not implement the port in this map.
- **Parity contract.** `GLOSSARY.md`, `docs/adr/0001` and `0002`, and `docs/architecture/staging-ownership.md` define the run semantics that must survive (Run Outcome classification, Temporary Ownership, Safety Cleanup, Archive Precedence, Loose-over-Archived precedence, Dry Run, staged publication). Internal structure is free. Output must be **equivalent**, not byte-identical.
- **Interface.** GUI only. Structural parity with `src/MainWindow.ui` and `src/TexturesFormatSelectDialog.ui`: same windows, tabs, groups, controls, labels, enable/disable rules, and workflow, using a dark Slint style. `AboutSlint` replaces "About Qt". Wrap all strings in `@tr`, but ship English only.
- **Engines.** Keep the same native engines: DirectXTex through the `directxtex` crate, including GPU BC7 encoding (mechanism to be decided); nifly through a hand-written shim; `hkxcmd.exe` as a subprocess; `ba2` for archives. The performance bar is no meaningful regression on a BC7-heavy run.
- **Compatibility.** Existing `profiles/` (`common.ini`, `settings.ini`, `profile.ini`, auxiliary text files) work unchanged, including raw `btu::Game`, `DXGI_FORMAT`, and `NiFileVersion` integers and QSettings list encoding. Read, recover, and write the `CAO-STAGING` v1–v3 ownership manifest. Keep the HTML log location and format.
- **Licensing.** Slint under GPL-3.0 (the binary already links GPL-3.0 nifly).
- **Transition.** A Cargo workspace grows alongside the C++ tree in this repo. A C++ CLI build (`CAO_BUILD_GUI=OFF`) serves as the differential parity oracle until parity, then all C++ is deleted. When running the oracle, set its working directory to its exe folder so both builds resolve `profiles/` the same way. Migrate in vertical slices.
- **Parity oracle.** A differential corpus: run the C++ oracle and a Rust test driver on the same generated mod trees, and compare outcomes, evidence, and output trees under an equivalence rule. Port the C++ test scenarios as Rust tests too, but only if the C++ tests cover enough.
- **Fix, don't copy (deviation list).** Fix clearly unintended behavior instead of replicating it, and record each fix here:
  - Resolve `profiles/`, `logs/`, `bin/hkxcmd.exe`, and `translations/` relative to the exe, not the working directory. A one-time effect: users who launched C++ CAO from another start-in directory will find their profiles and logs there, not next to the exe.
  - "Cancel" in the unwanted-formats dialog must revert edits.
  - Apply the Dry Run and Several-mods UI rules when settings are loaded from the INI, not only on click.
  - The translation loader bug (`src/main.cpp:45`). This is moot while the port ships English only.
  - The `wchar_t[1024]` path overflow in the texture loader (`src/TexturesOptimizer.cpp`).
  - The no-op `compareInfo` check (uses `||`) in `src/TexturesOptimizer.cpp`.
  - Extracting FO4 DX10 cubemaps in DX10-only formats (BC7, BC6H, sRGB) fails in C++ (rsm-bsa against DirectXTex may2026). It works with `ba2`. Source: the `ba2` research ticket.
  - A BSA over 4 GiB, reachable only through a hand-edited INI, must error instead of being written corrupt. `ba2` already errors.
  - An all-digit plugin name such as `2.esp` triggers undefined behaviour in bethutil's name parsing. The port must handle it.
  - Leave dead data files alone (`customLandscape.txt`, `profiles/*/DummyPlugin.esp`, `animationFormat`, `bBsaLeastBSA`).
  - A `bsaGame` value other than TES5 (3), SSE (4) or FO4 (5) makes the profile unreadable, naming the value. There is no SSE fallback, and no TES3, TES4 or FNV tables. Source: the undocumented-behaviours ticket, as are the entries below.
  - A plain scalar where an INI list is expected (`texturesUnwantedFormats=85`) reads as a one-element list, not an empty one.
  - A profile INI line starting with `#` is a comment. Any other non-`;` line with no `=` still makes the profile unreadable.
  - A profile INI without a BOM is decoded as UTF-8 when valid, and as Latin-1 otherwise. The INI writer is unchanged and still writes what Qt writes.
  - Device-name detection for Unsafe Game Paths trims trailing spaces and dots from the stem (`NUL .txt`).
  - Packing Exclusions (`FilesToNotPack.txt`) match against the path within the Mod Root, not the absolute path.
  - Headpart Meshes:
    - Dry Run applies the `facegen` rule.
    - Paths are matched within the Mod Root.
    - The plugin scan skips CAO staging directories.
    - The HDPT parser can't overflow.
  - A separator Mod Exclusion is a child name ending in `_separator` (case-sensitive), not any name containing `separator`. The full-path "separator" rule in empty-directory pruning is removed.
- **Platform and scope.** Windows is the target. Profiles: FO4, SSE, TES5. TES4 has no `profile.ini` and is not discoverable today.
- **Skills.** Work grilling tickets with `/grilling` and `/domain-modeling`, research tickets with `/research`, and prototype tickets with `/prototype`.

## Decisions so far

<!-- one line per closed ticket: [title](link): gist -->

- [Assess C++ test coverage for scenario porting](https://github.com/evildarkarchon/CAO/issues/465): Port the roughly 235 of 404 C++ scenarios that the corpus can't reach (fault injection, cancellation, crash windows) as Rust tests, and turn about 105 static ones into corpus seeds. Engine-output parity stays with the differential corpus alone.
- [Map the Windows file-safety primitives to Rust](https://github.com/evildarkarchon/CAO/issues/463): Open handles through `std` `OpenOptionsExt`, and do every query and mutation through `windows-sys` on the raw handle. Add `getrandom` and `dunce`, but not `same-file` or `fs4`. Write an MSVC-canonical path helper so C++ staging manifests still recover. Resolve the exe directory with `current_exe().parent()` plus `dunce::simplified`.
- [Keep GPU BC7 encoding with the directxtex crate](https://github.com/evildarkarchon/CAO/issues/459): Fork `directxtex-rs` (staying on 1.x) to bind the D3D11 `Compress` path using prebuilt shaders. Patch it in workspace-wide with `[patch.crates-io]` so `ba2` shares the same copy. Create the device with the `windows` crate and fall back to CPU, as today. A separate shim crate and CPU-only BC7 were both rejected.
- [Port btu::bsa archive behaviour onto the ba2 crate](https://github.com/evildarkarchon/CAO/issues/461): Pin `ba2` to `=3.0.1` and use it unpatched to replace rsm-bsa, setting the option defaults that differ explicitly. Hand-port bethutil's per-game tables, the dummy plugins, file-type classification, `ArchiveData` split and merge, and `FilePath` naming into a small archive module. Add `rayon` for parallel compression.
- [Match QSettings INI compatibility in Rust](https://github.com/evildarkarchon/CAO/issues/464): No INI crate fits. Write a small dependency-free port of the Qt 5.15 QSettings INI reader and writer, about 300 lines. It must handle `@Variant` and `@Invalid()` list encodings, `\x` escapes, and QVariant's lenient conversions, and keep unknown keys when saving.
- [Reproduce the main window's hard parts in Slint](https://github.com/evildarkarchon/CAO/issues/462): Slint 1.18 (pinned `~1.18`), `rfd`, and winit 0.30 cover every hard part.
  - Custom components are needed for the tab bar, checkable group boxes, a double spin box, and grid-placed radio buttons.
  - Workarounds are needed for folder drops (the unstable winit `DroppedFile` hook) and modal dialogs (disable the main window).
  - The native menu bar follows the Windows theme rather than CAO's dark/light toggle.
  - Log lines become parsed severity and text rows instead of HTML.
- [Bind nifly mesh operations from Rust](https://github.com/evildarkarchon/CAO/issues/460): Write a hand-written plain C ABI shim of about 150 lines, with opaque handles and `noexcept` functions that catch every exception. Build it with the `cc` crate over nifly sources vendored at `5504832` with the patch applied, and do not bump nifly. Neither autocxx nor cxx is used. A prototype produced output byte-identical to nifly's fixtures.
- [Prototype the main window in Slint](https://github.com/evildarkarchon/CAO/issues/466): Use Slint's Fluent style with the Slint-drawn menu bar (`SLINT_NO_MUDA`), so the menu follows CAO's dark/light toggle. Keep structural parity with the Qt windows, and accept movable tabs, CAO-drawn clamped tooltips, the help-cursor overlay, a titled formats dialog, and coloured non-selectable Log rows. The resolution also lists constraints the port must follow (no winit `focus_window()`, re-enable the owner before hiding a dialog, a depth-counted modal guard) and about 400 lines of custom widgets. Prototype: branch `prototype/slint-main-window`.
- [Choose the Rust workspace crates and seams](https://github.com/evildarkarchon/CAO/issues/468): A virtual workspace under `crates/` with eight crates: `cao-core`, `cao-winfs`, `cao-profiles`, `cao-archive`, `nifly-sys`, `cao-optimizers` (backends plus the shared composition root), `cao-gui`, and `cao-parity`. `cao-core` depends only on `cao-winfs` and reads Archives through a trait, so it builds without C++. The C++ seams are ported one-to-one as `dyn` traits, with no new filesystem, clock, or nonce seams. Errors are typed `thiserror` enums, with panics caught at the C++ boundaries. Each run gets one `std::thread` worker, with no async runtime. Libraries log through the `log` facade. `unsafe` is confined to `cao-winfs`, `nifly-sys`, and the D3D11 module.
- [Classify undocumented tested behaviours as contract or deviation](https://github.com/evildarkarchon/CAO/issues/469): Quarantine, the Archive Capacity Check, Unsafe Game Paths, Packing Exclusions, Headpart Meshes (run-wide across the Mod Selection), and separator Mod Exclusions all stay in the contract and are now glossary terms. Most carry a small fix on the deviation list. Capacity estimate formulas are internal. Four INI hand-edit behaviours become deviations.

## Not yet specified

- **Differential harness design:** the Rust headless test driver, generating the corpus, how the C++ oracle runs, and how results are compared. Waits on the output-equivalence decision. `cao-parity` is a headless binary over the `cao-optimizers` composition root, so it runs the same wiring as the app. The test-coverage research found:
  - The oracle's machine-readable surface is the CLI event stream plus its exit codes.
  - Run IDs, staging nonces, and absolute paths must be normalized.
  - Faults can be provoked through the generated tree.
  - TES5 must be added, because the C++ tests cover only SSE and FO4.
  - Each deviation on the fix-don't-copy list needs an expected-difference rule, or its trigger must stay out of the generated corpus. Examples: hand-edited INIs, `_separator` names, Packing Exclusion paths above the Mod Root.
- **"Parity reached" criteria** for deleting the C++ tree.
- **GLOSSARY and ADR updates** for removing the CLI and moving to Rust.
- **Slice order and implementation backlog breakdown:** the last step before the destination.

## Out of scope

- **The CLI.** It was never publicly released and is dropped. The C++ CLI build survives only as a parity oracle.
- **New features,** for example Starfield archives, even though `ba2` supports them.
- **Activating translations.** The Qt catalogs never loaded; ship English only.
- **Cloning QDarkStyle pixel for pixel.**
- **POSIX or other non-Windows support.**
- **Release packaging or an installer.** None exists today.
- **Reusing the abandoned "Plan the Rust and Slint conversion" map,** its tickets, or its blueprint.










