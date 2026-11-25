# Cathedral Assets Optimizer

Cathedral Assets Optimizer is a tool aiming to automate asset conversion and optimization for several Bethesda games, such as Skyrim and Skyrim Special Edition.

# Documentation

This fork has no documentation of its own yet. The original application's documentation, written for its C++ releases, is at [g_ka.gitlab.io/sse-assets-optimiser](https://g_ka.gitlab.io/sse-assets-optimiser/); the options it describes are largely the same, but details may differ.

# Build instructions

CAO is written in Rust with a Slint GUI ([ADR 0003](docs/adr/0003-port-cao-to-rust-and-slint.md)); the C++ implementation it was ported from has been deleted. The Cargo workspace lives at the repository root, with its crates under `crates/`:

```
cargo build
cargo test
cargo clippy --all-targets
cargo fmt --check
```

CI runs the same four commands on every pull request ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)), and checks the dependency graph with [`cargo deny check`](https://github.com/EmbarkStudios/cargo-deny) against [`deny.toml`](deny.toml): advisories, licences, duplicate versions and sources. Run it after changing a dependency (`cargo install --locked cargo-deny`). It turns `LongPathsEnabled` off, because CAO must handle Mod Roots deeper than `MAX_PATH` on a default Windows install, and it fails if any test is reported as ignored.

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
- `dist` lays out `cao-gui.exe`, `profiles/`, `bin/hkxcmd.exe`, `LICENSE` and `LICENSE-nifly` (nifly's GPL-3.0 text; see [License](#license)). It replaces its default folder under `target/dist/`; a folder given with `--out` must be absent or empty.
- **hkxcmd:** `hkxcmd.exe` converts Animations and is not in this repository. Download it from [figment/hkxcmd's releases](https://github.com/figment/hkxcmd/releases) and put it at `bin/hkxcmd.exe` in the repository root (which `.gitignore` keeps untracked), or pass `--hkxcmd <path>`. `dist` refuses to lay out an install without it unless given `--without-hkxcmd`; such an install cannot convert Animations.

# Features and use instructions

See [the NexusMods page](https://www.nexusmods.com/skyrimspecialedition/mods/23316).

# Credits

- G'k, for the original Cathedral Assets Optimizer, which this fork ports to Rust
- Ryan McKenzie, for [ba2](https://github.com/Ryan-rsm-McKenzie/bsa-rs), which reads and writes CAO's Archives, and [directxtex-rs](https://github.com/Ryan-rsm-McKenzie/directxtex-rs), the base of CAO's fork
- Microsoft, for [DirectXTex](https://github.com/Microsoft/DirectXTex)
- Ousnius, for [nifly](https://github.com/ousnius/nifly), which reads and writes Meshes
- Figment, for [hkxcmd](https://github.com/figment/hkxcmd), which converts Animations
- The [Slint](https://slint.dev) team, for the toolkit the window is built with
- Zilav, Deorder, Feles Noctis, Hishy, Alsa, Aerisarn, and many others, for their assistance, tests and advice

# License

CAO's own code is under the Mozilla Public License 2.0 ([`LICENSE`](LICENSE)). The exception is [`crates/nifly-sys/vendor/nifly`](crates/nifly-sys/vendor/nifly), nifly's own source, which is under the GNU General Public License 3.0 only (its `LICENSE` is beside it).

`cao-gui.exe` compiles nifly in statically, so the binary combines MPL-2.0 and GPL-3.0-only code. The MPL-2.0 permits that combination (its section 3.3), and the combined program is distributed under the GPL-3.0's terms: whoever passes the exe on must offer its complete source under the GPL-3.0. CAO's own files stay available under the MPL-2.0 on their own. [`deny.toml`](deny.toml) allows GPL code in `nifly-sys` alone, and records the licences of the other dependencies.

Slint is used under its Royalty-free License 2.0, whose attribution Help > About Slint provides; Slint is also available under the GPL-3.0.
