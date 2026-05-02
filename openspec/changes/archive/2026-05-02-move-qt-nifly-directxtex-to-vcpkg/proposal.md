# Move Qt, nifly, and DirectXTex to vcpkg

## Summary

Move build dependency ownership for Qt, nifly, and DirectXTex into the vcpkg manifest and overlay ports, keeping the migration scoped to dependency discovery and package ownership.

## Motivation

The Windows build currently relies on developer-local Qt package paths and an overlay bethutil port that can point at a local checkout. That makes a fresh configure depend on machine-specific state instead of the vcpkg manifest and local overlay metadata.

## Scope

- Add explicit Qt 5 manifest dependencies for Core/Widgets and Linguist tools.
- Keep `nifly` and `directxtex` as direct dependencies because source code uses both directly.
- Keep bethutil feature metadata unchanged unless configure proves a target or feature mismatch.
- Remove only machine-specific Qt package hints from `src/CMakeSettings.json`.
- Preserve the existing CMake layout, translation generation, and imported target links.

## Non-Goals

- No Qt 6 upgrade.
- No runtime refactor.
- No broad CMake restructuring.
