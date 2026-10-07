# Windows file-safety primitives: C++ to Rust mapping

Research for [#463](https://github.com/evildarkarchon/CAO/issues/463), part of the Rust and Slint
port map [#458](https://github.com/evildarkarchon/CAO/issues/458). Checked 2026-10-06 against:

- the C++ tree at `8767f86`;
- Rust std **1.99.0** source (`library/std/src/sys/fs/windows.rs`, `sys/path/windows.rs`,
  `sys/paths/windows.rs`, `os/windows/fs.rs`, `fs.rs`, `path.rs`, `env.rs`);
- the source of `windows-sys` 0.61.2, `same-file` 1.0.6, `winapi-util` 0.1.11, `fs4` 0.12.0,
  `dunce` 1.0.5, `getrandom` 0.3.4, and `ba2` 3.0.1;
- the MSVC STL 14.51 `<filesystem>` header;
- Microsoft Learn Win32 reference pages (listed under [Sources](#sources)).

Claims marked **[std src]**, **[STL src]**, or **[crate src]** were read in that source. Claims
marked **[MS]** come from Microsoft Learn.

## Answer in brief

- **Open every handle through `std::fs::OpenOptions` + `std::os::windows::fs::OpenOptionsExt`.**
  `access_mode`, `share_mode`, and `custom_flags` reproduce every `CreateFileW` call in the C++
  code. `std` also prefixes long paths with `\\?\` for you, owns the handle, and maps errors.
- **Query and mutate file objects through `windows-sys` on `file.as_raw_handle()`.** That covers
  `GetFileInformationByHandle`, `GetFileInformationByHandleEx` (`FileBasicInfo`, `FileIdInfo`,
  `FileAttributeTagInfo`), and `SetFileInformationByHandle` (`FileRenameInfo`,
  `FileDispositionInfo`). The `std` accessors for volume serial, link count, file index, and
  change time are still **unstable**. `std` has no handle-based rename or delete.
- **Call path-level functions through `windows-sys`:** `MoveFileExW(REPLACE_EXISTING |
  WRITE_THROUGH)`, `GetVolumePathNameW`, `GetVolumeNameForVolumeMountPointW`,
  `CompareStringOrdinal`, `GetDiskFreeSpaceExW`, and `GetFinalPathNameByHandleW` for the
  manifest-compatible canonical path. `std::fs::rename` drops `WRITE_THROUGH`, and `std` has no
  API for the others.
- **Crate set:** `windows-sys` 0.61 (features `Win32_Foundation`, `Win32_Security`,
  `Win32_Storage_FileSystem`, `Win32_Globalization`) and `getrandom` for staging nonces. Use
  `dunce` only to tidy the exe directory. **Reject** `same-file`, `fs4`, and the heavier `windows`
  crate for this layer; the reasons are in [Helper crate evaluation](#helper-crate-evaluation).
- **Biggest compatibility trap:** the `CAO-STAGING` manifest stores the Mod Root as the exact
  generic UTF-8 text of MSVC `std::filesystem::canonical`. Recovery compares it byte for byte.
  Neither `std::fs::canonicalize` nor `dunce::canonicalize` produces that text in every case. The
  port needs a small `msvc_canonical` function, described in
  [The canonical-path trap](#the-canonical-path-trap).
- **Exe directory:** use `std::env::current_exe()?.parent()` without canonicalizing, pass it
  through `dunce::simplified`, and join components one at a time. Details are in
  [Resolving the exe directory](#resolving-the-exe-directory).

## Inventory and mapping

"Layer" names where the Rust call comes from. **std** means `std::fs`. **std-ext** means
`std::os::windows::fs` / `std::os::windows::io`. **sys** means `windows-sys`.

| # | Primitive (C++ sites) | What it guarantees | Rust mapping | Layer | Gap |
|---|---|---|---|---|---|
| 1 | `CreateFileW` directory pin: `FILE_LIST_DIRECTORY \| FILE_READ_ATTRIBUTES`, share `READ\|WRITE` (or `READ` only), `OPEN_EXISTING`, `BACKUP_SEMANTICS \| OPEN_REPARSE_POINT` (`NativeFilePins.cpp:60`, `TemporaryArtifactRegistry.cpp:151`, `StagingRecovery.cpp:98`) | No `FILE_SHARE_DELETE`, so the directory cannot be renamed or replaced while pinned. Read access makes the sharing check bite (`NativeFilePins.cpp:52-53`). | `OpenOptions::new().access_mode(FILE_LIST_DIRECTORY \| FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ \| FILE_SHARE_WRITE).custom_flags(FILE_FLAG_BACKUP_SEMANTICS \| FILE_FLAG_OPEN_REPARSE_POINT).open(p)` | std-ext (+ sys constants) | None. `access_mode` overrides read/write; `share_mode` replaces std's default, which includes `FILE_SHARE_DELETE` **[std src]**. |
| 2 | `CreateFileW` source/plugin pin: `GENERIC_READ` (or `FILE_READ_DATA \| FILE_READ_ATTRIBUTES`), share `READ` (± `DELETE`), `OPEN_REPARSE_POINT` (`NativeFilePins.cpp:93,220,257`, `AssetExecutor.cpp:78`, `TemporaryArtifactRegistry.cpp:259`) | Denies writers, and denies renames unless `FILE_SHARE_DELETE` is passed. Opens the link itself, not its target. | Same as #1 without `BACKUP_SEMANTICS` | std-ext | None |
| 3 | `CreateFileW` cleanup handle: `DELETE \| FILE_READ_ATTRIBUTES` (± `GENERIC_READ`), share `READ` (± `DELETE`), with a guardian handle held across the switch (`NativeFilePins.cpp:179,189`, `AssetExecutor.cpp:111`, `ArchiveFinalizationLoadingPlugins.cpp:98`, `StagingRecovery.cpp:98` `TemporaryFile`) | Delete authority bound to one verified file object | Same as #1 with `DELETE` in `access_mode` | std-ext | None |
| 4 | `CreateFileW` staged publication handle: `GENERIC_WRITE \| DELETE \| FILE_READ_ATTRIBUTES`, share `0`, `OPEN_EXISTING`, `OPEN_REPARSE_POINT \| FILE_FLAG_WRITE_THROUGH` (`TemporaryArtifactRegistry.cpp:299`) | Exclusive handle used to flush and then rename the staged bytes | `.access_mode(GENERIC_WRITE \| DELETE \| FILE_READ_ATTRIBUTES).share_mode(0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT \| FILE_FLAG_WRITE_THROUGH)` | std-ext | None. `custom_flags` ORs into `dwFlagsAndAttributes` **[std src]**. |
| 5 | `CreateFileW` exclusive create: `GENERIC_WRITE`, share `0`, `CREATE_NEW`, `OPEN_REPARSE_POINT` (manifest scratch, `StagingRecovery.cpp:404`) | Never truncates; a collision fails with `ERROR_FILE_EXISTS` | `.write(true).create_new(true).share_mode(0)` | std / std-ext | None. `create_new` adds `FILE_FLAG_OPEN_REPARSE_POINT` itself (`windows.rs:329-334`) **[std src]**. |
| 6 | **`owner.lock`**: `GENERIC_READ`, share `0`, `CREATE_NEW` (bootstrap) or `OPEN_EXISTING` (recovery), `OPEN_REPARSE_POINT`. Then checks: not a reparse point, not a directory, `nNumberOfLinks == 1`. `ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION` maps to `StagingActive` (`StagingRecovery.cpp:93-120`) | Ownership is the share-mode-0 open itself. The handle lives until Safety Cleanup ends. The OS releases it when the process dies. | Recovery: `.access_mode(GENERIC_READ).share_mode(0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(p)`. Bootstrap: also `.write(true).create_new(true)`; **see the trap below**. Map `e.raw_os_error()` 32 or 33 to `StagingActive`. | std-ext + sys (link count) | **Do not use `File::lock`/`try_lock`** (stable since 1.89). They are `LockFileEx` byte-range locks **[std src]**, not share-mode exclusion. A C++ run would not see such a lock, and the C++ share-0 open would not see it either. |
| 7 | Reparse-point rejection: `dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT` on an open handle (all pins), `GetFileAttributesW` on a path (`StagingRecovery.cpp:69`, `ArchiveFirstAssetDiscovery.cpp:161,246`, `ArchiveFinalizationPlanning.cpp:108`), `FileAttributeTagInfo` (`ArchiveExtraction.cpp:59`) | Rejects **every** reparse tag: symlinks, junctions, mounted folders, cloud placeholders, dedup files, and so on | Handle: `file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT` (stable `MetadataExt::file_attributes`). `File::metadata` calls `GetFileInformationByHandle` and, for reparse points, `FileAttributeTagInfo` (`windows.rs:523-555`) **[std src]**. Path: `fs::symlink_metadata(p)?.file_attributes()`. | std / std-ext | **Never use `FileType::is_symlink()` as the test.** It is true only for reparse tags with the name-surrogate bit, `0x20000000` (`windows.rs:1196-1205`) **[std src]**. Non-surrogate reparse points would pass as ordinary files or directories. |
| 8 | `GetFileInformationByHandle` → `BY_HANDLE_FILE_INFORMATION`: volume serial, 64-bit file index, link count, size, times (`NativeFilePins.cpp:14,70,101`, `TemporaryArtifactRegistry.cpp:169,269`, `AssetExecutor.cpp:51`, `StagingRecovery.cpp:114`) | Identity, the single-link check, and change detection | `windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(file.as_raw_handle(), &mut info)` | sys | The std wrappers `volume_serial_number`, `number_of_links`, and `file_index` are **unstable** (`windows_by_handle`, #63010) **[std src `os/windows/fs.rs:598-617`]**. Call the API directly. |
| 9 | `GetFileInformationByHandleEx(FileBasicInfo)` → `ChangeTime`, `LastWriteTime` (`NativeFilePins.cpp:15`, `TemporaryArtifactRegistry.cpp:275`) | Change detection that catches in-place edits | `GetFileInformationByHandleEx(h, FileBasicInfo, &mut FILE_BASIC_INFO, size)` | sys | `MetadataExt::change_time` is unstable (`windows_change_time`, #121478), and std leaves it `None` on desktop Windows anyway (`windows.rs:545`) **[std src]** |
| 10 | `GetFileInformationByHandleEx(FileIdInfo)` → 128-bit ID, with a 64-bit fallback when it fails (Wine, older file systems) (`TemporaryArtifactRegistry.cpp:177`, `AssetExecutor.cpp:57`) | Unique identity on ReFS, where the 64-bit index is not unique **[MS BY_HANDLE_FILE_INFORMATION]** | `GetFileInformationByHandleEx(h, FileIdInfo, &mut FILE_ID_INFO, size)`; on failure, use #8's `dwVolumeSerialNumber` + `nFileIndex*` | sys | No std API. Keep the fallback flag, because C++ compares `fullFileId` as part of identity. |
| 11 | `GetFileInformationByHandleEx(FileAttributeTagInfo)` on destination parent pins (`ArchiveExtraction.cpp:59`) | Reparse check that works on a `FILE_READ_ATTRIBUTES`-only handle | Direct call, or `file.metadata()?.file_attributes()`. Both work with attribute-only access. | sys or std-ext | None |
| 12 | `SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = TRUE/FALSE, RootDirectory = NULL, absolute FileName)` (`TemporaryArtifactRegistry.cpp:326-345` publication `Replace`/`NoReplace`; `NativeFilePins.cpp:117-140` `.bak` backup) | Renames *the opened file object*, not a pathname. `NoReplace` fails on an occupied leaf (`STATUS_OBJECT_NAME_COLLISION`) **[MS FileRenameInformation]**. No copy fallback, so a cross-volume rename fails (`STATUS_NOT_SAME_DEVICE`). | Build a zeroed, 8-byte-aligned buffer of `size_of::<FILE_RENAME_INFO>() + name_bytes`. Set `Anonymous.ReplaceIfExists`, `RootDirectory = null`, and `FileNameLength = name_bytes`, then copy the UTF-16 name. Call `SetFileInformationByHandle(h, FileRenameInfo, buf, len)`. | sys | No public std API. std has this code only as a private `ACCESS_DENIED` fallback inside `fs::rename`, which uses `FileRenameInfoEx` + POSIX semantics (`windows.rs:1321-1390`) **[std src]**. Keep **`FileRenameInfo`**, not the Ex form; the Ex form's POSIX semantics replace targets that have open handles. Keep the absolute name: C++ found `RootDirectory`-relative names rejected (`TemporaryArtifactRegistry.cpp:329`). |
| 13 | `SetFileInformationByHandle(FileDispositionInfo{TRUE})` (`NativeFilePins.cpp:243`, `StagingRecovery.cpp:162`, `AssetExecutor.cpp:121`, `ArchiveFinalizationLoadingPlugins.cpp:131`) | Deletes the verified file object. The name goes away when the last handle closes. | `SetFileInformationByHandle(h, FileDispositionInfo, &FILE_DISPOSITION_INFO { DeleteFile: true }, size)` | sys | **`std::fs::remove_file` deletes by path with `DeleteFileW`** (`windows.rs:1298-1319`) **[std src]**, so it is not identity-bound. std's handle delete (`File::delete`/`posix_delete`) is private. Keep Win32, not POSIX, disposition for parity. |
| 14 | `FlushFileBuffers` (`TemporaryArtifactRegistry.cpp:308` staged bytes; `StagingRecovery.cpp:415` manifest scratch) | Durability before publication | `File::sync_all()`, which is exactly `FlushFileBuffers` (`windows.rs:400-403`) **[std src]** | std | The handle needs `GENERIC_WRITE` **[MS FlushFileBuffers]**. Row 4's handle has it. `sync_data` is the same call. |
| 15 | `MoveFileExW(scratch, manifest, REPLACE_EXISTING \| WRITE_THROUGH)` (`StagingRecovery.cpp:455`) | Same-volume replacement of `ownership.manifest` | `windows_sys::…::MoveFileExW(wide(scratch), wide(manifest), MOVEFILE_REPLACE_EXISTING \| MOVEFILE_WRITE_THROUGH)` | sys | `std::fs::rename` passes only `MOVEFILE_REPLACE_EXISTING` (`windows.rs:1322`) **[std src]**. `WRITE_THROUGH` mainly matters for copy+delete moves **[MS MoveFileExW]**, but keep the call for parity; it costs nothing. |
| 16 | `GetVolumePathNameW` with a growing buffer up to 32 768, plus the "one char short drops the trailing `\`" rule (`NativeVolume.h:13-32`) | Mount point of the containing volume, including mounted folders deeper than `MAX_PATH` | Port `volumeMountPoint` line for line on `windows-sys` | sys | No std API. The trailing-separator quirk is documented **[MS GetVolumePathNameW]**. |
| 17 | `GetVolumeNameForVolumeMountPointW` → `\\?\Volume{GUID}\` (`NativeVolume.h:41-50`); used for the same-volume staging check (`StagingRecovery.cpp:489-497`) and Archive capacity grouping (`ArchiveCapacity.cpp:12`) | Same-volume proof and capacity grouping | Direct call with a buffer of at least 50 wide chars, after #16 | sys | No std API |
| 18 | `CompareStringOrdinal(..., bIgnoreCase = TRUE)` (`ArchivePathComparison.h:24` ordering and equality of game paths; `TemporaryArtifactRegistry.cpp:30` ownership path aliasing; `StagingRecovery.cpp:495` volume GUID equality) | Case-insensitive comparison that uses the OS uppercase table, ordinal (not linguistic) **[MS CompareStringOrdinal]** | `windows_sys::Win32::Globalization::CompareStringOrdinal(a, len_a, b, len_b, 1) - 2` gives `<0`/`0`/`>0`. Wrap it in an `Ord` newtype for `BTreeMap` keys. | sys | No pure-Rust equivalent. Rust's `to_uppercase`/`to_lowercase` use full Unicode mappings that can expand (ß → SS), which C++ explicitly avoids (`ArchivePathComparison.h:61`). Pass `bIgnoreCase = 1`; the docs say other non-zero values are an invalid parameter. |
| 19 | `ReadFile` / `SetFilePointerEx` on pinned handles (`TemporaryArtifactRegistry.cpp:131`, `AssetExecutor.cpp:136-142`, `ArchiveFinalizationLoadingPlugins.cpp:118`) | Fingerprints the bytes of the pinned file object, never a pathname | `Read` and `Seek::seek(SeekFrom::Start(0))` on the same `File` | std | None |
| 20 | `fs::hard_link_count(path) != 1` (`StagingRecovery.cpp:77`) | Rejects hard-linked staging entries | Open (row 2 flags) and read `nNumberOfLinks` (row 8) | sys | std has no stable link count on Windows (row 8) |
| 21 | `std::filesystem::space(root).available` (`ArchiveCapacity.h:30`) | Free space for capacity batching | `GetDiskFreeSpaceExW(wide(root), &mut avail_to_caller, …)` | sys | No std API. Avoid `fs4`; see [Helper crate evaluation](#helper-crate-evaluation). |
| 22 | `std::random_device` nonces (`StagingRecovery.cpp:394`) | Unpredictable run IDs and nonces | `getrandom::fill(&mut [u8; 16])`. It uses `ProcessPrng` on Windows 10+ **[crate src `getrandom-0.3.4/src/backends/windows.rs`]**. | crate | Not a file primitive, but on the same safety path |
| 23 | `fs::canonical` / `weakly_canonical` (`TemporaryArtifactRegistry.cpp:234,454,495`, `StagingRecovery.cpp:481,482,522,709`, `ArchiveExtraction.cpp:77`, `RunExecutor.cpp:97,150`, others) | Canonical Mod Root, used as the manifest root text and the map key; checks that parents are not linked | `msvc_canonical()`; see [The canonical-path trap](#the-canonical-path-trap) | std + sys | **Semantic gap.** See the next section. |

### Two `std::fs::OpenOptions` traps found in std source

1. **`create_new(true)` requires `.write(true)` or `.append(true)`, even when `access_mode` is
   set.** `get_cmode_disposition` decides from the `write` and `append` booleans, not from
   `access_mode`. It returns `InvalidInput` ("creating or truncating a file requires write or
   append access") otherwise (`windows.rs:289-319`) **[std src]**. `get_access_mode` checks
   `access_mode` first, so the bootstrap `owner.lock` open is:

   ```rust
   OpenOptions::new()
       .write(true)                       // satisfies the create_new guard only
       .access_mode(GENERIC_READ)         // the access actually requested (overrides write)
       .share_mode(0)
       .create_new(true)                  // CREATE_NEW; std also adds FILE_FLAG_OPEN_REPARSE_POINT
       .open(staging.join("owner.lock"))
   ```

2. **The default `share_mode` is `READ | WRITE | DELETE`** (`windows.rs:206`) **[std src]**. Any
   pin that forgets `.share_mode(..)` silently allows renames and replacement. Wrap pin
   construction in one helper that takes an explicit share mode.

## The canonical-path trap

C++ writes the Mod Root into the manifest as `pathText(fs::canonical(modRoot))`. `pathText` is
`generic_u8string()`, so every `\` becomes `/` **[STL src `generic_wstring`]**. Recovery rejects
the manifest unless the stored string is **byte-identical** to the freshly computed one
(`StagingRecovery.cpp:288`). The port must read and recover C++-written v1-v3 manifests, so it
must reproduce MSVC's `canonical` text exactly.

MSVC STL 14.51 `_Canonical` (`<filesystem>` lines 3039-3089) **[STL src]**:

1. Opens the path with `FILE_READ_ATTRIBUTES` and `FILE_FLAG_BACKUP_SEMANTICS`. This follows links.
2. Calls `GetFinalPathNameByHandleW` with `FILE_NAME_NORMALIZED | VOLUME_NAME_DOS`, growing the
   buffer as needed.
3. If that fails with `ERROR_PATH_NOT_FOUND`, retries with `VOLUME_NAME_NT`. This is a volume with
   no DOS name.
4. DOS result: `\\?\X:…` → strips `\\?\` **regardless of length**. `\\?\UNC\server\share…` →
   `\\server\share…`. NT result: prefixes `\\?\GLOBALROOT`.

Compare the Rust options:

| | `\\?\C:\…` short | `\\?\C:\…` > 260 chars | `\\?\UNC\…` | No DOS name |
|---|---|---|---|---|
| MSVC `canonical` | `C:\…` | `C:\…` | `\\server\…` | `\\?\GLOBALROOT\Device\…` |
| `std::fs::canonicalize` **[std src `windows.rs:1593-1610`]** | `\\?\C:\…` | `\\?\C:\…` | `\\?\UNC\…` | **error** |
| `dunce::canonicalize` **[crate src]** | `C:\…` (if names are valid) | `\\?\C:\…` (keeps the prefix past 260) | `\\?\UNC\…` (never strips UNC) | **error** |

**Recommendation.** Write `fn msvc_canonical(p: &Path) -> io::Result<PathBuf>` (about 30 lines):

- Open with `OpenOptions` using `access_mode(FILE_READ_ATTRIBUTES)` and
  `custom_flags(FILE_FLAG_BACKUP_SEMANTICS)`.
- Call `GetFinalPathNameByHandleW` (`windows-sys`) with the DOS-then-NT fallback.
- Apply the same two prefix rewrites.

A stripped long `C:\…` result is still safe for every `std::fs` call. std's `maybe_verbatim`
re-adds `\\?\` for paths of 248 or more UTF-16 units before calling Win32
(`sys/path/windows.rs:81-175`) **[std src]**. Raw `windows-sys` calls that take paths, such as
`MoveFileExW` and the rename `FileName`, rely on the long-path manifest instead; see
[Implications for #468](#implications-for-468-workspace-architecture).

Also port `weakly_canonical` on top of `msvc_canonical`. MSVC canonicalizes the longest existing
prefix and appends the lexically normalized rest (`<filesystem>` lines 4109+) **[STL src]**. The
"parent is not linked" checks (`fs::canonical(parent) != parent`) need the same text form to
compare equal.

`pathText` also needs a Rust twin: `path.to_str()` with `\` replaced by `/`. Treat a non-UTF-8
path (unpaired surrogate) as an error, as C++'s UTF-8 conversion would.

## Helper crate evaluation

- **`windows-sys` 0.61.2: adopt.** Raw `extern "system"` bindings through `windows-link`, MSRV
  1.71 **[crate src Cargo.toml]**. Every needed item is present. These live in
  `Win32_Storage_FileSystem`: `CreateFileW`, `GetFileInformationByHandle(Ex)`,
  `SetFileInformationByHandle`, `MoveFileExW`, `GetVolumePathNameW`,
  `GetVolumeNameForVolumeMountPointW`, `GetFinalPathNameByHandleW`, `GetDiskFreeSpaceExW`,
  `FILE_RENAME_INFO` (with the `ReplaceIfExists`/`Flags` union), `FILE_ID_INFO`, `FILE_BASIC_INFO`,
  `FILE_ATTRIBUTE_TAG_INFO`, `FILE_DISPOSITION_INFO`, `DELETE`, and `FILE_LIST_DIRECTORY`.
  `CompareStringOrdinal` and `CSTR_EQUAL` are in `Win32_Globalization`, and `GENERIC_READ` and the
  `ERROR_*` codes are in `Win32_Foundation`. `CreateFileW`'s signature needs `Win32_Security` for
  `SECURITY_ATTRIBUTES`. `HANDLE` is `*mut c_void`, the same type as `RawHandle`, so
  `file.as_raw_handle()` passes straight through. Neither `same-file` nor `fs4` would remove the
  need for this crate; both depend on it anyway.
- **`windows` (0.62): skip for this layer.** It is the COM/WinRT-friendly projection with
  `Result` wrappers. It is heavier and adds nothing for flat Win32 calls. It may still be wanted
  elsewhere, for example by the DirectXTex/GPU work, but that is not this ticket's call.
- **`same-file` 1.0.6: reject.** Identity is the 64-bit `nFileIndex` + volume serial only. The
  source says it does not use `FileIdInfo`. It opens with std's default share mode and *follows*
  reparse points (`winapi-util` `from_path_any` uses only `BACKUP_SEMANTICS`) **[crate src]**.
  CAO needs the 128-bit ID, no-follow opens, and identity on handles it already pins.
- **`fs4` 0.12.0: reject.** Its locks are `LockFileEx`, the wrong primitive for `owner.lock`, and
  std 1.89 has the same lock API anyway. Its `statvfs` calls `GetVolumePathNameW` into a fixed
  **261-unit** buffer (`fs4-0.12.0/src/windows.rs:90-91`) **[crate src]**, so it fails exactly on
  the deep mounted folders that `NativeVolume.h` was written to handle. Call
  `GetDiskFreeSpaceExW` directly.
- **`dunce` 1.0.5: adopt narrowly, for the exe directory and display paths only.** `simplified`
  does no I/O. It strips `\\?\` only for `VerbatimDisk` paths of 260 chars or fewer whose
  components are valid, non-reserved Win32 names **[crate src]**. That is safe for display and
  child-process arguments, but **not** a substitute for `msvc_canonical`.
- **`winapi-util`: not needed.** Its `file::information` is a thin `GetFileInformationByHandle`
  wrapper.
- **`getrandom` 0.3: adopt** for nonces (row 22).
- **`cap-std` / `cap-primitives`: not evaluated in depth; out of scope.** Handle-relative
  ("capability") opens would be a re-architecture of the pinning model. Map #458 says to port it
  simply.

**Recommended `Cargo.toml` fragment** (Windows-only target):

```toml
[dependencies]
windows-sys = { version = "0.61", features = [
    "Win32_Foundation",
    "Win32_Security",            # SECURITY_ATTRIBUTES in CreateFileW's signature
    "Win32_Storage_FileSystem",
    "Win32_Globalization",       # CompareStringOrdinal
] }
getrandom = "0.3"
dunce = "1"
```

## Resolving the exe directory

The port resolves `profiles/`, `logs/`, `bin/hkxcmd.exe`, and `translations/` relative to the exe
(map #458 deviation list).

- `std::env::current_exe()` calls `GetModuleFileNameW(NULL, …)` through `fill_utf16_buf`, which
  grows the buffer on `ERROR_INSUFFICIENT_BUFFER` (`sys/paths/windows.rs:98-100`) **[std src]**.
  Long install paths therefore work. The result "will use the same format that was specified
  when the module was loaded". It may be an 8.3 short name or carry `\\?\` **[MS
  GetModuleFileNameW]**. The std docs say a symlinked launch may return the link path rather than
  the target **[std src `env.rs`]**.
- **Do not canonicalize it.** Canonicalizing would send a symlinked or junctioned install to the
  `profiles/` folder beside the link target instead of the one the user sees beside the exe. That
  would be a second, unannounced deviation from C++ behavior, which resolves `profiles/` from the
  working directory (`src/Profiles.cpp:46`), and it gains nothing.
- **Do apply `dunce::simplified` to the directory.** A `\\?\` path does not treat `/` as a
  separator, and Win32 does not normalize it **[MS Maximum Path Length Limitation]**. A
  `join("profiles/SSE")` on a verbatim base would then name a literal `profiles/SSE` component.
  `simplified` removes the prefix whenever that is lossless.
- **Join one component at a time** (`dir.join("profiles").join(game)`). Never call
  `set_current_dir`. Pass absolute paths to `hkxcmd.exe`.
- Resolve once at startup into an `AppPaths { exe_dir, profiles, logs, hkxcmd, translations }`
  value, and fail loudly if `current_exe()` errors. There is no sensible fallback, and silently
  using the cwd would bring back the old bug.

```rust
/// Directory containing the running CAO executable, without resolving links.
fn exe_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("executable has no parent directory"))?;
    Ok(dunce::simplified(dir).to_path_buf())
}
```

## Implications for #468 (workspace architecture)

- **Embed the same application manifest in the Rust exe.** `src/Cathedral_Assets_Optimizer.manifest`
  declares `longPathAware` **and** `activeCodePage` = UTF-8. `longPathAware` (together with the
  `LongPathsEnabled` registry value, both required **[MS Maximum Path Length Limitation]**)
  covers raw path-taking calls such as `MoveFileExW`. `GetVolumePathNameW` is *not* on Microsoft's
  long-path list, so long-path parity there is best-effort, as it already is in C++. The UTF-8
  ANSI code page matters for any C/C++ engine (nifly, DirectXTex, `hkxcmd` arguments) that takes
  narrow `char*` paths. A build-script crate for the embedding is a #468 decision and is not
  evaluated here.
- **Put all of this in one small crate,** for example `cao-winfs`: `Pin`/`DirPin` constructors,
  `FileFacts` (rows 8-10), `rename_by_handle`, `delete_by_handle`, `msvc_canonical`,
  `volume_guid_path`, `compare_ordinal`, `available_space`. Every `unsafe` and every `windows-sys`
  import lives there. Everything else stays safe Rust on `std::fs`.
- **`ba2` can read from the pin itself.** `ba2` 3.0.1 implements `Reader<&std::fs::File>` and maps
  the file with `memmap2` (`ba2-3.0.1/src/derive.rs:27-41`) **[crate src]**. The extraction
  source can be parsed from the `GENERIC_READ`, share-`READ` pin, with no pathname reopen. That is
  a strict improvement on the C++ pathname reader.
- **Unverified caution:** drop the `ba2` archive and its `Arc<Mmap>` before the disposition or
  rename step on that source, so a live section mapping never meets the delete. This was not
  verified against Windows behavior here. Test it in the extraction slice.
- **Wine:** keep the `FileIdInfo` → 64-bit fallback (row 10). std has Wine fallbacks of its own,
  for example truncation falls back from `FileAllocationInfo` to `FileEndOfFileInfo`
  (`windows.rs:382-392`) **[std src]**. CAO's direct `windows-sys` calls do not get those
  fallbacks for free.

## Remaining risks

- **Not verified on a live system here:** the MSVC-vs-Rust canonical text on a real mounted-folder
  volume with no drive letter (the `GLOBALROOT` path), and on a mapped network drive. Add both to
  the differential corpus as manifest-recovery cases.
- The bootstrap `owner.lock` trap (create_new needs `write(true)`) fails loudly, not silently. A
  test that creates a fresh staging area will catch a regression.
- The `FILE_RENAME_INFO` buffer is the one hand-laid-out struct. Size it as
  `offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + 2`, zeroed and `u64`-aligned. std's
  private implementation does the same (`windows.rs:1340-1366`) **[std src]**.

## Sources

**Local primary sources:**

- C++ (at `8767f86`): `src/Run/NativeFilePins.{h,cpp}`, `src/Run/NativeVolume.h`,
  `src/Run/TemporaryArtifactRegistry.cpp`, `src/Run/StagingRecovery.cpp`,
  `src/AssetExecution/AssetExecutor.cpp`, `src/Run/ArchiveExtraction.cpp`,
  `src/Run/ArchiveFinalizationLoadingPlugins.cpp`, `src/Run/ArchivePathComparison.h`,
  `src/Run/ArchiveCapacity.{h,cpp}`, `src/Cathedral_Assets_Optimizer.manifest`,
  `docs/architecture/staging-ownership.md`.
- Rust std 1.99.0: `library/std/src/sys/fs/windows.rs`, `sys/path/windows.rs`,
  `sys/paths/windows.rs`, `os/windows/fs.rs`, `fs.rs`, `path.rs`, `env.rs`.
- MSVC STL 14.51.36231: `include/filesystem` (`_Canonical`, `weakly_canonical`,
  `generic_wstring`).
- Crates (from the cargo registry cache): `windows-sys-0.61.2`, `same-file-1.0.6`,
  `winapi-util-0.1.11`, `fs4-0.12.0`, `dunce-1.0.5`, `getrandom-0.3.4`, `ba2-3.0.1`.

**Microsoft Learn:**

- [CreateFileW](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-createfilew):
  share modes; attribute access is not affected by share flags.
- [MoveFileExW](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw):
  `MOVEFILE_WRITE_THROUGH` semantics.
- [FILE_RENAME_INFO](https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_rename_info)
  and [MS-FSCC FileRenameInformation](https://learn.microsoft.com/openspecs/windows_protocols/ms-fscc/1d2673a8-8fb9-4868-920a-775ccaa30cf8):
  `ReplaceIfExists`, collision and not-same-device statuses.
- [FILE_RENAME_INFORMATION (ntifs)](https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information):
  `FILE_RENAME_POSIX_SEMANTICS`.
- [FILE_INFO_BY_HANDLE_CLASS](https://learn.microsoft.com/windows/win32/api/minwinbase/ne-minwinbase-file_info_by_handle_class)
  and [FILE_INFORMATION_CLASS](https://learn.microsoft.com/windows-hardware/drivers/ddi/wdm/ne-wdm-_file_information_class):
  `FileIdInfo` needs Windows 8+; the Ex classes need Windows 10 1709+.
- [FILE_ID_INFO](https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_id_info),
  [BY_HANDLE_FILE_INFORMATION](https://learn.microsoft.com/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information),
  and [MS-FSCC 128-bit file ID](https://learn.microsoft.com/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0).
- [FlushFileBuffers](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).
- [GetVolumePathNameW](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-getvolumepathnamew).
- [CompareStringOrdinal](https://learn.microsoft.com/windows/win32/api/stringapiset/nf-stringapiset-comparestringordinal).
- [GetModuleFileNameW](https://learn.microsoft.com/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulefilenamew).
- [Maximum Path Length Limitation](https://learn.microsoft.com/windows/win32/fileio/maximum-file-path-limitation).
