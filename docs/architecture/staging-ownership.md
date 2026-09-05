# Staging ownership and crash recovery

Issue #407 defines recovery during Apply-mode Preparing. Recovery runs after configuration,
Routing Policy compilation, and canonical Mod Root resolution, before any work phase. Dry Run
does not inspect or mutate staging. The recovery reader does not create staging; the staged
operation services will be its producers. The current Run Executor still rejects requested work
until those services are available.

Each Mod Root reserves `.cao-staging`. Other names beginning with `.cao-staging`, compared using
ASCII case-insensitive matching, are unknown staging-like entries: Apply fails with their path
and recovery instructions. Discovery excludes this namespace from both Archive and Asset passes.
It never attempts to extract, optimize, or infer ownership of these contents.

The dedicated area contains a stable `owner.lock`, `ownership.manifest`, and one unpredictable
Run-ID-derived child. Producers must hold the OS lock before publishing ownership or creating
temporary entries and through Safety Cleanup. On Windows this is an existing-file open with
`GENERIC_READ` and no sharing, including no delete sharing. On POSIX it is an exclusive,
nonblocking `flock`. Lock files, PIDs, and timestamps alone do not prove an active owner.
Recovery never unlinks or replaces either control file or the reserved directory, so a competing
run cannot acquire a new lock identity while an earlier run still owns the old one.

The v1 manifest is UTF-8, bounded to 8 MiB and 100,000 registrations. Whitespace separates fields;
strings use C++ `std::quoted` double-quote/backslash escaping. All string fields must be quoted.
The grammar is:

```text
CAO-STAGING 1
"<canonical generic UTF-8 Mod Root path>"
"<Run ID>" "run-<Run ID>-<32 lowercase hexadecimal nonce characters>"
<registration count>
D "run-<Run ID>-<nonce>"
F "run-<Run ID>-<nonce>/temporary.dds"
```

The nonce must be generated unpredictably by the producer. Run IDs contain 1–128 ASCII letters,
digits, or hyphens. The first registration is the run child directory. Subsequent registrations
are unique relative paths beneath that child, with `D` for a directory or `F` for a regular file.
Parents must be registered before children. Absolute paths, traversal components, Windows stream
or ambiguous names, backslash separators, extra records, and unsupported versions are invalid.
The format records explicit temporary ownership; it is not authentication against someone who
can forge the manifest and edit the Mod Root.

Producers must durably register each path before creating it. Backups, committed outputs, and
failed-output evidence must never remain registered as temporary. Before retaining or committing
an artifact, durably release its recovery ownership and move it outside the staging area before
resuming automatic use of that area. A crash in between leaves unknown material that recovery
preserves. An incomplete manifest write is likewise preserved for manual inspection. Missing
registered entries are valid because registration precedes creation and a prior cleanup may
already have removed them.

Recovery acquires the OS lock, pins the manifest against Windows writes/replacement, validates
root and run-child identity, and checks the entire present tree against the recorded entries
before deleting anything. Links, junctions, reparse points, hard links, unknown children, type
mismatches, and inaccessible contents fail closed. Windows handles pin temporary files against
replacement and delete those file identities; directory removal is nonrecursive. POSIX producers
must cooperate with `owner.lock`; file identity is checked again immediately before unlinking.
This protects staging from competing CAO processes, without claiming a sandbox against arbitrary
filesystem changes by the same operating-system user.

Only recorded, present entries are removed, in reverse registration order. Control files remain
byte-for-byte unchanged. A cleanup error stops Preparing and leaves the remaining entries for
inspection or a later retry. Every terminal path still performs the normal Safety Cleanup pass;
the recovery lock remains held until that pass finishes.
Cancellation is observed during read-only traversal and between atomic removals. Unattempted
registrations remain available for the next run, and the executor still performs Safety Cleanup.

Failures expose the affected path and one of `StagingActive`, `StagingOwnershipUnverified`, or
`StagingRecoveryFailed`. Active ownership calls for waiting for its run to finish. Unverifiable
contents call for inspecting the manifest and moving unrecognized material out of the reserved
namespace; recovery never advises deleting contents merely because their names resemble staging.
