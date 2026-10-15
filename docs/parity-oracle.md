# The parity oracle

The C++ CLI is the **parity oracle** for the Rust port ([ADR 0003](adr/0003-port-cao-to-rust-and-slint.md)).
The `cao-parity` harness runs it and the Rust driver on the same corpus case, then compares
what each produced. The oracle and its oracle-only changes are deleted with the C++ tree.

## Building it

Build a Release CLI, with the GUI off, in its own binary directory:

```
cmake --preset vs2026-windows -B build/oracle -DCAO_BUILD_GUI=OFF -DBUILD_TESTING=OFF
cmake --build build/oracle --config Release --target Cathedral_Assets_Optimizer
```

The exe is `build/oracle/src/Release/Cathedral_Assets_Optimizer.exe`.

- **Own binary directory.** The GUI build produces a target with the same name. The oracle
  must not share a build directory with it.
- **Release.** Qt is linked statically, so the exe needs only the release Visual C++
  runtime. A Debug build needs the Debug CRT, which only machines with Visual Studio have.
- **DirectXTex.** `vcpkg.json` pins DirectXTex to `2026-05-07` (`may2026`) with an
  `overrides` entry. This is the release the Rust port builds, so both sides make the same
  texture decisions. Configure output names the version: `directxtex[...]@2026-05-07`.

The harness never runs CMake. Rebuild the oracle by hand after changing C++ or `vcpkg.json`.

## Giving the harness the exe

The harness takes the oracle exe from `--oracle <exe>`. If that flag is absent, it uses the
`CAO_ORACLE` environment variable. It does not search for the exe. A run without either one
is an error.

The harness runs the oracle with each case's `oracle/` folder as its working directory, not
the exe's folder. `profiles/`, `logs/` and `bin/hkxcmd.exe` resolve against the working
directory.

## Running one case

```
cargo run -p cao-parity -- case tracer-dry-run-textures --oracle <exe>
```

`case <id>` materialises a committed seed, or rebuilds a case kept in the work directory
from its own `case.json`. It runs the oracle, then `cao-parity run` as the Rust driver,
and prints the run-fact and output-tree verdicts. A Dry Run case also checks that each side's
tree still matches `input/` byte for byte. A passing case's folder is deleted; any other is
kept with a `report.md` holding the fact diff and the replay command.

- **Work directory.** `--work <dir>`, by default `target/parity/`.
- **Other flags.** `--profiles <dir>` (default: the repository's `profiles/`),
  `--hkxcmd <exe>` (or `CAO_HKXCMD`, then `bin/hkxcmd.exe`), `--local-assets <dir>` (or
  `CAO_LOCAL_ASSETS`, then `tests/local`; see [The local asset pool](#the-local-asset-pool)),
  `--timeout <seconds>` (default 600).
- **Exit code.** 0 when both verdicts pass, 1 for Different, 2 for a harness error, 3 when
  the case cannot run here (it needs `hkxcmd.exe` and none was found, a file symlink and
  the process lacks symlink rights, or a local asset the pool lacks or holds changed).
- **Paths.** Pass Windows-style paths (`C:\...` or `C:/...`). From Git Bash, a `/c/...` path
  reaches the harness unconverted, and Windows reads it as `C:\c\...`.

## Cases and recipes

A case is one `case.json` with three parts: the `spec` (what both builds are asked to do),
optional `profile_overrides` (GUI-reachable `profile.ini` values), and a `tree` recipe. The
recipe's `content` entries (`texture`, `text`, `raw`, `mesh`, `local_asset`, `plugin`,
`directory`, `archive`) are written once into `input/`, seeded by the case id, and copied byte
for byte to `oracle/` and `rust/`. An `archive` entry packs its own `texture`, `text`, `raw`,
`mesh`, `local_asset` and `plugin` entries, whose paths are the game paths it stores, in the
container its `game` and `type` give: TES5 BSA v104, SSE BSA v105, FO4 `GNRL`, or FO4 `DX10`
for `textures`. It is written with the pinned `ba2` writer directly, never with `cao-archive`,
so the port's own packing code never builds its test inputs (#529). A packed `raw` entry with
`"verbatim": true` is stored under exactly its `path`, a name the writer would not produce
(other case, `/` separators, `..`, device names, control characters, non-ASCII): the writer's
lowercase, `\`-separated name is overwritten after writing, as the C++ tests'
`createRawArchive` does, so the spelling must be the stored name's length. An `archive` may
take a `fault` of its own, which damages the written Archive. The
`fs_shape` operations (`hardlink`, `junction`, `file_symlink`, `readonly`, `reserved_name`,
`mod_root_text`) are then applied to all three copies. `mod_root_text` writes text with each
`{mod_root}` replaced by that copy's own Mod Root, spelled as C++ writes it into a staging
manifest (MSVC `canonical`, `/`-separated), so a leftover `CAO-STAGING` manifest names the
copy it sits in. Recipe paths must keep game paths ASCII and every
absolute path within 400 UTF-16 units. `crates/cao-parity/src/recipe.rs` documents each
field.

A `mesh` entry is a synthetic Mesh that nifly builds through the `nifly-sys` `corpus` feature,
using the calls the C++ tests use: `Create` at an LE, SSE or FO4 `version`, then one
single-triangle shape per `shapes` entry, with its `textures` in texture-set slot order:

```json
{"kind": "mesh", "path": "mods/Mod/meshes/bowl.nif", "version": "sse",
 "shapes": [{"name": "Bowl", "textures": ["textures\\bowl.tga", "textures\\bowl_n.dds"]}]}
```

Its path ends in `.nif`, `.btr` or `.bto`, and it may take a `fault`. Unlike other content, its
bytes are not seeded: nifly builds them from the recipe alone, so every case id gets the same
Mesh. Only `cao-parity` enables the `corpus` feature. Cargo unifies features across one build's
packages, so build the app as its own package (`-p <app>`), not with `--workspace`, to keep
Mesh creation out of it.

A `plugin` entry is a port of the C++ tests' `writeHeadpartPlugin`: a `TES4` header, an empty
group per `extra_groups` label, then one HDPT group with a record holding a single MODL per
`headparts` path, each written byte for byte, with or without its `meshes\` prefix. With no
headparts it is a bare full plugin. With `dummy` (`tes5`, `sse` or `fo4`) it is instead that
game's canonical 49-byte Dummy Plugin from `cao-archive`'s tables, and with `flip_byte` too, a
near-dummy one byte off:

```json
{"kind": "plugin", "path": "mods/Mod/Mod.esp",
 "headparts": ["Mod\\Hair01.nif", "Meshes\\Mod\\Hair02.nif"], "extra_groups": ["GMST"]}
{"kind": "plugin", "path": "mods/Mod/Mod.esl", "dummy": "sse", "flip_byte": 48}
```

Its path ends in `.esp`, `.esm` or `.esl`, and it may take a `fault`. No recipe can write
deviation 17's triggers (compressed records, unterminated MODL fields or ones of 1024 bytes or
more); a `truncate` fault that cuts into the HDPT group is one, and the deviation guard rejects
it.

Committed seeds live under `crates/cao-parity/seeds/`, in any subfolder; a seed's case id
is its file stem. `raw` entries may name a fixture file under
`crates/cao-parity/fixtures/`.

- `seeds/<Suite>/` holds the transcribed C++ scenarios (#507). Each names the C++ tests it
  stands for in `origin`, as `Suite::slot` or `Suite::slot [row]`. Scenarios that cannot
  disturb each other and share their options are folded into one Several Mods seed, one Mod Root
  per scenario, to keep the seeds within the case budget.
- `seeds/UNTRANSCRIBED.md` lists every C++ scenario that is not a seed, or only partly one,
  with the reason. It is the backlog of scenarios to port as Rust tests. `tests/seeds.rs`
  checks that every C++ test slot is one or the other.
- `seeds/fault/` holds the fault seeds, each provoking a fault through its tree alone.
- `seeds/hand-written/` holds the tracer and the other hand-written cases.

## The local asset pool

Real Skyrim LE and SSE Meshes and Animations cannot be committed, and no freely licensed LE
animation exists. A `local_asset` entry therefore takes its bytes from the **pool**, a
gitignored folder holding your own Skyrim BSAs:

```json
{"kind": "local_asset", "path": "mods/Mod/meshes/hair01.nif", "asset": "sse-headpart-hair01.nif"}
```

`asset` names an entry of the **pinned list**, `crates/cao-parity/local-assets.toml`, which
is committed. Each entry gives the edition, the BSA, the internal path and the SHA-256 of the
extracted bytes. A `local_asset` entry may also take a `fault`, as a `texture` does, and its
path must keep the pinned entry's extension.

- **Missing or changed.** When the pool lacks the BSA or the entry, or the entry's bytes no
  longer match the pinned SHA-256 (another game patch, say), every case using it is
  reported as **not run**, with the reason. It never passes.
- **Unknown id.** An `asset` the pinned list lacks is a harness error, even with no pool.

### Populating it

The harness looks for the pool at `--local-assets <dir>`, then `CAO_LOCAL_ASSETS`, then the
repository's `tests/local/`, which is gitignored. It holds one folder per edition, each
with that edition's BSAs under their own names:

```
tests/local/
  le/   Skyrim - Animations.bsa  Skyrim - Meshes.bsa
  sse/  Skyrim - Animations.bsa  Skyrim - Meshes0.bsa  Skyrim - Meshes1.bsa
```

1. Copy the BSAs from each install's `Data` folder: Skyrim LE (`Skyrim`) into `le/` and
   Skyrim Special Edition into `sse/`. The pinned list's `archive` fields name the files it
   needs. A directory junction to the `Data` folder works too, and copies nothing:

   ```
   mklink /J tests\local\sse "C:\...\Skyrim Special Edition\Data"
   ```

2. Check the pool:

   ```
   cargo run -p cao-parity -- local-assets
   ```

   It prints `ok`, or `missing` with the reason, for each pinned entry, and exits 0 when the
   pool supplies them all.

### Adding entries

Add an `[[asset]]` table to `local-assets.toml` with `id`, `edition` (`le` or `sse`),
`archive`, `path` (the internal path, `/`-separated), `category` (`static`, `skinned`,
`headpart`, `facegen`, `lod` or `animation`) and, for now, a `sha256` of 64 zeros. Run
`cao-parity local-assets`: it reports the entry's actual SHA-256 as a mismatch. Check that
the pool is the one you mean to pin, then copy the hash in. Ids start with the edition, as
in `sse-static-<file name>`.

The list pins SSE Meshes and Animations, and 20 LE Animations from LE's
`Skyrim - Animations.bsa` as `hkxcmd` conversion inputs (#502). The seeds
`apply-le-animations`, `apply-le-animation-faults` and `dry-run-le-animations` use them, so
they also need `hkxcmd.exe` (`--hkxcmd`, `CAO_HKXCMD` or the repository's `bin/`).

It also pins 34 LE Meshes from LE's `Skyrim - Meshes.bsa` (#504): the SSE Meshes' own
internal paths, so both editions cover the same categories, and four Meshes with triangle
strips. None of the other 30 has strips, so under the SSE profile only those four are the
critical issues the necessary mesh level converts. The mesh seeds use them:

- `apply-meshes-resave`, `apply-meshes-necessary`, `apply-meshes-medium` and
  `apply-meshes-full` run each mesh level (resaving alone, then 1 to 3) over the LE static,
  skinned and LOD Meshes and three synthetic ones;
- `dry-run-meshes` runs the full level with resaving as a Dry Run.

They are not run without the LE BSA. `apply-meshes-le-target`, `apply-meshes-fo4-target`
(the two ported nifly quirks: a TES5 target makes every Mesh a critical issue, and
`OptimizeFor` is a no-op towards FO4) and `apply-mesh-quarantine` use synthetic Meshes only.
None of those seeds puts a Mesh on a headpart or facegen path, nor on one the SSE profile's
`customHeadparts.txt` lists. `apply-meshes-headparts` does (#505): synthetic Meshes that a
`plugin` entry's HDPT records name (with and without the `meshes\` prefix), that the profile
lists, and that lie on a facegen path, at the necessary level in Apply. It holds no deviation
17 trigger: the plugin's framing is whole and uncompressed, and a Dry Run over a facegen
path, which only Rust treats as a Headpart Mesh, is left out.

A pin is the SHA-256 of an entry's extracted bytes, so an entry can be hashed from a loose
copy extracted from the same BSA. The pool itself still has to hold the BSA.

## Calibrating the BC7/BC6H threshold

The Texture rule lets BC7 and BC6H pixels differ, as long as every decoded mip and face
reaches `MIN_BC7_BC6H_PSNR_DB` (`crates/cao-parity/src/textures.rs`). The constant is set by
calibrating the oracle against itself (#513):

```
cargo run -p cao-parity -- calibrate --oracle <exe>
```

- **What runs.** Every corpus case (seeds and generated) that is not a Dry Run, turns on a
  texture option and holds a `texture` entry, loose or packed. Each runs through the oracle
  twice: over its `oracle/` copy on the GPU, and over its `rust/` copy with
  `CAO_ORACLE_FORCE_CPU_BC` set. The Rust driver never runs.
- **What is measured.** Every BC7 and BC6H Texture both runs wrote at the same path, loose or
  inside a `.bsa` or `.ba2`, image by image, with the same PSNR the Texture rule applies.
- **Checks.** The host needs a D3D11 device on adapter 0. The GPU run must not log
  "DirectCompute is not available", and the CPU run must log it once any BC7 or BC6H
  Texture was measured; otherwise the oracle predates the override and must be rebuilt.
- **Output.** Each case's floor, then the observed floor and the threshold: 40 dB if the floor
  is not below it, otherwise the floor minus 1 dB, rounded down to 0.01 dB. The host, adapter
  and driver are printed with it. The summary is also written to
  `<work>/calibration-report.md`. Measured cases are deleted; a case with an error keeps its
  directory, with `gpu.*` and `cpu.*` captures.
- **Exit code.** 0 when calibrated, 2 when any case hit an error (the floor may be
  incomplete), 3 when no BC7 or BC6H Texture was measured.
- **Recording.** Set `MIN_BC7_BC6H_PSNR_DB` to the printed threshold, and record the host,
  adapter and driver beside it in `CALIBRATION_HOST`.

## Oracle-only behaviour

- **Archive options.** `--bcomp`, `--bdum`, `--bmi`, `--bmt` and `--bds` set the archive
  options that the GUI reads from `settings.ini` ([cli.md](cli.md)).
- **Event escaping.** Text fields in the `EVENT:` stream are escaped, so the stream parses
  strictly ([cli.md](cli.md)).
- **Forcing CPU BC6H/BC7.** If `CAO_ORACLE_FORCE_CPU_BC` is set to any non-empty value,
  D3D11 device creation fails on purpose and BC6H/BC7 use the CPU codec.
  `cao-parity calibrate` uses it to compare the GPU encoder with the CPU encoder.
  The log then says that DirectCompute is not available. The oracle inherits the
  harness's environment, but the Rust side has no such override: it encodes BC6H/BC7 on
  the GPU whenever it gets a D3D11 device (#495). So run BC7 and BC6H cases such as
  `apply-texture-bc7` and `apply-texture-bc6h` without it. With it set on a host that has
  a GPU, the oracle's CPU encoder is compared with Rust's GPU encoder, which is
  calibration's question, not a parity one.
