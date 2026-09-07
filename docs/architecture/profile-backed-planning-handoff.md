# Handoff: Deepen Profile-Backed Optimization Planning

Use this handoff when resuming architectural grilling on profiles, user choices, defaults, validation, and the run-scoped optimization policy. The next agent should settle the design tree with the user before changing code.

## Goal

Concentrate profile capabilities, defaults, user choices, and validation in a deep planning module so each optimizer receives complete run-scoped knowledge without querying global profile state.

## Evidence

- `src/OptionsCAO.h:20-85` exposes almost all state as public fields while also handling CLI, INI, GUI mapping, and validation.
- `src/Profiles.h:20-85` exposes profile mutation, GUI mapping, settings objects, file lookup, and more than twenty static access points.
- `Manager` and `MainOptimizer` retain `const OptionsCAO&`, while BSA, mesh, and texture modules also query `Profiles::` directly.
- Texture decisions read profile formats and exclusions; mesh decisions read target versions and TGA policy; archive decisions read game and size rules.
- GUI, CLI, and INI are three distinct input adapters, yet ownership of each setting is distributed between `OptionsCAO`, `Profiles`, and `MainWindow`.
- The build defines no test target.

## Architecture Assessment

**Deletion test**: deleting `OptionsCAO` or `Profiles` would spread configuration facts into parameters, widgets, globals, and optimizers. They concentrate real complexity, but their combined interface nearly matches their implementation and is therefore shallow.

**Dependencies**:

- QSettings and profile files are local-substitutable during plan creation.
- A resolved optimization plan should be in-process and independent of widgets or settings storage.
- GUI, CLI, and INI provide three justified adapters at the input seam.

## Scope Guardrails

- Separate run-scoped optimization knowledge from profile persistence and widgets.
- Make one owner resolve defaults, capabilities, overrides, and validation.
- Remove global profile reads from optimization modules through migration, not an additional pass-through interface.
- Preserve profile fallback behavior until the user deliberately changes it.
- Keep `CONTEXT.md` about domain language, not configuration implementation.

## Grilling Start

Begin with these prerequisite decisions:

1. Is a profile a capability definition, a default policy, a saved user preset, or a combination that should be separated?
2. Which source wins when profile defaults, saved options, GUI choices, and CLI choices disagree?
3. Is the resolved plan immutable for the duration of a run?
4. Are invalid combinations rejected, normalized, or downgraded with warnings?
5. Which settings are truly global across profiles, and which belong to a run?

Use the `grilling` skill for the full decision tree. Use `domain-modeling` immediately when terms such as Profile, Preset, Capability, or Optimization Plan are resolved. If the precedence model is hard to reverse and surprising, evaluate it for an ADR.

## Completion Criterion

Grilling is complete when profile meaning, source precedence, mutability, validation, persistence ownership, adapter responsibilities, optimizer inputs, migration order, and surviving tests are explicit; the user has confirmed the shared understanding; and no configuration ownership rule remains silent.
