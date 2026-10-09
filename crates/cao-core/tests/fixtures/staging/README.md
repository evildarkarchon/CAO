# `CAO-STAGING` manifest fixtures

`tests/staging_recovery.rs` recovers each of these. A manifest records its Mod Root as an
absolute path, so the test replaces only the second line with the scratch Mod Root's
canonical generic text. Every other byte is used as is. Git must never convert their line
endings (see `.gitattributes`).

- `v1.manifest` and `v2.manifest` are hand-written to the documented grammar
  (`docs/architecture/staging-ownership.md`). The C++ suites had no fixture for either
  version.
- `oracle-v3-completed.manifest` was written by the C++ oracle
  (`docs/parity-oracle.md`) in a TES5 Apply over two loose Textures that ran to
  completion. Every C++ Apply run that staged something leaves this: the control files
  and a manifest that still records the run child Safety Cleanup removed.
- `oracle-v3-killed.manifest` was written by the C++ oracle in a TES5 Apply over 120
  1024x1024 Textures that the harness killed mid-run (`cao-parity case --timeout 4`). It
  still owns the run child and one complete staged Texture sibling,
  `textures/set2/.cao-staging-texture-…dds`, which had not been published yet.
