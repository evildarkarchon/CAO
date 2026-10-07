# Porting `btu::bsa` archive behaviour onto the `ba2` crate

Research for [#461](https://github.com/evildarkarchon/CAO/issues/461) on the map
[#458 "Port CAO to Rust and Slint"](https://github.com/evildarkarchon/CAO/issues/458).

**Question.** Which `btu::bsa` (bethutil) and `rsm-bsa` behaviour does CAO depend on, and what
must be ported on top of the `ba2` crate to reproduce it?

**Answer.** `ba2` 3.0.1 replaces all of `rsm-bsa`: reading, writing, hashing, compression, and
DX10 chunking. It needs no patches, but five option defaults differ from `rsm-bsa`, and the port
must set them explicitly (see [ba2 gaps and risks](#ba2-gaps-and-risks)). Everything bethutil
adds on top of `rsm-bsa` is plain logic, about 400 lines, and must be ported by hand into a small
`archive` module:

- the per-game `Settings` tables and dummy plugin bytes;
- file-type classification;
- `ArchiveData` size accounting and `merge`;
- the `write` orchestration (the compression decision, parallel read and compress, and key
  construction);
- `FilePath` name parsing;
- `read_archive` name derivation.

Production CAO does not use bethutil's `split`, `find_archive_name`, `unpack`,
`make_dummy_plugins`, or `clean_dummy_plugins`. CAO reimplements each of them in `src/Run`, so
those reimplementations are what gets ported. One behaviour changes on its own: `ba2` can extract
a DX10 cubemap in a DX10-only format (BC7, BC6H, sRGB BCn) from a BA2. The C++ build fails on
these today.

Sources were checked on 2026-10-06 against:

- bethutil at the pinned commit
  [`81f882ed`](https://github.com/Guekka/bethutil/tree/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02).
  This is the overlay port `cmake/ports/bethutil/portfile.cmake`. The installed headers under
  `build/vcpkg_installed/x64-windows-static-md/include/btu/{bsa,common}` match the commit, except
  for line endings.
- `rsm-bsa` 4.1.0 ([`e5c979c`](https://github.com/Ryan-rsm-McKenzie/bsa/tree/4.1.0)). This is the
  installed version (`build/vcpkg_installed/vcpkg/info/rsm-bsa_4.1.0_x64-windows-static-md.list`),
  and the installed headers match the tag.
- `ba2` 3.0.1 from crates.io. This is the latest release, published 2024-12-22. The repository is
  [`Ryan-rsm-McKenzie/bsa-rs`](https://github.com/Ryan-rsm-McKenzie/bsa-rs); its only later commit
  is a documentation change.
- `directxtex` 1.3.0 (its vendored DirectXTex), and upstream DirectXTex tag `may2026`.
- This repo's `src/` at `8767f86`.

## What CAO calls

| Call site | bethutil / `rsm-bsa` API used |
|---|---|
| `src/OptimizerProfileSnapshot.h:48-55`, `src/Profiles.cpp:20-22` | `Settings::get(btu::Game).max_size`, used as the floor for `maxBsaUncompressedSize` |
| `src/Run/ArchiveFinalization.cpp:82-87` (`archiveSettings`) | `Settings::get`, then raise `max_size` to the profile value when that is larger |
| `src/Run/ApplicationRunSetup.cpp:20-21, 103-104` | `Settings::get(game).extension` (`.bsa` or `.ba2`) |
| `src/MeshesOptimizer.cpp:63` | `Settings::get(game).plugin_extensions`, for the headpart plugin scan |
| `src/Run/ArchiveFinalizationPlanning.cpp:54-211` (`planFinalization`) | `list_archive`, `list_plugins`, `FilePath`, `default_is_allowed_path`, `get_filetype`, `ArchiveData(sets, type)`, `add_file`, `get_type`, `merge`, `s_dummy_plugin`, `suffix`, `texture_suffix`, `extension` |
| `src/Run/ArchiveFinalizationAttempt.cpp:71, 126` | `write(compress, ArchiveData&&, root)`, and `read_archive(...).has_value()` as post-commit verification |
| `src/Run/ArchiveFinalizationLoadingPlugins.cpp:155, 208, 251-263` | `list_archive`, `list_plugins`, `FilePath::full_path`; `loadingPluginNames` is CAO code |
| `src/Run/ArchiveExtraction.cpp:94-117` | `read_archive`, then `File::write(path)` for each entry (decompresses, and rebuilds the DDS header for DX10) |
| `src/BsaOptimizer.cpp:61` | `read_archive(...).has_value()` as recovery verification |
| `src/Run/ArchiveFirstAssetDiscovery.cpp:31-74` (`inspectArchiveInventory`) | Direct `rsm-bsa` calls: `guess_file_format`, and `tes3`/`tes4`/`fo4::archive::read`. It reads names plus `compressed() ? decompressed_size() : size()` per file or chunk, and adds 148 bytes for each DX10 file |
| `src/Run/ArchiveFirstAssetDiscovery.cpp:24-28` (`extractArchiveNoOverwrite`) | `btu::bsa::unpack`. Only tests call it (`tests/ArchiveFirstAssetDiscoveryTests.cpp`); it is not a production path |

`Q_ENUM(btu::bsa::ArchiveVersion)` in `src/Profiles.h:35` is declared but nothing persists or
reads it.

## Per-game settings

Source: `include/btu/bsa/settings.hpp:86-215` and `include/btu/common/games.hpp`. `btu::Game` is
`TES3=0, TES4=1, FNV=2, SLE=3, SSE=4, FO4=5, Custom=6`. The shipped profiles store
`BSA/bsaGame=3` (TES5), `4` (SSE), and `5` (FO4) (`profiles/*/profile.ini`). `Settings::get` uses
SSE as its default table; every other game copies it and overrides fields. Any unlisted value
(`TES3`, `Custom`, or out of range) returns the SSE table.

| Field | TES5 profile (`SLE`) | SSE profile (`SSE`) | FO4 profile (`FO4`) |
|---|---|---|---|
| `format` (Standard and Incompressible) | `tes5` = BSA v104 | `sse` = BSA v105 | `fo4` = BA2 `GNRL` |
| `texture_format` | none, so it falls back to v104 | v105 | `fo4dx` = BA2 `DX10` |
| `max_size` (btu) | 2000 MiB = 2,097,152,000 | 2000 MiB | 4000 MiB = 4,194,304,000 |
| Effective `max_size` with the shipped profile | 2,104,533,975 (INI `2104533975.04` is larger and wins) | 2,104,533,975 | 4,194,304,000 (INI `4187593113.6` is smaller) |
| `extension` | `.bsa` | `.bsa` | `.ba2` |
| `suffix` | none | none | `Main` |
| `texture_suffix` | none | `Textures` | `Textures` |
| `plugin_extensions` (order matters) | `.esm`, `.esp` | `.esl`, `.esm`, `.esp` | `.esl`, `.esm`, `.esp` |
| `s_dummy_plugin` | `dummy::tes5` | `dummy::sse` | `dummy::fo4` |
| `texture_files` | as SSE | `.dds` in `textures` or `interface`; `.png` in `textures` | `.dds` in `textures` or `interface` only |
| `standard_files` additions | — | — | `+ .png` in `textures`, `+ .uvd` in `vis` |

`archiveSettings` assigns the profile's `double` to `uintmax_t`, which truncates it
(`src/Run/ArchiveFinalization.cpp:82-87`). The TES4 and FNV branches also exist (v103 and v104;
only `.esm` and `.esp`; their own dummies). No shipped profile reaches them, so port only the three
shipped games plus the SSE fallback for unknown values. See the open point at the end.

### Classification tables (SSE base; the other games apply the changes above)

`AllowedPath{ext, dirs}` matches when the file extension equals `ext`, compared without regard to
case, and the lowercased **first component of the path relative to the Mod Root** is in `dirs`
(`settings.hpp:217-231`).

- **Standard:**
  - `.bgem`, `.bgsm` in `materials`;
  - `.bto`, `.btr`, `.btt`, `.dtl`, `.egm`, `.hkb` (listed twice), `.hkx`, `.lst`, `.nif`,
    `.tri` in `meshes`;
  - `.cgid` in `grass`;
  - `.dlodsettings` in `lodsettings`;
  - `.jpg` in `root`;
  - `.psc` in `scripts` or `source`;
  - `.tga` in `textures`.
- **Texture:** `.dds` in `textures` or `interface`; `.png` in `textures` (SSE and TES5 only).
- **Incompressible:**
  - `.dlstrings`, `.ilstrings`, `.strings` in `strings`;
  - `.fuz`, `.lip`, `.wav` in `sound`; `.xwm` in `music` or `sound`; `.ogg` in `sound`;
  - `.fxp` in `shadersfx`;
  - `.gid`, `.lnk` in `grass`;
  - `.gfx`, `.swf` in `interface`;
  - `.hkc`, `.hkt`, `.hkp`, `.ini` in `meshes`;
  - `.lod` in `lodsettings`;
  - `.pex` in `scripts`;
  - `.seq` in `seq`;
  - `.txt` in `interface`, `meshes`, or `scripts`;
  - `.xml` in `dialogueviews`.

`get_filetype` (`settings.hpp:234-262`) checks in this order: Standard, Texture, Incompressible,
then the plugin extensions (exact match on the lowercased extension), then the archive extension.
Anything else is `Blacklist`. CAO packs only Standard, Texture, and Incompressible. Anything else
stays loose.

**Quirk to port as is.** The `root` directory token only matches when the relative path is empty
(`settings.hpp:223-229`). A root-level file's first component is its own file name, so `.jpg`
packs only under a top-level folder literally named `root`. CAO never offers root-level files
anyway (see below).

### Dummy plugin bytes

All three are 49 bytes (`settings.hpp:15-39`): a `TES4` record with data size 25, a `HEDR`
subrecord (version float, 0 records, next object ID `0x800`), and an empty `CNAM`. Bytes 8-11 are
the record flags. In the hex below, bytes 20-21 are the form version and bytes 30-33 the `HEDR`
version.

| Game | Record flags | Form version | `HEDR` version | Bytes (hex) |
|---|---|---|---|---|
| TES5 | `0x000` | 43 | 1.7 | `54 45 53 34 19 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 2b 00 00 00 48 45 44 52 0c 00 9a 99 d9 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00` |
| SSE | `0x200` (ESL) | 44 | 1.7 | `54 45 53 34 19 00 00 00 00 02 00 00 00 00 00 00 00 00 00 00 2c 00 00 00 48 45 44 52 0c 00 9a 99 d9 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00` |
| FO4 | `0x200` (ESL) | 131 | 0.95 | `54 45 53 34 19 00 00 00 00 02 00 00 00 00 00 00 00 00 00 00 83 00 00 00 48 45 44 52 0c 00 33 33 73 3f 00 00 00 00 00 08 00 00 43 4e 41 4d 01 00 00` |

CAO always uses these embedded bytes. It does not use `profiles/*/DummyPlugin.esp`, which is dead
data per the map. "Exact Dummy Plugin" means byte-for-byte equality with this array
(`hasExactDummyBytes`, `ArchiveFinalizationLoadingPlugins.cpp:276-291`). bethutil's own
`clean_dummy_plugins` compares only the size; CAO does not use it.

## Packing

### Source selection (CAO code, uses two btu predicates)

`planFinalization` (`ArchiveFinalizationPlanning.cpp:100-122`) walks each canonical Mod Root
recursively. It skips reserved staging, symlinks, and reparse points (without descending into
them). It keeps regular files that satisfy two predicates:

- `default_is_allowed_path`, which drops directories and **files directly in the root**
  (`pack.cpp:21-29`);
- CAO's `FilesToNotPack` rule, a case-insensitive substring match on the full path
  (`btu::common::str_contain(path, rule, false)`).

It then **sorts the sources** with `std::filesystem::path` ordering. That order decides which
files land in which split archive, so the Rust port must sort the same way. Both orderings agree
for ASCII; they can diverge only for code points that need surrogate pairs, compared with
U+E000-U+FFFF.

### `ArchiveData` and splitting

Source: `archive_data.cpp`.

- The constructor (`:12-17`) takes `max_size` from settings. Its version is
  `texture_format.value_or(format)` for `Textures`, otherwise `format`.
- `add_file(path)` (`:39-49`) uses `fs::file_size` for both the "compressed" and "uncompressed"
  sizes (CAO never passes an override). It rejects the file when
  `size.compressed + file > max_size` (strict `>`), so the limit applies to the summed **source**
  bytes.
- CAO's splitter (`ArchiveFinalizationPlanning.cpp:123-143`) keeps one open partition per type
  (Standard, Incompressible, Textures). When a partition is full, CAO pushes it and starts a fresh
  one. If even an empty partition cannot take the file, CAO **throws** ("An Asset exceeds the
  output Archive size limit."). bethutil's `split` would silently drop the file; CAO does not use
  it.
- Finally CAO appends the three open partitions in the fixed order Standard, Incompressible,
  Textures, which `merge` requires.

### `merge`

Source: `pack.cpp:103-143`. `merge` looks only at the last three entries.

- With `MergeIncompressible`, Incompressible joins Standard when
  `incompressible.uncompressed + standard.uncompressed < max_size` (strict `<`).
- With `MergeTextures`, Textures joins Standard when
  `textures.compressed + standard.compressed < max_size`.
- `ArchiveData::operator+=` (`archive_data.cpp:51-66`) changes the result's type:
  - if either side is `Incompressible`, the result is `Incompressible`;
  - otherwise, if the types differ, it is `Standard`.

  **The version is never updated.** A merged archive keeps Standard's version (v104, v105, or FO4
  `GNRL`).
- Empty archives are erased.

There are two consequences, and the port must reproduce both:

1. **Merging incompressible files disables compression for the whole merged archive.**
   `write()` clears `compressed` for `Incompressible`-typed data (`pack.cpp:36`).
2. **FO4 with `MergeTextures` packs `.dds` into the `GNRL` "Main" BA2.** No DX10 chunking is
   applied, and the merged archive takes the non-texture suffix. The GUI does not prevent this
   for FO4. `bBsaMergeTexture` is simply the inverse of the "create texture archive" checkbox
   (`src/OptionsCAO.cpp:176`).

### `write(compress, ArchiveData&&, root)`

Source: `pack.cpp:31-60`.

- It writes nothing for empty data.
- `compressed = compress && type != Incompressible`, and the result is **forced true for `fo4dx`**
  (DX10 is always compressed).
- For each file, in parallel (`for_each_mt`, `std::execution::par`), it reads with the archive
  version, compresses if requested, and keys the file by
  `fpath.lexically_relative(root).u8string()`. That key is the UTF-8 bytes of the relative path,
  with native `\` separators.
- A failure in one file is collected, and the rest are still written to the staged path. CAO
  throws on `errors.front()` (`ArchiveFinalizationAttempt.cpp:71-72`), so the staged archive is
  discarded. The Rust port can stop at the first error before writing anything.

`write_archive` (`archive.cpp:169-227`) works per format:

- **TES4 (v104 and v105).** The directory key is
  `parent_path().lexically_normal().generic_string()` and the file key is the file name. The
  archive flags are `directory_strings | file_strings`, plus `compressed` when any file is
  compressed. That is `0x3`, or `0x7` when compressed. No archive types are set (`0`), and there
  are no embedded names or retain flags.
- **FO4.** `ba2.write(path, format)` with `rsm-bsa`'s default `strings = true`, so a name table is
  always written. The header is `BTDX` version 1.

### Compression, file data, and chunking

Source: `rsm-bsa` 4.1.0.

- **TES5 v104:** zlib `::compress` (default level, window 15).
- **SSE v105:** an LZ4 frame with `compressionLevel = LZ4HC_CLEVEL_DEFAULT` (9) and `autoFlush = 1`
  (`src/bsa/tes4.cpp:60-66, 696-712`).
- **FO4 `GNRL`:** one chunk per file, compressed with zlib `::compress` (`src/bsa/fo4.cpp:298-316`).
- **FO4 `DX10`:** `file::read_directx` (`src/bsa/fo4.cpp:563-643`):
  - it loads the DDS with `LoadFromDDSMemory(DDS_FLAGS_NONE)`;
  - the header is height, width, mip count, format, flags (`1` for a cubemap), and
    `tile_mode = 8`;
  - mips are packed greedily into chunks capped by the 512x512 slice pitch of the format, at most
    4 chunks, with the remainder in the last chunk;
  - a cubemap is always a single chunk.

## Output naming (`FilePath`)

Source: `plugin.cpp:23-104`; separator `" - "` (`plugin.hpp:14`). For each file:

- `FilePath::make(path, sets, type)` splits `dir`, `name` (the stem), and `ext`. For
  `type == Plugin`, it returns nothing unless `ext` is in `plugin_extensions`. For `type == BSA`,
  it returns nothing unless `ext == extension`. **Both comparisons are case-sensitive**, so
  `Foo.ESP` and `Foo.BSA` are not listed.
- It then parses the name in three steps:
  1. `eat_digits`: trailing digits become `counter`.
  2. `eat_suffix`: the text after the last `" - "` is taken only if it equals `suffix` or
     `texture_suffix`, compared case-sensitively.
  3. If no counter was found yet, `eat_digits` runs again.

  `full_name() = name + counter + (" - " + suffix if suffix is non-empty)`, and
  `full_path() = dir / (full_name + ext)`. `Foo - Textures2` therefore parses as `Foo`, counter 2,
  `Textures`, and re-renders as `Foo2 - Textures`. Port it as is.
- `list_plugins` and `list_archive` (`plugin.hpp:43-70`) apply `make` to a non-recursive
  `directory_iterator` and skip directories.
- `operator<=>` is the default member order: `dir`, `name`, `suffix`, `ext`, `counter` (empty
  sorts first), `type`. CAO sorts plugins with it (`ArchiveFinalizationPlanning.cpp:95`), so a
  Rust `#[derive(Ord)]` with the same field order matches.

CAO's planning uses this naming:

- **Plugin candidates.** It lists the plugins and drops exact dummies. If none remain, it uses one
  synthetic plugin: `root.filename()` with `.esp`.
- **Archive suffix.** `texture_suffix` for `Textures`-typed archives, otherwise `suffix`, with
  `value_or("")`.
- **Choosing a name.** For each archive, it takes the first plugin whose
  `name+counter+suffix+extension` path neither exists (it checks `symlink_status`) nor is reserved
  earlier in the run. Otherwise it takes `plugins.front()` with counter 0..254, the same range as
  bethutil's `find_archive_name`.

  For TES5 (no suffixes), the Standard and Textures archives compete for the same name. The second
  gets the next plugin's name or `Name0.bsa`.
- **Loading Plugin names.** `loadingPluginNames` lists, for each plugin extension in order, the
  name with the suffix and the name without it. The fallback dummy is the last entry: the
  unsuffixed `.esp`.

## Read and listing

- **`read_archive`** (`archive.cpp:123-167`):
  - it returns `nullopt` only when the magic is unknown, and throws on parse errors;
  - TES4 names are `dir + '/' + file`, then mapped to native `\` and passed through
    `make_valid('_')`, which replaces invalid UTF-8 (`archive.hpp:12-34`);
  - FO4 and TES3 names are the raw key names.
- **`File::write(path)`** decompresses. For DX10 it re-encodes a DDS header with
  `EncodeDDSHeader` (`arraySize = 1`, with `TEXTURECUBE` for cubemaps) and concatenates the
  chunks.
- **Names in both archive formats are normalized at insertion.** `rsm-bsa` keys are built with
  `hash_*_in_place`, which lowercases and maps `/` to `\` in place
  (`include/bsa/detail/common.hpp:1010-1016`, `src/bsa/detail/common.cpp:84-101`). `ba2` does the
  same (`derive.rs:302-315`, `hashing.rs:36-53`). Written name tables are therefore lowercase with
  backslashes in both.
- **`guess_file_format`.** It reads the first u32: `0x100` is TES3, `"BSA\0"` is TES4, and `BTDX`
  is FO4. `ba2::guess_format` is byte-for-byte the same (`guess.rs`).
- **Non-UTF-8 names.** CAO's discovery converts each raw name with `pathFromUtf8`, which builds a
  `std::filesystem::path` from a `u8string`. MSVC throws on invalid UTF-8 there, and discovery
  reports `ArchiveEntryInvalid` before extraction. So the `make_valid` mismatch never reaches
  extraction. In Rust, decode names as strict UTF-8 and map a failure to the same failure code.

## `ba2` 3.0.1 equivalents

| bethutil / `rsm-bsa` behaviour | `ba2` 3.0.1 equivalent | Port required |
|---|---|---|
| `guess_file_format` | `ba2::guess_format(&mut impl Read) -> Option<FileFormat>` | — |
| `tes3::archive::read` (inventory only) | `tes3::Archive::read(&Path)`; `File::len()` | — |
| `tes4::archive::read` | `tes4::Archive::read(&Path) -> (Archive, ArchiveOptions)`; `File::decompressed_len()`, `File::len()` | Name join (`dir/file`; just `file` when the directory name is empty) |
| `fo4::archive::read` and DX10 size | `fo4::Archive::read(&Path) -> (Archive, ArchiveOptions)`; `Chunk::decompressed_len().unwrap_or(len())`; `options.format()` | +148 bytes for DX10 (4 magic + 124 header + 20 DX10 extension) |
| `File::write(path)` (extract one entry) | `tes4::File::write(&mut out, &(&opts).into())`; `fo4::File::write(&mut out, &(&opts).into())` (decompresses; DX10 rebuilds the header) | — |
| `File(ver).read(path)` and `compress()`, TES4 | `tes4::File::read(&Path, &FileReadOptions::builder().version(v).compression_result(Compressed or Decompressed).build())` | — |
| Same, FO4 `GNRL` and `DX10` | `fo4::File::read(&Path, &FileReadOptions::builder().format(GNRL or DX10).compression_format(Zip).compression_level(FO4).compression_result(..).build())`; mip chunks default to 512x512 | — |
| `write_archive`, TES4 | Build `tes4::Archive` from `ArchiveKey`s (directory) and `Directory` values (file key to `File`); `write(&mut out, &ArchiveOptions::builder().version(v104 or v105).flags(DIRECTORY_STRINGS \| FILE_STRINGS [\| COMPRESSED]).build())` | Group by parent; the flag decision |
| `write_archive`, FO4 | `fo4::Archive` from `(ArchiveKey, File)`; `write(&mut out, &ArchiveOptions::builder().format(GNRL or DX10).version(v1).strings(true).build())` | — |
| `Settings::get`, `AllowedPath`, `get_filetype`, `FileTypes` | none | **Port** the tables and lookup (pure data) |
| `ArchiveData`, `add_file`, `+=`, `merge`, `MergeSettings` | none | **Port** (about 80 lines) |
| `write(compress, data, root)` | none | **Port**: the compression decision, parallel read and compress (e.g. `rayon`), keys from the relative path |
| `default_is_allowed_path` | none | **Port** (one line) |
| `FilePath`, `list_plugins`, `list_archive` | none | **Port**, including the case-sensitive extension and suffix matches |
| `s_dummy_plugin` bytes | none | **Port** the three 49-byte arrays |
| `virtual_to_local_path`, `make_valid` | none | Fold into one strict-UTF-8 name function shared by inventory and extraction |
| `unpack`, `split`, `find_archive_name`, `make_dummy_plugins`, `clean_dummy_plugins` | — | **Not needed**; CAO has its own implementations or only tests use them |

Compression matches at the codec and parameter level.

- **TES5:** `flate2` `ZlibEncoder` with `Compression::default()` (`tes4/file.rs`
  `compress_into_zlib`).
- **SSE:** an `lzzzz` LZ4 frame at level 9 with auto-flush (`tes4/file.rs`
  `compress_into_lz4`).
- **FO4:** zlib at the default level with window 15 for `CompressionLevel::FO4`
  (`fo4/chunk.rs:compress_into`).
- **DX10 chunking** (`fo4/file.rs:make_chunks`) is the same greedy rule as `rsm-bsa`'s `chunk<4>`:
  it stops after three chunks and puts the remainder in a fourth. Cubemaps are not chunked, and
  the header is written the same way (`tile_mode` 8, the cubemap flag).
- **Ordering.** Both crates order TES4 entries by the same numeric hash (`tes4/hashing.rs`
  `Hash::numeric`), and `ba2`'s default feature uses C zlib through `flate2/zlib`. Output should
  be equivalent; byte identity is not guaranteed (LZ4 and zlib library versions), and the map does
  not require it.

## `ba2` gaps and risks

1. **Option defaults differ from `rsm-bsa`. Set them explicitly.**
   - `fo4::ArchiveOptions` derives `Default`, so `strings` is **`false`** (`fo4/archive.rs`
     `Options`). `rsm-bsa` defaults to `true`. Without `.strings(true)`, CAO's own inventory and
     extraction would read empty names and fail. Set `.version(Version::v1)`; it is also the
     default.
   - `tes4::Version` defaults to **`v103`** (Oblivion) (`tes4/mod.rs`). Always set
     `v104` or `v105`.
   - `tes4::ArchiveFlags::default()` is `DIRECTORY_STRINGS | FILE_STRINGS`. Add `COMPRESSED` when
     packing compressed, and leave `types` empty to match.
   - `fo4::FileReadOptions` defaults to `GNRL` and `Decompressed`. Set `.format(DX10)` for texture
     archives, and `Compressed` for compressed archives and for every DX10 file.
2. **No parallelism inside `ba2`.** bethutil reads and compresses files with
   `std::execution::par`. Bring the parallelism ourselves (`rayon` over files, then insert
   serially), or packing on large mods regresses.
3. **Memory mapping and file lifetime.** `Archive::read(&Path)` and `File::read(&Path)` memory-map
   the file (`io.rs` `MappedSource`), and the returned objects own the mapping (`'static`). The
   C++ build behaves the same way: `rsm-bsa` maps through `mmio`. Rust ownership makes it easy to
   keep a mapping alive by accident, though. Windows cannot delete a file with a live user mapping,
   so drop the archive or file objects before source deletion, backup or removal of the extracted
   archive, and publication. `ba2` also implements `Reader<&std::fs::File>`. The port could
   therefore read through the already-pinned handle instead of reopening the pathname, but that
   is an internal choice, not a parity requirement.
4. **Overflow is an error, not corruption.** `ba2` rejects u32 offset or size overflow with
   `IntegralOverflow` or `IntegralTruncation` (see bsa-rs
   [#11](https://github.com/Ryan-rsm-McKenzie/bsa-rs/issues/11)); `rsm-bsa` silently wraps.
   Shipped limits (about 2.1e9 for a BSA) stay below u32. A user INI that raises
   `maxBsaUncompressedSize` past 4 GiB for a BSA would write a corrupt archive in C++ and fail
   cleanly in Rust. Treat this as an acceptable deviation.
5. **DX10 cubemap extraction differs: the C++ build fails, `ba2` succeeds.** `rsm-bsa`'s
   `write_directx` passes `arraySize = 1` with `TEX_MISC_TEXTURECUBE`
   (`src/bsa/fo4.cpp:645-680`). DirectXTex `may2026`, which CAO's vcpkg build links, returns
   `E_INVALIDARG` for a cubemap whose `arraySize % 6 != 0` when it needs the DX10 header extension
   (`DirectXTexDDS.cpp` `EncodeDDSHeader`, the cubemap branch). That covers BC6H, BC7, and sRGB
   BCn without `FORCE_DX9_LEGACY`. `rsm-bsa` then throws "failed to encode dds header", so C++
   CAO cannot extract a FO4 BA2 that contains such a cubemap. Legacy-header cubemaps (BC1-BC5,
   RGBA8) are unaffected. `ba2` 3.0.1 passes `array_size = 6` for cubemaps (`fo4/file.rs`
   `write_dx10`; fixed upstream in bsa-rs
   [#3](https://github.com/Ryan-rsm-McKenzie/bsa-rs/issues/3)).
6. **`ba2` depends on `directxtex ^1.1`.** It uses the crate's vendored DirectXTex for DX10 read
   and write, and resolves to 1.3.0 today. The workspace must unify it with CAO's own
   `directxtex`. If the texture engine forks `directxtex` (see `docs/research/gpu-bc7-directxtex.md`
   on its branch), patch it for the whole workspace with `[patch.crates-io]` so `ba2` uses the
   same copy. Upstream bsa-rs
   [#14](https://github.com/Ryan-rsm-McKenzie/bsa-rs/issues/14) (open) proposes dropping the
   dependency, so pin `ba2 = "=3.0.1"`.
7. **No other known bugs affect CAO.** The other closed bsa-rs issues (#1, #2, #5, #8, #10, #12,
   #13) are usage questions or downstream mod-list problems; none is a library defect in paths CAO
   uses. `ba2` refuses `GNMF` (PS4) writes with `NotImplemented`; that format is out of scope.

## Quirks to port as is (for equivalence)

- Merging Incompressible into Standard writes the whole merged archive uncompressed.
- A merged archive keeps Standard's version: FO4 textures go into `GNRL`.
- Split sizes are summed source bytes, with a strict `>` limit; merges need a strict `<`.
- CAO throws, rather than drops, an Asset larger than the limit.
- Extension and suffix matching in `FilePath` is case-sensitive.
- The `.jpg` / `root` token never matches a root-level file, and CAO excludes root-level files
  anyway.
- `Foo - Textures2` re-renders as `Foo2 - Textures`.
- TES5's Standard and Textures archives compete for one name.
- Archive name tables are lowercase with backslashes.
- Keys are the UTF-8 bytes of the relative path.

## Deviation candidates for the map's "Fix, don't copy" list

- **DX10 cubemap extraction** (gap 5). The C++ build fails; Rust succeeds with a correct header.
  This is automatic once on `ba2`.
- **BSA larger than 4 GiB** (gap 4). The C++ build writes a corrupt archive; Rust reports an
  error. This is automatic.
- **An all-digit plugin or archive stem** (for example `2.esp`). `FilePath::eat_digits`
  (`plugin.cpp:57-80`) walks off the front of the string, which is undefined behaviour. The port
  should yield `name = ""`, `counter = 2`.

## Open points for other tickets

- **#467 (output equivalence).**
  - Compare archives by decoded content (entry names, lowercase with backslashes; decompressed
    bytes; the DX10 header fields and mip ranges per chunk; the TES4 flags and version), not by
    bytes.
  - Keep DX10-extension cubemaps out of the C++ oracle corpus, or expect the C++ build to fail on
    them.
  - Both builds must sort sources the same way, or split archives receive different files.
- **#468 (workspace architecture).**
  - The archive module needs `ba2 = "=3.0.1"`, `rayon` or equivalent for parallel compression,
    and one shared `directxtex` (patched if forked).
  - Drop mapped archive objects before any native delete or rename.
- **Unknown `bsaGame` values.** Porting only SLE, SSE, and FO4 plus the SSE fallback changes the
  behaviour for a hand-edited `bsaGame=1` (TES4) or `2` (FNV). No shipped or discoverable profile
  uses them, and the map scopes them out.
