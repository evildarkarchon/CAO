# Handoff: Deepen the Texture Optimization Transaction

Use this handoff when resuming architectural grilling on texture optimization. The next agent should settle the design tree with the user before changing code.

## Goal

Turn texture optimization into a deep module whose interface is the file-level optimization test surface. Callers should not need to know transform ordering, mutable image state, output naming, save suppression, or source cleanup.

## Evidence

- `src/TexturesOptimizer.h:16-101` exposes loading, metadata, planning, individual transforms, saving, and the mutable `modifiedCurrentTexture` flag.
- `src/MainOptimizer.cpp:103-165` assembles the transaction: enablement, target sizing, dry run, optimization, save suppression, TGA-to-DDS naming, and TGA deletion.
- `src/TexturesOptimizer.cpp:152-182` and `src/TexturesOptimizer.cpp:248-289` calculate real and dry-run decisions separately. Their target-size calculations already differ.
- Texture policy also enters through global `Profiles::` reads in `TexturesOptimizer.cpp`, so a caller-supplied run configuration is not the complete source of truth.
- The constructor creates COM and GPU state (`src/TexturesOptimizer.cpp:8-19`), coupling object construction to platform availability.
- The build defines no test target.

## Architecture Assessment

**Deletion test**: deleting `TexturesOptimizer` would move hundreds of lines of DirectX implementation into its caller. The module earns its existence, but its interface remains shallow because callers must understand most of the transaction.

**Dependencies**:

- DirectXTex, COM, and compression computation are in-process platform dependencies.
- File input and output are local-substitutable through temporary files or memory fixtures.
- CPU and GPU branches are implementation details today, not established adapters at a seam.

## Scope Guardrails

- Deepen the existing behavior; do not add a pass-through module in front of the current interface.
- Keep dry run and real execution on one decision path.
- Treat observable file or memory outcomes as the test surface.
- Preserve DirectX transform details inside the implementation.
- Establish an adapter only when two justified implementations exist.

## Grilling Start

Begin with these prerequisite decisions:

1. Is one transaction responsible for both file-backed and memory-backed textures?
2. Does dry run return the same planned outcome that real execution consumes?
3. Which outcomes must callers distinguish: unchanged, changed, converted, rejected, or failed?
4. Does the module own TGA source deletion, or only produce the replacement?
5. Is platform initialization run-scoped, process-scoped, or hidden inside the module?

Use the `grilling` skill for the full decision tree. If alternative interfaces need comparison, use the `codebase-design` design-it-twice process. Update `CONTEXT.md` through `domain-modeling` only when project-specific texture terms are resolved.

## Completion Criterion

Grilling is complete when responsibility for policy, planning, transformation, output ownership, platform lifetime, observable outcomes, and surviving tests is explicit; the user has confirmed the shared understanding; and no interface or behavior remains silently assumed.
