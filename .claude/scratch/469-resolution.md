## Resolution

**All six tested behaviours stay in the contract.** Most carry a small recorded fix. The four INI and profile hand-edit items from the comments become deviations. Glossary terms landed in 25a8642 (`GLOSSARY.md`): **Quarantine**, **Unsafe Game Path**, **Capacity Check**, **Packing Exclusion**, **Mod Exclusion**, and **Headpart Mesh**. **Mod Root** and **Mod Selection** were amended.

### Tested behaviours

- **Quarantine: contract, unchanged.**
  - Applies in Apply only, to a Texture (DDS/TGA) or Mesh (NIF) whose backend load fails.
  - The file is renamed to `<file>.caobad`, or `.caobad.N` with N counting up from 1. An existing quarantine file is never overwritten.
  - The rename is a Committed Mutation, and the attempt stays an Operation Failure (the run ends Completed With Failures).
  - If the rename fails, the Operation Failure is not safe to continue, and the run ends Failed.
  - Animation load failures and broken Archives are not quarantined. Dry Run never quarantines.
  - Implementation note, not contract: use a no-replace rename in Rust instead of check-then-rename.
- **Free-space preflight (Capacity Check): contract on behaviour; the estimate formulas are internal.**
  - These parts are contract:
    - a shortage at the discovery preflight is a Run Failure, and the whole run ends Failed;
    - a shortage at the extraction recheck is a contained Operation Failure;
    - a shortage at the finalization preflight stops Archive Finalization while the run stays safe to continue (Completed With Failures);
    - space is checked per volume, with the whole batch's total checked when a volume's identity is unknown;
    - unknown free space means proceed;
    - Dry Run never checks.
  - The Rust formulas only need to be conservative. The ported ArchiveFirstAssetDiscovery and ArchiveFinalization scenarios pin the behaviour.
- **Unsafe Archive entry names (Unsafe Game Path): contract, plus one fix.**
  - Every selected Archive's entries are checked during discovery, shadowed entries included. Any hit is a Run Failure before extraction, and publication runs the same check again.
  - Rejected: empty or absolute names, C0 control characters, `: * ? " < > |`, `.`/`..` escapes, components ending in `.` or a space, staging-name components, and device stems (CON, PRN, AUX, NUL, CONIN$, CONOUT$, COM/LPT 1–9 and ¹²³).
  - **Deviation:** trim trailing spaces and dots from the stem before comparing device names (`NUL .txt`).
- **`FilesToNotPack.txt` (Packing Exclusion): contract, plus one fix.**
  - Matching files stay loose and are never deleted as packed sources.
  - The SSE fallback and the "not found" error log stay.
  - **Deviation:** match a rule as a case-insensitive substring of the `/`-separated path **within the Mod Root**, not the absolute path. All 13 shipped rules behave the same.
- **Headparts (Headpart Mesh): contract, plus fixes.**
  - Sources stay the same: `customHeadparts.txt` with the SSE fallback, plus HDPT MODL records from every plugin under the selection, plus `facegen` paths.
  - The list stays **run-wide** across the Mod Selection, because a patch's plugin can name headpart meshes that live in another mod. Recorded as the one exception on Mod Root.
  - With `bMeshesHeadparts=false`, a headpart Mesh that has critical issues is still saved without `OptimizeFor`, as today.
  - **Deviations:**
    - Dry Run applies the `facegen` rule, as Apply does.
    - Paths are matched within the Mod Root, instead of cutting at the first `/meshes/` in the absolute path.
    - The plugin scan skips CAO staging directories. It still scans mods with a Mod Exclusion.
    - The plugin parser can't overflow its record buffer. Compressed records stay unsupported.
- **Separator exclusion (Mod Exclusion): contract, with fixes.**
  - It applies in Several Mods only. One Run Diagnostic is recorded per exclusion, and the Run Outcome is unaffected. `ignoredMods.txt` keeps its whole-name, case-folded match.
  - **Deviation:** a separator is a child name **ending in** `_separator` (MO2's convention, case-sensitive), not any name containing `separator`.
  - **Deviation:** delete the case-insensitive full-path "separator" rule in empty-directory pruning (`FilesystemOperations.cpp:25-27`).

### INI and profile hand edits (all deviations)

- `bsaGame` other than TES5 (3), SSE (4) or FO4 (5) makes the profile unreadable at run setup, with a message naming the value. TES3, TES4 and FNV tables are not ported, and the SSE fallback is dropped.
- A plain scalar where a list is expected (`texturesUnwantedFormats=85`) reads as `[85]`, not `[]`.
- A profile INI line starting with `#` is a comment. Any other non-`;` line with no `=` still makes the profile unreadable.
- An INI without a BOM is decoded as UTF-8 when the whole file is valid UTF-8, and as Latin-1 otherwise.
- The writer is unchanged in every case: ASCII with `\x` escapes, `@Variant` for one-element lists, `@Invalid()` for empty lists. The C++ oracle can therefore still read Rust-written files.

All deviations above are added to the map's fix-don't-copy list. The differential harness must keep these hand-edit inputs out of the generated corpus, or mark them as expected differences.

Facts traced from the C++ tree at `8767f86` (MainOptimizer, ArchiveFirstAssetDiscovery, ArchiveCapacity, ArchiveFinalizationPlanning, MeshesOptimizer, PluginsOperations, RunExecutor, FilesystemOperations).
