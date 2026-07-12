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

**Portable state tree**:
The fork-owned, executable-relative directory tree that holds its mutable profiles, configuration, logs, and other writable runtime state. It is isolated from every legacy CAO installation so the fork and legacy application can run side by side without sharing mutable files.
_Avoid_: Working directory, shared profile tree

**Legacy profile import**:
An explicit, non-destructive transfer of compatible legacy profile data into the fork's portable state tree. Import never uses the legacy profile directory as the fork's live store and never modifies the source installation.
_Avoid_: In-place migration, live legacy reuse

**Built-in profile**:
A fork-supplied, immutable game profile whose definition, rules, and support assets remain canonical. Legacy built-in data cannot replace these assets, although compatible user choices may be imported as mutable overrides.
_Avoid_: Base profile, imported built-in

**Custom profile**:
An editable, user-owned profile created by the fork or imported from a legacy custom profile. It remains separate from the immutable built-in profile it was derived from.
_Avoid_: Modified built-in, mutable base profile

**Deterministic configuration fallback**:
The rule that absent user settings resolve to explicit, documented fork defaults without inheriting state from another profile. Missing or invalid required custom-profile definition fields instead produce field-specific import errors and are never silently coerced to false, zero, or empty values.
_Avoid_: Qt coercion, previous-profile fallback

**Import provenance snapshot**:
An immutable copy of a legacy custom profile plus its import report, retained inside the portable state tree so unrecognized source material is not silently lost. Snapshot content is inactive, is never written back to the legacy installation, and does not create a legacy export or round-trip guarantee.
_Avoid_: Active legacy profile, compatibility export

**Materialized rule set**:
The explicit active rules produced when a custom profile is imported. Each recognized legacy rule file resolves from the custom profile first and the legacy SSE fallback second; the validated result is stored with the imported profile so runtime processing never depends on another profile's files.
_Avoid_: Runtime profile fallback, implicit inherited rules

**Scoped profile rule**:
A validated profile rule that affects only its named asset class. Landscape-texture rules and headpart-mesh rules are independent inputs; neither may be substituted for or interpreted as the other.
_Avoid_: Cross-wired rule file, shared landscape/headpart rule

**Canonical dummy plugin**:
A game-specific blank plugin shipped by the fork as a hash-verified fallback when native dummy-plugin generation cannot be validated. Canonical dummy plugins are fork-owned support assets; similarly named files found in imported profiles remain inactive provenance data.
_Avoid_: Imported dummy plugin, arbitrary profile plugin

**Runtime scratch directory**:
A unique per-run directory in the operating system's temporary storage used only for disposable intermediate files. It is not durable application state, must not be shared between runs, and is removed after success or on a best-effort basis after failure.
_Avoid_: Portable state, shared temp files

**Run log**:
A unique UTF-8 diagnostic record for one processing run, stored in the portable state tree and containing its profile, effective configuration, significant events, and run outcome. Run logs are never shared or appended across runs and are retained under the configured count and aggregate-size limits.
_Avoid_: HTML log, session log, shared log file

**State-tree ownership lock**:
The exclusive startup lease that permits one fork process to read and modify a portable state tree. A separate legacy installation may run concurrently, but a second fork process cannot share the same portable state tree until the lease is released or safely identified as stale.
_Avoid_: Process lock, global single-instance lock

**Active profile**:
The built-in or custom profile whose validated definition and user choices currently govern processing. Legacy import preserves the prior selection only when it resolves to a successfully imported custom profile or a matching fork built-in; otherwise the fork selects its documented default and reports the fallback.
_Avoid_: Profile mode, implicit fallback profile

**Asset path**:
An absolute Windows path selected by the user as processing input or output. Asset paths may be remembered even while unavailable, but are revalidated before each run and are never resolved relative to the executable, current working directory, or portable state tree.
_Avoid_: Resource path, portable path

**Legacy configuration mapping**:
The evidence-backed translation from recognized legacy INI sections, keys, aliases, and encodings into validated fork settings. It preserves supported values, reports conflicts or unsupported data, and never invents an alias meaning that the source or behavioral oracle does not establish.
_Avoid_: Best-effort coercion, guessed migration

**Profile identity**:
The stable reserved ID of a built-in profile or generated ID of a custom profile, stored in a validated versioned manifest. A profile's editable display name and storage directory are not its identity and cannot make an arbitrary directory active.
_Avoid_: Directory-name identity, display-name ID

**Legacy import transaction**:
An explicit, previewed import of selected profiles from a user-chosen legacy tree. Each profile is validated and committed independently with source provenance; repeated imports never synchronize with or silently overwrite an existing custom profile.
_Avoid_: Automatic migration, profile synchronization

**Executable root**:
The canonical absolute directory containing the running fork executable, captured at startup and used to locate bundled resources and the portable state tree. The process current working directory has no effect on application behavior and is never used as a fallback root.
_Avoid_: Working directory, launch directory

**Text compatibility encoding**:
The explicit decoding contract for legacy INIs and rule files: proven Qt 5 encodings and escaping are supported, ordinary text prefers strict UTF-8, and ambiguous invalid input requires a user-selected encoding. Fork-owned text artifacts are always written as UTF-8.
_Avoid_: ANSI fallback, replacement-character decoding

**Configuration migration**:
A version-to-version transformation of fork-owned manifests and settings, applied forward in order and committed transactionally after creating a restorable backup. It is distinct from legacy profile import and never silently downgrades newer state.
_Avoid_: Legacy import, in-place upgrade

**Default profile**:
The built-in SSE profile selected by a fresh installation or when a saved active-profile reference is missing or invalid. Failure to load the bundled SSE definition is an installation error, not a reason to synthesize a profile or select another game.
_Avoid_: Last available profile, implicit game fallback

**Profile definition**:
The validated capabilities, format targets, rules, and support assets that describe how a profile processes assets. A built-in profile definition is immutable; only a custom profile definition may be edited.
_Avoid_: Profile settings, user preferences

**Profile overlay**:
The mutable, per-profile processing choices and remembered asset paths applied over a profile definition. Removing an overlay resets that profile to documented defaults without changing its definition, and overlay values never carry between profiles.
_Avoid_: Profile definition, shared settings

**Processing run**:
One execution of a frozen optimization plan against a selected asset path, with its own scratch directory, log, progress, and run outcome.
_Avoid_: Job, session, optimization task

**Run plan**:
The immutable snapshot of validated policy inputs and resolved processing choices used by one processing run. It does not require assets created or exposed by earlier processing phases to be enumerated in advance.
_Avoid_: Live settings, mutable job configuration

**Runtime work manifest**:
The ordered asset inventory a processing run derives and records phase by phase as extraction and discovery reveal work governed by its run plan.
_Avoid_: Run plan, precomputed asset list
