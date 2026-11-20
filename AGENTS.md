## Platform scope

CAO targets Windows only. The Rust workspace carries no POSIX code paths; validate changes against Windows runtime behavior.

## Build workflow

- CAO is a Cargo workspace at the repository root, with crates under `crates/`; `rust-toolchain.toml` pins the toolchain. The C++/CMake/vcpkg tree has been deleted.
- Validate changes with `cargo build`, `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check`.
- If a build is blocked, diagnose and report the blocker rather than silently switching to another workflow.
- CI (`.github/workflows/ci.yml`) runs those four commands on every pull request with `LongPathsEnabled` off, and fails if any test is ignored: make a host-dependent test runnable on CI rather than skipped.
- A path handed to a raw Win32 call goes through `cao_winfs::extended_length_path` first, as std does internally, so deep Mod Roots work whatever the host's `LongPathsEnabled` setting.
- `cargo xtask dev` puts `profiles/` beside `target/debug/cao-gui.exe`, which `cargo run -p cao-gui` needs to start; `cargo xtask dist` lays out an install folder.

## C++ parity

C++ parity is no longer a priority. Design each change around what CAO needs now, and change behavior wherever that serves CAO better. Comments, tests, specs, and docs that cite the deleted C++ implementation record history, not requirements: ignore them as constraints, and when a change touches code justified only by C++ parity, remove or rewrite that justification.

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues for `evildarkarchon/CAO`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five canonical default label names. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses the single-context layout. See `docs/agents/domain.md`.
