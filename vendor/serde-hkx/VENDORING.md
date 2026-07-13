# `serde-hkx` vendoring record

- Upstream: `SARDONYX-sard/serde-hkx`
- Version: `1.0.1`
- Commit: `6c1bee56d42de7def991cf6fba025a9df7492d83`
- Source archive SHA-256: `34f5cb574f8bc5a67354aa3d5c184abfa79e41007cd7955786d47ed950dcc430`
- Selected license: Apache-2.0

Only the low-level binary serializer/deserializer, generated class map, and their path-dependency closure are vendored. CLI, broad format wrappers, generators, and FFI packages are excluded so Tokio and development-only capabilities cannot enter the HKX helper graph.
