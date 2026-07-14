# Baseline 1 to 2 semantic diff

Baseline 2 changes the Tracetide production graph from the static MSVC runtime to the standard dynamic runtime. Native code now uses `/MD`, Rust uses the target's default dynamic CRT, and release documentation makes the supported Microsoft Visual C++ Redistributable an explicit prerequisite.

This amendment removes the Cargo `+crt-static` override. As a result, `skia-bindings` selects its published dynamic-CRT-compatible Windows archive for ordinary and hosted validation. Authenticated production source builds remain supported and build the same dynamic CRT configuration.

No behavioral-oracle expectation, parity classification, fixture case, evidence observation, source revision, dependency feature, or Windows support-floor value changed. Governed records advance only their schema and baseline metadata; setup fixture and evidence revisions remain unchanged because their expectations and observations are byte-for-byte semantically identical.
