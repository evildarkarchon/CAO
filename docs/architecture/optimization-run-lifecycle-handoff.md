# Handoff: Deepen the Optimization Run Lifecycle

Use this handoff when resuming architectural grilling on optimization-run lifetime, scheduling, cancellation, progress, and completion. The next agent should settle the design tree with the user before changing code.

## Goal

Make the optimization run a deep module whose interface hides construction-time I/O, scheduling, phase lifetime, cancellation mechanics, progress accounting, and completion semantics from GUI and CLI adapters.

## Evidence

- `src/Manager.cpp:7-32` performs logger setup, validation, ignored-mod reads, directory discovery, and file discovery in the constructor.
- `src/Manager.cpp:120-178` owns extract, rescan, optimize, pack, cleanup, progress, cancellation checks, and completion emission.
- `src/MainWindow.cpp:272-312` must construct the manager, connect signals, start a log timer, launch `runOptimization` through `QtConcurrent`, then cancel and disconnect it.
- `src/Manager.h:52-82` holds mutable counters and work lists, retains options by reference, contains a raw `QSettings*`, and uses a plain boolean for cross-thread cancellation.
- GUI and CLI are two real adapters at this seam, but `src/main.cpp:46` attempts a `Manager(QStringList)` construction not declared in `src/Manager.h`.
- `src/CMakeLists.txt:15-18` labels the CLI path untested, and the build defines no test target.

## Architecture Assessment

**Deletion test**: deleting `Manager` would spread phase orchestration into the GUI and intended CLI. The module has depth, but lifetime and concurrency knowledge leak through its interface.

**Dependencies**:

- Filesystem discovery and mutation are local-substitutable with temporary directory trees.
- Asset optimizers and scheduling are in-process.
- GUI and CLI are two presentation adapters, so their shared seam is real.

## Scope Guardrails

- Keep presentation concerns in GUI and CLI adapters.
- Give construction no surprising filesystem or logger side effects.
- Make terminal outcomes explicit, including cancellation and failure.
- Concentrate progress accounting and cancellation semantics inside the run module.
- Test a run through observable progress and terminal results, not widget or thread choreography.

## Grilling Start

Begin with these prerequisite decisions:

1. Is a run synchronous at its core with scheduling owned by adapters, or does the module own asynchronous execution?
2. Is cancellation cooperative at file boundaries, phase boundaries, or inside individual optimizers?
3. Which terminal outcomes must be distinct: completed, cancelled, partially completed, and failed?
4. Is progress defined by discovered files, completed operations, weighted work, or named phases?
5. Which cleanup must occur after cancellation or failure?

Use the `grilling` skill for the full decision tree. Use `domain-modeling` to capture only project-specific run terms once resolved; record an ADR only if a hard-to-reverse concurrency trade-off meets the ADR threshold.

## Completion Criterion

Grilling is complete when ownership of scheduling, lifetime, cancellation, progress, phase sequencing, cleanup, terminal outcomes, adapter responsibilities, and surviving tests is explicit; the user has confirmed the shared understanding; and no concurrency contract remains silent.
