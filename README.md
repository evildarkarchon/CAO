# Cathedral Assets Optimizer

Cathedral Assets Optimizer is a tool aiming to automate asset conversion and optimization for several Bethesda games, such as Skyrim and Skyrim Special Edition.

# Documentation

Documentation is incomplete. It is available [here](https://g_ka.gitlab.io/sse-assets-optimiser/).

# Build instructions

CAO is written in Rust with a Slint GUI ([ADR 0003](docs/adr/0003-port-cao-to-rust-and-slint.md)); the C++ implementation it was ported from has been deleted. The Cargo workspace lives at the repository root, with its crates under `crates/`:

```
cargo build
cargo test
cargo clippy --all-targets
cargo fmt --check
```

CI runs the same four commands on every pull request ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)). It turns `LongPathsEnabled` off, because CAO must handle Mod Roots deeper than `MAX_PATH` on a default Windows install, and it fails if any test is reported as ignored.

- **Rust:** `rust-toolchain.toml` pins Rust 1.99.0 with rustfmt and clippy; rustup installs it on first use.
- **MSVC and Windows SDK:** Visual Studio 2026 (v145) Build Tools and Windows SDK 10.0.26100 are the documented minimum. They are not enforced; the build uses whichever Visual Studio installation it finds, or the environment of a Developer Command Prompt.
- **CRT:** the workspace links the dynamic CRT, so running CAO needs the Visual C++ redistributable.
- **Long paths:** CAO reaches paths longer than `MAX_PATH` whatever the host's `LongPathsEnabled` setting: every raw Win32 path call converts a long path to its `\\?\` form, as std does. `hkxcmd.exe` is a separate program, so an Animation deeper than `MAX_PATH` converts only if `hkxcmd` itself handles long paths; otherwise that Animation fails on its own.
- **DirectXTex:** the root `Cargo.toml` patches `directxtex` to CAO's fork, [evildarkarchon/directxtex-rs](https://github.com/evildarkarchon/directxtex-rs) (`cao` branch), pinned by commit. The fork builds DirectXTex `may2026`, and `ba2` links the same copy. Its `FORK.md` records the upstream base and delta. Cargo fetches the fork and its submodules with git on first build.

# Running and packaging

CAO's app directory is the directory of its exe: it reads `profiles/` and runs `bin/hkxcmd.exe` from there. `cargo xtask` (see `cargo xtask help`) puts them in place:

```
cargo xtask dev                 # copy profiles/ beside target/debug/cao-gui.exe
cargo run -p cao-gui

cargo xtask dist --release      # build and lay out target/dist/release/
```

- `dev` keeps profile files already beside the exe, since the app edits them; `--force` replaces them with the tracked ones.
- `dist` lays out `cao-gui.exe`, `profiles/`, `bin/hkxcmd.exe` and `LICENSE`. It replaces its default folder under `target/dist/`; a folder given with `--out` must be absent or empty.
- **hkxcmd:** `hkxcmd.exe` converts Animations and is not in this repository. Download it from [figment/hkxcmd's releases](https://github.com/figment/hkxcmd/releases) and put it at `bin/hkxcmd.exe` in the repository root (which `.gitignore` keeps untracked), or pass `--hkxcmd <path>`. `dist` refuses to lay out an install without it unless given `--without-hkxcmd`; such an install cannot convert Animations.

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
