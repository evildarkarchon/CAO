# Cathedral Assets Optimizer

Cathedral Assets Optimizer is a tool aiming to automate asset conversion and optimization for several Bethesda games, such as Skyrim and Skyrim Special Edition.

# Documentation

Documentation is incomplete. It is available [here](https://g_ka.gitlab.io/sse-assets-optimiser/).

# Build instructions

See [the wiki](https://gitlab.com/G_ka/sse-assets-optimiser/wikis/Build-instructions).

## Rust workspace

CAO is being ported to Rust and Slint ([ADR 0003](docs/adr/0003-port-cao-to-rust-and-slint.md)). The Cargo workspace lives at the repository root, with its crates under `crates/`, and builds independently of the CMake tree:

```
cargo build
cargo test
cargo clippy --all-targets
```

- **Rust:** `rust-toolchain.toml` pins Rust 1.99.0 with rustfmt and clippy; rustup installs it on first use.
- **MSVC and Windows SDK:** Visual Studio 2026 (v145) Build Tools and Windows SDK 10.0.26100 are the documented minimum. They are not enforced; the build uses whichever Visual Studio installation it finds, or the environment of a Developer Command Prompt. This is the same install the C++ build needs.
- **CRT:** the workspace links the dynamic CRT, as the C++ build does, so running CAO still needs the Visual C++ redistributable.
- **DirectXTex:** the root `Cargo.toml` patches `directxtex` to CAO's fork, [evildarkarchon/directxtex-rs](https://github.com/evildarkarchon/directxtex-rs) (`cao` branch), pinned by commit. The fork builds DirectXTex `may2026`, the same release the parity oracle builds, and `ba2` links the same copy. Its `FORK.md` records the upstream base and delta. Cargo fetches the fork and its submodules with git on first build.

# Features and use instructions

See [the NexusMods page](https://www.nexusmods.com/skyrimspecialedition/mods/23316).

# Credits

Zilav, for his assistance and [BSArch](https://github.com/TES5Edit/TES5Edit/tree/dev/Tools/BSArchive)
Ousnius, for the [NIF Library](https://github.com/ousnius/BodySlide-and-Outfit-Studio/tree/dev/lib/NIF)
Microsoft, for [DirectXTex](https://github.com/Microsoft/DirectXTex)
Figment, for [hkxcmd](https://github.com/figment/hkxcmd)
Deorder, for his assistance and [Libbsarch](https://github.com/deorder/libbsarch)
Francesc M., for [QLogger](https://github.com/francescmm/QLogger)
Feles Noctis, Hishy, Alsa, Aerisarn, and many others, for tests and advice
