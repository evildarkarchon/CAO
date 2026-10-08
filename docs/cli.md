# CLI Optimization Runs

The CLI keeps the existing `folder mode profile` arguments and work-selection options.
Use `--help` to list them. `om` selects one Mod Root; `sm` selects its immediate child
Mod Roots. The Optimization Run service resolves selection, applies exclusions, and owns
execution and cleanup.

The CLI never reads `settings.ini`. Five archive options, which the GUI reads from it, are
set on the command line instead. Each takes an explicit `0` or `1` as a separate argument
(`--bcomp 0`), and anything else is an invalid argument. When a flag is omitted, its default
applies. They exist for the parity oracle (see [parity-oracle.md](parity-oracle.md)).

| Flag | Option | Default |
| --- | --- | ---: |
| `--bcomp` | Compress created Archives | 1 |
| `--bdum` | Create Dummy Plugins to load created Archives | 1 |
| `--bmi` | Merge Incompressible files into the main Archive | 1 |
| `--bmt` | Merge Textures into the main Archive | 0 |
| `--bds` | Delete loose files after packing them | 1 |

An Archive that holds Incompressible files is never compressed, whatever `--bcomp` says.
With the default `--bmi 1`, that includes the main Archive whenever the Mod Root has
Incompressible files.

Standard output renders ordered Run Events. Each event starts with
`EVENT:|<Run ID>|<sequence>|`. Progress lines contain
`PROGRESS:|<phase>|<completed>|<total>|succeeded=<count>|failed=<count>`.
These are the service's phase-local counters, not an overall percentage. Failed attempts
advance completed progress; cancelled, unattempted work does not. Indeterminate and
skipped phases have no invented progress total. Skipped phases include their reason.

Fields are separated by `|`, and every record ends at a newline. Text fields (details,
messages, operations, service details and paths) are escaped so that neither character can
appear inside them:

| Character | Escape |
| --- | --- |
| `\` | `\\` |
| `\|` | `\p` |
| CR | `\r` |
| LF | `\n` |

`|` becomes `\p`, not `\|`. A parser can therefore split a line on every `|` and then
unescape each field. The Run ID, sequence, labels and counters are never escaped.

The terminal event names the outcome and final work phase. Its sealed details include
Run Failures, Operation Failures, Safety Cleanup failures, Archive Collisions, cancellation
observation, and committed mutation counts by Mod Root and operation kind. Safety Cleanup
is reported before the terminal event. Committed outputs remain after cancellation or failure.
Each detail is a record on its own line. It starts with its label (`Asset Failure|...`),
not with the `EVENT:` prefix. Late diagnostics can still follow the terminal event.

Ctrl+C requests cooperative cancellation. The CLI keeps waiting for the in-flight
attempt and Safety Cleanup. Repeated Ctrl+C requests do not force the worker to stop.
On Windows, Ctrl+Break requests the same cooperative cancellation.

| Result | Exit code |
| --- | ---: |
| Succeeded (or `--help`) | 0 |
| Completed With Failures | 1 |
| Failed, Start Error, or invalid arguments | 2 |
| Cancelled | 130 |
