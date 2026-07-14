# Authenticated built-in profile inputs

`built-ins.state` is a fork-owned, UTF-8 inventory derived from the authenticated
v5.3.15 behavioral-oracle archive pinned in
`verification/baseline/implementation-inputs.json`. It records the exact archive
identity and every FO4, SSE, and TES5 profile/support artifact identity; it does not
copy restricted oracle bytes into the release.

The Rust domain exposes reviewed typed definitions derived from these identities.
Startup authenticates this inventory before accepting mutable portable state. The
separate custom-profile compatibility work owns Qt decoding, materialized rule sets,
and canonical dummy generation; imported or shipped legacy dummy files are never made
active by this inventory.
