# Staging ownership and crash recovery

Issue #407 established recovery during Apply-mode Preparing. Recovery runs after configuration,
Routing Policy compilation, and canonical Mod Root resolution, before any work phase. Dry Run
does not inspect or mutate staging. The recovery entry point does not create staging. The
integrated Optimization Run uses the same Temporary Ownership scope for recovery and production.
Windows is the supported target; Wine runs the same Windows executable and ownership protocol.

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
Run-ID-derived child for control-area artifacts. A manifest may also own generated Texture, Mesh,
and Animation staging siblings elsewhere beneath the same Mod Root. Producers must hold the OS
lock before publishing ownership or creating temporary entries and through Safety Cleanup. On
Windows this is an existing-file open with `GENERIC_READ` and no sharing, including no delete
sharing. Lock files, PIDs, and timestamps alone do not prove an
active owner.
Recovery never unlinks or replaces either control file or the reserved directory, so a competing
run cannot acquire a new lock identity while an earlier run still owns the old one.

The writer emits v3 manifests; recovery also accepts existing v1 and v2 manifests. All are UTF-8,
bounded to 8 MiB and 100,000 registrations. Whitespace separates fields;
strings use C++ `std::quoted` double-quote/backslash escaping. All string fields must be quoted.
The grammar is:

```text
CAO-STAGING 3
"<canonical generic UTF-8 Mod Root path>"
"<Run ID>" "run-<Run ID>-<32 lowercase hexadecimal nonce characters>"
<registration count>
D "run-<Run ID>-<nonce>"
S "textures/example/.cao-staging-texture-<Run ID>-<nonce>.dds"
S "meshes/example/.cao-staging-mesh-<Run ID>-<nonce>.nif"
S "animations/example/.cao-staging-animation-<Run ID>-<nonce>.hkx"
F "run-<Run ID>-<nonce>/archive-entry-<nonce>"
```

The nonce must be generated unpredictably by the producer. Run IDs contain 1–128 ASCII letters,
digits, or hyphens. The first registration is the run child directory. Later `D` and `F` records
are unique relative paths beneath that child; Archive extraction entries, output Archives, and
staged loading plugins use `F` records there. Version 3 also uses `S` for a generated Asset sibling
relative to the Mod Root. The supported
names are `.cao-staging-texture-<Run ID>-<nonce>.dds`,
`.cao-staging-mesh-<Run ID>-<nonce>.nif` (also `.btr` or `.bto`), and
`.cao-staging-animation-<Run ID>-<nonce>.hkx`. Each must be beside its destination and outside
the reserved control area. Parents must be registered before control-area children. Absolute
paths, traversal components, Windows stream or ambiguous names, backslash separators, extra
records, and unsupported versions are invalid.
The format records explicit temporary ownership; it is not authentication against someone who
can forge the manifest and edit the Mod Root.

Producers flush a complete registration snapshot before exclusively creating each temporary file.
One snapshot may register many files: Archive extraction registers every entry file of an Archive
in a single snapshot and then creates them, because a snapshot per entry rewrites a manifest that
grows with each one. A name registered by that snapshot whose file was never created is absent,
which cleanup and recovery skip. A name that collides on exclusive creation was never acquired,
so it is withdrawn again together with the names after it that were not yet created.
A v2 or v3 manifest additionally authorizes the fixed `ownership.manifest.next` scratch file used to
publish its next snapshot. The writer exclusively creates and flushes that scratch file, then
replaces `ownership.manifest` on the same volume. A replacement refused with Access denied or a
sharing violation is retried for about a second, since a real-time scanner can briefly hold the
manifest a run has just written; the scratch file stays in place until it succeeds or the error
is reported. Recovery can discard a partial scratch file
only after validating the authoritative v2 or v3 manifest and the owned tree. A v1 manifest does
not authorize this extra control file.

Durable producers write and validate bytes at a move-only `PublicationReceipt` path. Texture
(`.dds`), Mesh (`.nif`, `.btr`, `.bto`), and Animation (`.hkx`) publish from sibling staging with
`Replace`, bound to the destination selected at staging. A Texture conversion is the exception:
its DDS output is a different file from its source, so it never replaces. When a file already
stands at that destination the attempt completes without staging or touching either file, and
otherwise it publishes with `NoReplace`. Archive extraction writes entry bytes in
the run child and publishes with `NoReplace` after case-correct game-path resolution and occupied
leaf checks; it keeps destination parents pinned through publication. Archive Finalization writes
each planned output Archive in the run child and publishes with `NoReplace`. A needed loading
plugin is staged and published separately with `NoReplace` after the Archive, unless the exact
planned dummy is already present.

An Asset producer captures its destination, and a Texture conversion pins its source, before the
backend loads anything and before anything is staged. A failure there changes nothing, so the
attempt is contained and safe to continue. A destination or source that is a link or has more than
one hard link is never replaced: renaming output over a hard link would leave its other names on
the old bytes, and a hardlink-deploying mod manager makes every file it deploys one. Its attempt is
`Ineligible`, and Dry Run reports the same result by reading the same facts without capturing.
From staging onward only a path lookup failure, which precedes any manifest change, is safe to
continue; any other staging failure stops the run.

The receipt's single publication attempt revalidates an absolute destination beneath its canonical
Mod Root, outside reserved staging, on the same volume, through an ordinary parent. It flushes
staged bytes before a native same-volume publication with no copy fallback; `NoReplace` also
rejects a leaf occupied after preflight. Publication consumes the receipt even on failure, and it
never throws: it is declared `noexcept`, and a failure while building the error detail returns the
state reached so far with an empty detail, so no producer needs defensive handling around it. The
native destination mutation precedes the snapshot that releases the temporary registration.
Generic `commit` retains only non-durable registrations; it cannot release a durable claim even
after a separate move. Registrations identify deletion candidates by their temporary paths:
source Assets, committed destinations, backups, and retained evidence are never registered.

The receipt returns one of three states, which also describe the crash windows:

- `NotPublished`: no destination mutation occurred in this attempt. A flushed registration may
  name an absent file before creation. The failed publication abandons its staged file: it deletes
  the file through a handle proving its identity, then releases the record, so a crash between the
  two leaves a registered, absent name. A producer that stops after staging, because its save or
  converter failed or it found nothing to change, abandons its receipt the same way. Whatever an
  abandonment cannot remove stays owned, and Safety Cleanup or later recovery removes only the
  registered temporary path and retains the original Asset or occupied destination.
- `PublishedStillOwned`: the native destination mutation succeeded, but the ownership snapshot
  could not release the old temporary name. That name may already be absent after a move; recovery
  skips an absent temporary and never follows it to the committed destination. The returned state
  preserves the committed fact for producer mutation evidence even when release fails.
- `PublishedAndReleased`: the destination committed and the release snapshot succeeded. Cleanup
  preserves the destination. Texture conversion-source removal follows this state; Archive
  Finalization creates any planned loading plugin and then performs its requested source cleanup.

Archive extraction publishes each entry with its release deferred, then releases every published
name in one snapshot once the merge ends, for the reason one snapshot registered them. Between an
entry's publication and that snapshot, its name is registered and absent: the same condition as
`PublishedStillOwned`, entered on purpose, and recovered the same way. If the joint release fails,
the destinations stay Committed and the names stay owned, and the attempt is unsafe to continue
exactly as after a single failed release.

The publication result also carries the mutation fact, so producers read it instead of translating
states. Its mutation is None for `NotPublished` and Committed for both published states. It is safe
to continue except after a release failure: `PublishedStillOwned` is always Committed and unsafe.
A release failure belongs to the run's Temporary Ownership scope, not to the output, and the next
staging operation would go through the same snapshot path that just failed. "Safe to continue"
means only that no unknown disk state makes continuing dangerous; a phase may be stricter than
this verdict but never looser.

During manifest publication, the previous complete snapshot remains authoritative. Its v2 or v3
ownership permits removal of an interrupted scratch snapshot. Archive extraction reports an
attempt-level `PartialOrUnknown` mutation and unsafe continuation only for a merge in which at
least one entry was committed, including a merge whose joint release then failed, so a half-merged
Effective Asset Tree is never optimized as if it were complete. A merge failure before the first
commit, such as a not-published entry or a containment rejection of a linked parent, reports no
mutation and is safe to continue; every later Archive attempt repeats its own containment checks.
The Run Executor retains terminal mutation evidence, cancellation, Safety Cleanup, and Run Outcome
precedence independently of the receipt state.

Bootstrap claims only a newly created reserved directory. The initial complete manifest is
published before any run child or staged Asset or Archive bytes are created. A bootstrap that
fails before that initial ownership proof is published removes what it created: its scratch
snapshot, its `owner.lock` and the reserved directory, each only while it is still the ordinary
entry bootstrap made, and the directory only if empty. A bootstrap interrupted in that window
leaves only `owner.lock` and perhaps the scratch snapshot. Preparation preserves them, and its
`StagingOwnershipUnverified` failure says that this state means a run stopped before staging
anything. Existing empty directories, incomplete initial manifests, and other unproven collisions
are never silently adopted. No Asset mutation has begun in that bootstrap window.

Recovery acquires the OS lock, pins the manifest against Windows writes/replacement while
reading it, validates root and run-child identity, checks the entire control tree, and pins every
present sibling staging file against the recorded entries before deleting anything. Links,
junctions, reparse points, hard links, unknown children, type
mismatches, and inaccessible contents fail closed. Windows handles pin temporary files against
replacement and delete those file identities. Windows also pins every ordinary ancestor of a
present sibling before opening its file by name, then holds the ancestor pins through deletion.
Production keeps sibling parent pins from stage creation through Safety Cleanup; the run child
stays pinned until its staged Archive files have been removed. Directory removal is nonrecursive.
This protects staging from competing CAO processes, without claiming a sandbox against arbitrary
filesystem changes by the same operating-system user.

Only recorded, present entries are removed, in reverse registration order. The authoritative
manifest and stable ownership lock remain byte-for-byte unchanged; v2 or v3 scratch is disposable.
A cleanup error stops Preparing and leaves the remaining entries for inspection or a later retry.
Every terminal path still performs the normal Safety Cleanup pass; the recovery lock remains held
until that pass finishes. Normal cleanup attempts every owned
entry despite individual failures, then removes the empty run child nonrecursively. It also
covers durable records whose file creation failed before returning to the caller.
Cancellation is observed during read-only traversal and between atomic removals. Unattempted
registrations remain available for the next run, and the executor still performs Safety Cleanup.

Failures expose the affected path and one of `StagingActive`, `StagingOwnershipUnverified`, or
`StagingRecoveryFailed`. Active ownership calls for waiting for its run to finish. Unverifiable
contents call for inspecting the manifest and moving unrecognized material out of the reserved
namespace; recovery never advises deleting contents merely because their names resemble staging.
The one exception is a reserved directory holding only its lock, perhaps with a scratch snapshot:
only an interrupted bootstrap leaves that, so the failure says the directory can be deleted once
no CAO run uses the Mod Root.

Validation includes interrupted Asset and Archive publication with a committed destination still
owned by an older snapshot, abandoned output, active locks, absent temporary paths, corrupt
ownership, interrupted scratch snapshots, and reuse of a recovered ownership scope. The same
Windows crash tests run under Wine when that runtime is available.
