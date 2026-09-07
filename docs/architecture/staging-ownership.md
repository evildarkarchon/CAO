# Staging ownership and crash recovery

Issue #407 defines recovery during Apply-mode Preparing. Recovery runs after configuration,
Routing Policy compilation, and canonical Mod Root resolution, before any work phase. Dry Run
does not inspect or mutate staging. The recovery entry point does not create staging. The same
ownership module now also produces durable Texture registrations. The current Run Executor still rejects requested work until its
remaining execution services are available; the legacy run adapter uses this ownership module.

Texture output uses the dedicated area on the same volume as its destination. This supersedes
issue #410's same-directory requirement and its former exception for sibling temporary files.
The destination is committed before conversion-source removal. Windows is the supported target;
Wine runs the same Windows executable and ownership protocol.

`TemporaryArtifactRegistry` owns a recovery/production scope, or borrows the run's existing scope.
Apply preparation recovers selected Mod Roots before traversal, and the scope retains ownership
locks through the terminal cleanup pass. The production adapter supplies the actual selected
Mod Root, including each immediate child in Several Mods mode. Standalone Asset calls can treat
the Asset's parent as their Mod Root. Dry Run does not create, recover, or clean staging.

Each Mod Root reserves `.cao-staging`. Other names beginning with `.cao-staging`, compared using
ASCII case-insensitive matching, are unknown staging-like entries: Apply fails with their path
and recovery instructions. Discovery excludes this namespace from both Archive and Asset passes.
It never attempts to extract, optimize, or infer ownership of these contents.

The dedicated area contains a stable `owner.lock`, `ownership.manifest`, and one unpredictable
Run-ID-derived child. Producers must hold the OS lock before publishing ownership or creating
temporary entries and through Safety Cleanup. On Windows this is an existing-file open with
`GENERIC_READ` and no sharing, including no delete sharing. The existing POSIX implementation
uses an exclusive, nonblocking `flock`; it does not represent a supported native Linux or macOS
release. Lock files, PIDs, and timestamps alone do not prove an
active owner.
Recovery never unlinks or replaces either control file or the reserved directory, so a competing
run cannot acquire a new lock identity while an earlier run still owns the old one.

The writer emits v2 manifests; recovery also accepts existing v1 manifests. Both are UTF-8,
bounded to 8 MiB and 100,000 registrations. Whitespace separates fields;
strings use C++ `std::quoted` double-quote/backslash escaping. All string fields must be quoted.
The grammar is:

```text
CAO-STAGING 2
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

Producers flush a complete registration snapshot before exclusively creating each temporary file.
A v2 manifest additionally authorizes the fixed `ownership.manifest.next` scratch file used to
publish its next snapshot. The writer exclusively creates and flushes that scratch file, then
replaces `ownership.manifest` on the same volume. Recovery can discard a partial scratch file
only after validating the authoritative v2 manifest and the owned tree. A v1 manifest does not
authorize this extra control file.

Texture saving closes its handles and the commit step flushes staged bytes before replacing the
destination without any cross-volume copy fallback. After the rename, the producer publishes a
snapshot releasing the temporary path's registration. Registrations identify deletion candidates
by their temporary paths: original Assets, destination paths, and backups are never registered.
Retained evidence must likewise be moved out of staging before its registration is released.

The crash states are intentional:

- Before temporary creation, a flushed registration may name an absent file. Recovery skips it.
- During saving, recovery removes the registered partial output and retains the original Asset.
- After destination commit but before deregistration, the temporary path is absent. Recovery
  skips it and never follows the renamed file to the destination.
- After deregistration but before conversion-source removal, both usable files remain. Cleanup
  preserves both; a later optimization attempt may complete the conversion.
- During manifest publication, the previous complete snapshot remains authoritative. Its v2
  ownership permits removal of an interrupted scratch snapshot.

Bootstrap claims only a newly created reserved directory. The initial complete manifest is
published before any run child or Texture data is created. An interruption before that initial
ownership proof is published can leave unverifiable control files; preparation preserves them
for inspection. Existing empty directories, incomplete initial manifests, and other unproven
collisions are never silently adopted. No Asset mutation has begun in that bootstrap window.

Recovery acquires the OS lock, pins the manifest against Windows writes/replacement while
reading it, validates root and run-child identity, and checks the entire present tree against the recorded entries
before deleting anything. Links, junctions, reparse points, hard links, unknown children, type
mismatches, and inaccessible contents fail closed. Windows handles pin temporary files against
replacement and delete those file identities; directory removal is nonrecursive. POSIX producers
must cooperate with `owner.lock`; file identity is checked again immediately before unlinking.
This protects staging from competing CAO processes, without claiming a sandbox against arbitrary
filesystem changes by the same operating-system user.

Only recorded, present entries are removed, in reverse registration order. The authoritative
manifest and stable ownership lock remain byte-for-byte unchanged; v2 scratch is disposable. A cleanup error stops Preparing and leaves the remaining entries for
inspection or a later retry. Every terminal path still performs the normal Safety Cleanup pass;
the recovery lock remains held until that pass finishes. Normal cleanup attempts every owned
entry despite individual failures, then removes the empty run child nonrecursively. It also
covers durable records whose file creation failed before returning to the caller.
Cancellation is observed during read-only traversal and between atomic removals. Unattempted
registrations remain available for the next run, and the executor still performs Safety Cleanup.

Failures expose the affected path and one of `StagingActive`, `StagingOwnershipUnverified`, or
`StagingRecoveryFailed`. Active ownership calls for waiting for its run to finish. Unverifiable
contents call for inspecting the manifest and moving unrecognized material out of the reserved
namespace; recovery never advises deleting contents merely because their names resemble staging.

Validation includes Windows subprocess termination during a partial Texture save, after output
commit, after conversion-source removal, and between rename and deregistration. Recovery tests
also cover active locks, absent temporary paths, corrupt ownership, interrupted scratch snapshots,
and reuse of a recovered ownership scope. The same Windows crash tests run under Wine when that
runtime is available.
