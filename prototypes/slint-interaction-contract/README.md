# Slint interaction-contract prototype

> **THROWAWAY PROTOTYPE:** this is not production port code. Keep only the interaction decision recorded in `VERDICT.md`, then delete or replace this directory.

Question: what screen structure, option grouping, disclosure model, profile workflow, target selection, run/progress/log presentation, validation feedback, and explicit cancellation interaction should the redesigned Slint GUI use while preserving interaction parity?

The prototype contains three deliberately different layouts over the same mocked state:

- **A — Guided flow:** a staged setup path with a persistent run-readiness summary.
- **B — Workbench:** dense category navigation with a live effective-plan inspector.
- **C — Run desk:** a compact setup header and operations board that turns into the run dashboard.

Run from the repository root:

```powershell
cargo run --locked --manifest-path .\prototypes\slint-interaction-contract\Cargo.toml
```

The first build downloads and compiles Slint dependencies. Use the bottom prototype bar to switch layouts and scenarios. Buttons inside the mock let you exercise validation, run phases, safe-boundary cancellation, and terminal outcomes; no filesystem or optimizer mutation occurs.

