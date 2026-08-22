# Cathedral Assets Optimizer

Cathedral Assets Optimizer recognizes files in Bethesda game mods and decides how each should participate in an optimization run.

## Language

**Asset**:
A file recognized as a candidate for optimization or archive handling during an optimization run.
_Avoid_: Resource, input file

**Asset Kind**:
An Asset's broad behavioral category: Texture, Mesh, Animation, or Archive.
_Avoid_: File type, extension

**Asset Variant**:
A distinction within an Asset Kind that changes how the Asset is handled, such as a standard or terrain Mesh and a native or convertible Texture. Enabled conversion is sufficient work to route a convertible Texture.
_Avoid_: Subtype, extension

**Loose Asset**:
An Asset stored directly in a mod's directory tree. When it shares a game path with an Archived Asset, the Loose Asset takes precedence in Bethesda games.
_Avoid_: Unpacked Asset, standalone file

**Archived Asset**:
An Asset stored inside an Archive. It is shadowed by a Loose Asset with the same game path.
_Avoid_: Packed file, archive entry

**Profile Capability**:
An Asset Kind or optimization behavior supported by the selected game profile. A run request that contradicts a Profile Capability is invalid.
_Avoid_: Feature flag, UI availability

**Dry Run**:
A non-mutating optimization run that evaluates eligible Loose Assets without extracting or creating Archives.
_Avoid_: Preview mode, simulation

**Optimization Run**:
One attempt to evaluate or mutate selected mod Assets under a single immutable Routing Policy, ending in one Run Outcome.
_Avoid_: Asset Run, process, job

**Run Request**:
The immutable Mod Selection, Archive Precedence, execution mode, profile identity, and requested work used to start one Optimization Run.
_Avoid_: Options, command arguments

**Start Error**:
A structural Run Request conflict detected before an Optimization Run starts. It produces no Run Outcome.
_Avoid_: Failed run, exception

**Run Outcome**:
The terminal classification of an Optimization Run: Succeeded, Completed With Failures, Cancelled, or Failed. Partial mutation is outcome detail, not a separate outcome.
_Avoid_: Status, partial completion

**Run ID**:
The unique identity of one Optimization Run, carried by its events and terminal result so observations from different runs cannot be confused.
_Avoid_: Thread ID, task ID

**Run Phase**:
A stable lifecycle stage of an Optimization Run, from preparation through Safety Cleanup. A Run Phase may report determinate or indeterminate Run Progress.
_Avoid_: Step, task

**Run Progress**:
A phase-local account of completed Asset or Archive attempts and the known total for the current Optimization Run phase. It describes advancement, not success or overall elapsed work.
_Avoid_: Global percentage, work completed

**Run Diagnostic**:
An informational or warning-level observation emitted by an Optimization Run that never determines its Run Outcome.
_Avoid_: Operation Failure, log message

**Safety Cleanup**:
The terminal Run Phase that releases run resources and removes only temporary artifacts owned by the Optimization Run. It neither rolls back completed mutations nor deletes backups or failed-output evidence.
_Avoid_: Rollback, finalization

**Committed Mutation**:
A durable filesystem change completed by an Optimization Run. It is retained even when a later operation causes cancellation or failure.
_Avoid_: Temporary artifact, partial write

**Operation Failure**:
A structured unsuccessful Archive or Asset operation that records its mutation state and whether the Optimization Run can safely continue.
_Avoid_: Exception, error log

**Archive Precedence**:
The explicit high-to-low ordering used when Archives within one Mod Root contain the same game path. The first Archive wins among Archived Assets, while a Loose Asset always takes precedence.
_Avoid_: Filesystem order, discovery order

**Archive Collision**:
The presence of the same canonical game path in more than one Archive within one precedence scope. Archive Precedence selects the winner, and the collision is reported before extraction.
_Avoid_: Duplicate file, overwrite

**Archive Finalization**:
The Apply-only Run Phase that commits planned output Archives and then cleans their source sets. It does not run after cancellation or fatal failure.
_Avoid_: Safety Cleanup, packing callback

**Mod Root**:
The directory tree of one selected mod, processed independently and defining one Archive Precedence scope.
_Avoid_: Input directory, filesystem root

**Mod Selection**:
The request to process either one Mod Root or the child Mod Roots beneath a selected mods directory. It is resolved into an ordered set of Mod Roots during Preparing.
_Avoid_: Mode, input path

**Mesh Reference Maintenance**:
Updating a Mesh when a referenced convertible Texture is converted. It is Mesh work even when mesh optimization is otherwise disabled.
_Avoid_: Mesh optimization, path cleanup

**Effective Asset Tree**:
The definitive directory tree discovered after enabled Archives are extracted while preserving Loose Asset precedence. Non-Archive Assets are routed from this tree exactly once.
_Avoid_: Merged filesystem, extracted files

**Routing Decision**:
The policy-aware outcome that identifies an Asset and determines whether and where it participates in an optimization run.
_Avoid_: Dispatch result, file filter result

**Routing Ledger**:
The batch routing outcome that groups Routed Assets by run phase and counts recognized exclusions by Skip Reason without retaining unsupported paths, scheduling, or executing work.
_Avoid_: Routing plan, work queue

**Routing Policy**:
The immutable, run-scoped facts used to decide whether and where an Asset participates in an optimization run.
_Avoid_: Options, profile settings

**Routing Disposition**:
The outcome carried by a Routing Decision: route the Asset, skip the recognized Asset for a stated reason, or report that the path is unsupported.
_Avoid_: Status, result code

**Routed Asset**:
A recognized Asset assigned to a run phase and optimizer by its Routing Decision. Only Routed Assets count as scheduled work and contribute to progress totals.
_Avoid_: Queued file, eligible input

**Asset Operation**:
A closed description of work carried by a Routed Asset, such as extraction, optimization, conversion, or Mesh Reference Maintenance.
_Avoid_: Option flag, optimizer setting

**Skip Reason**:
A stable category explaining why Routing Policy excludes a recognized Asset. A disabled run phase takes precedence, followed by a disabled Asset Kind when none of its Variants has work, then an excluded Asset Variant when its Kind has other work.
_Avoid_: Log message, error text
