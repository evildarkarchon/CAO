## Read-only areas

- Treat the entire `vendor/` directory as read-only. Do not create, edit, delete, move, format, regenerate, or otherwise modify files under `vendor/` unless the user specifically requests a vendored dependency update or another change within that directory.
- Treat the legacy, non-fork implementation as read-only unless the user specifically requests changes to it. This includes all C and C++ source and header files, CMake files, and supporting native-build or integration files such as the legacy `src/` tree, `cmake/`, `CMakeLists.txt`, `CMakePresets.json`, and `vcpkg.json`.
- Reading, searching, compiling, and analyzing these areas is allowed when useful, provided those actions do not modify their contents.
- If it is unclear whether a file belongs to the fork or to the protected legacy/native implementation, ask the user before modifying it.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues; external pull requests are not a triage surface. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the canonical `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix` labels. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. See `docs/agents/domain.md`.

## Game Locations On My Current System
Classic Skyrim: C:\Games\Steam\steamapps\common\Skyrim
Skyrim Special Edition: E:\SteamLibrary\steamapps\common\Skyrim Special Edition
Fallout 4: E:\SteamLibrary\steamapps\common\Fallout 4