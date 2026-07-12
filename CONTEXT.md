# Cathedral Assets Optimizer

This context defines the language used to describe compatibility between the legacy Cathedral Assets Optimizer and its planned Rust/Slint replacement.

## Language

**Behavioral oracle**:
The SHA-256-pinned v5.3.15 Windows distribution whose observed behavior is the authoritative legacy reference, with the v5.3.14 source commit used only as an explanatory companion.
_Avoid_: Golden source, reference build

**Functional parity**:
Agreement with the behavioral oracle across the observable behaviors included in the initial-release contract, except for explicitly approved discrepancies.
_Avoid_: Pixel parity, source parity, general compatibility

**Interaction parity**:
Agreement in available user decisions, validation gates, confirmation points, terminal outcomes, diagnostic meaning, and meaningful progress phases. It does not require legacy wording, layout, event counts, timing, or presentation.
_Avoid_: UI parity, pixel parity

**Tiered output equivalence**:
Byte identity for operations proven repeatably deterministic; otherwise normalized filesystem agreement plus format-aware structural or decoded invariants, supplemented by manual game or visual validation only where automation is insufficient.
_Avoid_: Approximate output, semantic-only comparison

**Safe cancellation boundary**:
A point at which processing can stop without leaving the operation currently in flight structurally invalid. Cancellation preserves valid completed outputs, starts no new work, and does not imply whole-job rollback.
_Avoid_: Immediate cancellation, transactional rollback

**Run outcome**:
One of four terminal classifications: Succeeded, Succeeded with errors, Failed, or Cancelled. Recoverable item failures may continue to a Succeeded-with-errors outcome; setup failures or loss of job integrity produce Failed.
_Avoid_: Done status, exit state

**Discrepancy**:
Any observable difference between the replacement and the behavioral oracle, regardless of whether the difference is intentional, beneficial, or harmful.
_Avoid_: Bug fix, harmless difference

**Discrepancy register**:
The authoritative record of approved discrepancies, including each difference's rationale, compatibility impact, expected replacement behavior, and regression evidence. An unregistered discrepancy blocks functional parity.
_Avoid_: Known-issues list, exception list
