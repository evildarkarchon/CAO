# CLI Optimization Runs

The CLI keeps the existing `folder mode profile` arguments and work-selection options.
Use `--help` to list them. `om` selects one Mod Root; `sm` selects its immediate child
Mod Roots. The Optimization Run service resolves selection, applies exclusions, and owns
execution and cleanup.

Standard output renders ordered Run Events. Each event starts with
`EVENT:|<Run ID>|<sequence>|`. Progress lines contain
`PROGRESS:|<phase>|<completed>|<total>|succeeded=<count>|failed=<count>`.
These are the service's phase-local counters, not an overall percentage. Failed attempts
advance completed progress; cancelled, unattempted work does not. Indeterminate and
skipped phases have no invented progress total. Skipped phases include their reason.

The terminal event names the outcome and final work phase, with operation and cleanup
failures, cancellation observation, and retained mutation details. Safety Cleanup is
reported before the terminal event. Committed outputs remain after cancellation or failure.

Ctrl+C requests cooperative cancellation. The CLI keeps waiting for the in-flight
attempt and Safety Cleanup. Repeated Ctrl+C requests do not force the worker to stop.
On Windows, Ctrl+Break requests the same cooperative cancellation.

| Result | Exit code |
| --- | ---: |
| Succeeded (or `--help`) | 0 |
| Completed With Failures | 1 |
| Failed, Start Error, or invalid arguments | 2 |
| Cancelled | 130 |
