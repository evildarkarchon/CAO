## Resolution

The generator is part of `cao-parity`. Each case's tree is a **declarative recipe** in `case.json`, beside the `CaseSpec`. One **materialiser** turns a recipe into files. Generated cases come from a deterministic **pairwise covering array**. Hand-written **seeds** and **fault seeds** use the same recipe format. A **deviation guard** rejects any trigger on the map's fix-don't-copy list.

### Case shape

- **`case.json`** has three parts:
  - the `CaseSpec` options fixed by "Decide how the harness runs both builds and compares their facts";
  - `profile_overrides` (see below);
  - a `tree` recipe.
- **Recipe entries:**
  - **Content entries.** Textures (format, size, mips, header kind, cubemap or array, content pattern), meshes (synthetic or from the local pool), animations (from the local pool, or placeholder bytes), plugins, input Archives with their own entries, and fault decorators such as `truncate`, `garbage` and `zero`.
  - **`fs_shape` operations:** `hardlink`, `junction`, `file_symlink`, `readonly`, and `reserved_name` (created through `\\?\`).
  - **A `raw` escape:** inline base64, or a path to a small committed fixture, for shapes the vocabulary can't express. Examples are archive entry names the `ba2` writer won't produce (patched after writing, as `createRawArchive` does) and staging manifests templated per side.
- **Materialisation:**
  - Content is written **once** into `input/` and copied byte for byte to `oracle/` and `rust/`. Input encoder nondeterminism (BC7 inputs) therefore doesn't matter.
  - `fs_shape` operations are then applied identically to all three, because a plain copy breaks hard links, junctions, symlinks, attributes and `\\?\`-only names.
  - A `file_symlink` case is **not run** when the process can't create symlinks.
- **Determinism:**
  - Content is seeded by the case ID.
  - Generated cases are never committed. The corpus is regenerated on every run, and `case <id>` rebuilds the same bytes.
  - A `GENERATOR_VERSION` constant is recorded in each `report.md`. A replay against a different version warns that the bytes may differ.
  - Case IDs are stable descriptive names, e.g. `pw-017-sse-sm-apply`.

### Profile overrides

The C++ CLI has no flags for profile settings, and both sides otherwise get pristine copies of `profiles/`. Without overrides, BC1, BC5 and uncompressed outputs and TES5 TGA conversion would never be compared.

- **What `profile_overrides` holds.** Only values the GUI can produce:
  - **output format:** BC7, BC5, BC3, BC1, or R8G8B8A8;
  - **unwanted formats:** the profile default, empty, or one alternative set from the dialog's list;
  - **compress interface:** on or off;
  - **TGA conversion:** on or off;
  - **mesh target:** one of three game presets, LE `20.2.0.7/12/83`, SSE `12/100`, or FO4 `12/130`. Arbitrary user/stream/version combinations are not allowed.
- **Not overridable:**
  - the archive game, which is tied to the profile's identity;
  - the maximum archive size, because C++ takes the larger of it and bethutil's maximum, so it can never lower the limit. Archive splitting would need about 2 GiB of input, so it stays a Rust-only test.
- **How it is applied.** The harness writes the overrides into each side's private `profiles/<P>/profile.ini` with the QSettings-compatible writer, so both sides read the same bytes the GUI's save would produce. These are not hand-edits; the guard still rejects anything the GUI can't produce.

### Parameter space

- **A deterministic pairwise covering array**, generated in code. The dimensions are:
  - profile, Apply or Dry Run, `om` or `sm`, requested work, texture, mesh and animation options, archive extract/create/delete-backup, the five archive options, and the profile overrides;
  - **tree features:**
    - input Archives: none, one, or two colliding;
    - Loose-over-Archived overlap;
    - a headpart plugin present;
    - the texture-format mix;
    - LE or SSE input meshes;
    - an existing Loading Plugin: none, exact dummy, near-dummy, or full plugin.
- **Constraints:**
  - FO4 has meshes and animations disabled, and requesting them is a `PolicyConflict`.
  - Only SSE has animations.
  - FO4 never merges textures into `GNRL`.
- **Start Error specs:** a handful are kept deliberately, because the comparator checks Start Error.
- **Faults are not dimensions.** They live in the fault seed family, so a single fault doesn't flood the array with Completed With Failures outcomes.
- **No random soak mode,** until a pairwise run passes and a bug suggests that three-way interactions matter.

### Budget

- A whole `corpus` run takes **about 30 minutes** on the dev machine with a GPU.
- **150–250 cases** in total, seeds included.
- Per case: **at most 64 files and 32 MB of input**. Most textures are 512² or smaller, with a few sized above the resize limits.
- The per-case timeout is the harness default.
- If the budget is overrun, generated cases are trimmed, not seeds.

### Asset inputs

- **Textures:** synthetic only. The materialiser uses the `directxtex` crate on seeded procedural images: gradients, noise, hard edges, alpha ramps and binary masks, and normal-map-like content for `_n` paths.
  - **Formats:** BC1/3/5/7 and uncompressed.
  - **Header and layout:** legacy or DX10 header; cubemaps and arrays; odd and non-power-of-two sizes; with or without mips.
  - **TGA:** written with `SaveToTGAFile`.
- **Meshes and animations: a local asset pool.**
  - **Lookup:** `--local-assets`, then `CAO_LOCAL_ASSETS`, then `<workspace>/tests/local`, which is gitignored and holds the user's own Skyrim LE and SSE `Animations` and `Meshes` BSAs.
  - **Selection:** a committed `crates/cao-parity/local-assets.toml` lists each usable entry by archive, internal path and SHA-256. That is about 20 LE animations, a few SSE animations, and about 30 LE and 30 SSE meshes, covering static, skinned, headpart, facegen, `.btr` and `.bto`.
  - **Missing or changed pool:** the case is reported as **not run**, never as passing.
  - **Reading the pool:** the materialiser extracts entries with `ba2`.
- **Animations:**
  - **Conversion inputs:** LE 32-bit animations. `hkxcmd convert -v AMD64` on them is deterministic, and its output was **byte-identical to Bethesda's SSE files** in all three samples checked.
  - **SSE 64-bit animations** cover the failure path: `hkxcmd` prints "File is not loadable" and exits 0, which CAO records as an Asset Failure.
  - **Malformed animations:** placeholder bytes.
  - **Not used:** synthetic HKX made from XML.
- **Synthetic meshes:** built by nifly through a `corpus` Cargo feature on `nifly-sys` (`Create`, `CreateShapeFromData`, `SetTextureSlot`), as the C++ tests do. They cover Mesh Reference Maintenance and structural cases, so mesh parity doesn't depend entirely on the pool. nifly's own test fixtures are not used, because some are game-extracted.
- **Plugins:**
  - **Headpart plugins:** a port of `writeHeadpartPlugin`, as `{kind: plugin, ext, headparts, extra_groups}`. It writes a TES4 header, optional non-HDPT GRUPs, and one HDPT record with a single MODL per headpart. MODL paths are written both with and without the `meshes/` prefix.
  - **Loading Plugins:**
    - exact Dummy Plugins, using the canonical bytes from the Rust archive module's per-game tables;
    - near-dummies one byte off;
    - full plugins named after an existing Archive.
  - **Never generated:** compressed records, MODL fields of 1024 bytes or more, unterminated MODL fields, and truncated HDPT groups or records.
- **Input Archives:** written by the materialiser with the pinned `ba2 =3.0.1` writer, with explicit options.
  - **Formats:** TES5 BSA v104, SSE v105 (compressed and uncompressed), FO4 `GNRL`, and FO4 `DX10`.
  - The oracle reads them with rsm-bsa, so any reader disagreement still shows up as Different.
  - Names the writer won't produce use the `raw` escape.
  - No DX10-format cubemaps (BC7, BC6H or sRGB) go into FO4 `DX10` BA2s.

### Seeds

- **Transcribed C++ scenarios:** each static C++ scenario becomes one recipe at `crates/cao-parity/seeds/<CppSuite>/<test_row>.json`, with an `origin` field naming the C++ test and row. Exact duplicates are merged.
- **Untranscribable scenarios:** a scenario that needs something the oracle CLI can't express (explicit Archive Precedence, an injected fault, an INI hand-edit) is listed in `seeds/UNTRANSCRIBED.md` with the reason, so the porting backlog picks it up as a Rust test.
- **Fault seeds:** a hand-written family, each provoked purely by the tree.

| Family | Seeds |
|---|---|
| Preparing | missing selection; a drive root; a Several Mods child junctioned to its parent; two children junctioned to the same target; unverified `.cao-staging` (a file, no `owner.lock`, a bad manifest) |
| Staging residue | leftover v1, v2 and v3 manifests with `owner.lock` and no live lock, templated per side with the MSVC-canonical Mod Root path |
| Archive preflight | zero-byte or garbage archive; each Unsafe Game Path class (empty, absolute, `..`, control or forbidden characters, trailing dot or space, device stems, invalid UTF-8); aliased entries; a file and a directory at the same game path; an entry over an occupied leaf; a declared size of 1 PiB or more, so the Capacity Check fails whatever the estimate formula |
| Collisions | two or three colliding Archives; Loose-over-Archived shadowing |
| Extraction | a valid index with a corrupt payload; a hard-linked Archive; an existing `.bak` |
| Loose Assets | garbage, zero-byte or truncated DDS, TGA and NIF (Quarantine, and an existing `.caobad`); malformed HKX; SSE HKX under conversion; a reserved-name, hard-linked or symlinked Loose Asset; a zero-byte TGA under conversion; read-only DDS and TGA |
| Finalization | a non-plugin occupying a dummy-plugin name; all 255 output names taken; read-only packed sources with delete-sources on |

- **Excluded from the corpus:**
  - near-threshold capacity, because the estimate formulas are internal;
  - a single file over the archive limit, because it is beyond the size budget;
  - anything needing a race, a held lock, or free space that changes mid-run.

  These belong to the ported Rust tests.

### Keeping deviation triggers out

- **A deviation guard** in `cao-parity`.
  - It holds one rule per entry on the map's fix-don't-copy list. A unit test checks that every deviation has a rule.
  - Any recipe or spec, seed or generated, that contains a trigger is a **harness error**, never a Different.
  - The guard also checks the work directory's absolute path before a run.
  - It rejects profile values the GUI can't produce.
- **A fixed name vocabulary** for case IDs, Mod Root names and directories avoids every trigger:
  - `separator` in any case, except an `<name>_separator` Several Mods child containing nothing that leaves an empty directory (both sides exclude it);
  - `facegen` and `meshes` above the Mod Root;
  - the `FilesToNotPack.txt` lines;
  - `.cao-staging`;
  - all-digit plugin stems;
  - device stems with trailing spaces or dots;
  - `facegen` meshes in Dry Run with mesh work.

  Absolute paths are capped at **400 UTF-16 units**, well under the 1024-unit texture-path overflow.
- **Non-ASCII names:**
  - **Game paths stay ASCII,** because the games crash on non-ASCII asset paths.
  - **Allowed non-ASCII:** the Several Mods parent, Mod Roots, and plugin names, and therefore the names of Archives and Loading Plugins created from a Mod Root. The oracle CLI embeds the same manifest as the GUI, with `activeCodePage UTF-8` (`src/CMakeLists.txt:161`), so C++ opens non-ASCII plugin paths correctly.
  - **Exception:** the invalid-UTF-8 Unsafe Game Path seed stays, because it tests that both sides reject the entry before extraction.

### New deviations (added to the map)

- **HDPT parser:** it can't overflow **or hang**. A truncated HDPT group or record makes C++ loop forever, because `tellg()` returns -1. The port reports the plugin as unreadable and carries on.
- **Compressed HDPT records:** the port decompresses them. C++ silently misreads them.
- **Resize target validation:** the width and height are validated only when resizing by size is enabled. C++ raises a Start Error for an odd value even when that resize mode is off.
- **`.cao-staging` Mod Roots:** a Several Mods child named `.cao-staging…` is never a Mod Root, and is skipped silently. In C++ it becomes a Mod Root that discovery skips but finalization plans.

Each is kept out of the corpus by the guard and pinned by a Rust-only test.

### Not glossary or ADR material

This is verification machinery, not CAO's domain, so `GLOSSARY.md` is unchanged. No decision meets the ADR bar: all of it is internal to `cao-parity` and deleted or reshaped freely.

### Facts relied on

Traced at `c7c549d`:

- **No binary asset fixtures exist in the repo.** C++ tests build everything in a `QTemporaryDir`:
  - DDS: `tests/MainOptimizerTests.cpp:194`;
  - NIF: `:114`;
  - HDPT plugin: `:152-173`;
  - archives: `tests/ArchiveFirstAssetDiscoveryTests.cpp:85-193`.
- **Profile capabilities** (`profiles/*/profile.ini`):
  - FO4 sets `meshesEnabled=false` and `animationsEnabled=false`, and TES5 sets `animationsEnabled=false` and `texturesConvertTga=false`;
  - a request contradicting them is a `PolicyConflict` (`src/Run/ApplicationRunSetup.cpp`).
- **GUI-reachable profile values:**
  - output formats and mesh combos: `src/MainWindow.cpp:25-49`;
  - unwanted formats: `TexturesFormatSelectDialog`;
  - `bsaMaximumSize` is a `QDoubleSpinBox` (`src/MainWindow.ui:466`), and the effective limit is the larger of it and bethutil's maximum.
- **Fault shapes:**
  - `src/Run/ArchiveFirstAssetDiscovery.cpp:31-648`;
  - `src/Run/RunExecutor.cpp:97-163`;
  - `src/Run/StagingRecovery.cpp:657-690`;
  - `src/MainOptimizer.cpp:17-29,134-153`;
  - `src/Run/TemporaryArtifactRegistry.cpp`;
  - `src/Run/ArchiveFinalizationPlanning.cpp:100-187`;
  - `src/Run/ArchiveFinalizationLoadingPlugins.cpp:152-248`.
- **HDPT parser hang and overflow:** `src/PluginsOperations.cpp:44-58`.
- **Resize validation:** `src/Run/ApplicationRunSetup.cpp:125-138`.
- **`hkxcmd`:**
  - version 1.4.0.0 reads XML and binary packfiles and writes `WIN32` or `AMD64`;
  - checked locally on the user's own LE (`BSA v104`) and SSE (`BSA v105`) `Skyrim - Animations.bsa`: LE→AMD64 conversion was deterministic and byte-equal to the SSE originals, and SSE input gave "File is not loadable" with exit 0.
- **No freely licensed Skyrim LE `.hkx` animation was found** in public repositories (Pandora, Nemesis, serde-hkx, hkxpack, ck-cmd, pyffi).
