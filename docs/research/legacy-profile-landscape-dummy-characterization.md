# Legacy profile encoding, landscape rules, and dummy plugins

Research date: 2026-07-12

## Resolution

The Rust port must implement legacy profile import as an explicit decoder for the supported Qt 5.15.2 `QSettings::IniFormat` subset, not as a generic UTF-8 INI reader. It must recognize the historical `bBsaLeastBSA` setting only as the evidenced predecessor of `bBsaMergeIncomp` and `bBsaMergeTexture`, ignore it when either replacement key occurs, and report conflicts.

The v5.3.15 landscape rule is a confirmed inert legacy feature. The authenticated executable contains `customHeadparts.txt` but no `customLandscape.txt`; the source companion calls the headpart scanner again instead of the landscape loader and never consumes the landscape list. Activating an independently scoped `customLandscape.txt` in the port is therefore an approved defect correction that must enter the discrepancy register. There is no legacy landscape matching behavior to preserve.

Native dummy-plugin generation is feasible and preferable. The authenticated legacy payload is a fixed, game-specific byte array; the source companion derives its `.esp` name from archive and plugin paths. Fallout 4 and classic Skyrim's shipped profile files equal the native arrays byte-for-byte; SSE's shipped 128-byte `DummyPlugin.esp` is a different, master-bearing artifact and is not the native generated output. The port should generate or ship the authenticated native bytes and verify their hashes. Oblivion remains outside the initial release and its embedded native bytes are evidence only.

## Evidence boundary

The behavioral oracle is the authenticated Nexus v5.3.15 archive established by [Establish a reproducible legacy reference baseline](https://github.com/evildarkarchon/CAO/issues/3):

| Artifact | Size | SHA-256 |
| --- | ---: | --- |
| `Cathedral Assets Optimizer 64-23316-5-3-15-1687526925.7z` | 10,410,192 | `B25CF0C0C97160B602DD47C252AF2EDE735C27ABE565C9AB84D272306308ABB6` |
| `Cathedral_Assets_Optimizer.exe` | 5,020,160 | `561C93CFCC95D900ECF3AA861567932B78E94A15A91CD128C93C3712E9F33AA1` |
| bundled `Qt5Core.dll` | 6,023,664 | `8D2FF4CE9096DDCCC4F4CD62C2E41FC854CFD1B0D6E8D296645A7F5FD4AE565A` |

The source companion is commit [`9969c3f9aad0f9f6f0a3d4489cc573f16486826f`](https://github.com/evildarkarchon/CAO/tree/9969c3f9aad0f9f6f0a3d4489cc573f16486826f), which declares v5.3.14. Observations below are labeled as authenticated package evidence, exact Qt behavior, or source inference. The source companion's BethUtil port nominally pins commit `81f882ed4d3fbb3c04b0c90658a94b3b2eaade02`, but then substitutes a developer-local checkout, so exact byte matches inside the v5.3.15 executable are required before BethUtil details are treated as oracle evidence.

A narrow GUI check launched the authenticated extraction with its extraction directory as the working directory. It discovered exactly `FO4`, `SSE`, and `TES5` and selected SSE on first run, corroborating the package inventory and fallback path. The remaining encoding conclusions come from CAO's proven use of uncustomized Qt 5.15.2 `QSettings` and Qt's exact implementation, not from mutating a user's live installation.

## Qt 5.15.2 INI contract

CAO constructs direct-file `QSettings(..., QSettings::IniFormat)` objects for `profiles/common.ini`, `profile.ini`, and `settings.ini` and never calls `setIniCodec`; see [`Profiles.cpp`](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/Profiles.cpp#L14-L19) and its selected-profile setup at [lines 45-59](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/Profiles.cpp#L45-L59). The import decoder must therefore reproduce these Qt 5.15.2 rules:

| Concern | Exact legacy behavior | Import constraint |
| --- | --- | --- |
| Byte decoding | Without an INI codec, raw bytes decode deterministically as Latin-1. A leading UTF-8 BOM selects UTF-8. BOM-less UTF-8 is not auto-detected. | Decode BOM-less bytes as Latin-1 and BOM-marked bytes as UTF-8. Never substitute the current ANSI code page. |
| Key/group case | Windows `IniFormat` keys and groups compare case-insensitively while preserving original spelling. | Match recognized sections and keys case-insensitively. |
| Duplicate case variants | Entries are parsed in file order into the same case-insensitive map; the last occurrence wins. Duplicate sections accumulate in file order. | Use last-occurrence precedence and report duplicate/conflicting recognized values. |
| Key escaping | `/` is the hierarchy separator and serializes as `\`; only ASCII alphanumeric, `_`, `-`, and `.` remain literal. Other Latin-1 characters use `%HH`; higher UTF-16 code units use `%UHHHH`. Top-level keys live in `[General]`; a literal `General/...` group serializes as `[%General]`. | Decode Qt percent escapes before key recognition; do not treat percent-encoded and literal spellings as distinct keys. |
| Value escaping | Qt recognizes named control escapes, variable-length hexadecimal/octal escapes, escaped quotes/backslashes, and line continuations. An unknown escape discards both the backslash and its following character. A hand-written single-backslash Windows path is therefore unsafe; Qt writes doubled backslashes. | Implement the supported Qt escape subset, including list/quoted-value syntax, before type validation. Preserve raw source text in provenance and reject malformed or suspicious escapes rather than silently losing data. |
| Lists | Unquoted commas form `QStringList` values; the shipped `texturesUnwantedFormats=85, 86, 115` is an example. | Parse Qt lists before converting each element to the typed target. |
| Boolean conversion | After lowercasing, only an empty string, `0`, and `false` are false; every other string is true. Whitespace retained inside quotes matters. | Accept this form during legacy decoding, then store a canonical boolean and warn on noncanonical truthy spellings such as `yes`. |
| Numeric conversion | CAO discards conversion-success flags. Missing or invalid values therefore coerce to zero in the legacy program. | Validate rather than reproduce coercion in live fork state; report the approved deterministic-fallback discrepancy. |

Primary references are the official [Qt 5.15 QSettings documentation](https://doc.qt.io/archives/qt-5.15/qsettings.html), Qt 5.15.2's [`qsettings.cpp`](https://github.com/qt/qtbase/blob/v5.15.2/src/corelib/io/qsettings.cpp), [`qsettings_p.h`](https://github.com/qt/qtbase/blob/v5.15.2/src/corelib/io/qsettings_p.h), and [`qvariant.cpp`](https://github.com/qt/qtbase/blob/v5.15.2/src/corelib/kernel/qvariant.cpp).

### Shipped INI fixtures

All authenticated shipped INIs are ASCII-only, have no BOM, and use CRLF. The archive contains no `profiles/common.ini` and no FO4/TES5 `settings.ini`.

| File | Size | SHA-256 |
| --- | ---: | --- |
| `profiles/FO4/profile.ini` | 376 | `72CFCE00CBFD878A7F3CE0495C1ED26D25524ABC6F28126B13CC338471EC961D` |
| `profiles/SSE/profile.ini` | 375 | `E569440F1AC9E7DF0F9B1A6485476F186A6BB0AC0A36F3F7BAACECE1649388C1` |
| `profiles/SSE/settings.ini` | 658 | `46B31F2F151DF9516E4232D6E497975117211B1FA3D962E445A3F8A9C2C7E91E` |
| `profiles/TES5/profile.ini` | 368 | `DCFD90F4687680DC3891BDE676003E7F09E1A1AE19F979A2D62844E9B0762111` |

### Historical key alias and precedence

Commit [`ce88caf2a6caa4e1786d024242b090eef7a42f0a`](https://github.com/evildarkarchon/CAO/commit/ce88caf2a6caa4e1786d024242b090eef7a42f0a) replaced `bBsaLeastBSA` with two independent keys: `bBsaMergeIncomp` and `bBsaMergeTexture`. Comparing the old and new archive merge branches establishes this mapping:

| Historical value | `bBsaMergeIncomp` | `bBsaMergeTexture` |
| --- | --- | --- |
| absent | no alias contribution | no alias contribution |
| `false` | `true` | `false` |
| `true` | `true` | `true` |

The v5.3.15 executable contains the two replacement key literals and does not contain `bBsaLeastBSA`; the source companion reads only the replacements at [`OptionsCAO.cpp` lines 69-80](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/OptionsCAO.cpp#L69-L80). The old and new generations never had coexistence handling in CAO: v5.3.15 ignores the old key entirely. Import precedence is therefore:

1. Decode every physical occurrence with Qt case-insensitive, last-occurrence rules.
2. When neither replacement occurs, translate `bBsaLeastBSA` with the historical mapping above.
3. When either replacement occurs, ignore `bBsaLeastBSA`, import every present replacement, and apply the approved deterministic baseline to any absent replacement.
4. When an old and replacement generation coexist, report the conflict even if their effective values agree.

This atomic generation precedence is the narrow product rule implied by “prefer the replacement” in the resolved compatibility contract. It is not a claim that v5.3.15 supplied a missing replacement value; the legacy reader ignored the old key and coerced a missing replacement to false.

`animationFormat` is obsolete data, not an alias. It remains in shipped `profile.ini`, but the source companion reads only `animationsEnabled` at [`Profiles.cpp` lines 135-137](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/Profiles.cpp#L135-L137).

Missing state must be characterized separately from encoding. A missing whole `settings.ini` returns without changing the shared in-memory options vector, while missing individual keys in an existing file coerce to false/zero; `userPath` alone retains the previous value when empty. The port intentionally replaces this leakage with the deterministic configuration fallback already approved by [Define the portable profile and configuration compatibility contract](https://github.com/evildarkarchon/CAO/issues/9).

## Landscape-rule characterization

### Authenticated assets

| File | Size | Intended entries | SHA-256 |
| --- | ---: | ---: | --- |
| `profiles/SSE/customLandscape.txt` | 6,202 | 68 | `10DA47F392E79741C872C8538CE1F432AD23C74A8C20EB08A9681EFF8A44A9C4` |
| `profiles/SSE/customHeadparts.txt` | 33,801 | 474 | `A2E6162A2B420B1CA5396A2743B09102A986BE201366CE5729694641DE66104C` |

When decoded as UTF-16LE, the landscape file's 68 non-comment data entries are unique diffuse `.dds` paths with backslashes and no `_n.dds` entries. The file has a UTF-16LE BOM and ordinary UTF-16LE CRLF line endings. The unused legacy loader uses raw `QFile::readLine()` byte reads followed by implicit string conversion, not a BOM-aware `QTextStream`, so the source does not establish a working decoder for this file.

### Confirmed inert behavior

The following independent observations converge:

- The v5.3.15 executable's byte strings contain `customHeadparts.txt` and the landscape loader symbol but no `customLandscape.txt` literal.
- [`MainOptimizer.cpp` lines 11-18](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/MainOptimizer.cpp#L11-L18) calls both setup functions, but [`addLandscapeTextures()`](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/MainOptimizer.cpp#L40-L49) calls `_meshesOpt.listHeadparts()` again.
- The otherwise-unused loader reads `customHeadparts.txt`, not `customLandscape.txt`, at [`TexturesOptimizer.cpp` lines 21-39](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/TexturesOptimizer.cpp#L21-L39).
- `_landscapeTextures` is written only by that unused loader and is never read. `convertLandscapeTextures()` is declared but has no definition or call. Generic texture processing never consults landscape data.

If the unreachable loader were called, it would read `customHeadparts.txt` through the no-callback raw-line helper without `QDir::cleanPath`, append plugin-derived paths, use a case-sensitive `_n.dds` suffix test, insert `_n` before the final four characters of every other entry, and remove only exact case-sensitive duplicates. These are dead source-internal transformations, not observable landscape matching semantics.

Consequently, legacy v5.3.15 defines no active lookup, normalization, diffuse/normal counterpart, or matching semantics for `customLandscape.txt`. The materialized rule set in the port should use the already-approved replacement contract—custom profile first, SSE fallback second; normalized texture-relative exact matching; independent headpart and landscape scopes—but record the entire activation as a discrepancy rather than claiming that a particular normalization reproduces the oracle.

Required discrepancy regression fixtures are: custom-landscape-only mutation versus custom-headparts-only mutation; slash and case variants; leading/trailing/collapsed whitespace; diffuse and `_n` counterparts; plugin-derived LTEX/TNAM-to-TXST/TX00 paths; malformed and duplicate entries. The legacy side should remain inert for custom landscape input, while the fork side should demonstrate the documented corrected behavior.

## Dummy-plugin characterization

### Native payload identities

The pinned BethUtil [`settings.hpp` arrays](https://github.com/Guekka/bethutil/blob/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02/include/btu/bsa/settings.hpp#L14-L40) embed fixed payloads. Exact full-array patterns for every listed game occur nine times in the authenticated v5.3.15 executable, strongly authenticating the bytes despite the source build's developer-local dependency override.

| Game/backend identity | Size | Header version | SHA-256 | Initial-release status |
| --- | ---: | --- | --- | --- |
| Classic Skyrim / SLE | 49 | `0x2B`, HEDR 1.7 | `852F2DB6923C2203D60DAA176D5FA27D61A0D0E717B6819386BBFCBFF7FFFEFD` | required |
| Skyrim SE native output | 49 | `0x2C`, HEDR 1.7 | `08F228B84E6798D468472D30E74D550F786226A87F4F69F2DBFDDEC576E5799A` | required |
| Fallout 4 | 49 | `0x83`, HEDR 0.95 | `AFFCBDEA9D14FE2199440912B326B9F5B704C34F436B51CEF03730319F404CA1` | required |
| Oblivion / TES4 | 52 | legacy 20-byte header, HEDR 1.0 | `F1837BD9C5BED1215B813FFF4512AA88FC9E2A804A736357E419463C5C6B5C8F` | evidence only; out of scope |

Each is one top-level `TES4` record containing `HEDR` plus `CNAM`, zero child records, and no timestamp or random data. The payload bytes are therefore deterministic by construction. The current CAO invokes cleanup before splitting and generation after writing archives at [`BsaOptimizer.cpp` lines 73-139](https://github.com/evildarkarchon/CAO/blob/9969c3f9aad0f9f6f0a3d4489cc573f16486826f/src/BsaOptimizer.cpp#L73-L139).

Exact serializer fields are:

| Game | Record header/data | Flags | `HEDR` | `CNAM` |
| --- | --- | --- | --- | --- |
| SLE | 24-byte header; data size 25; version `0x2B` | `0` | float 1.7; record count 0; next ID `0x800` | one NUL byte |
| SSE | 24-byte header; data size 25; version `0x2C` | `0x200` | float 1.7; record count 0; next ID `0x800` | one NUL byte |
| FO4 | 24-byte header; data size 25; version `0x83` | `0x200` | float 0.95; record count 0; next ID `0x800` | one NUL byte |
| TES4 | legacy 20-byte header; data size 32 | `0` | float 1.0; record count 0; next ID `0x800` | `DEFAULT` plus NUL |

### Shipped profile files are not all generation templates

| Authenticated shipped file | Size | SHA-256 | Relation to native output |
| --- | ---: | --- | --- |
| `profiles/FO4/DummyPlugin.esp` | 49 | `AFFCBDEA9D14FE2199440912B326B9F5B704C34F436B51CEF03730319F404CA1` | exact FO4 native bytes |
| `profiles/TES5/DummyPlugin.esp` | 49 | `852F2DB6923C2203D60DAA176D5FA27D61A0D0E717B6819386BBFCBFF7FFFEFD` | exact SLE native bytes |
| `profiles/SSE/DummyPlugin.esp` | 128 | `ED29D3A93C5802E61EB78227629DB289BC14D71C6AC96D19B7E1A3BA5D13BD51` | not native output |

The 128-byte SSE file declares `Skyrim.esm` and `Update.esm` masters plus `INTV=1`, and its next object ID is `0xD61`. Its exact bytes do not occur in the executable. No CAO source path looks up a profile `DummyPlugin.esp`; generation writes the embedded backend payload. The shipped files are therefore provenance/reference material, not runtime generation templates.

### Name derivation and collision rules

The pinned BethUtil [`plugin.cpp`](https://github.com/Guekka/bethutil/blob/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02/src/bsa/plugin.cpp#L23-L109) and [game settings](https://github.com/Guekka/bethutil/blob/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02/include/btu/bsa/settings.hpp#L90-L208) establish these source-derived rules. The authenticated executable corroborates the payloads and BethUtil error literals, but the name matrix remains a required dynamic oracle gate because the v5.3.15 build used an unknown developer-local BethUtil checkout.

- Recognized archives are `.bsa` for SSE/SLE/TES4 and `.ba2` for FO4. The source comparisons are case-sensitive.
- Generated plugin extension is always `.esp`; existing `.esl`, `.esm`, or `.esp` at an exact or suffix-cleared association suppresses generation where that extension is supported.
- Trailing decimal digits are parsed as a counter and serialized numerically, so leading zeroes disappear (`Foo01` becomes `Foo1`).
- SSE recognizes the exact ` - Textures` suffix. FO4 recognizes ` - Main` and inherited ` - Textures`. Generation clears the recognized suffix, so `Foo - Textures.bsa` and `Foo - Main.ba2` both target `Foo.esp`.
- Main/texture archives that converge on one base name produce one plugin. Once it exists, later archives skip it.
- If no source plugin exists, CAO uses the processed folder's name as the archive/plugin base.

Directory enumeration is unsorted, and `find_archive_name` selects the first usable plugin. A single parsed path has deterministic source-derived naming, but a full run with multiple candidate plugins can be order-dependent. Do not claim whole-workflow determinism until the fixture matrix proves it.

The Rust implementation should reproduce these associations as explicit typed policy, but it should reject or report case-insensitive Windows collisions before writing. A fixture matrix must cover suffixes, counters including `01`, existing plugin extensions, case variants, colliding main/texture archives, Unicode names, and permission failures.

### Cleanup discrepancy

The pinned source's [`clean_dummy_plugins`](https://github.com/Guekka/bethutil/blob/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02/src/bsa/plugin.cpp#L143-L159) identifies a dummy only by file length, not bytes. It would delete any recognized top-level plugin of 49 bytes for SSE/SLE/FO4, or 52 bytes for TES4; conversely, the shipped 128-byte SSE artifact would not be recognized. This destructive hazard is source-derived pending a dynamic oracle deletion fixture. The port must require an exact authenticated hash/structure match. If the oracle confirms the hazard, record the safer behavior in the discrepancy register.

### Generation versus canonical fallback

Native generation requires only a fixed byte write and is therefore feasible without a plugin-format dependency. The pinned [`make_dummy_plugins`](https://github.com/Guekka/bethutil/blob/81f882ed4d3fbb3c04b0c90658a94b3b2eaade02/src/bsa/plugin.cpp#L162-L179) truncates/writes the target but does not check stream open or write status, so permission or disk failures can be silent in the source-derived workflow. The port must report and verify writes; if dynamically confirmed, that improvement is a discrepancy.

Implement generation as a small first-party serializer or authenticated byte constant behind a game-specific function, and assert exact size and SHA-256 in unit tests. For a fallback resource, use the native payload hashes above—not the shipped 128-byte SSE artifact—and preserve source/provenance metadata.

Before the functional-parity gate, repeat each required game's generation three times on one clean oracle snapshot and once on a clean clone, then validate loadability with an independent parser/game tool. Any byte instability would demote the comparison to structural equivalence, but none is expected because the payloads contain no variable fields.

## Reproduction checklist

1. Verify the archive hash with `Get-FileHash -Algorithm SHA256`, then use `tar -tf` and `tar -xOf` so package entries can be inspected without modifying the archive.
2. Hash the shipped INIs, rule files, dummy plugins, executable, and Qt runtime independently.
3. Search both ASCII and UTF-16 views of the executable for the exact INI keys, rule filenames, dummy arrays, and BethUtil error strings.
4. Compare current source files with source companion `9969c3f9...`; the relevant profile, landscape, filesystem, option, plugin, and BSA files currently have no diff.
5. Inspect `ce88caf2^` and `ce88caf2` to reproduce the historical BSA alias mapping.
6. Run the importer fixture table below against the supported Qt-compatible decoder and the typed fork importer. Malformed or lossy Qt constructs outside the supported subset are import errors with raw provenance, not behaviors the fork must emulate.

| Fixture | Expected decoded/import result |
| --- | --- |
| BOM-less byte `E9` | U+00E9 under Qt Latin-1 |
| BOM-less bytes `C3 A9` | U+00C3 U+00A9, not U+00E9 |
| UTF-8 BOM plus `C3 A9` | U+00E9 |
| Qt value escape `\xe9` | U+00E9 |
| Mixed-case section/key | same recognized key |
| Duplicate case variants | last physical occurrence wins; conflict reported |
| `C:\\Mods` in file | decoded `C:\Mods` |
| boolean empty / `0` / `FALSE` | false |
| boolean `1` / `TRUE` / `yes` | true; `yes` warned as noncanonical |
| old alias false / true | `(true,false)` / `(true,true)` |
| old alias plus either replacement | old generation ignored; present replacements win, absent replacements use deterministic baseline; conflict reported |
| invalid BOM-marked UTF-8 | import error; raw bytes retained |
| malformed/truncated percent or value escape | import error; raw text retained |
| unterminated quote or trailing backslash | import error; raw text retained |
| invalid numeric/list element | field-specific import error |
| missing whole settings file | legacy leakage noted; fork defaults used |
| missing individual key | legacy zero/false coercion noted; fork defaults used |

## Implementation constraints

- Keep raw legacy bytes and decoded values in the import provenance snapshot; never rewrite the source.
- Decode the supported Qt syntax before case-folding, alias resolution, or type validation; reject malformed syntax instead of emulating Qt's lossy recovery.
- Make duplicate precedence, alias conflicts, coercion differences, malformed syntax, and unsupported fields visible in import preview/reporting.
- Treat activated landscape rules and exact-hash dummy cleanup as discrepancies with regression evidence.
- Use game-specific native dummy payloads and never import a profile's `DummyPlugin.esp` as active state.
- Refuse unsupported game combinations rather than substituting another game's payload.
- Gate the implementation with the exact fixture matrices above and the authenticated hashes in this document.
