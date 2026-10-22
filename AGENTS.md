## Platform scope

CAO targets Windows only. The Rust workspace carries no POSIX code paths; validate changes against Windows runtime behavior.

## Build workflow

- CAO is a Cargo workspace at the repository root, with crates under `crates/`; `rust-toolchain.toml` pins the toolchain. The C++/CMake/vcpkg tree has been deleted.
- Validate changes with `cargo build`, `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check`.
- If a build is blocked, diagnose and report the blocker rather than silently switching to another workflow.

## C++ parity

C++ parity is no longer a priority. Design each change around what CAO needs now, and change behavior wherever that serves CAO better. Comments, tests, specs, and docs that cite the deleted C++ implementation record history, not requirements: ignore them as constraints, and when a change touches code justified only by C++ parity, remove or rewrite that justification.

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues for `evildarkarchon/CAO`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five canonical default label names. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses the single-context layout. See `docs/agents/domain.md`.
