## Platform scope

CAO targets Windows only. The Rust workspace carries no POSIX code paths; validate changes against Windows runtime behavior.

## Build workflow

- CAO is a Cargo workspace at the repository root, with crates under `crates/`; `rust-toolchain.toml` pins the toolchain. The C++/CMake/vcpkg tree has been deleted.
- Validate changes with `cargo build`, `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check`.
- If a build is blocked, diagnose and report the blocker rather than silently switching to another workflow.

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues for `evildarkarchon/CAO`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five canonical default label names. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses the single-context layout. See `docs/agents/domain.md`.
