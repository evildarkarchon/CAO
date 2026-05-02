# Tasks

- [x] Verify the local vcpkg registry provides `qt5-base` and `qt5-tools` from the active registry.
- [x] Add `qt5-base` to the root manifest for Qt Core and Widgets.
- [x] Add `qt5-tools` to the root manifest for Qt Linguist tools.
- [x] Keep `directxtex` as a direct root manifest dependency.
- [x] Keep `nifly` as a direct root manifest dependency.
- [x] Leave bethutil overlay feature metadata unchanged.
- [x] Remove the hard-coded `Qt5Core_DIR` variable from `src/CMakeSettings.json`.
- [x] Remove the hard-coded `Qt5Widgets_DIR` variable from `src/CMakeSettings.json`.
- [x] Remove the hard-coded `Qt5Gui_DIR` variable from `src/CMakeSettings.json`.
- [x] Ensure overlay ports configure from pinned sources instead of developer-local paths.
- [x] Run `cmake --preset ninja-windows` and document the result.
- [x] Build the generated `build` directory with `cmake --build build`.
- [x] Run final OpenSpec and diff validation checks.

## Validation Notes

- `ninja-linux` was not run on this Windows workspace because the Linux compiler/toolchain for the `x64-linux` triplet is not available here.
