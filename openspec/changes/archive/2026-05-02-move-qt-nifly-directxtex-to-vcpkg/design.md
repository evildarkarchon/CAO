# Design

## Dependency Ownership

The root manifest owns libraries the application source directly includes or links. Qt is represented by `qt5-base` for Core/Widgets and `qt5-tools` for Linguist tooling. DirectXTex and nifly remain direct dependencies because CAO code calls DirectXTex APIs and uses nifly types/targets directly.

Bethutil remains an overlay dependency with its existing feature metadata. The application's DirectXTex usage is independent from bethutil's optional texture feature, so the root manifest should not rely on `bethutil[tex]` for that direct usage.

## CMake Integration

`src/CMakeLists.txt` remains focused on package discovery, Qt translation generation, and imported target links. The migration avoids changing target structure or runtime behavior.

## Overlay Ports

Overlay ports should be self-contained and reproducible from pinned upstream sources. The bethutil port should use the `SOURCE_PATH` produced by `vcpkg_from_github`. A minimal libflow overlay supplies bethutil's header-only dependency. The nifly overlay carries a small MSVC/C++20 compatibility patch when needed by the vcpkg-driven build.

## Validation

Windows validation uses `cmake --preset ninja-windows` followed by `cmake --build build`, because this repository defines configure presets but no build presets. Linux configure is documented as unavailable on this Windows host when the required Linux toolchain is not present.
