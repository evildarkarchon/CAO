# QSettings INI compatibility in Rust

Research for [#464](https://github.com/evildarkarchon/CAO/issues/464) on the map [#458](https://github.com/evildarkarchon/CAO/issues/458) (Port CAO to Rust and Slint).

**Question.** How should Rust read and write CAO's INI files so that existing `profiles/` stay compatible with what Qt 5.15's `QSettings` (IniFormat) wrote? Which Qt rules does CAO's data actually exercise, and should we use a Rust INI crate or a small purpose-built reader and writer?

**Answer.** Write a small, dependency-free reader and writer (about 300 lines plus tests). Port four Qt routines directly over bytes: `readIniLine`, `iniUnescapedKey`/`iniEscapedKey`, `iniUnescapedStringList`/`iniEscapedString`, and the `@`-prefixed variant decoding. Add a tiny QDataStream decoder and encoder for exactly one case: a one-element integer list, which Qt writes as `@Variant(...)`. Then add typed getters that copy `QVariant`'s lenient conversions (`toBool`, `toInt`, `toUInt`, `toDouble`, `toList`).

None of the Rust INI crates can be configured to round-trip QSettings files. Several of them reject files that Qt reads cleanly, or silently misread them.

## Sources

Every Qt claim below was read in Qt 5.15 source on the `5.15` branch of `qt/qtbase`. Line numbers refer to that branch as fetched on 2026-10-06.

- `src/corelib/io/qsettings.cpp` ([raw](https://raw.githubusercontent.com/qt/qtbase/5.15/src/corelib/io/qsettings.cpp))
  - `variantListToStringList`/`stringListToVariantList`: 363–393
  - `variantToString`: 395
  - `stringToVariant`: 477
  - `iniEscapedKey`: 534
  - `iniUnescapedKey`: 561
  - `iniEscapedString`: 617
  - `iniEscapedStringList`: 707
  - `iniUnescapedStringList`: 728
  - `syncConfFile`: 1397 (`QLockFile` at 1424, `QSaveFile` at 1500)
  - `charTraits`: 1556
  - `readIniLine`: 1582
  - `readIniFile`: 1653 (UTF-8 BOM check at 1676–1684)
  - `readIniSection`: 1729
  - `writeIniFile`: 1821
  - `set()` and `nextPosition`: 1262, 1132
  - `mergedKeyMap`: 165
  - The class documentation for IniFormat: around 2480–2520. It is published as the [QSettings docs, "INI Files"](https://doc.qt.io/qt-5/qsettings.html#Format-enum).
- `src/corelib/io/qsettings_p.h`: 70–110. On Windows `IniCaseSensitivity` is `Qt::CaseInsensitive`, and keys keep their original case and position.
- `src/corelib/kernel/qvariant.cpp`
  - `qt_convertToBool`: 348
  - bool → string: 486
  - double → string: 469–472
  - `QStringList` → `QString`: 491
  - → `QVariantList`: 909
  - string → enum: 1352–1392
  - `QVariant::save`/`load`: 2547 and 2482
- `src/corelib/text/qlocale.cpp`
  - `doubleToString` shortest/`'g'` cutoff: 3652–3670
  - `numberToCLocale` whitespace skipping: around 3925–3935
- `src/corelib/codecs/qutfcodec.cpp:517`: `QString::fromUtf8` skips a leading UTF-8 BOM.
- `src/corelib/text/qchar.cpp:788`: `QChar::isSpace`.
- Local readers and writers:
  - `src/OptionsCAO.cpp:7-98`
  - `src/Profiles.cpp:12-138`
  - `src/Profiles.h:14-16,102`
  - `src/OptimizerProfileSnapshot.h:46-95`
  - `src/Run/ApplicationRunSetup.cpp:12-61`
  - `src/MainWindow.cpp:171-195,445-459`
  - `src/FilesystemOperations.cpp:124-152` (has no callers)
- Shipped data, all inspected byte-wise: `profiles/common.ini`, `profiles/{FO4,SSE,TES5}/{settings,profile}.ini`, `profiles/SSE/*.txt` and `profiles/*/isBase`.
- Rust crates: the published `.crate` tarballs from static.crates.io were read, and a scratch probe exercised rust-ini and configparser. Versions are listed under [Crate evaluation](#crate-evaluation).

## What CAO's files contain in practice

| File | Written by | Keys and types |
|---|---|---|
| `profiles/common.ini` | `Profiles::loadProfile`: `profile` is rewritten on every profile load. `MainWindow::saveUi`/`firstStart` write the rest. | `[General]`: `profile` (string), and the bools `bShowAdvancedSettings`, `bDarkMode`, `showTutorial` (read with default `true`) and `notFirstStart`. |
| `profiles/<P>/settings.ini` | `OptionsCAO::saveToIni`, called from `MainWindow::saveUi` | `[General]`: `bDryRun`, `bDebugLog`, `mode` (a `Q_ENUM`, written as int), `userPath` (string).<br>`[BSA]`, `[Textures]`, `[Meshes]`, `[Animations]`: bools and ints (`iTextures*`, `iMeshesOptimizationLevel`). |
| `profiles/<P>/profile.ini` | `Profiles::saveToIni` (GUI)<br>Also read by `Profiles::readFromIni`, `OptimizerProfileSnapshot::fromSettings` and `ApplicationRunConfigurationProvider::load` | `[BSA]`: `bsaEnabled`, `maxBsaUncompressedSize` (double), `bsaGame` (`btu::Game` as int).<br>`[Meshes]`: `meshesFileVersion` (`NiFileVersion` as int, e.g. `335675399`), `meshesStream`, `meshesUser` (uint), `meshesEnabled`.<br>`[Animations]`: `animationFormat` (dead), `animationsEnabled`.<br>`[Textures]`: `texturesFormat` (`DXGI_FORMAT` as int), `texturesConvertTga`, `texturesUnwantedFormats` (**list of ints**), `texturesCompressInterface`, `texturesEnabled`. |

What actually occurs in the shipped files:

- All of them are pure ASCII with CRLF line endings, have no BOM and no comments, and use `key=value` with no spaces.
- `[General]` appears only in `common.ini` and `settings.ini`. `profile.ini` has no root keys, so it has no `[General]`.
- Bools are written as `true`/`false`. Doubles look like `4187593113.6` and `2104533975.04`.
- Lists look like `texturesUnwantedFormats=85, 86, 115` and `98, 99`. FO4 has `86, 85, 115`, so order is user data.
- `userPath=` holds an empty string.
- `settings.ini` carries a key the code no longer reads or writes (`bBsaLeastBSA`). It lacks two keys the code does write (`bBsaMergeIncomp`, `bBsaMergeTexture`). QSettings keeps the unknown key on save and appends the new ones. See "Ordering on write" under [Writing](#writing-writeinifile-iniescapedkey-iniescapedstring-varianttostring).
- None of these appear in the shipped data: `%` key escapes, `\` subkeys, quoting, `\x` escapes, `@Variant`, `@Invalid` or `[%General]`. The features below that the shipped data does not exercise **are nonetheless reachable from the GUI**: a one-element or empty unwanted-format list, or a non-ASCII, comma- or semicolon-bearing `userPath`.

## The QSettings IniFormat rules CAO exercises

### Reading lines (`readIniLine`, `readIniFile`, `readIniSection`)

- **Encoding.** The file is read as bytes.
  - With a UTF-8 BOM (`EF BB BF`), Qt skips the BOM and switches `iniCodec` to UTF-8 (qsettings.cpp:1676–1684).
  - Otherwise each byte becomes one Latin-1 character (qsettings.cpp:834–845).
  - Keys and section names are **always** decoded byte-as-Latin-1, even under the BOM codec, because `iniUnescapedKey` uses `QLatin1Char`.
  - The class docs say: "QSettings will accept Latin-1 encoded INI files, but generate pure ASCII files, where non-ASCII values are encoded using standard INI escape sequences."
- **Line splitting.** Leading space, tab, CR and LF are skipped. The special bytes are `\n`, `\r`, `"`, `;`, `=` and `\` (the `charTraits` table, qsettings.cpp:1556).
  - A line ends at CR or LF outside double quotes. A quoted value may span physical lines.
  - `\` escapes the next byte, so backslash + newline continues the line. `\r\n` and `\n\r` count as one newline.
  - The first `=` outside quotes splits the key from the value.
  - A `;` at the start of a line is a comment through the end of the line. A `;` later in the line, outside quotes, ends the logical line, which makes it an inline comment.
- **`#` is not a comment character in QSettings.**
  - `#foo=1` is a key named `#foo`.
  - A non-`;` line with no `=` makes `readIniSection` return false, which sets `QSettings::FormatError` (qsettings.cpp:1745–1749).
- **Section headers.** A line starting with `[` is a section header.
  - The name runs to the first `]` on the line and is trimmed. Anything after `]` is ignored.
  - If the `]` is missing, the rest of the line is used and `FormatError` is set (qsettings.cpp:1696–1701).
  - `[general]` in any case means the root group, i.e. keys with no `/`. `[%general]` in any case means a real group named `General`. Any other name is `%`-unescaped.
  - Keys before the first header are root keys.
  - Repeated sections are concatenated: `FLUSH_CURRENT_SECTION` appends to the same map entry.
- **Keys.** The key is the text before `=`, minus trailing spaces and tabs, run through `iniUnescapedKey`:
  - `\` becomes `/`, which makes it a subkey.
  - `%XX` becomes the Latin-1 character and `%UXXXX` the UTF-16 unit. A malformed `%` is kept literally.
  - The full key is `Section/key`.
- **Duplicates.** If a key appears twice, the **last value wins** (QMap insert), but the key keeps its first position.
- **Lookup is case-insensitive on Windows.** `QSettingsKey` lowercases for comparison but keeps the original case and file position for writing (qsettings_p.h:95–110). Non-Windows builds are case-sensitive; that is irrelevant here.

### Value decoding (`iniUnescapedStringList`, then `stringToVariant`)

- **Whitespace.** Leading spaces and tabs are skipped. Trailing spaces and tabs are chopped **unless that part was quoted**.
- **Quotes.** `"` toggles quoting and the quote characters are removed. `"a" b` concatenates. Commas and `;` inside quotes are literal.
- **Escapes.**
  - `\a \b \f \n \r \t \v \" \? \' \\` map to the usual characters.
  - `\x` takes **any number** of hex digits (greedy, accumulated into a 16-bit unit).
  - `\` followed by an octal digit is a greedy octal escape (`\0` is NUL).
  - Backslash + newline is a continuation.
  - **Any other escaped character is dropped together with its backslash.**
- **Lists.** An unquoted `,` makes the value a **`QStringList`**. Spaces after each comma are skipped, and trailing spaces of unquoted elements are chopped. So `85, 86, 115` becomes `["85","86","115"]`. A value with no unquoted comma is a single `QString`.
- **`@` prefixes** (`stringToVariant`, qsettings.cpp:477). These apply only when the string starts with `@` and ends with `)`:
  - `@ByteArray(…)` and `@String(…)`.
  - `@Variant(…)`: the payload is a Latin-1 byte string fed to `QDataStream`, version Qt_4_0, big-endian.
  - `@DateTime(…)`, `@Rect(…)`, `@Size(…)` and `@Point(…)`.
  - `@Invalid()` means an invalid `QVariant`.
  - `@@x` is the literal `@x`.
  - If any element of a list starts with a single `@`, every element goes through `stringToVariant` and the result is a `QVariantList` (qsettings.cpp:374–393).

### Typed reads CAO performs (`QVariant` conversions)

These decide what a Rust getter must return. **A missing key is an invalid `QVariant`**, which reads as `false`, `0`, `0.0`, `""` or `[]`. It does *not* fall back to the shipped default. The one exception is `showTutorial`, read with default `true`.

- **`toBool()` on a string** is false iff the lowercased string is empty, `"0"` or `"false"` (qvariant.cpp:348–352). So `True` and `FALSE` behave as expected, `no` is **true**, and `1` is true.
- **`toInt()`, `toUInt()`, `toDouble()` and enum `value<T>()` on a string** use C-locale parsing that skips leading and trailing whitespace (qlocale.cpp around 3925). On failure the result is **0**.
  - `toInt` covers `bsaGame`, `meshesFileVersion`, `iMeshesOptimizationLevel`.
  - `toUInt` covers `meshesStream`, `meshesUser`, `iTextures*`. It rejects negatives, which therefore read as 0.
  - Enums (`DXGI_FORMAT`, `mode`) go through `qConvertToNumber`, i.e. `toLongLong` (qvariant.cpp:1376–1392).
  - `mode` is a `Q_ENUM`, so a key name such as `SeveralMods` is also accepted first (qvariant.cpp:1352–1374). Only integers are ever written.
- **`toString()`** on a one-element `QStringList` returns that element. On a longer list it returns `""` (qvariant.cpp:491–494).
- **`toList()`** (`texturesUnwantedFormats`):
  - A `QStringList` gives a list of string variants, each later converted with `toLongLong`. Invalid elements become `0`, i.e. `DXGI_FORMAT_UNKNOWN`.
  - A `QVariantList` decoded from `@Variant` is returned as is.
  - `@Invalid()` or a missing key gives `[]`.
  - **A plain scalar such as `texturesUnwantedFormats=85` gives `[]`**, because `QString` → `QVariantList` is not a supported conversion (qvariant.cpp:909–919).

### Writing (`writeIniFile`, `iniEscapedKey`, `iniEscapedString`, `variantToString`)

- **Layout.**
  - Each section is `[Name]` followed by `key=value` lines, with no spaces around `=`.
  - On Windows lines end in **CRLF**. There is a blank line *between* sections, but none before the first and none at the end.
  - Root keys go under `[General]`, and a real group called `General` is written `[%General]` (qsettings.cpp:1858–1874).
  - Qt **re-serialises the whole file** from its parsed map, so comments, blank lines and the original formatting are not preserved. Neither the Rust writer nor its tests need to preserve them.
- **Atomicity.** The write goes through `QSaveFile` (temporary file, then rename) under a `QLockFile` named `<file>.lock` (qsettings.cpp:1424, 1500).
- **Key and section escaping.**
  - `[A-Za-z0-9_.-]` are kept, `/` becomes `\`, other characters up to 0xFF become `%XX` (uppercase hex), and anything above that becomes `%UXXXX`.
  - All of CAO's keys are plain ASCII identifiers, so none are escaped.
- **Scalars** (`QVariant::toString`):
  - bool is written `true`/`false`, and ints and uints in decimal.
  - A double is written `QString::number(d, 'g', QLocale::FloatingPointShortest)`. That is the shortest round-trip digits in fixed notation, with exponent form only when it is shorter. For example `2e+09` for 2000000000 (qlocale.cpp:3652–3670), while `2147483648` and `4187593113.6` stay fixed.
  - A string that starts with `@` is written `@@…`. A string containing NUL is written `@String(…)`.
- **String escaping** (`iniEscapedString`, qsettings.cpp:617–705, with no codec):
  - `\0 \a \b \f \n \r \t \v \" \\` are written as named escapes.
  - Other characters at or below 0x1F, **and every UTF-16 unit at or above 0x7F**, become `\x` + lowercase hex with no padding. For example `é` becomes `\xe9`, and characters outside the BMP become two surrogate escapes such as `\xd83d\xde00`.
  - After a `\x…` or `\0` escape, a following hex-digit character is also hex-escaped, so that the greedy reader stops in the right place.
  - The value is wrapped in `"…"` if it contains `;`, `,` or `=`, or starts or ends with a space. For example `C:/Mods/Café, Stuff` is written `userPath="C:/Mods/Caf\xe9, Stuff"`.
- **Lists.**
  - A `QStringList`, or a `QVariantList` whose size is **not 1**, is written as its elements joined by `", "`, each element escaped.
  - An empty list is written `@Invalid()`.
  - **A one-element `QVariantList` falls through to `variantToString` and is written as a QDataStream blob.** `texturesUnwantedFormats` is a `QList<QVariant>` of ints (Profiles.h:102, Profiles.cpp:187–193), so a user who leaves exactly one unwanted format in the GUI gets, for example:

    ```
    texturesUnwantedFormats=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2\0\0\0U)
    ```

    The payload bytes are `00000009 00000001 00000002 00000055`. That is type `QVariantList` (9), count 1, type `Int` (2), value 85, big-endian, with no null flag because Qt_4_0 is older than Qt_4_2 (qvariant.cpp:2547–2583).
  - The trailing byte follows the escaping rules: 98 becomes `\x62`, because `b` is a hex digit following `\0`, and 200 becomes `\xc8`. A value of 61 (`=`) makes Qt quote the whole blob.
  - These examples come from a Python emulation of `iniEscapedString`; Qt was not run.
- **Ordering on write:**
  - Keys read from the file keep their file position. Keys set in this session get positions starting at `0x40000000`, in `setValue` order (qsettings.cpp:1132, 1270).
  - Sections are ordered by their smallest key position, and keys within a section by position (qsettings.cpp:1838–1856).
  - So the original order is kept, keys that are new to an existing section go at the end of that section, and new sections go at the end of the file.
  - Setting an existing key with different case keeps the original spelling and position.

### Auxiliary text files

- **`customHeadparts.txt` and `FilesToNotPack.txt`** are read by `OptimizerProfileSnapshot::loadAuxiliaryLists` (OptimizerProfileSnapshot.h:79–95). **`ignoredMods.txt`** is read by `ApplicationRunConfigurationProvider::load` (ApplicationRunSetup.cpp:44–59). Each file:
  - Is resolved in the selected profile first, then per file in `profiles/SSE`.
  - Is read with `QFile::readLine`, which splits on LF only.
  - Has each line decoded with `QString::fromUtf8` and passed through `simplified()`. `fromUtf8` uses U+FFFD for invalid bytes and drops a leading UTF-8 BOM (qutfcodec.cpp:517). `simplified()` trims and collapses internal runs of `QChar::isSpace` characters to a single space: ASCII `\t\n\v\f\r `, plus U+0085, U+00A0 and the Unicode Zs, Zl and Zp categories (qchar.cpp:788).
  - Skips empty lines and lines starting with `#` (after simplification).
- **Error handling.** A missing file means an empty list. For `ignoredMods.txt` only, a file that exists but cannot be opened or read is a run-setup error.
- **Rust equivalent:** `from_utf8_lossy`, strip a leading `\u{FEFF}`, `split_whitespace().join(" ")`, then skip `""` and `#…`. Rust's `char::is_whitespace` (Unicode `White_Space`) covers the same set as `QChar::isSpace` for practical purposes.
- **`customLandscape.txt`** is UTF-16LE with a BOM and CRLF line endings. Nothing reads it: no code references it, and `FilesystemOperations::readFile` and `Profiles::getFile` have no callers. The map says to leave it alone.
- **`isBase`** is a 2-byte file (`FF FE`, an empty UTF-16LE BOM). Only its existence is tested (`Profiles.cpp:52`; profile creation deletes it from the copy, `Profiles.cpp:69`).

## Crate evaluation

| Crate (version inspected) | Verdict | Disqualifying behaviour (source line in the published crate) |
|---|---|---|
| `rust-ini` 0.21.3 (lib `ini`, MIT; depends on `ordered-multimap` → `dlv-list` → `const-random`, `hashbrown`) | No | See the list below this table. |
| `configparser` 3.3.0 (MIT/LGPL-3.0, zero deps by default) | No | Inline comments are on by default and cut at the first `;`/`#` regardless of quotes, so `k="a;b"` gives `"a` (ini.rs:895). No escapes, quotes or lists. `Ini::new()` lowercases names so original case is lost (ini.rs:962–967). Unordered unless the `indexmap` feature is on. UTF-8 only, and a BOM is not stripped. |
| `ini` 2.0.0 | No | A macro wrapper over configparser 3.3.0, not rust-ini, with identical semantics. |
| `tini` 1.3.0 (unmaintained since 2021) | No | Cuts at any `;`/`#` regardless of quotes. `get_vec` is a naive `split(",")`. Writes `key = value` with LF. |
| `ini_core` 0.2.0, `ini-roundtrip` 0.2.1 | No | Tokenisers only: no escapes, quotes, continuations or BOM handling. `[S] ;c` is an error. `#` is a comment in ini-roundtrip. |
| `serde_ini` 0.2.0 (unmaintained since 2019) | No | No escapes, and a line without `=` is a syntax error. |
| "qsettings" crates | None exist | No crate on crates.io claims QSettings compatibility. Three unpublished hand ports of `qsettings.cpp` inside other applications (about 260, 275 and 710 lines) confirm the size estimate. Their licences have not been vetted, so do not copy them. |

rust-ini 0.21.3 fails for these reasons:
- `;` or `#` at the start of a line is always a comment, so `#foo=1` is dropped (lib.rs:1378–1388).
- `[S] ; c` is a hard error without the `inline-comment` feature (lib.rs:1385), and that feature is not Qt-compatible either.
- `\x` requires exactly 4 hex digits (lib.rs:1582–1598), so Qt's own `\xe9` is rejected.
- `\v`, `\f`, `\?` and `\'` are mis-decoded, and the `group\key` subkey syntax is mangled.
- Quote stripping erases the information Qt needs to split lists.
- `:` is also a key/value separator, and the first duplicate wins where Qt keeps the last.
- Backslash + CRLF continuations are a parse error, and Latin-1 bytes are rejected because the input must be UTF-8.
- The writer pads `\x` to 4 digits with no next-digit guard, so `é1` becomes `\x00e91`, which Qt reads as U+0E91. It never quotes values.

**rust-ini as a raw line splitter.** One option is rust-ini with quote and escape handling disabled, plus our own value decoding. The probe showed this still:
- Merges an indented `; comment`, or a line without `=`, into the next key.
- Breaks quoted multi-line values.
- Hard-fails on `[S] ; c` and on backslash + CRLF.
- Leaves us to write all of the decoding, encoding, quoting, `[General]` mapping and Latin-1 handling ourselves.

The only part it would save is the writer, which is about 15 lines.

## Recommendation

A purpose-built module, for example `cao_profiles::qsettings_ini` or wherever [#468](https://github.com/evildarkarchon/CAO/issues/468) puts profile I/O. It needs no dependencies and works on `&[u8]`.

1. **Parse:**
   - Port `readIniLine`, the header and `[General]`/`[%General]` handling, `iniUnescapedKey` and `iniUnescapedStringList` one-for-one.
   - Decode bytes as Latin-1, or as UTF-8 when there is a BOM. Values decode into UTF-16 units so that surrogate escapes pair correctly (`String::from_utf16_lossy`).
   - Store an ordered `Vec<Entry { section, key, raw_value: Value }>` with an ASCII-case-insensitive index, last value wins.
   - `Value` is `Str(String) | List(Vec<String>) | Invalid | Variant(Vec<u8>)`. Keep the `@ByteArray`/`@String`/`@@` cases. Treat any other `@…)` form as an opaque string, which round-trips.
   - Record a `format_error` flag for the two Qt conditions: a header without `]`, and a non-`;` line without `=`.
2. **Typed getters that copy `QVariant`:**
   - `get_bool` uses the `""`/`"0"`/`"false"` rule.
   - `get_i32`, `get_u32` and `get_f64` trim ASCII whitespace, parse, and return 0 on failure.
   - `get_string`.
   - `get_int_list` handles a comma list, the `@Variant` `QVariantList` of `Int`(2)/`UInt`(3)/`QString`(10), and `@Invalid()`, missing or unrecognised input as `[]`.
   - Missing keys give the zero value, except where the C++ passes a default (`showTutorial`).
3. **Writer:**
   - Write `[General]` and the sections in Qt's position order. Keep unknown keys such as `bBsaLeastBSA` and `animationFormat`, and append new keys.
   - Use `key=value` with CRLF and a blank line between sections.
   - Port `iniEscapedKey` and `iniEscapedString` exactly: lowercase `\x`, the next-hex-digit guard, and quoting.
   - Lists are joined with `", "`, an empty list is `@Invalid()`, and **a one-element int list is the `@Variant(…)` blob**. That is about 15 lines of encoding and keeps files readable by the C++ build and by older CAO.
   - Bools are `true`/`false`. f64 uses Rust's `{}` formatting, which gives the same shortest round-trip digits. It never uses exponent form, where Qt would write `2e+09`; both forms read back to the same value, so this is equivalent output, not byte-identical.
   - Save with temp file + `std::fs::rename` in the same directory. That is atomic replace on Windows, the same guarantee as `QSaveFile`.
   - Skip the `.lock` file. There is one process per user, and Qt only uses it to serialise concurrent writers.
4. **Tests:**
   - Golden round-trip tests on the shipped `profiles/`.
   - Unit tests transcribed from the rules above: the `@Variant` examples, `userPath="…\xe9, …"`, `[%General]`, `\x` greediness, inline `;`, duplicate-last-wins, `#` lines giving a format error, and a plain scalar list giving `[]`.
   - While the C++ oracle exists, a differential check that Qt reads Rust-written files to the same values.
5. **Auxiliary text files:** a 10-line `read_list(path)` with the per-file SSE fallback, as described above. Do not touch `customLandscape.txt`.

## Risks and open points

- **One-element and empty unwanted-format lists.** These are the only Qt-binary encodings CAO can produce: `@Variant(…)` and `@Invalid()`. They are absent from the shipped files but reachable from the GUI. Missing them would silently turn a user's single unwanted format into "none".
- **Plain scalar list quirk (candidate deviation).** Qt reads a hand-edited `texturesUnwantedFormats=85` as an empty list. That is clearly unintended. The simplest fix is for the Rust reader to treat a plain scalar as a one-element list. If adopted, record it in the map's "Fix, don't copy" list. Copying Qt is equally cheap.
- **`#` comment lines.** A user who adds `# note` without `=` to `profile.ini` triggers `FormatError`. `ApplicationRunConfigurationProvider::load` checks `settings.status()`, so the run fails with "Selected profile could not be read". The GUI loaders never check status.
  - Qt parses sections lazily, so strictly the error only fires for sections that were read. All four sections are read during run setup.
  - Port the flag and fail the same way, or decide to ignore `#` lines and record the deviation.
- **Non-ASCII hand edits.** Qt-written files are pure ASCII. A user who hand-types UTF-8 into `userPath` without a BOM gets Latin-1 mojibake in Qt.
  - Copy that for parity, or decode as UTF-8 when valid (a candidate deviation). Either choice is invisible for Qt-written files.
  - Related Qt quirk: after reading a BOM file, Qt *writes* raw UTF-8 without a BOM, and the next read is mojibake. Writing ASCII-only, as recommended, avoids this.
- **Case-insensitive lookup.** If lookup is case-sensitive, hand-edited `[bsa]` or `BSAGAME=` stops matching. Keep it case-insensitive and preserve the original case on write.
- **Values are lenient by design.** Unparseable numbers become 0 and unknown bool strings become true. Rust `Result`-based parsing must deliberately fall back the Qt way, or a corrupt `profile.ini` will behave differently from C++. That matters for the differential oracle.
- **Not executed against Qt.** Every rule above is read from source, and the `@Variant` examples come from an emulation. The first implementation slice should confirm one GUI-saved one-element list and one `userPath` with `é,` against the C++ build.

## Notes for other tickets

- **#468 (workspace architecture).**
  - The INI module is pure, about 300 lines, needs no dependencies and no GUI, and should sit in the profile/config core crate next to the domain types it fills. The relevant types are `btu::Game`, `DXGI_FORMAT` and `NiFileVersion`, all kept as raw integers.
  - Run setup reads `profile.ini` independently of the GUI's live copy and fails on `FormatError`. The core API should expose a load result that can carry that error.
  - Writes happen only from the GUI's save path and from `common.ini` profile selection.
- **Exe-relative `profiles/`** (a map deviation): the INI layer takes explicit paths and never resolves them itself.
