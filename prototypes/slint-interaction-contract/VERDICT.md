# Prototype verdict

## Question

Which Slint interaction structure best preserves the agreed interaction contract while improving disclosure, validation, progress, diagnostics, and cancellation?

## Maintainer decision

**Selected: B — Workbench.** The Guided Flow and Run Desk are rejected as the primary application structure. Their prototype code remains temporarily available only as comparison material and must not be treated as production code.

The selection fixes the interaction contract, not the visual styling or final component hierarchy:

- A persistent left navigation exposes Overview, Archives, Textures, Meshes, Animations, Profile Definition, and Run History as stable destinations.
- Profile actions and target selection remain in persistent workspace context rather than becoming transient wizard steps.
- The center workspace edits one operation category at a time with ordinary processing choices separated from custom-profile definition controls.
- A persistent right inspector shows the effective plan, validation gates, run readiness, progress, cancellation state, terminal outcome, and live diagnostic summary.
- Unsupported profile capabilities stay visible but disabled with an explanation. Mode constraints change the effective plan visibly instead of silently clearing saved choices.
- Starting a run freezes the effective plan. The Workbench remains the stable shell while its inspector becomes the run/progress surface; users do not enter a separate modal workflow.
- Run History is the durable entry point for unique retained run logs and terminal summaries.

The exact Slint component decomposition, callback/property boundary, keyboard behavior, adaptive sizing, final wording, branding, and visual tokens remain implementation-blueprint decisions.

## Candidate contract observed in all variants

- Profile and target context remain visible from setup through the terminal result.
- Profile capabilities disable unsupported operations with an explanation; they do not silently erase saved choices.
- Validation appears beside the affected input and again in the run-readiness summary.
- Dry run previews the same effective plan while guaranteeing no persistent writes.
- Starting a run freezes the effective plan and opens a dedicated progress/diagnostic surface.
- Cancel immediately changes the run to **Stopping**, starts no new work, and completes at a named safe cancellation boundary.
- Terminal state is explicitly one of **Succeeded**, **Succeeded with errors**, **Failed**, or **Cancelled** and links to the unique run log.
