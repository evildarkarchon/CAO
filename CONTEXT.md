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
