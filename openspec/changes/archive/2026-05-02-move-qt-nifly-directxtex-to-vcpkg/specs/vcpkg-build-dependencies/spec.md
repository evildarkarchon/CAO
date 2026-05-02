# vcpkg-build-dependencies Specification

## Purpose

Keep CAO's build dependency ownership explicit in the vcpkg manifest and local overlay ports so Windows configure/build no longer depends on hard-coded developer machine paths.

## Requirements

### Requirement: Direct dependencies are declared in the manifest

The root `vcpkg.json` SHALL declare the libraries and host/tool packages directly used by the application build.

#### Scenario: Qt build integration is resolved from vcpkg

- **GIVEN** a Windows configure using the vcpkg toolchain
- **WHEN** CMake resolves Qt Core, Widgets, and Linguist tooling
- **THEN** the root manifest includes `qt5-base` and `qt5-tools`

#### Scenario: Source-level library usage remains direct

- **GIVEN** source files include or link DirectXTex and nifly APIs
- **WHEN** dependency ownership is reviewed
- **THEN** `directxtex` and `nifly` remain direct root manifest dependencies

### Requirement: Overlay ports are self-contained

The local overlay ports SHALL build from their pinned source definitions without requiring developer-specific local source paths.

#### Scenario: bethutil configures from the downloaded source

- **GIVEN** vcpkg fetches the pinned bethutil source
- **WHEN** the bethutil overlay port configures
- **THEN** the port uses the fetched `SOURCE_PATH` rather than a hard-coded local checkout

#### Scenario: bethutil dependencies are available through overlay ports

- **GIVEN** bethutil depends on libflow
- **WHEN** vcpkg resolves overlay ports
- **THEN** a local libflow overlay port provides its headers and package metadata

### Requirement: CMake uses package targets from vcpkg

The application CMake files SHALL rely on `find_package` and imported targets for vcpkg-provided dependencies, while preserving existing translation generation and target structure.

#### Scenario: Qt package hints are not machine-specific

- **GIVEN** a Visual Studio CMakeSettings configure
- **WHEN** Qt package discovery runs
- **THEN** `Qt5Core_DIR`, `Qt5Widgets_DIR`, and `Qt5Gui_DIR` are not hard-coded to a developer-local Qt installation

#### Scenario: DirectXTex memory loading accepts untyped archive data

- **GIVEN** archive callers pass texture memory as `const void *`
- **WHEN** TGA or DDS memory loaders are invoked
- **THEN** the pointer is adapted to DirectXTex's byte-buffer API before calling the loader
