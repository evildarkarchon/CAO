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
sharing. The existing POSIX implementation
uses an exclusive, nonblocking `flock`; it does not represent a supported native Linux or macOS
release. Lock files, PIDs, and timestamps alone do not prove an
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
A v2 or v3 manifest additionally authorizes the fixed `ownership.manifest.next` scratch file used to
publish its next snapshot. The writer exclusively creates and flushes that scratch file, then
replaces `ownership.manifest` on the same volume. Recovery can discard a partial scratch file
only after validating the authoritative v2 or v3 manifest and the owned tree. A v1 manifest does
not authorize this extra control file.

Durable producers write and validate bytes at a move-only `PublicationReceipt` path. Texture
(`.dds`), Mesh (`.nif`, `.btr`, `.bto`), and Animation (`.hkx`) publish from sibling staging with
`Replace`, bound to the destination selected at staging. Archive extraction writes entry bytes in
the run child and publishes with `NoReplace` after case-correct game-path resolution and occupied
leaf checks; it keeps destination parents pinned through publication. Archive Finalization writes
each planned output Archive in the run child and publishes with `NoReplace`. A needed loading
plugin is staged and published separately with `NoReplace` after the Archive, unless the exact
planned dummy is already present.

The receipt's single publication attempt revalidates an absolute destination beneath its canonical
Mod Root, outside reserved staging, on the same volume, through an ordinary parent. It flushes
staged bytes before a native same-volume publication with no copy fallback; `NoReplace` also
rejects a leaf occupied after preflight. Publication consumes the receipt even on failure. The
native destination mutation precedes the snapshot that releases the temporary registration.
Generic `commit` retains only non-durable registrations; it cannot release a durable claim even
after a separate move. Registrations identify deletion candidates by their temporary paths:
source Assets, committed destinations, backups, and retained evidence are never registered.

The receipt returns one of three states, which also describe the crash windows:

- `NotPublished`: no destination mutation occurred in this attempt. A flushed registration may
  name an absent file before creation; a save or preflight failure can leave partial staged bytes.
  Safety Cleanup or later recovery removes only the registered temporary path and retains the
  original Asset or occupied destination.
- `PublishedStillOwned`: the native destination mutation succeeded, but the ownership snapshot
  could not release the old temporary name. That name may already be absent after a move; recovery
  skips an absent temporary and never follows it to the committed destination. The returned state
  preserves the committed fact for producer mutation evidence even when release fails.
- `PublishedAndReleased`: the destination committed and the release snapshot succeeded. Cleanup
  preserves the destination. Texture conversion-source removal follows this state; Archive
  Finalization creates any planned loading plugin and then performs its requested source cleanup.

During manifest publication, the previous complete snapshot remains authoritative. Its v2 or v3
ownership permits removal of an interrupted scratch snapshot. Archive extraction retains its
attempt-level `PartialOrUnknown` mutation and unsafe continuation after a merge failure, even
when an entry destination was committed. The Run Executor retains terminal mutation evidence,
cancellation, Safety Cleanup, and Run Outcome precedence independently of the receipt state.

Bootstrap claims only a newly created reserved directory. The initial complete manifest is
published before any run child or staged Asset or Archive bytes are created. An interruption
before that initial ownership proof is published can leave unverifiable control files; preparation preserves them
for inspection. Existing empty directories, incomplete initial manifests, and other unproven
collisions are never silently adopted. No Asset mutation has begun in that bootstrap window.

Recovery acquires the OS lock, pins the manifest against Windows writes/replacement while
reading it, validates root and run-child identity, checks the entire control tree, and pins every
present sibling staging file against the recorded entries before deleting anything. Links,
junctions, reparse points, hard links, unknown children, type
mismatches, and inaccessible contents fail closed. Windows handles pin temporary files against
replacement and delete those file identities; directory removal is nonrecursive. POSIX producers
must cooperate with `owner.lock`; file identity is checked again immediately before unlinking.
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

Validation includes interrupted Asset and Archive publication with a committed destination still
owned by an older snapshot, abandoned output, active locks, absent temporary paths, corrupt
ownership, interrupted scratch snapshots, and reuse of a recovered ownership scope. The same
Windows crash tests run under Wine when that runtime is available.
