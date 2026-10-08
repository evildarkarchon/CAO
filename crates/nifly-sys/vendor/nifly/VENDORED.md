# Vendored nifly

This directory is [ousnius/nifly](https://github.com/ousnius/nifly) at commit
`5504832da8009248ff68a9d306973ee1ba61a0a4`, the same `REF` the C++ build's vcpkg overlay port
pins (`cmake/ports/nifly/portfile.cmake`). nifly is GPL-3.0; its licence is `LICENSE` here.

**Never bump the pin during the port (#476).** Upstream `main` is far ahead of it, and a bump
would change Mesh output relative to the C++ parity oracle.

## What was copied

- `src/*.cpp` (the 13 sources nifly's own `src/CMakeLists.txt` builds), `include/*.hpp`,
  `external/*.hpp` (`half.hpp`, `Miniball.hpp`) and `LICENSE`, unchanged.
- `tests/*.nif`, nifly's own fixtures, for the byte-identity tests in `../../tests/`.
  `TestNifFile.cpp` and the CMake files were left out.

## The one local change

`fix-const-matrix-equality.patch` is applied to `include/Object3d.hpp`. It is a copy of the
C++ build's `cmake/ports/nifly/fix-const-matrix-equality.patch`, which backports upstream
[`3fbe3a0`](https://github.com/ousnius/nifly/commit/3fbe3a0bfc5de0ff86bfc6190c3c4ee6ac189cf7).
It makes `Matrix3::operator==` and `Matrix4::operator==` `const`.

`build.rs` compiles nifly as C++17, which does not need the patch; C++20 does (C2666 at
`Object3d.hpp` 506 and 708). It is applied anyway so this tree is identical to what the oracle's
vcpkg port builds (#460).

## Reproducing this tree

```sh
git clone https://github.com/ousnius/nifly.git && git -C nifly checkout 5504832da8009248ff68a9d306973ee1ba61a0a4
# copy src/*.cpp, include/*.hpp, external/*.hpp, LICENSE and tests/*.nif here, then:
git apply --directory=crates/nifly-sys/vendor/nifly crates/nifly-sys/vendor/nifly/fix-const-matrix-equality.patch
```
