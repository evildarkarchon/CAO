//! Asset Routing: the Routing Policy, Routing Decisions and the Routing Ledger.
//!
//! Ported from the C++ `cao::routing` module (`src/AssetRouting`). Routing is
//! filename-only: it never touches the filesystem and never normalizes the
//! caller's path, so a decision is a pure function of the path and the policy.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// An Asset's broad behavioral category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssetKind {
    Texture,
    Mesh,
    Animation,
    Archive,
}

/// The Texture Asset Variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextureVariant {
    Native,
    Convertible,
}

/// The Mesh Asset Variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeshVariant {
    Standard,
    Terrain,
}

/// A closed description of work carried by a Routed Asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssetOperation {
    Extraction,
    Optimization,
    Conversion,
    MeshReferenceMaintenance,
}

/// Whether a run mutates (Apply) or only evaluates (Dry Run).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExecutionMode {
    Apply,
    DryRun,
}

/// The coarse stage of an Optimization Run in which one Routed Asset performs its work.
///
/// This categorizes a single Asset, unlike [`crate::run::RunPhase`], which is a
/// stage of the whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoutedAssetPhase {
    ArchiveExtraction,
    LooseAssetProcessing,
}

/// The optimizer a Routed Asset is executed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OptimizerTarget {
    Texture,
    Mesh,
    Animation,
    Archive,
}

/// Why Routing Policy excluded one recognized Asset.
///
/// A disabled run phase takes precedence, followed by a disabled Asset Kind when
/// none of its Variants has work, then an excluded Asset Variant when its Kind
/// has other work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    DisabledPhase,
    DisabledAssetKind,
    ExcludedAssetVariant,
}

impl SkipReason {
    /// Every Skip Reason, in precedence order.
    pub const ALL: [SkipReason; 3] = [
        SkipReason::DisabledPhase,
        SkipReason::DisabledAssetKind,
        SkipReason::ExcludedAssetVariant,
    ];
}

/// Closed work choices that an input adapter can place in a Run Request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequestedWork {
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction,
    /// Archive creation is a run finalization choice, not a per-Asset Routing
    /// Decision, but it is carried here so one compiled policy validates it
    /// against the selected profile.
    ArchiveCreation,
}

/// Closed game-profile capabilities understood by Routing Policy compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProfileCapability {
    NativeTextureOptimization,
    ConvertibleTextureConversion,
    StandardMeshOptimization,
    TerrainMeshOptimization,
    AnimationOptimization,
    ArchiveExtraction,
    MeshReferenceMaintenance,
    ArchiveCreation,
}

/// Each explicit work choice, the capability it needs, and the Variant it targets.
const REQUESTED_WORK_DEFINITIONS: [(RequestedWork, ProfileCapability, Option<AssetVariant>); 7] = [
    (
        RequestedWork::NativeTextureOptimization,
        ProfileCapability::NativeTextureOptimization,
        Some(AssetVariant::Texture(TextureVariant::Native)),
    ),
    (
        RequestedWork::ConvertibleTextureConversion,
        ProfileCapability::ConvertibleTextureConversion,
        Some(AssetVariant::Texture(TextureVariant::Convertible)),
    ),
    (
        RequestedWork::StandardMeshOptimization,
        ProfileCapability::StandardMeshOptimization,
        Some(AssetVariant::Mesh(MeshVariant::Standard)),
    ),
    (
        RequestedWork::TerrainMeshOptimization,
        ProfileCapability::TerrainMeshOptimization,
        Some(AssetVariant::Mesh(MeshVariant::Terrain)),
    ),
    (
        RequestedWork::AnimationOptimization,
        ProfileCapability::AnimationOptimization,
        None,
    ),
    (
        RequestedWork::ArchiveExtraction,
        ProfileCapability::ArchiveExtraction,
        None,
    ),
    (
        RequestedWork::ArchiveCreation,
        ProfileCapability::ArchiveCreation,
        None,
    ),
];

/// The Asset Kind each Profile Capability belongs to.
const fn capability_kind(capability: ProfileCapability) -> AssetKind {
    match capability {
        ProfileCapability::NativeTextureOptimization
        | ProfileCapability::ConvertibleTextureConversion => AssetKind::Texture,
        ProfileCapability::StandardMeshOptimization
        | ProfileCapability::TerrainMeshOptimization
        | ProfileCapability::MeshReferenceMaintenance => AssetKind::Mesh,
        ProfileCapability::AnimationOptimization => AssetKind::Animation,
        ProfileCapability::ArchiveExtraction | ProfileCapability::ArchiveCreation => {
            AssetKind::Archive
        }
    }
}

/// The built-in Asset extensions a profile Archive extension must not shadow.
const BUILT_IN_EXTENSIONS: [(&str, AssetKind); 6] = [
    (".dds", AssetKind::Texture),
    (".tga", AssetKind::Texture),
    (".nif", AssetKind::Mesh),
    (".btr", AssetKind::Mesh),
    (".bto", AssetKind::Mesh),
    (".hkx", AssetKind::Animation),
];

/// The read-only closed set of work execution must perform for one Routed Asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct AssetOperations([bool; 4]);

impl AssetOperations {
    /// Reports whether the Routed Asset carries one closed operation.
    pub fn contains(&self, operation: AssetOperation) -> bool {
        self.0[operation as usize]
    }

    /// Adds one operation while the router builds a Routed Asset.
    fn include(&mut self, operation: AssetOperation) {
        self.0[operation as usize] = true;
    }

    /// Reports whether no operation applies to the recognized Asset.
    fn is_empty(&self) -> bool {
        !self.0.iter().any(|included| *included)
    }
}

/// The execution mode and closed requested work one Routing Policy is compiled from.
///
/// This is not the Run Request of the glossary: it carries no Mod Selection,
/// Archive Precedence or profile identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingPolicyRequest {
    execution_mode: ExecutionMode,
    work: [bool; 7],
}

impl RoutingPolicyRequest {
    /// Owns the requested work as a closed set for the selected execution mode;
    /// repeated choices collapse.
    pub fn for_work(execution_mode: ExecutionMode, work: &[RequestedWork]) -> Self {
        let mut request = Self {
            execution_mode,
            work: [false; 7],
        };
        for choice in work {
            request.work[*choice as usize] = true;
        }
        request
    }

    /// The tracer's dedicated request: Apply-mode native Texture optimization.
    pub fn optimize_native_textures() -> Self {
        Self::for_work(
            ExecutionMode::Apply,
            &[RequestedWork::NativeTextureOptimization],
        )
    }
}

/// The Profile Capability facts a Routing Policy Request is validated against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileCapabilities {
    archive_extension: Option<String>,
    capabilities: [bool; 8],
}

impl ProfileCapabilities {
    /// Owns one raw Archive extension definition and a closed set of supported work.
    pub fn define(
        archive_extension: impl Into<String>,
        capabilities: &[ProfileCapability],
    ) -> Self {
        Self::from_definition(Some(archive_extension.into()), capabilities)
    }

    /// Omits the Archive extension, so compilation reports it as missing.
    pub fn without_archive_extension(capabilities: &[ProfileCapability]) -> Self {
        Self::from_definition(None, capabilities)
    }

    fn from_definition(
        archive_extension: Option<String>,
        capabilities: &[ProfileCapability],
    ) -> Self {
        let mut definition = Self {
            archive_extension,
            capabilities: [false; 8],
        };
        for capability in capabilities {
            definition.capabilities[*capability as usize] = true;
        }
        definition
    }

    fn supports(&self, capability: ProfileCapability) -> bool {
        self.capabilities[capability as usize]
    }

    /// Reports whether the profile supports any behavior within an Asset Kind.
    fn supports_kind(&self, kind: AssetKind) -> bool {
        CAPABILITIES
            .iter()
            .any(|capability| self.supports(*capability) && capability_kind(*capability) == kind)
    }
}

const CAPABILITIES: [ProfileCapability; 8] = [
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
    ProfileCapability::ArchiveCreation,
];

/// Why a profile Archive extension is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MalformedArchiveExtensionReason {
    MissingLeadingPeriod,
    EmptySuffix,
    InvalidCharacter,
}

/// A typed Asset Variant, qualified by its Kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetVariant {
    Texture(TextureVariant),
    Mesh(MeshVariant),
}

/// One structured conflict between a Routing Policy Request and Profile Capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PolicyValidationError {
    MissingArchiveExtension,
    MalformedArchiveExtension {
        extension: String,
        reason: MalformedArchiveExtensionReason,
    },
    AmbiguousArchiveExtension {
        extension: String,
        conflicting_extension: String,
        conflicting_kind: AssetKind,
    },
    UnsupportedRequestedAssetKind {
        request: RequestedWork,
        kind: AssetKind,
    },
    UnsupportedRequestedAssetVariant {
        request: RequestedWork,
        variant: AssetVariant,
    },
    UnsupportedDerivedOperation {
        cause: RequestedWork,
        operation: AssetOperation,
    },
}

/// The stable domain name used to present one Asset Kind.
fn asset_kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Texture => "Texture",
        AssetKind::Mesh => "Mesh",
        AssetKind::Animation => "Animation",
        AssetKind::Archive => "Archive",
    }
}

/// A user-facing phrase for one explicit work choice.
fn requested_work_name(work: RequestedWork) -> &'static str {
    match work {
        RequestedWork::NativeTextureOptimization => "native Texture optimization",
        RequestedWork::ConvertibleTextureConversion => "convertible Texture conversion",
        RequestedWork::StandardMeshOptimization => "standard Mesh optimization",
        RequestedWork::TerrainMeshOptimization => "terrain Mesh optimization",
        RequestedWork::AnimationOptimization => "Animation optimization",
        RequestedWork::ArchiveExtraction => "Archive extraction",
        RequestedWork::ArchiveCreation => "Archive creation",
    }
}

/// The kind-qualified name of a typed Asset Variant.
fn asset_variant_name(variant: AssetVariant) -> &'static str {
    match variant {
        AssetVariant::Texture(TextureVariant::Native) => "native Texture",
        AssetVariant::Texture(TextureVariant::Convertible) => "convertible Texture",
        AssetVariant::Mesh(MeshVariant::Standard) => "standard Mesh",
        AssetVariant::Mesh(MeshVariant::Terrain) => "terrain Mesh",
    }
}

/// Presents one conflict the way C++ `policyValidationErrorMessage` did.
impl fmt::Display for PolicyValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArchiveExtension => {
                write!(f, "The selected profile is missing its Archive extension.")
            }
            Self::MalformedArchiveExtension { extension, reason } => {
                let reason = match reason {
                    MalformedArchiveExtensionReason::MissingLeadingPeriod => {
                        "it must begin with a period"
                    }
                    MalformedArchiveExtensionReason::EmptySuffix => {
                        "it must include characters after the period"
                    }
                    MalformedArchiveExtensionReason::InvalidCharacter => {
                        "it may contain only ASCII letters and digits after the period"
                    }
                };
                write!(
                    f,
                    "The selected profile Archive extension '{extension}' is invalid because {reason}."
                )
            }
            Self::AmbiguousArchiveExtension {
                extension,
                conflicting_extension,
                conflicting_kind,
            } => write!(
                f,
                "The selected profile Archive extension '{extension}' conflicts with the built-in {} extension '{conflicting_extension}'.",
                asset_kind_name(*conflicting_kind)
            ),
            Self::UnsupportedRequestedAssetKind { request, kind } => write!(
                f,
                "The selected profile does not support requested {} for the {} Asset Kind.",
                requested_work_name(*request),
                asset_kind_name(*kind)
            ),
            Self::UnsupportedRequestedAssetVariant { request, variant } => write!(
                f,
                "The selected profile does not support requested {} for the {} Asset Variant.",
                requested_work_name(*request),
                asset_variant_name(*variant)
            ),
            Self::UnsupportedDerivedOperation { cause, .. } => write!(
                f,
                "Requested {} requires unsupported Mesh Reference Maintenance.",
                requested_work_name(*cause)
            ),
        }
    }
}

/// Validates the profile Archive extension grammar without treating it as a path.
fn malformed_reason(extension: &str) -> Option<MalformedArchiveExtensionReason> {
    let Some(suffix) = extension.strip_prefix('.') else {
        return Some(MalformedArchiveExtensionReason::MissingLeadingPeriod);
    };
    if suffix.is_empty() {
        return Some(MalformedArchiveExtensionReason::EmptySuffix);
    }
    if !suffix
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        return Some(MalformedArchiveExtensionReason::InvalidCharacter);
    }
    None
}

/// The immutable, run-scoped facts used to make Routing Decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingPolicy {
    execution_mode: ExecutionMode,
    work: [bool; 7],
    mesh_reference_maintenance: bool,
    archive_extension: String,
}

impl RoutingPolicy {
    /// Compiles a request against Profile Capabilities into one policy, or
    /// every validation error at once.
    ///
    /// Errors come in compiler order: the Archive extension first, then each
    /// explicit work choice in enumeration order, then derived work, so an
    /// adapter receives the complete conflict set in one pass.
    pub fn compile(
        request: RoutingPolicyRequest,
        capabilities: ProfileCapabilities,
    ) -> Result<RoutingPolicy, Vec<PolicyValidationError>> {
        let mut errors = Vec::new();

        let mut archive_extension = String::new();
        match capabilities.archive_extension.as_deref() {
            None | Some("") => errors.push(PolicyValidationError::MissingArchiveExtension),
            Some(extension) => {
                if let Some(reason) = malformed_reason(extension) {
                    errors.push(PolicyValidationError::MalformedArchiveExtension {
                        extension: extension.to_owned(),
                        reason,
                    });
                } else {
                    archive_extension = extension.to_ascii_lowercase();
                    if let Some((built_in, kind)) = BUILT_IN_EXTENSIONS
                        .iter()
                        .find(|(built_in, _)| *built_in == archive_extension)
                    {
                        errors.push(PolicyValidationError::AmbiguousArchiveExtension {
                            extension: extension.to_owned(),
                            conflicting_extension: (*built_in).to_owned(),
                            conflicting_kind: *kind,
                        });
                    }
                }
            }
        }

        for (work, capability, variant) in REQUESTED_WORK_DEFINITIONS {
            if !request.work[work as usize] || capabilities.supports(capability) {
                continue;
            }
            let kind = capability_kind(capability);
            // A Kind the profile cannot handle at all is reported as such, even
            // for Variant work, because no Variant of it could be offered.
            match variant {
                Some(variant) if capabilities.supports_kind(kind) => {
                    errors.push(PolicyValidationError::UnsupportedRequestedAssetVariant {
                        request: work,
                        variant,
                    })
                }
                _ => errors.push(PolicyValidationError::UnsupportedRequestedAssetKind {
                    request: work,
                    kind,
                }),
            }
        }

        let mesh_reference_maintenance =
            request.work[RequestedWork::ConvertibleTextureConversion as usize];
        if mesh_reference_maintenance
            && !capabilities.supports(ProfileCapability::MeshReferenceMaintenance)
        {
            errors.push(PolicyValidationError::UnsupportedDerivedOperation {
                cause: RequestedWork::ConvertibleTextureConversion,
                operation: AssetOperation::MeshReferenceMaintenance,
            });
        }

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(RoutingPolicy {
            execution_mode: request.execution_mode,
            work: request.work,
            mesh_reference_maintenance,
            archive_extension,
        })
    }

    /// The Apply-or-Dry-Run mode fixed for the duration of the run.
    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    /// Reports whether the compiled policy contains one explicit work choice.
    pub fn requests(&self, work: RequestedWork) -> bool {
        self.work[work as usize]
    }

    /// Reports whether convertible Texture work derived Mesh Reference Maintenance.
    pub fn maintains_mesh_references(&self) -> bool {
        self.mesh_reference_maintenance
    }

    /// The validated, ASCII-lowercase profile Archive extension, including its period.
    pub fn archive_extension(&self) -> &str {
        &self.archive_extension
    }

    /// Reports whether any Variant of a Kind has work under this policy.
    fn kind_has_work(&self, kind: AssetKind) -> bool {
        match kind {
            AssetKind::Texture => {
                self.requests(RequestedWork::NativeTextureOptimization)
                    || self.requests(RequestedWork::ConvertibleTextureConversion)
            }
            AssetKind::Mesh => {
                self.requests(RequestedWork::StandardMeshOptimization)
                    || self.requests(RequestedWork::TerrainMeshOptimization)
                    || self.maintains_mesh_references()
            }
            AssetKind::Animation => self.requests(RequestedWork::AnimationOptimization),
            AssetKind::Archive => self.requests(RequestedWork::ArchiveExtraction),
        }
    }
}

/// The kind-specific identity of a recognized Asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetIdentity {
    Texture(TextureVariant),
    Mesh(MeshVariant),
    Animation,
    Archive,
}

impl AssetIdentity {
    /// The Asset Kind this identity belongs to.
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Texture(_) => AssetKind::Texture,
            Self::Mesh(_) => AssetKind::Mesh,
            Self::Animation => AssetKind::Animation,
            Self::Archive => AssetKind::Archive,
        }
    }
}

/// A recognized Asset selected to participate in an Optimization Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAsset {
    execution_path: PathBuf,
    identity: AssetIdentity,
    phase: RoutedAssetPhase,
    target: OptimizerTarget,
    execution_mode: ExecutionMode,
    operations: AssetOperations,
}

impl RoutedAsset {
    /// The caller's execution path, exactly as supplied to the router.
    pub fn execution_path(&self) -> &Path {
        &self.execution_path
    }

    /// The Asset Kind implied by the identity.
    pub fn kind(&self) -> AssetKind {
        self.identity.kind()
    }

    /// The kind-specific identity selected by routing.
    pub fn identity(&self) -> AssetIdentity {
        self.identity
    }

    /// The Routed Asset Phase, so callers never reinterpret the policy.
    pub fn phase(&self) -> RoutedAssetPhase {
        self.phase
    }

    /// The optimizer selected for execution.
    pub fn target(&self) -> OptimizerTarget {
        self.target
    }

    /// The Apply-or-Dry-Run mode fixed by the Routing Policy.
    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    /// The complete closed operation set selected for execution.
    pub fn operations(&self) -> AssetOperations {
        self.operations
    }
}

/// A recognized Asset excluded by Routing Policy before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedAsset {
    execution_path: PathBuf,
    identity: AssetIdentity,
    reason: SkipReason,
}

impl SkippedAsset {
    /// The caller's execution path, exactly as supplied to the router.
    pub fn execution_path(&self) -> &Path {
        &self.execution_path
    }

    /// The Asset Kind implied by the identity.
    pub fn kind(&self) -> AssetKind {
        self.identity.kind()
    }

    /// The kind-specific identity recognized before exclusion.
    pub fn identity(&self) -> AssetIdentity {
        self.identity
    }

    /// The highest-precedence reason for the exclusion.
    pub fn reason(&self) -> SkipReason {
        self.reason
    }
}

/// The policy-aware outcome of routing one path (its Routing Disposition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingDecision {
    Routed(RoutedAsset),
    Skipped(SkippedAsset),
    /// The terminal extension names no supported Asset.
    Unsupported,
}

/// The batch routing outcome: Routed Assets in caller order, and recognized
/// exclusions counted by Skip Reason. Unsupported paths are not retained.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingLedger {
    routed_assets: Vec<RoutedAsset>,
    skipped_asset_counts: BTreeMap<SkipReason, usize>,
}

impl RoutingLedger {
    /// Every Routed Asset, in caller input order, duplicates included.
    pub fn routed_assets(&self) -> &[RoutedAsset] {
        &self.routed_assets
    }

    /// The Routed Assets of one Routed Asset Phase, in their original relative order.
    pub fn routed_assets_in_phase(&self, phase: RoutedAssetPhase) -> Vec<&RoutedAsset> {
        self.routed_assets
            .iter()
            .filter(|asset| asset.phase == phase)
            .collect()
    }

    /// The Routed Assets of one optimizer, in their original relative order.
    pub fn routed_assets_for(&self, target: OptimizerTarget) -> Vec<&RoutedAsset> {
        self.routed_assets
            .iter()
            .filter(|asset| asset.target == target)
            .collect()
    }

    /// How many recognized Assets were excluded for one Skip Reason.
    pub fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.skipped_asset_counts.get(&reason).copied().unwrap_or(0)
    }
}

/// Returns the ASCII-lowercased terminal extension with its period, or an empty
/// string when the name has none or it contains non-ASCII text.
fn normalized_terminal_extension(path: &Path) -> String {
    let Some(extension) = path.extension() else {
        return String::new();
    };
    match extension.to_str() {
        Some(text) if text.is_ascii() => format!(".{}", text.to_ascii_lowercase()),
        _ => String::new(),
    }
}

/// Makes deterministic, filename-only Routing Decisions from one immutable policy.
#[derive(Debug, Clone)]
pub struct AssetRouter {
    policy: RoutingPolicy,
}

impl AssetRouter {
    /// Owns the policy every decision of this router uses.
    pub fn new(policy: RoutingPolicy) -> Self {
        Self { policy }
    }

    /// The policy this router decides with.
    pub fn policy(&self) -> &RoutingPolicy {
        &self.policy
    }

    /// Routes one path without filesystem access or path normalization.
    pub fn route(&self, execution_path: &Path) -> RoutingDecision {
        let extension = normalized_terminal_extension(execution_path);
        let mut operations = AssetOperations::default();
        let (identity, phase, target) = match extension.as_str() {
            ".dds" => {
                if self
                    .policy
                    .requests(RequestedWork::NativeTextureOptimization)
                {
                    operations.include(AssetOperation::Optimization);
                }
                (
                    AssetIdentity::Texture(TextureVariant::Native),
                    RoutedAssetPhase::LooseAssetProcessing,
                    OptimizerTarget::Texture,
                )
            }
            ".tga" => {
                if self
                    .policy
                    .requests(RequestedWork::ConvertibleTextureConversion)
                {
                    operations.include(AssetOperation::Conversion);
                }
                (
                    AssetIdentity::Texture(TextureVariant::Convertible),
                    RoutedAssetPhase::LooseAssetProcessing,
                    OptimizerTarget::Texture,
                )
            }
            ".nif" | ".btr" | ".bto" => {
                let (variant, work) = if extension == ".nif" {
                    (
                        MeshVariant::Standard,
                        RequestedWork::StandardMeshOptimization,
                    )
                } else {
                    (MeshVariant::Terrain, RequestedWork::TerrainMeshOptimization)
                };
                if self.policy.requests(work) {
                    operations.include(AssetOperation::Optimization);
                }
                // Convertible Texture conversion changes referenced names, so
                // both Mesh Variants carry maintenance independently of
                // optimization.
                if self.policy.maintains_mesh_references() {
                    operations.include(AssetOperation::MeshReferenceMaintenance);
                }
                (
                    AssetIdentity::Mesh(variant),
                    RoutedAssetPhase::LooseAssetProcessing,
                    OptimizerTarget::Mesh,
                )
            }
            ".hkx" => {
                if self.policy.requests(RequestedWork::AnimationOptimization) {
                    operations.include(AssetOperation::Optimization);
                }
                (
                    AssetIdentity::Animation,
                    RoutedAssetPhase::LooseAssetProcessing,
                    OptimizerTarget::Animation,
                )
            }
            other if !other.is_empty() && other == self.policy.archive_extension() => {
                if self.policy.requests(RequestedWork::ArchiveExtraction) {
                    operations.include(AssetOperation::Extraction);
                }
                (
                    AssetIdentity::Archive,
                    RoutedAssetPhase::ArchiveExtraction,
                    OptimizerTarget::Archive,
                )
            }
            _ => return RoutingDecision::Unsupported,
        };

        let execution_path = execution_path.to_path_buf();
        // Dry Run disables Archive extraction before Kind and Variant
        // eligibility is considered.
        if phase == RoutedAssetPhase::ArchiveExtraction
            && self.policy.execution_mode() == ExecutionMode::DryRun
        {
            return RoutingDecision::Skipped(SkippedAsset {
                execution_path,
                identity,
                reason: SkipReason::DisabledPhase,
            });
        }
        if !operations.is_empty() {
            return RoutingDecision::Routed(RoutedAsset {
                execution_path,
                identity,
                phase,
                target,
                execution_mode: self.policy.execution_mode(),
                operations,
            });
        }
        let reason = if self.policy.kind_has_work(identity.kind()) {
            SkipReason::ExcludedAssetVariant
        } else {
            SkipReason::DisabledAssetKind
        };
        RoutingDecision::Skipped(SkippedAsset {
            execution_path,
            identity,
            reason,
        })
    }

    /// Routes every path once into an owned ledger that keeps routed input order
    /// and duplicates.
    pub fn route_all<P: AsRef<Path>>(
        &self,
        execution_paths: impl IntoIterator<Item = P>,
    ) -> RoutingLedger {
        let mut ledger = RoutingLedger::default();
        for path in execution_paths {
            match self.route(path.as_ref()) {
                RoutingDecision::Routed(asset) => ledger.routed_assets.push(asset),
                RoutingDecision::Skipped(asset) => {
                    *ledger.skipped_asset_counts.entry(asset.reason).or_default() += 1;
                }
                // Unsupported paths are neither work nor recognized exclusions.
                RoutingDecision::Unsupported => {}
            }
        }
        ledger
    }
}
