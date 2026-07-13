# `ba2` vendoring record

- Package: `ba2` 3.0.1
- Crate archive SHA-256: `7fdb05c5c954898b463887df1145016492deee06e9a778f8af491c7cde14c210`
- Local patch: `verification/baseline/patches/ba2-3.0.1-explicit-zlib-feature.patch`
- Patch SHA-256: `2e9d3e348ba64f2066c8eae47d662a3d8df7015daa8198cb39d2f279447513d1`

The patch disables the implicit default and exposes zlib as an explicit feature so the production feature graph can be audited without relying on defaults.

