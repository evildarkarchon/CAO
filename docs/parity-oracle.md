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
recipe's `content` entries (`texture`, `text`, `raw`, `mesh`, `local_asset`, `directory`,
`archive`) are written once into `input/`, seeded by the case id, and copied byte for byte to
`oracle/` and `rust/`. An `archive` entry packs its own `texture`, `text`, `raw`, `mesh` and
`local_asset` entries, whose paths are the game
paths it stores, with `cao-archive`, in the container its `game` and `type` give. Its
`fs_shape` operations (`hardlink`, `junction`, `file_symlink`, `readonly`, `reserved_name`)
are then applied to all three copies. Recipe paths must keep game paths ASCII and every
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

Committed seeds live under `crates/cao-parity/seeds/`, in any subfolder; a seed's case id
is its file stem. `raw` entries may name a fixture file under
`crates/cao-parity/fixtures/`.

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
No mesh seed puts a Mesh on a headpart or facegen path, nor on one the SSE profile's
`customHeadparts.txt` lists: recognising Headpart Meshes is #505's.

A pin is the SHA-256 of an entry's extracted bytes, so an entry can be hashed from a loose
copy extracted from the same BSA. The pool itself still has to hold the BSA.

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
