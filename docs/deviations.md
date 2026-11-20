# Deviation list

Comments and tests across the workspace cite "deviation N". This file defines those numbers.

The list was drawn up for the port of CAO from C++ and Qt to Rust and Slint ([#458](https://github.com/evildarkarchon/CAO/issues/458), [ADR 0003](adr/0003-port-cao-to-rust-and-slint.md)). The port was meant to produce output equivalent to the C++ application's, so each place where it fixed clearly unintended C++ behaviour instead of copying it was recorded here and pinned by a Rust test. Entries 1 to 24 come from the port's specification, which drew them from the research tickets #467, #469, #470 and #473; entry 25 was added during the port, in commit `2f31da5`.

**This is a closed historical record.** The C++ implementation and the parity harness that compared against it have been deleted, and C++ parity no longer constrains CAO (see `AGENTS.md`). Do not add entries or renumber them: a behaviour change today is designed around what CAO needs and explained where it lives. When a comment that leans on a deviation number is next touched, restate the reason in the comment itself so that it stands without this lookup.

1. `profiles/`, `logs/`, `bin/hkxcmd.exe` and `translations/` resolve relative to the exe, not the working directory. A one-time effect: users who launched the C++ application from another start-in directory find their profiles and logs there, not beside the exe.
2. Cancel in the unwanted-formats dialog reverts the edits made in it.
3. The Dry Run and Several Mods UI rules also apply when settings are loaded from the INI, not only on click.
4. The translation loader bug. This is moot while only English ships.
5. No fixed-size path buffer overflow in the texture loader (C++ used a `wchar_t[1024]`).
6. The texture metadata check really compares. In C++ it used `||` and was a no-op.
7. FO4 DX10 cubemaps in BC7, BC6H and sRGB formats extract successfully.
8. An Archive over 4 GiB, reachable only through a hand-edited INI, errors instead of being written corrupt.
9. All-digit plugin stems (`2.esp`) are handled; bethutil's name parsing had undefined behaviour on them.
10. Dead data files are left alone (`customLandscape.txt`, the profile `DummyPlugin.esp`, `animationFormat`, `bBsaLeastBSA`).
11. A `bsaGame` outside 3, 4 and 5 (TES5, SSE, FO4) makes the profile unreadable, naming the value. There is no SSE fallback and there are no TES3, TES4 or FNV tables.
12. A plain scalar where an INI list is expected (`texturesUnwantedFormats=85`) reads as a one-element list, not an empty one.
13. A profile INI line starting with `#` is a comment. Any other non-`;` line with no `=` still makes the profile unreadable.
14. A profile INI without a BOM decodes as UTF-8 when it is valid UTF-8, and as Latin-1 otherwise. The writer is unchanged.
15. Device-name detection for Unsafe Game Paths trims trailing spaces and dots from the stem (`NUL .txt`).
16. Packing Exclusions (`FilesToNotPack.txt`) match the path within the Mod Root, not the absolute path.
17. Headpart Meshes:
    - Dry Run applies the `facegen` rule.
    - Paths are matched within the Mod Root.
    - The plugin scan skips CAO staging directories.
    - The HDPT parser cannot overflow or hang. A truncated HDPT group or record makes the plugin unreadable, and the run carries on.
    - Compressed HDPT records are decompressed, not misread.
18. The resize target width and height are validated only when resizing by size is enabled. C++ raised a Start Error for an odd value even when that resize mode was off.
19. A Several Mods child named `.cao-staging…` is never a Mod Root and is skipped silently.
20. A separator Mod Exclusion is a child name ending in `_separator` (case-sensitive), not any name containing `separator`. The full-path "separator" rule in empty-directory pruning is removed.
21. FO4 never merges textures into the `GNRL` Main BA2: it always writes a separate DDS-format Textures BA2, and "create texture archive" is checked and disabled under FO4, including on INI load. C++ packed unchunked, possibly uncompressed `.dds` files into `GNRL` when the box was unchecked, which can make the game unstable.
22. Application Log format: the debug layout shows `{module::path@line}` instead of `{Func@line}`, `fatal` is written as `ERROR` (both forced by the `log` crate), message text is HTML-escaped, and each record ends with `\n`.
23. Toggling debug logging between runs no longer drops the log. In C++, two log appenders each opened the file without write sharing, so the newly selected one could not open it.
24. The Log tab is fed by the Application Log sink. It is not truncated to the current rotation chunk, and it shows startup records written before the first run.
25. Replacing a file with `cao_winfs::move_file_write_through`, which is how staging replaces `ownership.manifest`, retries a move refused with Access denied or a sharing violation, with backoff for about a second, before it returns that error. A real-time scanner such as Defender briefly holds a file a run has just written, and C++ moved once, so the scan could fail the run.
