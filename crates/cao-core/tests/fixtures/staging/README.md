# `CAO-STAGING` manifest fixtures

`tests/staging_recovery.rs` recovers each of these. A manifest records its Mod Root as an
absolute path, so the test replaces only the second line with the scratch Mod Root's
canonical generic text. Every other byte is used as is. Git must never convert their line
endings (see `.gitattributes`).

- `v1.manifest` and `v2.manifest` are hand-written to the documented grammar
  (`docs/architecture/staging-ownership.md`).

v3 manifests need no fixture: the Rust writer produces them, and `tests/durable_staging.rs`
recovers what it writes.
