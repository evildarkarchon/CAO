//! Preparing: the Run Configuration Provider seam and the facts Preparing establishes.
//!
//! Ported from `src/Run/RunPreparation.h` and `src/Run/RunSetup.h`.

use std::path::PathBuf;

use crate::Error;
use crate::routing::{
    PolicyValidationError, ProfileCapabilities, RoutingPolicy, RoutingPolicyRequest,
};
use crate::run::RunToolset;

/// How Archives within one Mod Root are ordered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ArchivePrecedence {
    /// The run's deterministic discovery order within each Mod Root.
    #[default]
    DeterministicDiscovery,
    /// Caller-supplied high-to-low Archive paths relative to the Mod Root. An
    /// empty list is still explicit; discovery validates it against the enabled
    /// Archives.
    ExplicitOrder(Vec<PathBuf>),
}

/// The Selected Profile: the profile facts one Optimization Run reads once,
/// during Preparing, owning no profile object or application state.
///
/// It holds the facts core uses: the typed Profile Capabilities, which carry
/// the profile's Archive extension; the Conversion Preference; the
/// ignored-mods list; and the Packing Exclusions. The backend's own profile
/// facts, such as the Texture and Mesh targets and the custom Headpart Meshes,
/// come from the same read but stay private to the provider that built it.
///
/// The default profile declares no Archive extension, no Profile Capability
/// and no Conversion Preference, and excludes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedProfile {
    capabilities: ProfileCapabilities,
    prefers_conversion: bool,
    ignored_mods: Vec<String>,
    packing_exclusions: Vec<String>,
}

impl Default for SelectedProfile {
    fn default() -> Self {
        Self::new(ProfileCapabilities::without_archive_extension(&[]))
    }
}

impl SelectedProfile {
    /// A Selected Profile with `capabilities`, no Conversion Preference, no
    /// ignored mods and no Packing Exclusions.
    pub fn new(capabilities: ProfileCapabilities) -> Self {
        Self {
            capabilities,
            prefers_conversion: false,
            ignored_mods: Vec::new(),
            packing_exclusions: Vec::new(),
        }
    }

    /// Sets the Conversion Preference: whether requested Texture work also
    /// converts convertible Textures. It is not a Profile Capability, so it
    /// never contradicts a Run Request.
    #[must_use]
    pub fn with_conversion_preference(mut self, prefers_conversion: bool) -> Self {
        self.prefers_conversion = prefers_conversion;
        self
    }

    /// Sets the ignored-mods list: child names that Several Mods Preparing
    /// excludes as Mod Exclusions, matched case-insensitively.
    #[must_use]
    pub fn with_ignored_mods(mut self, ignored_mods: Vec<String>) -> Self {
        self.ignored_mods = ignored_mods;
        self
    }

    /// Sets the Packing Exclusions: the rules naming Loose Assets that Archive
    /// Finalization keeps loose. They are kept as the profile declares them.
    #[must_use]
    pub fn with_packing_exclusions(mut self, packing_exclusions: Vec<String>) -> Self {
        self.packing_exclusions = packing_exclusions;
        self
    }

    /// The Profile Capabilities, including the profile's Archive extension.
    pub fn capabilities(&self) -> &ProfileCapabilities {
        &self.capabilities
    }

    /// The Conversion Preference. Routing does not consult it yet: the Run
    /// Request still carries convertible Texture conversion.
    pub fn prefers_conversion(&self) -> bool {
        self.prefers_conversion
    }

    /// The ignored-mods list, as the profile declares it.
    pub fn ignored_mods(&self) -> &[String] {
        &self.ignored_mods
    }

    /// The Packing Exclusions, as the profile declares them.
    pub fn packing_exclusions(&self) -> &[String] {
        &self.packing_exclusions
    }

    /// Compiles a Routing Policy Request against this profile's Profile
    /// Capabilities.
    ///
    /// # Errors
    /// Every Policy Conflict between the request and the capabilities, in
    /// compiler order.
    pub fn compile_policy(
        &self,
        request: RoutingPolicyRequest,
    ) -> Result<RoutingPolicy, Vec<PolicyValidationError>> {
        RoutingPolicy::compile(request, self.capabilities.clone())
    }
}

/// Loads a run's Selected Profile during Preparing, on the Run Worker.
///
/// Implementations return owned facts and never borrow application state. An
/// error becomes a `ConfigurationLoadingFailed` Run Failure, never a Start Error.
pub trait RunConfigurationProvider: Send + Sync {
    /// Reads the named profile once, returning its Selected Profile together
    /// with the Run Toolset built from that same read, so the run's work uses
    /// exactly the profile facts its Routing Policy is compiled from.
    ///
    /// A `None` Toolset means the provider supplies no means to do work: a
    /// request with work then fails Preparing with `RequestedWorkUnavailable`.
    ///
    /// # Errors
    /// Any error becomes a `ConfigurationLoadingFailed` Run Failure.
    fn load(&self, profile_identity: &str) -> Result<(SelectedProfile, Option<RunToolset>), Error>;
}

/// The immutable facts a successful Preparing establishes, retained by the terminal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPreparation {
    mod_roots: Vec<PathBuf>,
    selected_profile: SelectedProfile,
    policy: RoutingPolicy,
    archive_precedence: ArchivePrecedence,
}

impl RunPreparation {
    /// Owns the canonical Mod Roots, the Selected Profile and the policy
    /// compiled from it.
    pub fn new(
        mod_roots: Vec<PathBuf>,
        selected_profile: SelectedProfile,
        policy: RoutingPolicy,
        archive_precedence: ArchivePrecedence,
    ) -> Self {
        Self {
            mod_roots,
            selected_profile,
            policy,
            archive_precedence,
        }
    }

    /// The resolved Mod Roots, in run order.
    pub fn mod_roots(&self) -> &[PathBuf] {
        &self.mod_roots
    }

    /// The Selected Profile Preparing read, which every part of the run uses.
    pub fn selected_profile(&self) -> &SelectedProfile {
        &self.selected_profile
    }

    pub fn policy(&self) -> &RoutingPolicy {
        &self.policy
    }

    /// Ordering intent; its completeness is checked during Archive discovery.
    pub fn archive_precedence(&self) -> &ArchivePrecedence {
        &self.archive_precedence
    }
}
